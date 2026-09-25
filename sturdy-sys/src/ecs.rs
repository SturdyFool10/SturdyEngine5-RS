//! Raw `cxx` bindings for the engine's type-erased ECS (`SFT::Ecs::World`).
//!
//! Everything here addresses components and resources by id/string rather than by C++ type — the
//! Rust side has no way to name a C++ type — and moves their bytes rather than their C++ value, so
//! this stays a thin, honest mirror of `World`'s `*_erased` API. See `sturdy::ecs` for the
//! ergonomic wrapper built on top.

#[cxx::bridge(namespace = "sturdy_rs::ecs")]
pub mod ffi {
    /// Mirrors `SFT::Ecs::Entity`.
    #[derive(Debug, Clone, Copy)]
    struct EntityId {
        index: u32,
        generation: u32,
    }

    /// Mirrors `SFT::Ecs::ResourceKey`.
    #[derive(Debug, Clone, Copy)]
    struct ResourceId {
        high: u64,
        low: u64,
    }

    /// Mirrors `SFT::Ecs::WorldErasedErrorCode`, plus a couple of cases this layer's own argument
    /// validation reports before ever reaching the engine. Only meaningful when the surrounding
    /// result's `ok` is `false`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum EcsErrorCode {
        DeadEntity,
        UnknownComponent,
        DuplicateComponent,
        MissingComponent,
        SizeMismatch,
        NotTriviallyCopyable,
        NoComponents,
        ScheduleRunning,
        InvalidArgument,
    }

    #[derive(Debug, Clone)]
    struct EcsStatus {
        ok: bool,
        code: EcsErrorCode,
        message: String,
    }

    #[derive(Debug, Clone)]
    struct ComponentLookup {
        ok: bool,
        code: EcsErrorCode,
        message: String,
        id: u32,
    }

    #[derive(Debug, Clone)]
    struct SpawnResult {
        ok: bool,
        code: EcsErrorCode,
        message: String,
        entity: EntityId,
    }

    #[derive(Debug, Clone)]
    struct ResourceLookup {
        ok: bool,
        code: EcsErrorCode,
        message: String,
        id: ResourceId,
    }

    /// Snapshot produced by `ecs_query`: every live entity that carried all requested components
    /// at the moment of the call, with each requested component's bytes copied out.
    ///
    /// `data` is entity-major: entity `i`'s components sit at
    /// `sum(sizes) * i .. sum(sizes) * (i + 1)`, and within that span component `j` (0-based, in
    /// request order) sits at the running sum of `sizes[..j]`. `sizes` has the same length and
    /// order as the requested component ids.
    #[derive(Debug, Clone)]
    struct QuerySnapshot {
        ok: bool,
        code: EcsErrorCode,
        message: String,
        entities: Vec<EntityId>,
        sizes: Vec<u32>,
        data: Vec<u8>,
    }

    /// Result of `ecs_read_events`: every event currently buffered in a channel, drained out as a
    /// flat byte blob (`count` fixed-size records of `element_size` bytes each, in send order).
    /// See `ecs_read_events`'s doc comment for why draining, not just copying, is what "read" means
    /// here.
    #[derive(Debug, Clone)]
    struct EventBatch {
        ok: bool,
        code: EcsErrorCode,
        message: String,
        count: u32,
        element_size: u32,
        data: Vec<u8>,
    }

    unsafe extern "C++" {
        include!("sturdy_rs/ecs.hpp");

        /// `SFT::Engine::Engine`; reused from the root bridge rather than redeclared.
        ///
        /// The `#[namespace]` override is required: without it cxx derives this alias's
        /// `ExternType::Id` from *this* bridge's own namespace (`sturdy_rs::ecs::EngineView`),
        /// which doesn't match the root bridge's actual identity (`sturdy_rs::EngineView`) and
        /// fails `verify_extern_type` at Rust compile time.
        #[namespace = "sturdy_rs"]
        type EngineView = crate::ffi::EngineView;

        /// Derives the key a resource/event-channel name hashes to, without touching the engine.
        fn ecs_resource_key(name: &str) -> ResourceId;

        /// Registers a component under `name`, or returns the existing id if one is already
        /// registered with the same name, size, and alignment.
        fn ecs_register_component(engine: Pin<&mut EngineView>, name: &str, size: u32, align: u32) -> ComponentLookup;
        /// Looks up a component previously registered under `name` (by this binding, by the C++
        /// engine itself, or by any other caller sharing this `World`).
        fn ecs_find_component(engine: &EngineView, name: &str) -> ComponentLookup;

        /// Spawns an entity carrying the given components. `component_ids` and `sizes` are
        /// parallel; `data` is their concatenation in the same order.
        fn ecs_spawn(
            engine: Pin<&mut EngineView>,
            component_ids: &[u32],
            sizes: &[u32],
            data: &[u8],
        ) -> SpawnResult;
        /// Destroys an entity. Not an error if it is already dead.
        fn ecs_despawn(engine: Pin<&mut EngineView>, entity: EntityId);
        fn ecs_is_alive(engine: &EngineView, entity: EntityId) -> bool;

        fn ecs_has_component(engine: &EngineView, entity: EntityId, component: u32) -> bool;
        fn ecs_add_component(engine: Pin<&mut EngineView>, entity: EntityId, component: u32, data: &[u8]) -> EcsStatus;
        fn ecs_remove_component(engine: Pin<&mut EngineView>, entity: EntityId, component: u32) -> EcsStatus;
        /// Copies the component's bytes into `out`, which must be exactly its registered size.
        fn ecs_read_component(engine: &EngineView, entity: EntityId, component: u32, out: &mut [u8]) -> EcsStatus;
        fn ecs_write_component(engine: Pin<&mut EngineView>, entity: EntityId, component: u32, data: &[u8]) -> EcsStatus;

        /// Binds a resource, allocating and owning its storage on the C++ side (so the binding
        /// keyed to it does not depend on a Rust allocation outliving the resource). Re-binding
        /// under the same name with the same size is a no-op that reports success.
        fn ecs_create_resource(engine: Pin<&mut EngineView>, name: &str, data: &[u8]) -> ResourceLookup;
        /// Unbinds a resource this binding created, freeing its storage.
        fn ecs_destroy_resource(engine: Pin<&mut EngineView>, key: ResourceId) -> EcsStatus;
        fn ecs_has_resource(engine: &EngineView, key: ResourceId) -> bool;
        /// Copies the resource's bytes into `out`, which must be exactly its bound size.
        fn ecs_get_resource(engine: Pin<&mut EngineView>, key: ResourceId, out: &mut [u8]) -> EcsStatus;
        fn ecs_set_resource(engine: Pin<&mut EngineView>, key: ResourceId, data: &[u8]) -> EcsStatus;

        /// Snapshots every live entity carrying all of `component_ids`, copying their bytes out.
        /// See `QuerySnapshot` for the layout of the result.
        fn ecs_query(engine: Pin<&mut EngineView>, component_ids: &[u32]) -> QuerySnapshot;

        /// Creates (or, idempotently, re-resolves) an event channel under `name`, sized for records
        /// of `element_size` bytes. Backed by a heap-allocated `SFT::Ecs::ErasedEvents`, owned by
        /// this binding the same way `ecs_create_resource`'s storage is.
        fn ecs_create_event_channel(engine: Pin<&mut EngineView>, name: &str, element_size: u32) -> ResourceLookup;
        /// Unbinds an event channel this binding created, freeing its storage.
        fn ecs_destroy_event_channel(engine: Pin<&mut EngineView>, key: ResourceId) -> EcsStatus;
        /// Appends one event's bytes to the channel. `data` must be exactly the channel's element
        /// size.
        fn ecs_send_event(engine: Pin<&mut EngineView>, key: ResourceId, data: &[u8]) -> EcsStatus;
        /// Drains every event currently buffered in the channel and returns it. See `EventBatch`;
        /// this call both reads and clears (see the doc comment on the C++ definition for why).
        fn ecs_read_events(engine: Pin<&mut EngineView>, key: ResourceId) -> EventBatch;
        /// Discards every buffered event without returning them.
        fn ecs_clear_event_channel(engine: Pin<&mut EngineView>, key: ResourceId) -> EcsStatus;
    }
}
