//! Real, parallel ECS systems, registered once and run automatically by the engine's own scheduler
//! (`SFT::Ecs::Schedule`) every tick — distinct from the ad-hoc [`World`] access in `ecs.rs`, which
//! a caller drives itself from [`GameLogic::frame`](crate::GameLogic::frame).
//!
//! The engine already calls `Schedule::run` for you, twice per frame at two different points
//! (`Engine::update_schedule()` inside `Engine::update`, `Engine::render_extraction_schedule()`
//! during frame preparation — see [`ScheduleTarget`]); this module only ever *registers* into one
//! of those, typically once from [`GameLogic::init`](crate::GameLogic::init).
//!
//! ## Byte-level vs. typed
//!
//! Like `ecs.rs`, this stays byte-level at the FFI boundary — the C++ side hands a system's body a
//! `void **components` array, so [`World::add_system_bytes`]/[`World::add_global_system_bytes`]
//! hand you `&mut [&mut [u8]]` — and builds a typed convenience layer on top for the common case of
//! 1-12 [`Component`] types, via the [`R`]/[`W`] marker types and the [`SystemTuple`] trait
//! (implemented for tuples the same way `QueryTuple` is in `ecs.rs`), plus [`World::add_global_system`]
//! for 1-12 [`Resource`] types via [`ResourceTuple`].
//!
//! Unlike the byte-level primitive (which takes an explicit [`Access`] — it has no static type
//! information to derive one from, mirroring the C++ erased API it wraps), the typed
//! [`World::add_system`]/[`World::add_global_system`] derive their `SystemAccess` *and* the
//! component/resource delivery order directly from the tuple type argument, the same way the
//! engine's own native C++ `Schedule::add_system<F>` deduces access from a callable's parameter
//! types. This is a deliberate difference from pairing a separately-built `Access` object with a
//! same-order closure: with two independent values that must agree, a caller could get the order or
//! read/write-ness wrong with no compiler check (exactly the "one way to defeat the scheduler's
//! safety" the C++ docs warn about). Deriving both from one tuple type makes that mismatch
//! unrepresentable.
//!
//! ## Ownership and lifetime
//!
//! `SFT::Ecs::Schedule` has no `remove_system` (confirmed by reading its full public interface —
//! see `sturdy-sys/src/schedule.rs`'s module doc). A system registered here runs every tick for the
//! rest of the process; there is no unregister API because the engine itself has none. Registering
//! the same logical system twice (e.g. by calling `init` more than once) would run it twice per
//! tick — this module does not deduplicate, matching the C++ layer it wraps.
//!
//! ## Panic safety
//!
//! Every system body ultimately runs from inside `Schedule::run`, on a scheduler worker thread,
//! potentially concurrently with other systems' bodies. `sturdy_sys::schedule`'s trampolines already
//! wrap every call in `catch_unwind` + log + `abort()` (see its module doc); nothing further is
//! needed here.
//!
//! ## Out of scope
//!
//! - **Event access from inside a system.** [`Access`] cannot declare event reads/writes: the
//!   erased `ErasedSystemFn` C++ API gives a system body no way to actually drain an event channel
//!   (only resource pointers are resolved for delivery, and only for global systems; see
//!   `sturdy-sys/src/schedule.rs`'s module doc for why). Use the existing ad-hoc
//!   `World::read_events` from [`GameLogic::frame`] instead.
//! - **Ordering/dependency control beyond conflict detection.** The engine's own scheduler decides
//!   what can run concurrently purely from declared access; there is no "run after system X" hook
//!   to bind to, so none is exposed here either.
//!
//! ## `Commands`: deferred structural changes from inside a system
//!
//! A system body only ever sees the bytes of the components/resources it declared access to — it
//! cannot spawn, destroy, or add/remove components directly, the same restriction the C++ side has
//! (a schedule holds the world for its whole run). Every system body (byte-level or typed, entity
//! or global) is therefore always handed a [`Commands`] as its last argument: queue work on it and
//! the engine applies it once `Schedule::run` finishes every system in that dispatch (see
//! `SFT::Ecs::Commands`'s own doc comment). [`Commands`] only exposes the byte-level/erased
//! operations (`spawn`/`add_component`/`remove_component`/`destroy`) — unlike [`World`], it cannot
//! offer a `T: Component`-generic API, since resolving `T`'s [`ComponentId`] needs [`World::register`],
//! which needs the world exclusively (not safely callable from a scheduler worker mid-run). Resolve
//! any [`ComponentId`]s you'll need up front (e.g. in [`GameLogic::init`](crate::GameLogic::init),
//! via [`World::register`]) and capture them into the system closure instead.

