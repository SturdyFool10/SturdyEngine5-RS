//! Raw `cxx` bindings for registering real, parallel Rust ECS systems into the engine's own
//! `SFT::Ecs::Schedule` (see `Ecs/src/Ecs/System.hpp`), so they run automatically every tick from
//! inside the engine's scheduler — distinct from the ad-hoc `World` access in `ecs.rs`, which a
//! caller drives itself from `GameLogic::frame`.
//!
//! ## Why this needs its own trampolines, not just `ecs.rs`'s query pattern
//!
//! `Schedule::add_erased_system`/`add_erased_global_system` take a bare C function pointer
//! (`ErasedSystemFn`, `noexcept`) plus a `void *user_data` — there is no `std::function`, so a Rust
//! closure cannot be registered directly. Instead: one C++ trampoline per system kind
//! (`entity_system_trampoline`/`global_system_trampoline` in `schedule.cpp`) is registered with the
//! engine; `user_data` points at a leaked `Box<RustEntitySystem>`/`Box<RustGlobalSystem>` holding
//! the real Rust closure. The trampoline calls back into Rust (`rust_entity_system_run`/
//! `rust_global_system_run` below) on every dispatch.
//!
//! ## Ownership: registered systems live for the engine's lifetime
//!
//! `SFT::Ecs::Schedule` has no `remove_system` — confirmed by reading its full public interface
//! (`add_system`, `add_erased_system`, `add_erased_global_system`, `run`, and nothing else). A
//! system registered once runs every tick for as long as the process does. This binding matches
//! that shape rather than inventing a removal API the engine doesn't have: `World::add_system`/
//! `add_global_system` (in the `sturdy` crate) box the closure and leak it
//! (`Box::into_raw`, never `Box::from_raw`'d back), exactly mirroring how `FFI/src/FFI/EcsSystems.cpp`
//! already leaks its own `RegisteredSystem` for the same reason (see its doc comment: "Schedule has
//! no way to unregister a system, so this lives as long as the engine does").
//!
//! ## Panic safety
//!
//! Every trampoline is called from inside `Schedule::run`, which may run entity systems' bodies
//! concurrently with each other across worker threads (that's the entire point of the scheduler).
//! `ErasedSystemFn` is `noexcept`, so a panic must never unwind into that C++ frame. Both
//! `rust_entity_system_run` and `rust_global_system_run` route the actual closure call through
//! `diagnostics::guard`, which catches the panic, logs it, and aborts the process — mirroring every
//! other C++-called-back-into-Rust boundary in this crate (`diagnostics::log_sink_call`,
//! `diagnostics::scheduler_task_run`), not `task.rs`'s catch-into-`JoinError` pattern (there is no
//! `JoinError` equivalent to hand a result to here; the scheduler doesn't see a return value at
//! all).
//!
//! ## What resource/event access a system body actually gets
//!
//! `ErasedSystemFn`'s signature (`Entity`, `void **components`, dispatch context, user data) has no
//! parameter for resource bytes at all — `SystemAccess::resource_reads`/`resource_writes` exist
//! purely for the scheduler's own conflict detection (see `FFI/src/FFI/EcsSystems.cpp`, whose
//! `sturdy_ecs_add_system_with_resources` declares resource access the same way but never delivers
//! a resource pointer to `invoke_system` either). To make `Resource` access actually usable from a
//! global system body (not just declarable), `schedule_add_global_system` additionally stashes a
//! `World *` (stable for the engine's lifetime, same assumption `ecs.cpp`'s `g_owned`/`g_events`
//! maps already rely on) alongside the declared resource keys, and the global trampoline resolves
//! each one via `World::resource_pointer_erased` *fresh on every dispatch* (a resource may not be
//! bound yet at registration time, or could be rebound later) immediately before calling into Rust
//! — mirroring how components are pre-resolved into `void **` for entity systems. This works safely
//! only because the trampoline runs synchronously inside `Schedule::run`'s own call stack, which
//! already holds the `World` exclusively for the duration of the run. Per-entity systems do not get
//! this treatment: only global systems combine resource access with delivery, since that is what the
//! demonstration and the common "count something in a resource" case need; a per-entity system can
//! still *declare* resource access via `Access` for correct conflict detection, it just won't see
//! the bytes.
//!
//! Event channels are declarable on `Access` for conflict-detection purposes only (mapped straight
//! through to `SystemAccess::event_reads`/`event_writes` would require draining semantics this pass
//! does not implement); see `sturdy::ecs::schedule`'s module doc for the explicit scope note.

