//! Ergonomic wrapper over the engine's type-erased ECS (`SFT::Ecs::World`).
//!
//! The engine resolves components purely by a string name it hashes into a `ComponentId`, so at
//! the bottom this stays byte-level (`&[u8]` in, `Vec<u8>` out) — the [`World`] methods that take
//! and return raw bytes directly (`register_component`, `add_component`, `read_component`, `query`,
//! ...) are kept exactly as-is for dynamic/reflection-driven callers.
//!
//! On top of that, the [`Component`] trait lets a caller's own `#[repr(C)]` Rust type describe
//! itself (a stable name plus `bytemuck::Pod`, which gives a checked byte view without `unsafe`),
//! and the typed `World` methods below (`register::<T>`, `get::<T>`, `set::<T>`, `insert::<T>`,
//! `spawn_one::<T>`, `query::<(A, B, ...)>`, ...) build entirely on the byte-level surface — no new
//! engine calls were needed. See the module-level docs in `sturdy-sys/src/ecs.rs` for the raw
//! surface this builds on.
//!
//! Three more typed primitives sit alongside components: [`Bundle`] (spawn several [`Component`]
//! types from one plain struct via `#[derive(sturdy::Bundle)]`), [`Event`] (buffered event
//! channels), and [`Resource`] (typed wrappers over the existing byte-level resource calls).

use core::mem::{align_of, size_of};
use core::pin::Pin;

use sturdy_sys::ecs::ffi;
use sturdy_sys::EngineView;

/// A handle to a spawned entity. Stays meaningful after despawn only to *detect* that: once the
/// slot is reused its generation moves on, so `World::is_alive` reports `false` for the old value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Entity {
    pub index: u32,
    pub generation: u32,
}

impl From<ffi::EntityId> for Entity {
    fn from(id: ffi::EntityId) -> Self {
        Self { index: id.index, generation: id.generation }
    }
}

impl From<Entity> for ffi::EntityId {
    fn from(entity: Entity) -> Self {
        ffi::EntityId { index: entity.index, generation: entity.generation }
    }
}

/// A component registered with the world, resolved once by name via [`World::register_component`]
/// or [`World::find_component`] and cheap to copy around afterward.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ComponentId(pub u32);

/// A bound resource (or event channel), addressed by a key derived from its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceId {
    pub high: u64,
    pub low: u64,
}

impl From<ffi::ResourceId> for ResourceId {
    fn from(id: ffi::ResourceId) -> Self {
        Self { high: id.high, low: id.low }
    }
}

impl From<ResourceId> for ffi::ResourceId {
    fn from(id: ResourceId) -> Self {
        ffi::ResourceId { high: id.high, low: id.low }
    }
}

/// Why an ECS operation failed. Mirrors `SFT::Ecs::WorldErasedErrorCode`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EcsError {
    /// The entity was never valid, or has since been destroyed.
    DeadEntity,
    /// No component or resource is registered under that id/name.
    UnknownComponent,
    /// The entity already carries that component.
    DuplicateComponent,
    /// The entity does not carry that component.
    MissingComponent,
    /// The supplied byte count does not match the component/resource's registered size.
    SizeMismatch,
    /// The component is not trivially copyable, so its bytes cannot be manipulated directly.
    NotTriviallyCopyable,
    /// No components were supplied; an entity or query needs at least one.
    NoComponents,
    /// A schedule is running; structural changes must wait until it finishes.
    ScheduleRunning,
    /// Some other invalid argument (bad name, size, or alignment); message from the engine.
    InvalidArgument(String),
}

impl core::fmt::Display for EcsError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            EcsError::DeadEntity => write!(f, "entity is not alive"),
            EcsError::UnknownComponent => write!(f, "unknown component or resource"),
            EcsError::DuplicateComponent => write!(f, "entity already has this component"),
            EcsError::MissingComponent => write!(f, "entity does not have this component"),
            EcsError::SizeMismatch => write!(f, "byte count does not match the registered size"),
            EcsError::NotTriviallyCopyable => write!(f, "component is not trivially copyable"),
            EcsError::NoComponents => write!(f, "at least one component is required"),
            EcsError::ScheduleRunning => write!(f, "a schedule is running"),
            EcsError::InvalidArgument(message) => write!(f, "{message}"),
        }
    }
}