use core::marker::PhantomData;
use core::mem::size_of;

use sturdy_sys::ecs::ffi as ecs_ffi;
use sturdy_sys::schedule::ffi as sched_ffi;
use sturdy_sys::schedule::{RustEntitySystem, RustGlobalSystem};

use crate::ecs::{status_result, Component, ComponentId, EcsError, Entity, Resource, ResourceId, World};

/// A system dispatch's deferred spawn/despawn/add-component/remove-component queue, handed to
/// every system body as its last argument (see the module doc's "`Commands`" section for why the
/// `T: Component`-generic convenience [`World`] offers isn't available here). Applied by the engine
/// once `Schedule::run` finishes every system in this dispatch — never immediately, so reading back
/// a change queued here within the same dispatch will not see it.
///
/// Borrowed for exactly the duration of one system-body call: the `'a` lifetime ties every method
/// call to that call's stack frame, so a `Commands` can't be stashed away and used later (which
/// would dangle once the dispatch's own `Commands` handle destructs in C++).
pub struct Commands<'a> {
    ptr: *mut sched_ffi::Commands,
    _marker: PhantomData<&'a mut sched_ffi::Commands>,
}

impl<'a> Commands<'a> {
    /// # Safety
    /// `ptr` must be a live `sched_ffi::Commands` for the duration of `'a`, and this must be the
    /// only `Commands` wrapper referencing it during that time (upheld by
    /// `sturdy_sys::schedule::rust_{entity,global}_system_run`'s own trampolines, which mint one
    /// fresh wrapper per call and never retain the pointer past it).
    unsafe fn from_raw(ptr: *mut sched_ffi::Commands) -> Self {
        Self { ptr, _marker: PhantomData }
    }

    /// Queues creation of an entity carrying the given components (each entry's `id` must be
    /// exactly `data`'s length in bytes — a mismatch, or an empty slice, is silently dropped by the
    /// engine when this dispatch's commands are applied, matching every other `Commands` operation's
    /// "no caller left to report a failure to" contract).
    pub fn spawn(&mut self, components: &[(ComponentId, &[u8])]) {
        let ids: Vec<u32> = components.iter().map(|(id, _)| id.0).collect();
        let sizes: Vec<u32> = components.iter().map(|(_, data)| data.len() as u32).collect();
        let data: Vec<u8> = components.iter().flat_map(|(_, data)| data.iter().copied()).collect();
        unsafe { sched_ffi::commands_spawn(self.ptr, &ids, &sizes, &data) };
    }

    /// Queues creation of an entity carrying a single component. Sugar for [`Commands::spawn`] with
    /// one entry.
    pub fn spawn_one<T: Component>(&mut self, id: ComponentId, value: T) {
        self.spawn(&[(id, bytemuck::bytes_of(&value))]);
    }

    /// Queues attaching `component` to `entity`. `data` must be exactly `component`'s registered
    /// size (silently dropped otherwise, same reasoning as [`Commands::spawn`]).
    pub fn add_component(&mut self, entity: Entity, component: ComponentId, data: &[u8]) {
        unsafe { sched_ffi::commands_add_component(self.ptr, entity.into(), component.0, data) };
    }

    /// Queues attaching a typed `T` component to `entity`. `id` must be `T`'s already-resolved id
    /// (see the module doc's "`Commands`" section for why this can't resolve `T` itself).
    pub fn insert<T: Component>(&mut self, entity: Entity, id: ComponentId, value: T) {
        self.add_component(entity, id, bytemuck::bytes_of(&value));
    }

    /// Queues detaching `component` from `entity`.
    pub fn remove_component(&mut self, entity: Entity, component: ComponentId) {
        unsafe { sched_ffi::commands_remove_component(self.ptr, entity.into(), component.0) };
    }

    /// Queues destroying `entity`.
    pub fn destroy(&mut self, entity: Entity) {
        unsafe { sched_ffi::commands_destroy(self.ptr, entity.into()) };
    }
}

/// Which of the engine's two built-in schedules to register a system into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleTarget {
    /// `Engine::update_schedule()`, run once per `Engine::update` (game-logic tick).
    Update,
    /// `Engine::render_extraction_schedule()`, run once per frame during render preparation —
    /// meant for systems that turn gameplay state into render-ready data.
    RenderExtraction,
}