#[cxx::bridge(namespace = "sturdy_rs::schedule")]
pub mod ffi {
    /// Which of the engine's two built-in schedules to register into. Both are already run by the
    /// engine itself (`EngineModule.cpp`'s `update_schedule_.run(...)` inside `Engine::update`,
    /// `EngineImpl.cpp`'s `render_extraction_schedule_.run(...)` inside frame preparation) — this
    /// binding only ever registers into them, it never calls `Schedule::run` itself.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ScheduleTarget {
        Update,
        RenderExtraction,
    }

    /// Mirrors the parts of `SFT::Ecs::SystemAccess` this binding can produce. `reads`/`writes` are
    /// component ids (already resolved via `World::register`/`find`); the `resource_*_high`/`_low`
    /// pairs are resource keys (pure name hashes, resolved without touching the engine), split into
    /// parallel primitive vectors rather than `Vec<ResourceId>` because `cxx` only auto-derives the
    /// `Vec<T>` binding for a shared struct in the bridge that originally defines it — `ResourceId`
    /// is `ecs.rs`'s, which never puts it in a `Vec` itself, so reusing it here without that
    /// wouldn't compile (`ResourceId: ImplVec` unsatisfied).
    #[derive(Debug, Clone, Default)]
    struct SystemAccessSpec {
        reads: Vec<u32>,
        writes: Vec<u32>,
        resource_reads_high: Vec<u64>,
        resource_reads_low: Vec<u64>,
        resource_writes_high: Vec<u64>,
        resource_writes_low: Vec<u64>,
    }

    extern "Rust" {
        type RustEntitySystem;
        type RustGlobalSystem;

        /// Called by `entity_system_trampoline` (schedule.cpp) once per matching entity.
        /// `components` points at `RustEntitySystem::sizes.len()` pointers, parallel to the
        /// component ids the system was registered with; each is valid for exactly this call.
        /// `commands` is this dispatch's deferred-mutation queue (see `commands_*` below);
        /// non-null for the lifetime of the whole dispatch (every entity plus finish), minted once
        /// per dispatch by `commands_prepare_trampoline`.
        ///
        /// # Safety
        /// `sys` must point at a live `RustEntitySystem` (true for the engine's whole lifetime,
        /// since these are never freed; see the module doc). `components` must have at least
        /// `sizes.len()` valid, non-aliasing, appropriately sized entries. `commands` must point at
        /// a live `Commands` for the duration of this call.
        unsafe fn rust_entity_system_run(
            sys: *mut RustEntitySystem,
            entity: EntityId,
            components: *mut *mut u8,
            commands: *mut Commands,
        );

        /// Called by `global_system_trampoline` once per dispatch. `resources` points at
        /// `RustGlobalSystem::sizes.len()` pointers, parallel to the declared resource reads then
        /// writes, in that order; a null entry means that resource is not currently bound, in which
        /// case the whole dispatch is skipped (logged once) rather than risking a null deref.
        /// `commands` is the same per-dispatch queue described on `rust_entity_system_run`.
        ///
        /// # Safety
        /// Same contract as `rust_entity_system_run`.
        unsafe fn rust_global_system_run(
            sys: *mut RustGlobalSystem,
            resources: *mut *mut u8,
            resource_count: usize,
            commands: *mut Commands,
        );
    }

    unsafe extern "C++" {
        // The ecs bridge's generated header: needed for the `EntityId`/`ResourceId`/`EcsStatus`
        // shared types reused below (their full C++ definitions live there, not in schedule.hpp) —
        // same reasoning as render.rs's own comment on reusing the root bridge's shared enums.
        include!("sturdy_rs/ecs_bridge.h");
        include!("sturdy_rs/schedule.hpp");

        // Reused from other bridges rather than redeclared; see ecs.rs's own comment on why the
        // `#[namespace]` override is required in each case (cxx otherwise derives the alias's
        // `ExternType::Id` from this bridge's namespace instead of the type's real one).
        #[namespace = "sturdy_rs"]
        type EngineView = crate::ffi::EngineView;
        #[namespace = "sturdy_rs::ecs"]
        type EntityId = crate::ecs::ffi::EntityId;
        #[namespace = "sturdy_rs::ecs"]
        type EcsStatus = crate::ecs::ffi::EcsStatus;

        /// Opaque handle onto `SFT::Ecs::Commands`, this dispatch's deferred spawn/despawn/
        /// add-component/remove-component queue. First owned by this bridge (unlike the types
        /// reused above) since nothing else in this crate needs it. Minted per-dispatch by the
        /// engine itself via `commands_prepare_trampoline` (schedule.cpp) and handed to every
        /// `rust_entity_system_run`/`rust_global_system_run` call for that dispatch; never
        /// constructed or freed from the Rust side.
        type Commands;

        /// Registers a per-entity system into `target`. `component_ids` is the visit order: index
        /// `i` is what `components[i]` addresses in every `rust_entity_system_run` call for this
        /// system. Leaks `system` into the engine's own scheduler storage forever (see module doc).
        ///
        /// Fails (without registering) if any id in `access`/`component_ids` is not a component
        /// registered in this world's registry.
        ///
        /// # Safety
        /// `system` must be a uniquely owned `Box<RustEntitySystem>` pointer, valid to call back
        /// into for as long as the engine keeps it registered — which, per the leak above, is
        /// forever (there is no `schedule_remove_system`), so this is really just "must be a
        /// fresh `Box::into_raw`".
        // clippy's `missing_safety_doc` can't see doc comments on an `unsafe fn` declared inside a
        // `#[cxx::bridge]` block (confirmed empirically: it still fires with the `# Safety` section
        // right above) -- silenced per-fn rather than module-wide so a real future doc gap on a
        // *safe* fn still gets caught.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn schedule_add_entity_system(
            engine: Pin<&mut EngineView>,
            target: ScheduleTarget,
            access: &SystemAccessSpec,
            component_ids: &[u32],
            system: *mut RustEntitySystem,
        ) -> EcsStatus;

        /// Registers a global (resource/event-only) system into `target`, run exactly once per
        /// dispatch. Leaks `system` the same way `schedule_add_entity_system` does.
        ///
        /// # Safety
        /// Same contract as `schedule_add_entity_system`: `system` must be a fresh
        /// `Box::into_raw(Box<RustGlobalSystem>)`.
        // clippy false positive on cxx-bridge unsafe fns, see `schedule_add_entity_system` above.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn schedule_add_global_system(
            engine: Pin<&mut EngineView>,
            target: ScheduleTarget,
            access: &SystemAccessSpec,
            system: *mut RustGlobalSystem,
        ) -> EcsStatus;

        /// Queues creation of an entity carrying the given components once this dispatch's commands
        /// are applied (at the end of `Schedule::run`, after every system in it has finished — see
        /// `SFT::Ecs::Commands`'s own doc comment). `component_ids`/`sizes` are parallel; `data` is
        /// every component's bytes concatenated in the same order, copied immediately (the caller's
        /// storage need not outlive this call). Silently does nothing if the lengths disagree — a
        /// deferred command has no caller left by the time it would fail, matching
        /// `Commands::spawn_erased`'s own contract.
        ///
        /// # Safety
        /// `commands` must point at a live `Commands` (i.e. this must be called from within a
        /// system body that was handed it).
        // clippy false positive on cxx-bridge unsafe fns, see `schedule_add_entity_system` above.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn commands_spawn(
            commands: *mut Commands,
            component_ids: &[u32],
            sizes: &[u32],
            data: &[u8],
        );

        /// Queues attaching one component to `entity`, applied the same way as `commands_spawn`.
        /// `data` must be exactly `component`'s registered size (silently dropped otherwise, same
        /// reasoning as `commands_spawn`).
        ///
        /// # Safety
        /// Same contract as `commands_spawn`.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn commands_add_component(commands: *mut Commands, entity: EntityId, component: u32, data: &[u8]);

        /// Queues detaching one component from `entity`.
        ///
        /// # Safety
        /// Same contract as `commands_spawn`.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn commands_remove_component(commands: *mut Commands, entity: EntityId, component: u32);

        /// Queues destroying `entity`.
        ///
        /// # Safety
        /// Same contract as `commands_spawn`.
        #[allow(clippy::missing_safety_doc)]
        unsafe fn commands_destroy(commands: *mut Commands, entity: EntityId);
    }
}