impl std::error::Error for EcsError {}

fn map_error(code: ffi::EcsErrorCode, message: String) -> EcsError {
    // `ffi::EcsErrorCode` is cxx's shared-enum type, not a real Rust `enum`, so it is compared
    // rather than matched.
    if code == ffi::EcsErrorCode::DeadEntity {
        EcsError::DeadEntity
    } else if code == ffi::EcsErrorCode::UnknownComponent {
        EcsError::UnknownComponent
    } else if code == ffi::EcsErrorCode::DuplicateComponent {
        EcsError::DuplicateComponent
    } else if code == ffi::EcsErrorCode::MissingComponent {
        EcsError::MissingComponent
    } else if code == ffi::EcsErrorCode::SizeMismatch {
        EcsError::SizeMismatch
    } else if code == ffi::EcsErrorCode::NotTriviallyCopyable {
        EcsError::NotTriviallyCopyable
    } else if code == ffi::EcsErrorCode::NoComponents {
        EcsError::NoComponents
    } else if code == ffi::EcsErrorCode::ScheduleRunning {
        EcsError::ScheduleRunning
    } else {
        EcsError::InvalidArgument(message)
    }
}

/// `pub(crate)`, not private: `schedule.rs` reuses this for the same `EcsStatus` shape returned by
/// the scheduler-registration bridge calls, rather than duplicating the mapping.
pub(crate) fn status_result(status: ffi::EcsStatus) -> Result<(), EcsError> {
    if status.ok { Ok(()) } else { Err(map_error(status.code, status.message)) }
}

/// Byte-level snapshot returned by [`World::query`]: every live entity that carried all requested
/// components at the moment of the call, with each requested component's bytes copied out.
#[derive(Debug, Clone)]
pub struct QuerySnapshot {
    pub entities: Vec<Entity>,
    /// Byte size of each requested component, same order as the ids passed to [`World::query`].
    pub sizes: Vec<u32>,
    /// Entity-major concatenation of every matched entity's requested-component bytes.
    pub data: Vec<u8>,
}

impl QuerySnapshot {
    /// Bytes for `component_index` (0-based, in the order passed to [`World::query`]) belonging to
    /// `self.entities[entity_index]`.
    ///
    /// Panics if either index is out of range.
    pub fn component_bytes(&self, entity_index: usize, component_index: usize) -> &[u8] {
        let row_size: usize = self.sizes.iter().map(|&s| s as usize).sum();
        let component_offset: usize =
            self.sizes[..component_index].iter().map(|&s| s as usize).sum();
        let start = entity_index * row_size + component_offset;
        let size = self.sizes[component_index] as usize;
        &self.data[start..start + size]
    }
}

/// Borrowed access to the live ECS world.
///
/// Like [`Engine`](crate::Engine), this only exists inside a game-logic callback and borrows from
/// it. See the module docs for why this API is byte-level.
pub struct World<'a> {
    view: Pin<&'a mut EngineView>,
}

impl<'a> World<'a> {
    pub(crate) fn new(view: Pin<&'a mut EngineView>) -> Self {
        Self { view }
    }

    fn view(&self) -> &EngineView {
        &self.view
    }

    /// `pub(crate)`: `schedule.rs`'s `World::add_system`/`add_global_system` (and their
    /// byte-level/typed variants) need the raw `EngineView` to call into `sturdy_sys::schedule`,
    /// the same way every method in this file does.
    pub(crate) fn view_mut(&mut self) -> Pin<&mut EngineView> {
        self.view.as_mut()
    }

    /// Derives the key a resource/event-channel name hashes to, without touching the world.
    pub fn resource_key(name: &str) -> ResourceId {
        ffi::ecs_resource_key(name).into()
    }