impl From<ScheduleTarget> for sched_ffi::ScheduleTarget {
    fn from(target: ScheduleTarget) -> Self {
        match target {
            ScheduleTarget::Update => sched_ffi::ScheduleTarget::Update,
            ScheduleTarget::RenderExtraction => sched_ffi::ScheduleTarget::RenderExtraction,
        }
    }
}

fn status_result_from_ecs(status: ecs_ffi::EcsStatus) -> Result<(), EcsError> {
    status_result(status)
}

/// Declares exactly what a byte-level system touches, for the scheduler's own conflict detection
/// (see `Ecs/src/Ecs/System.hpp`'s `SystemAccess` doc comment: understating this is "the one way to
/// defeat the scheduler's safety"). Used by [`World::add_system_bytes`]/
/// [`World::add_global_system_bytes`] only — the typed [`World::add_system`]/
/// [`World::add_global_system`] derive an equivalent access set automatically from their tuple type
/// argument, so a caller of those never builds one of these by hand.
///
/// Call order matters beyond conflict detection: it also fixes the order components/resources are
/// delivered to the byte-level closure (index `i` here is `slices[i]` in the closure).
#[derive(Default)]
pub struct Access {
    components: Vec<AccessComponent>,
    resources: Vec<AccessResource>,
}

/// See the comment on [`AccessComponent::register`] for why this can't be a plain fn pointer.
type RegisterFn = Box<dyn for<'w> Fn(&mut World<'w>) -> Result<ComponentId, EcsError> + Send + Sync>;

struct AccessComponent {
    // A plain `fn(&mut World<'_>) -> ...` pointer doesn't work here: `World::register::<T>` is
    // generic over both the method's own borrow and `World`'s lifetime parameter, and rustc cannot
    // coerce a fn item with two independent lifetime parameters into the fully higher-ranked fn
    // pointer type this field would need (`for<'w, 'a> fn(&'w mut World<'a>) -> ...`) — only one of
    // the two gets generalized. A boxed closure with an explicit `for<'a>` bound does work, since
    // trait objects support higher-ranked bounds directly.
    register: RegisterFn,
    size: usize,
    write: bool,
}

struct AccessResource {
    id: ResourceId,
    size: usize,
    write: bool,
}

impl Access {
    pub fn new() -> Self {
        Self::default()
    }

    /// Declares a read of component `T`, and that it is next in the byte-level delivery order.
    #[must_use]
    pub fn reads<T: Component>(mut self) -> Self {
        self.components.push(AccessComponent {
            register: Box::new(|world| world.register::<T>()),
            size: size_of::<T>(),
            write: false,
        });
        self
    }

    /// Declares a write of component `T`, and that it is next in the byte-level delivery order.
    #[must_use]
    pub fn writes<T: Component>(mut self) -> Self {
        self.components.push(AccessComponent {
            register: Box::new(|world| world.register::<T>()),
            size: size_of::<T>(),
            write: true,
        });
        self
    }

    /// Declares a read of resource `T`, and that it is next in [`World::add_global_system_bytes`]'s
    /// delivery order.
    #[must_use]
    pub fn reads_resource<T: Resource>(mut self) -> Self {
        self.resources.push(AccessResource { id: World::resource_key_for::<T>(), size: size_of::<T>(), write: false });
        self
    }

    /// Declares a write of resource `T`.
    #[must_use]
    pub fn writes_resource<T: Resource>(mut self) -> Self {
        self.resources.push(AccessResource { id: World::resource_key_for::<T>(), size: size_of::<T>(), write: true });
        self
    }

    /// Resolves every declared component's id (registering it if needed) and builds the C++-facing
    /// `SystemAccessSpec`, plus the ordered `(id, size)` pairs a byte-level entity system's closure
    /// needs (in call order — the same order `component_ids` is sent to the engine in, so it lines
    /// up with what `void **components` will actually contain).
    fn resolve_for_entity_system(
        &self,
        world: &mut World<'_>,
    ) -> Result<(sched_ffi::SystemAccessSpec, Vec<u32>, Vec<usize>), EcsError> {
        let mut spec = sched_ffi::SystemAccessSpec::default();
        let mut ids = Vec::with_capacity(self.components.len());
        let mut sizes = Vec::with_capacity(self.components.len());
        for component in &self.components {
            let id = (component.register)(world)?;
            ids.push(id.0);
            sizes.push(component.size);
            if component.write { spec.writes.push(id.0) } else { spec.reads.push(id.0) }
        }
        for resource in self.resources.iter().filter(|r| !r.write) {
            spec.resource_reads_high.push(resource.id.high);
            spec.resource_reads_low.push(resource.id.low);
        }
        for resource in self.resources.iter().filter(|r| r.write) {
            spec.resource_writes_high.push(resource.id.high);
            spec.resource_writes_low.push(resource.id.low);
        }
        Ok((spec, ids, sizes))
    }