use crate::diagnostics::guard;

/// Holds one registered per-entity system: the byte size of each declared component (parallel to
/// the `component_ids` it was registered with, used to turn `void **` into `&mut [u8]` slices of
/// the right length) and the closure itself.
///
/// Leaked (`Box::into_raw`, never reclaimed) by `sturdy::ecs::schedule::World::add_system`; see this
/// module's doc comment for why that matches the engine's own no-removal `Schedule` API.
/// See [`RustEntitySystem::body`].
pub type EntitySystemFn = Box<dyn FnMut(ffi::EntityId, &mut [&mut [u8]], *mut ffi::Commands) + Send>;
/// See [`RustGlobalSystem::body`].
pub type GlobalSystemFn = Box<dyn FnMut(&mut [&mut [u8]], *mut ffi::Commands) + Send>;

pub struct RustEntitySystem {
    pub sizes: Vec<usize>,
    pub body: EntitySystemFn,
}

/// Holds one registered global system: the byte size of each declared resource (parallel to the
/// resolved resource pointers the trampoline builds fresh every dispatch) and the closure itself.
pub struct RustGlobalSystem {
    pub sizes: Vec<usize>,
    pub body: GlobalSystemFn,
}

/// # Safety
/// See the bridge declaration's doc comment.
unsafe fn rust_entity_system_run(
    sys: *mut RustEntitySystem,
    entity: ffi::EntityId,
    components: *mut *mut u8,
    commands: *mut ffi::Commands,
) {
    guard("ecs entity system", || unsafe {
        let sys = &mut *sys;
        let mut slices: Vec<&mut [u8]> = Vec::with_capacity(sys.sizes.len());
        for (index, &size) in sys.sizes.iter().enumerate() {
            let pointer = *components.add(index);
            slices.push(core::slice::from_raw_parts_mut(pointer, size));
        }
        (sys.body)(entity, &mut slices, commands);
    });
}

/// # Safety
/// See the bridge declaration's doc comment.
unsafe fn rust_global_system_run(
    sys: *mut RustGlobalSystem,
    resources: *mut *mut u8,
    resource_count: usize,
    commands: *mut ffi::Commands,
) {
    guard("ecs global system", || unsafe {
        let sys = &mut *sys;
        debug_assert_eq!(resource_count, sys.sizes.len());
        let mut slices: Vec<&mut [u8]> = Vec::with_capacity(sys.sizes.len());
        for (index, &size) in sys.sizes.iter().enumerate() {
            let pointer = *resources.add(index);
            if pointer.is_null() {
                eprintln!(
                    "sturdy: global ECS system skipped this dispatch: a declared resource is not bound yet"
                );
                return;
            }
            slices.push(core::slice::from_raw_parts_mut(pointer, size));
        }
        (sys.body)(&mut slices, commands);
    });
}