    /// Registers a component under `name`, or returns the existing id if one is already
    /// registered with the same name, size, and alignment.
    pub fn register_component(&mut self, name: &str, size: u32, align: u32) -> Result<ComponentId, EcsError> {
        let result = ffi::ecs_register_component(self.view_mut(), name, size, align);
        if result.ok { Ok(ComponentId(result.id)) } else { Err(map_error(result.code, result.message)) }
    }

    /// Looks up a component previously registered under `name` — by this call, by the C++ engine
    /// itself (e.g. `"sturdy.engine.world_transform"`), or by any other caller sharing this world.
    pub fn find_component(&self, name: &str) -> Option<ComponentId> {
        let result = ffi::ecs_find_component(self.view(), name);
        result.ok.then_some(ComponentId(result.id))
    }

    /// Spawns an entity carrying the given components. Each entry's byte slice must be exactly
    /// that component's registered size.
    ///
    /// Byte-level counterpart to the typed [`World::spawn`] (bundles) and [`World::spawn_one`]
    /// (single component); kept under its own name for the same reason [`World::query_bytes`] is.
    pub fn spawn_bytes(&mut self, components: &[(ComponentId, &[u8])]) -> Result<Entity, EcsError> {
        let ids: Vec<u32> = components.iter().map(|(id, _)| id.0).collect();
        let sizes: Vec<u32> = components.iter().map(|(_, data)| data.len() as u32).collect();
        let data: Vec<u8> = components.iter().flat_map(|(_, data)| data.iter().copied()).collect();
        let result = ffi::ecs_spawn(self.view_mut(), &ids, &sizes, &data);
        if result.ok { Ok(result.entity.into()) } else { Err(map_error(result.code, result.message)) }
    }

    /// Destroys an entity. Not an error if it is already dead.
    pub fn despawn(&mut self, entity: Entity) {
        ffi::ecs_despawn(self.view_mut(), entity.into());
    }

    pub fn is_alive(&self, entity: Entity) -> bool {
        ffi::ecs_is_alive(self.view(), entity.into())
    }

    pub fn has_component(&self, entity: Entity, component: ComponentId) -> bool {
        ffi::ecs_has_component(self.view(), entity.into(), component.0)
    }

    /// Attaches `component` to `entity`. `data` must be exactly the component's registered size.
    pub fn add_component(&mut self, entity: Entity, component: ComponentId, data: &[u8]) -> Result<(), EcsError> {
        status_result(ffi::ecs_add_component(self.view_mut(), entity.into(), component.0, data))
    }

    pub fn remove_component(&mut self, entity: Entity, component: ComponentId) -> Result<(), EcsError> {
        status_result(ffi::ecs_remove_component(self.view_mut(), entity.into(), component.0))
    }

    /// Copies a component's bytes out. `size` must match the component's registered size.
    pub fn read_component(&self, entity: Entity, component: ComponentId, size: usize) -> Result<Vec<u8>, EcsError> {
        let mut buffer = vec![0u8; size];
        let status = ffi::ecs_read_component(self.view(), entity.into(), component.0, &mut buffer);
        if status.ok { Ok(buffer) } else { Err(map_error(status.code, status.message)) }
    }

    /// Overwrites a component's bytes in place. `data` must be exactly the component's registered
    /// size.
    pub fn write_component(&mut self, entity: Entity, component: ComponentId, data: &[u8]) -> Result<(), EcsError> {
        status_result(ffi::ecs_write_component(self.view_mut(), entity.into(), component.0, data))
    }

    /// Binds a resource under `name`, with storage owned by this binding (not by `initial`, which
    /// is only copied from). Re-binding under the same name with the same size is a no-op.
    pub fn create_resource(&mut self, name: &str, initial: &[u8]) -> Result<ResourceId, EcsError> {
        let result = ffi::ecs_create_resource(self.view_mut(), name, initial);
        if result.ok { Ok(result.id.into()) } else { Err(map_error(result.code, result.message)) }
    }

    /// Unbinds a resource this binding created, freeing its storage.
    pub fn destroy_resource(&mut self, id: ResourceId) -> Result<(), EcsError> {
        status_result(ffi::ecs_destroy_resource(self.view_mut(), id.into()))
    }