    /// Same as [`Access::resolve_for_entity_system`] but for a global system: no components (must
    /// be empty), and the delivery order/sizes come from `resources` (reads then writes — matching
    /// how `schedule.cpp`'s trampoline resolves pointers from `SystemAccessSpec`).
    fn resolve_for_global_system(&self) -> Result<(sched_ffi::SystemAccessSpec, Vec<usize>), EcsError> {
        if !self.components.is_empty() {
            return Err(EcsError::InvalidArgument(
                "a global system's Access must not declare component reads/writes".into(),
            ));
        }
        let mut spec = sched_ffi::SystemAccessSpec::default();
        let mut sizes = Vec::new();
        for resource in self.resources.iter().filter(|r| !r.write) {
            spec.resource_reads_high.push(resource.id.high);
            spec.resource_reads_low.push(resource.id.low);
            sizes.push(resource.size);
        }
        for resource in self.resources.iter().filter(|r| r.write) {
            spec.resource_writes_high.push(resource.id.high);
            spec.resource_writes_low.push(resource.id.low);
            sizes.push(resource.size);
        }
        if sizes.is_empty() {
            return Err(EcsError::NoComponents);
        }
        Ok((spec, sizes))
    }
}

impl<'a> World<'a> {
    /// Registers a byte-level per-entity system into `target`, run once per matching entity for the
    /// rest of the process (see the module doc for why there is no removal). `access` both declares
    /// conflict-detection info and fixes the order components are delivered to `body`.
    pub fn add_system_bytes(
        &mut self,
        target: ScheduleTarget,
        access: &Access,
        mut body: impl FnMut(Entity, &mut [&mut [u8]], &mut Commands<'_>) + Send + 'static,
    ) -> Result<(), EcsError> {
        let (spec, ids, sizes) = access.resolve_for_entity_system(self)?;
        if ids.is_empty() {
            return Err(EcsError::NoComponents);
        }

        let system = RustEntitySystem {
            sizes,
            body: Box::new(move |entity: ecs_ffi::EntityId, slices: &mut [&mut [u8]], commands| {
                let mut commands = unsafe { Commands::from_raw(commands) };
                body(entity.into(), slices, &mut commands);
            }),
        };
        let leaked: *mut RustEntitySystem = Box::into_raw(Box::new(system));
        let status =
            unsafe { sched_ffi::schedule_add_entity_system(self.view_mut(), target.into(), &spec, &ids, leaked) };
        status_result_from_ecs(status)
    }

    /// Registers a byte-level global (resource-only) system into `target`, run exactly once per
    /// dispatch. `access`'s resource reads then writes (in declaration order) are what `body`
    /// receives; a resource not currently bound skips that dispatch (logged once by the trampoline)
    /// rather than risking a null view.
    pub fn add_global_system_bytes(
        &mut self,
        target: ScheduleTarget,
        access: &Access,
        mut body: impl FnMut(&mut [&mut [u8]], &mut Commands<'_>) + Send + 'static,
    ) -> Result<(), EcsError> {
        let (spec, sizes) = access.resolve_for_global_system()?;
        let system = RustGlobalSystem {
            sizes,
            body: Box::new(move |slices: &mut [&mut [u8]], commands| {
                let mut commands = unsafe { Commands::from_raw(commands) };
                body(slices, &mut commands);
            }),
        };
        let leaked: *mut RustGlobalSystem = Box::into_raw(Box::new(system));
        let status = unsafe { sched_ffi::schedule_add_global_system(self.view_mut(), target.into(), &spec, leaked) };
        status_result_from_ecs(status)
    }
}

// ---------------------------------------------------------------------------------------------
// Typed per-entity systems: R<T>/W<T> markers + SystemTuple, mirroring ecs.rs's QueryTuple macro.
// ---------------------------------------------------------------------------------------------

/// Marks a typed system parameter as a read of component `T` (`&T` in the closure).
pub struct R<T>(PhantomData<T>);
/// Marks a typed system parameter as a write of component `T` (`&mut T` in the closure).
pub struct W<T>(PhantomData<T>);

/// One typed system parameter: a [`R<T>`] (read) or [`W<T>`] (write) over a [`Component`] type.
/// Implemented only by those two marker types; not meant to be implemented by hand.
pub trait SystemComponent {
    type Component: Component;
    type View<'a>;
    const WRITE: bool;

    /// # Safety
    /// `bytes` must be exactly `size_of::<Self::Component>()` long and hold a valid, initialized
    /// `Self::Component`.
    unsafe fn view<'a>(bytes: &'a mut [u8]) -> Self::View<'a>;
}

impl<T: Component> SystemComponent for R<T> {
    type Component = T;
    type View<'a> = &'a T;
    const WRITE: bool = false;
    unsafe fn view(bytes: &mut [u8]) -> &T {
        bytemuck::from_bytes(bytes)
    }
}

impl<T: Component> SystemComponent for W<T> {
    type Component = T;
    type View<'a> = &'a mut T;
    const WRITE: bool = true;
    unsafe fn view(bytes: &mut [u8]) -> &mut T {
        bytemuck::from_bytes_mut(bytes)
    }
}

/// A tuple of 1-12 [`SystemComponent`]s (`R<T>`/`W<T>`) usable with [`World::add_system`].
/// Implemented for `(A,)` through `(A, B, C, D)` via an internal macro; not meant to be implemented
/// by hand. Mirrors [`crate::ecs::QueryTuple`]'s shape, but describes read/write access (for the
/// scheduler) rather than always-owned-copy query results.
pub trait SystemTuple {
    type Views<'a>;