    pub fn has_resource(&self, id: ResourceId) -> bool {
        ffi::ecs_has_resource(self.view(), id.into())
    }

    /// Copies a resource's bytes out. `size` must match the resource's bound size.
    pub fn get_resource(&mut self, id: ResourceId, size: usize) -> Result<Vec<u8>, EcsError> {
        let mut buffer = vec![0u8; size];
        let status = ffi::ecs_get_resource(self.view_mut(), id.into(), &mut buffer);
        if status.ok { Ok(buffer) } else { Err(map_error(status.code, status.message)) }
    }

    /// Overwrites a resource's bytes in place. `data` must be exactly the resource's bound size.
    pub fn set_resource(&mut self, id: ResourceId, data: &[u8]) -> Result<(), EcsError> {
        status_result(ffi::ecs_set_resource(self.view_mut(), id.into(), data))
    }

    /// Snapshots every live entity carrying all of `component_ids`, copying each requested
    /// component's bytes out as of the moment of the call. See [`QuerySnapshot`].
    ///
    /// Byte-level counterpart to the typed [`World::query`]; kept under its own name (rather than
    /// overloaded as `query`, which Rust's inherent-method resolution doesn't support by argument
    /// count or generic bound alone) for dynamic/reflection-driven callers that don't have a
    /// [`QueryTuple`] to name.
    pub fn query_bytes(&mut self, component_ids: &[ComponentId]) -> Result<QuerySnapshot, EcsError> {
        let ids: Vec<u32> = component_ids.iter().map(|c| c.0).collect();
        let result = ffi::ecs_query(self.view_mut(), &ids);
        if !result.ok {
            return Err(map_error(result.code, result.message));
        }
        Ok(QuerySnapshot {
            entities: result.entities.into_iter().map(Entity::from).collect(),
            sizes: result.sizes,
            data: result.data,
        })
    }
}

// ---------------------------------------------------------------------------------------------
// Typed component ergonomics
// ---------------------------------------------------------------------------------------------

/// A plain-old-data Rust type that can be stored as an ECS component.
///
/// Implement this for your own `#[repr(C)]`, `#[derive(Clone, Copy, bytemuck::Pod,
/// bytemuck::Zeroable)]` structs. `NAME` is what the engine actually keys off of — it's hashed
/// into a [`ComponentId`] the same way a byte-level caller's string would be, so it must be stable
/// (changing it is a breaking change to any saved/shared world) and unique among the components
/// registered into a given [`World`].
///
/// `Pod` (from `bytemuck`) is what makes the byte view automatic: it guarantees the type has no
/// padding-sensitive invariants, no interior mutability, and no pointers, so its bytes can be
/// copied to/from the engine directly. `Send` matches [`GameLogic`](crate::GameLogic)'s own bound,
/// since components can end up read from engine worker threads.
pub trait Component: bytemuck::Pod + Send + 'static {
    const NAME: &'static str;
}

impl<'a> World<'a> {
    /// Registers `T` under [`Component::NAME`], or returns the existing id if `T` (or an equally
    /// named/sized/aligned byte-level component) is already registered. Cheap to call repeatedly —
    /// every other typed method that needs `T`'s id calls this or [`World::find`] internally.
    pub fn register<T: Component>(&mut self) -> Result<ComponentId, EcsError> {
        self.register_component(T::NAME, size_of::<T>() as u32, align_of::<T>() as u32)
    }

    /// Looks up `T`'s id, if [`Component::NAME`] has been registered (by this call, by another
    /// typed or byte-level caller, or by the engine itself).
    pub fn find<T: Component>(&self) -> Option<ComponentId> {
        self.find_component(T::NAME)
    }

    /// Reads `entity`'s `T` component. Fails with [`EcsError::UnknownComponent`] if `T` was never
    /// registered in this world (distinct from [`EcsError::MissingComponent`], which means `T` is
    /// registered but this particular entity doesn't carry it).
    pub fn get<T: Component>(&self, entity: Entity) -> Result<T, EcsError> {
        let id = self.find::<T>().ok_or(EcsError::UnknownComponent)?;
        let bytes = self.read_component(entity, id, size_of::<T>())?;
        Ok(bytemuck::pod_read_unaligned(&bytes))
    }