    /// Registers every element's component type and returns the resolved `SystemAccessSpec`
    /// together with the component ids and byte sizes *in tuple declaration order* — the order
    /// `World::add_system` sends to the engine as `component_ids`, and the order `decode` below
    /// expects `slices` back in. (Not reconstructible from `SystemAccessSpec` alone in general,
    /// since that splits into separate reads/writes lists — e.g. `(W<A>, R<B>)` would otherwise be
    /// ambiguous — so this returns the tuple order explicitly instead.)
    fn describe(world: &mut World<'_>) -> Result<(sched_ffi::SystemAccessSpec, Vec<u32>, Vec<usize>), EcsError>;

    /// Decodes one entity's component byte slices (in the same order `describe` returned) into
    /// typed views.
    ///
    /// # Safety
    /// `slices` must have one entry per tuple element, each exactly that element's component size.
    unsafe fn decode<'a>(slices: &mut [&'a mut [u8]]) -> Self::Views<'a>;
}

macro_rules! impl_system_tuple {
    ($(($ty:ident, $idx:tt)),+ $(,)?) => {
        impl<$($ty: SystemComponent),+> SystemTuple for ($($ty,)+) {
            type Views<'a> = ($($ty::View<'a>,)+);

            fn describe(world: &mut World<'_>) -> Result<(sched_ffi::SystemAccessSpec, Vec<u32>, Vec<usize>), EcsError> {
                let mut spec = sched_ffi::SystemAccessSpec::default();
                let mut ids = Vec::new();
                let mut sizes = Vec::new();
                $(
                    let id = world.register::<$ty::Component>()?;
                    ids.push(id.0);
                    sizes.push(size_of::<$ty::Component>());
                    if $ty::WRITE { spec.writes.push(id.0) } else { spec.reads.push(id.0) }
                )+
                Ok((spec, ids, sizes))
            }

            unsafe fn decode<'a>(slices: &mut [&'a mut [u8]]) -> Self::Views<'a> {
                let mut iter = slices.iter_mut();
                ($(
                    {
                        let _ = $idx;
                        let bytes = core::mem::take(iter.next().expect("slice count matches tuple arity"));
                        unsafe { $ty::view(bytes) }
                    },
                )+)
            }
        }
    };
}

impl_system_tuple!((A, 0));
impl_system_tuple!((A, 0), (B, 1));
impl_system_tuple!((A, 0), (B, 1), (C, 2));
impl_system_tuple!((A, 0), (B, 1), (C, 2), (D, 3));
impl_system_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_system_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_system_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_system_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7));
impl_system_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8));
impl_system_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9));
impl_system_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10));
impl_system_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11));