    /// Overwrites `entity`'s existing `T` component. Use [`World::insert`] if it might not have one
    /// yet.
    pub fn set<T: Component>(&mut self, entity: Entity, value: T) -> Result<(), EcsError> {
        let id = self.find::<T>().ok_or(EcsError::UnknownComponent)?;
        self.write_component(entity, id, bytemuck::bytes_of(&value))
    }

    /// Attaches a `T` component to `entity`, registering `T` first if this is the first time it's
    /// been seen in this world. Fails with [`EcsError::DuplicateComponent`] if `entity` already has
    /// one (use [`World::set`] to overwrite instead).
    pub fn insert<T: Component>(&mut self, entity: Entity, value: T) -> Result<(), EcsError> {
        let id = self.register::<T>()?;
        self.add_component(entity, id, bytemuck::bytes_of(&value))
    }

    /// Removes `entity`'s `T` component.
    pub fn remove<T: Component>(&mut self, entity: Entity) -> Result<(), EcsError> {
        let id = self.find::<T>().ok_or(EcsError::UnknownComponent)?;
        self.remove_component(entity, id)
    }

    /// Whether `entity` currently carries a `T` component. `false` (not an error) if `T` was never
    /// registered in this world.
    pub fn has<T: Component>(&self, entity: Entity) -> bool {
        match self.find::<T>() {
            Some(id) => self.has_component(entity, id),
            None => false,
        }
    }

    /// Spawns an entity carrying a single `T` component, registering `T` first if needed. For more
    /// than one component, use [`World::spawn`] with a `#[derive(Bundle)]` struct, or
    /// [`World::spawn_builder`].
    pub fn spawn_one<T: Component>(&mut self, value: T) -> Result<Entity, EcsError> {
        let id = self.register::<T>()?;
        self.spawn_bytes(&[(id, bytemuck::bytes_of(&value))])
    }

    /// Starts a builder for spawning an entity with several known component types at once, e.g.
    /// `world.spawn_builder().with(Position { .. }).with(Velocity { .. }).build()`.
    pub fn spawn_builder<'w>(&'w mut self) -> SpawnBuilder<'w, 'a> {
        SpawnBuilder { world: self, parts: Ok(Vec::new()) }
    }

    /// Queries every live entity carrying every component type in `Q` (a tuple of 1-12
    /// [`Component`] types, e.g. `(Position, Velocity)`), copying each one out.
    ///
    /// Replaces the old fixed-arity `query_one`/`query_two`/`query_three`: any tuple of
    /// [`Component`] types up to 12-wide implements [`QueryTuple`], so this one method covers every
    /// arity those covered and beyond. For a dynamic/reflection-driven caller with no static tuple
    /// type to name, fall back to the byte-level [`World::query_bytes`].
    pub fn query<Q: QueryTuple>(&mut self) -> Result<Vec<(Entity, Q)>, EcsError> {
        let ids = Q::component_ids(self)?;
        let snapshot = self.query_bytes(&ids)?;
        Ok(snapshot
            .entities
            .iter()
            .enumerate()
            .map(|(i, &entity)| (entity, Q::decode(&snapshot, i)))
            .collect())
    }
}

// ---------------------------------------------------------------------------------------------
// Variadic queries
// ---------------------------------------------------------------------------------------------