impl<'a> World<'a> {
    /// Registers a typed per-entity system into `target`: `Q` (e.g. `(R<Position>, W<Velocity>)`)
    /// both derives the `SystemAccess`/component order the scheduler needs and the closure's own
    /// argument types, so the two can never disagree (see the module doc for why that matters).
    ///
    /// Runs once per entity carrying every component in `Q`, for the rest of the process.
    pub fn add_system<Q, F>(&mut self, target: ScheduleTarget, mut body: F) -> Result<(), EcsError>
    where
        Q: SystemTuple + 'static,
        F: for<'e> FnMut(Entity, Q::Views<'e>, &mut Commands<'_>) + Send + 'static,
    {
        let (spec, ids, sizes) = Q::describe(self)?;
        let system = RustEntitySystem {
            sizes,
            body: Box::new(move |entity: ecs_ffi::EntityId, slices: &mut [&mut [u8]], commands| {
                let views = unsafe { Q::decode(slices) };
                let mut commands = unsafe { Commands::from_raw(commands) };
                body(entity.into(), views, &mut commands);
            }),
        };
        let leaked: *mut RustEntitySystem = Box::into_raw(Box::new(system));
        let status =
            unsafe { sched_ffi::schedule_add_entity_system(self.view_mut(), target.into(), &spec, &ids, leaked) };
        status_result_from_ecs(status)
    }
}

// ---------------------------------------------------------------------------------------------
// Typed global systems: same R<T>/W<T> markers reused over Resource instead of Component.
// ---------------------------------------------------------------------------------------------

/// One typed global-system parameter: a [`R<T>`] (read) or [`W<T>`] (write) over a [`Resource`]
/// type. Reuses the [`R`]/[`W`] marker types from the per-entity path (they're bare `PhantomData`
/// wrappers with no `Component`/`Resource`-specific state), but through this separate trait, since a
/// [`Resource`]'s key is resolved by name hash alone (no `World::register` needed).
pub trait SystemResource {
    type Resource: Resource;
    type View<'a>;
    const WRITE: bool;

    /// # Safety
    /// `bytes` must be exactly `size_of::<Self::Resource>()` long and hold a valid, initialized
    /// `Self::Resource`.
    unsafe fn view<'a>(bytes: &'a mut [u8]) -> Self::View<'a>;
}

impl<T: Resource> SystemResource for R<T> {
    type Resource = T;
    type View<'a> = &'a T;
    const WRITE: bool = false;
    unsafe fn view(bytes: &mut [u8]) -> &T {
        bytemuck::from_bytes(bytes)
    }
}

impl<T: Resource> SystemResource for W<T> {
    type Resource = T;
    type View<'a> = &'a mut T;
    const WRITE: bool = true;
    unsafe fn view(bytes: &mut [u8]) -> &mut T {
        bytemuck::from_bytes_mut(bytes)
    }
}

/// A tuple of 1-12 [`SystemResource`]s (`R<T>`/`W<T>`) usable with [`World::add_global_system`].
/// Implemented for `(A,)` through `(A, B, C, D)` via an internal macro; not meant to be implemented
/// by hand.
pub trait ResourceTuple {
    type Views<'a>;

    /// Builds the resolved `SystemAccessSpec` and each element's byte size, in tuple declaration
    /// order (reads before writes within the C++ trampoline's own delivery order — see
    /// `schedule.cpp`'s `global_system_trampoline`, which resolves `resource_reads` then
    /// `resource_writes` regardless of original declaration order). Because of that C++-side
    /// reordering, `describe` below builds `spec` the same way `Access::resolve_for_global_system`
    /// does (reads first, writes second) so this tuple's *element* order must match: put every
    /// `R<T>` before every `W<T>` when using this typed path, or the compile-time view types will
    /// silently pair with the wrong bytes. `add_global_system`'s doc comment repeats this.
    fn describe(world: &World<'_>) -> Result<(sched_ffi::SystemAccessSpec, Vec<usize>), EcsError>;

    /// # Safety
    /// `slices` must have one entry per tuple element, each exactly that element's resource size,
    /// in the reads-then-writes order `describe` produced.
    unsafe fn decode<'a>(slices: &mut [&'a mut [u8]]) -> Self::Views<'a>;
}