/// A tuple of 1-12 [`Component`] types usable with [`World::query`]. Implemented for `(A,)` through
/// `(A, B, C, D, E, F, G, H, I, J, K, L)` via an internal generator macro; not meant to be implemented by hand.
pub trait QueryTuple: Sized {
    /// Resolves every element type's [`ComponentId`], in tuple order. Fails with
    /// [`EcsError::UnknownComponent`] if any element was never registered in `world`.
    fn component_ids(world: &World<'_>) -> Result<Vec<ComponentId>, EcsError>;

    /// Decodes one matched entity's row (`entity_index` into `snapshot`) into `Self`, assuming
    /// `snapshot` was produced by querying exactly [`QueryTuple::component_ids`], in that order.
    fn decode(snapshot: &QuerySnapshot, entity_index: usize) -> Self;
}

/// Generates a [`QueryTuple`] impl for one tuple arity. Each element is given as `(Type, index)`,
/// where `index` is that element's 0-based position in the tuple (and thus in the underlying
/// [`QuerySnapshot`] row) — spelled out explicitly, rather than derived from macro repetition
/// counting, since `macro_rules!` has no built-in way to number repetitions.
macro_rules! impl_query_tuple {
    ($(($ty:ident, $idx:tt)),+ $(,)?) => {
        impl<$($ty: Component),+> QueryTuple for ($($ty,)+) {
            fn component_ids(world: &World<'_>) -> Result<Vec<ComponentId>, EcsError> {
                Ok(vec![$(world.find::<$ty>().ok_or(EcsError::UnknownComponent)?),+])
            }

            fn decode(snapshot: &QuerySnapshot, entity_index: usize) -> Self {
                ($(bytemuck::pod_read_unaligned::<$ty>(snapshot.component_bytes(entity_index, $idx)),)+)
            }
        }
    };
}

impl_query_tuple!((A, 0));
impl_query_tuple!((A, 0), (B, 1));
impl_query_tuple!((A, 0), (B, 1), (C, 2));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10));
impl_query_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11));

/// Builder for spawning an entity with several known [`Component`] types at once. Start one with
/// [`World::spawn_builder`].
///
/// Each `.with(value)` registers `value`'s type (idempotent) and stashes its bytes; `.build()`
/// makes the single byte-level [`World::spawn`] call. If any step fails (e.g. a name collision
/// with a differently-shaped byte-level component), the first error is remembered and returned
/// from `.build()` rather than panicking mid-chain.
/// One field's component id plus its already-serialized bytes, in [`Bundle::into_parts`]/
/// [`SpawnBuilder`]'s field-declaration order.
pub type ComponentParts = Vec<(ComponentId, Box<[u8]>)>;

pub struct SpawnBuilder<'w, 'a> {
    world: &'w mut World<'a>,
    parts: Result<ComponentParts, EcsError>,
}

impl<'w, 'a> SpawnBuilder<'w, 'a> {
    /// Adds a `T` component to the entity being built.
    #[must_use]
    pub fn with<T: Component>(mut self, value: T) -> Self {
        if let Ok(parts) = &mut self.parts {
            match self.world.register::<T>() {
                Ok(id) => parts.push((id, bytemuck::bytes_of(&value).to_vec().into_boxed_slice())),
                Err(error) => self.parts = Err(error),
            }
        }
        self
    }

    /// Spawns the entity with every component added via [`SpawnBuilder::with`] so far. Fails with
    /// [`EcsError::NoComponents`] if none were added.
    pub fn build(self) -> Result<Entity, EcsError> {
        let parts = self.parts?;
        let borrowed: Vec<(ComponentId, &[u8])> =
            parts.iter().map(|(id, bytes)| (*id, bytes.as_ref())).collect();
        self.world.spawn_bytes(&borrowed)
    }
}

// ---------------------------------------------------------------------------------------------
// Bundles
// ---------------------------------------------------------------------------------------------