macro_rules! impl_resource_tuple {
    ($(($ty:ident, $idx:tt)),+ $(,)?) => {
        impl<$($ty: SystemResource),+> ResourceTuple for ($($ty,)+) {
            type Views<'a> = ($($ty::View<'a>,)+);

            fn describe(_world: &World<'_>) -> Result<(sched_ffi::SystemAccessSpec, Vec<usize>), EcsError> {
                let mut spec = sched_ffi::SystemAccessSpec::default();
                $(
                    let key = World::resource_key_for::<$ty::Resource>();
                    if $ty::WRITE {
                        spec.resource_writes_high.push(key.high);
                        spec.resource_writes_low.push(key.low);
                    } else {
                        spec.resource_reads_high.push(key.high);
                        spec.resource_reads_low.push(key.low);
                    }
                )+
                // Delivery order from the trampoline is reads-then-writes; sizes must match that,
                // not tuple declaration order (see the trait doc comment).
                let mut sizes = Vec::new();
                $(
                    if !$ty::WRITE { sizes.push(size_of::<$ty::Resource>()); }
                )+
                $(
                    if $ty::WRITE { sizes.push(size_of::<$ty::Resource>()); }
                )+
                Ok((spec, sizes))
            }

            unsafe fn decode<'a>(slices: &mut [&'a mut [u8]]) -> Self::Views<'a> {
                // Reads-then-writes order coming in; peel reads first, then writes, then reassemble
                // into tuple-declaration order for the caller's closure.
                let mut reads: Vec<&'a mut [u8]> = Vec::new();
                let mut writes: Vec<&'a mut [u8]> = Vec::new();
                let mut iter = slices.iter_mut();
                $(
                    if !$ty::WRITE {
                        reads.push(core::mem::take(iter.next().expect("read slice present")));
                    }
                )+
                $(
                    if $ty::WRITE {
                        writes.push(core::mem::take(iter.next().expect("write slice present")));
                    }
                )+
                let mut reads = reads.into_iter();
                let mut writes = writes.into_iter();
                ($(
                    {
                        let _ = $idx;
                        if $ty::WRITE {
                            unsafe { $ty::view(writes.next().expect("write slice present")) }
                        } else {
                            unsafe { $ty::view(reads.next().expect("read slice present")) }
                        }
                    },
                )+)
            }
        }
    };
}

impl_resource_tuple!((A, 0));
impl_resource_tuple!((A, 0), (B, 1));
impl_resource_tuple!((A, 0), (B, 1), (C, 2));
impl_resource_tuple!((A, 0), (B, 1), (C, 2), (D, 3));
impl_resource_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4));
impl_resource_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5));
impl_resource_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6));
impl_resource_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7));
impl_resource_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8));
impl_resource_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9));
impl_resource_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10));
impl_resource_tuple!((A, 0), (B, 1), (C, 2), (D, 3), (E, 4), (F, 5), (G, 6), (H, 7), (I, 8), (J, 9), (K, 10), (L, 11));

impl<'a> World<'a> {
    /// Registers a typed global (resource-only) system into `target`: `R`/`W` (e.g.
    /// `(W<ScoreCounter>,)`) derives the `SystemAccess` the scheduler needs and the closure's
    /// argument types together, the same way [`World::add_system`] does for components.
    ///
    /// Runs exactly once per dispatch, for the rest of the process. If a declared resource is not
    /// currently bound, that dispatch is skipped (logged once by the trampoline) rather than
    /// invoking `body` with a dangling view.
    pub fn add_global_system<Q, F>(&mut self, target: ScheduleTarget, mut body: F) -> Result<(), EcsError>
    where
        Q: ResourceTuple + 'static,
        F: for<'e> FnMut(Q::Views<'e>, &mut Commands<'_>) + Send + 'static,
    {
        let (spec, sizes) = Q::describe(self)?;
        let system = RustGlobalSystem {
            sizes,
            body: Box::new(move |slices: &mut [&mut [u8]], commands| {
                let views = unsafe { Q::decode(slices) };
                let mut commands = unsafe { Commands::from_raw(commands) };
                body(views, &mut commands);
            }),
        };
        let leaked: *mut RustGlobalSystem = Box::into_raw(Box::new(system));
        let status = unsafe { sched_ffi::schedule_add_global_system(self.view_mut(), target.into(), &spec, leaked) };
        status_result_from_ecs(status)
    }
}