/// A plain struct whose every field implements [`Component`], spawnable in one call via
/// [`World::spawn`]. Implement this with `#[derive(sturdy::Bundle)]` (or, with
/// `use sturdy::prelude::*;` in scope, `#[derive(Bundle)]`) rather than by hand — the derive walks
/// the struct's fields for you.
///
/// ```ignore
/// #[derive(Clone, Copy, sturdy::Bundle)]
/// struct Player {
///     position: Position,
///     velocity: Velocity,
/// }
///
/// let entity = world.spawn(Player { position, velocity })?;
/// ```
pub trait Bundle: Sized {
    /// Registers every field type's component (idempotent), without spawning anything, and
    /// returns each one's id in field-declaration order.
    fn register_all(world: &mut World<'_>) -> Result<Vec<ComponentId>, EcsError>;

    /// Registers every field type (idempotent) and returns each one's `(ComponentId, bytes)` pair,
    /// in field-declaration order, ready for [`World::spawn_bytes`].
    fn into_parts(self, world: &mut World<'_>) -> Result<ComponentParts, EcsError>;
}

impl<'a> World<'a> {
    /// Spawns an entity carrying every field of `bundle` as its own component, registering any
    /// field type not yet seen in this world. See [`Bundle`].
    pub fn spawn<B: Bundle>(&mut self, bundle: B) -> Result<Entity, EcsError> {
        let parts = bundle.into_parts(self)?;
        let borrowed: Vec<(ComponentId, &[u8])> =
            parts.iter().map(|(id, bytes)| (*id, bytes.as_ref())).collect();
        self.spawn_bytes(&borrowed)
    }

    /// Attaches every field of `bundle` to `entity` as its own component, registering any field
    /// type not yet seen in this world. Fails with [`EcsError::DuplicateComponent`] on the first
    /// field type `entity` already carries.
    pub fn insert_bundle<B: Bundle>(&mut self, entity: Entity, bundle: B) -> Result<(), EcsError> {
        let parts = bundle.into_parts(self)?;
        for (id, bytes) in parts {
            self.add_component(entity, id, &bytes)?;
        }
        Ok(())
    }

    /// Removes every one of `B`'s field-component types from `entity`. Registers `B` first (so it
    /// is not an error to call this on a world `B` was never spawned into). Fails with
    /// [`EcsError::MissingComponent`] on the first field type `entity` doesn't carry, matching
    /// [`World::remove`]'s per-type behavior.
    pub fn remove_bundle<B: Bundle>(&mut self, entity: Entity) -> Result<(), EcsError> {
        let ids = B::register_all(self)?;
        for id in ids {
            self.remove_component(entity, id)?;
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Event channels
// ---------------------------------------------------------------------------------------------

/// A handle to a bound event channel, resolved once via [`World::create_event_channel`] and cheap
/// to copy around afterward. Distinct type from [`ResourceId`] even though it's the same
/// `(high, low)` name-derived key underneath, so a channel handle and a resource handle can't be
/// swapped by accident at a call site.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EventChannelId {
    pub high: u64,
    pub low: u64,
}

impl From<ffi::ResourceId> for EventChannelId {
    fn from(id: ffi::ResourceId) -> Self {
        Self { high: id.high, low: id.low }
    }
}

impl From<EventChannelId> for ffi::ResourceId {
    fn from(id: EventChannelId) -> Self {
        ffi::ResourceId { high: id.high, low: id.low }
    }
}

/// A plain-old-data Rust type that can be sent through an event channel. Same shape as
/// [`Component`]: implement it for your own `#[repr(C)]`, `bytemuck::Pod` structs, with `NAME`
/// unique among the event channels registered into a given [`World`] (it's what the channel's key
/// is hashed from, same as [`Component::NAME`]).
pub trait Event: bytemuck::Pod + Send + 'static {
    const NAME: &'static str;
}

impl<'a> World<'a> {
    /// Binds a byte-level event channel under `name`, sized for `element_size`-byte records.
    /// Idempotent: re-binding under the same name with the same size returns the existing channel.
    pub fn create_event_channel_bytes(&mut self, name: &str, element_size: u32) -> Result<EventChannelId, EcsError> {
        let result = ffi::ecs_create_event_channel(self.view_mut(), name, element_size);
        if result.ok { Ok(result.id.into()) } else { Err(map_error(result.code, result.message)) }
    }

    /// Unbinds an event channel this binding created, freeing its storage.
    pub fn destroy_event_channel(&mut self, channel: EventChannelId) -> Result<(), EcsError> {
        status_result(ffi::ecs_destroy_event_channel(self.view_mut(), channel.into()))
    }

    /// Appends one event's bytes to the channel. `data` must be exactly the channel's element size.
    pub fn send_event_bytes(&mut self, channel: EventChannelId, data: &[u8]) -> Result<(), EcsError> {
        status_result(ffi::ecs_send_event(self.view_mut(), channel.into(), data))
    }

    /// Drains every event currently buffered in the channel and returns their concatenated bytes
    /// plus how many records that is. See [`World::read_events`] for why this call *clears* the
    /// channel as it reads (drain-on-read): this binding layer never runs the engine's own
    /// `Ecs::Schedule`, which is what normally clears event buffers between runs, so nothing else
    /// will ever clear a channel bound through this API.
    pub fn read_events_bytes(&mut self, channel: EventChannelId) -> Result<(Vec<u8>, u32), EcsError> {
        let result = ffi::ecs_read_events(self.view_mut(), channel.into());
        if result.ok { Ok((result.data, result.count)) } else { Err(map_error(result.code, result.message)) }
    }

    /// Discards every buffered event without returning them.
    pub fn clear_event_channel(&mut self, channel: EventChannelId) -> Result<(), EcsError> {
        status_result(ffi::ecs_clear_event_channel(self.view_mut(), channel.into()))
    }

    /// Binds a typed event channel for `T`, registering it under [`Event::NAME`] if this is the
    /// first time it's been seen in this world.
    pub fn create_event_channel<T: Event>(&mut self) -> Result<EventChannelId, EcsError> {
        self.create_event_channel_bytes(T::NAME, size_of::<T>() as u32)
    }

    /// Sends a `T` event into `channel`.
    pub fn send_event<T: Event>(&mut self, channel: EventChannelId, event: T) -> Result<(), EcsError> {
        self.send_event_bytes(channel, bytemuck::bytes_of(&event))
    }

    /// Drains every `T` event currently buffered in `channel`, in send order. See
    /// [`World::read_events_bytes`] for the drain-on-read semantics: calling this twice in a row
    /// with no [`World::send_event`] in between returns a full batch, then an empty one.
    pub fn read_events<T: Event>(&mut self, channel: EventChannelId) -> Result<Vec<T>, EcsError> {
        let (data, count) = self.read_events_bytes(channel)?;
        let element_size = size_of::<T>();
        Ok((0..count as usize)
            .map(|i| bytemuck::pod_read_unaligned(&data[i * element_size..(i + 1) * element_size]))
            .collect())
    }
}

// ---------------------------------------------------------------------------------------------
// Typed resources
// ---------------------------------------------------------------------------------------------

/// A plain-old-data Rust type that can be bound as an ECS resource. Same shape as [`Component`]
/// and [`Event`]: implement it for your own `#[repr(C)]`, `bytemuck::Pod` structs, with `NAME`
/// unique among the resources bound into a given [`World`].
pub trait Resource: bytemuck::Pod + Send + 'static {
    const NAME: &'static str;
}

impl<'a> World<'a> {
    /// The key [`Resource::NAME`] hashes to, without touching the world — the typed counterpart to
    /// [`World::resource_key`].
    pub fn resource_key_for<T: Resource>() -> ResourceId {
        Self::resource_key(T::NAME)
    }

    /// Binds a `T` resource under [`Resource::NAME`], initialized to `value`. Re-binding under the
    /// same name with the same size is a no-op, matching [`World::create_resource`].
    pub fn insert_resource<T: Resource>(&mut self, value: T) -> Result<ResourceId, EcsError> {
        self.create_resource(T::NAME, bytemuck::bytes_of(&value))
    }

    /// Copies a bound `T` resource's value out.
    pub fn resource<T: Resource>(&mut self, id: ResourceId) -> Result<T, EcsError> {
        let bytes = self.get_resource(id, size_of::<T>())?;
        Ok(bytemuck::pod_read_unaligned(&bytes))
    }

    /// Overwrites a bound `T` resource's value in place.
    pub fn set_resource_typed<T: Resource>(&mut self, id: ResourceId, value: T) -> Result<(), EcsError> {
        self.set_resource(id, bytemuck::bytes_of(&value))
    }
}
