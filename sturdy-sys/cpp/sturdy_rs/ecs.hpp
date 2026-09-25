// Rust <-> SturdyEngine 5 ECS shim. Thin wrapper over `SFT::Ecs::World`'s type-erased API: it
// exists only for the bits cxx cannot express itself (std::expected, std::span, the erased visit
// callback). See src/ecs.rs for the bridge these prototypes are generated against.
#pragma once

#include <cstdint>

#include <rust/cxx.h>

#include "sturdy_rs/shim.hpp"

namespace sturdy_rs::ecs {

    /// Reuses the engine view the root bridge already defined; see shim.hpp.
    using EngineView = sturdy_rs::EngineView;

    // Forward declarations for the types cxx generates into ecs_bridge.h; only the ones that
    // appear directly in a function signature below need to be named here.
    struct EntityId;
    struct ResourceId;
    struct EcsStatus;
    struct ComponentLookup;
    struct SpawnResult;
    struct ResourceLookup;
    struct QuerySnapshot;
    struct EventBatch;

    ResourceId ecs_resource_key(rust::Str name) noexcept;

    ComponentLookup ecs_register_component(EngineView &engine, rust::Str name, std::uint32_t size, std::uint32_t align);
    ComponentLookup ecs_find_component(const EngineView &engine, rust::Str name);

    SpawnResult ecs_spawn(EngineView &engine,
                          rust::Slice<const std::uint32_t> component_ids,
                          rust::Slice<const std::uint32_t> sizes,
                          rust::Slice<const std::uint8_t> data);
    void ecs_despawn(EngineView &engine, EntityId entity) noexcept;
    bool ecs_is_alive(const EngineView &engine, EntityId entity) noexcept;

    bool ecs_has_component(const EngineView &engine, EntityId entity, std::uint32_t component) noexcept;
    EcsStatus ecs_add_component(EngineView &engine, EntityId entity, std::uint32_t component,
                                rust::Slice<const std::uint8_t> data);
    EcsStatus ecs_remove_component(EngineView &engine, EntityId entity, std::uint32_t component);
    EcsStatus ecs_read_component(const EngineView &engine, EntityId entity, std::uint32_t component,
                                 rust::Slice<std::uint8_t> out);
    EcsStatus ecs_write_component(EngineView &engine, EntityId entity, std::uint32_t component,
                                  rust::Slice<const std::uint8_t> data);

    ResourceLookup ecs_create_resource(EngineView &engine, rust::Str name, rust::Slice<const std::uint8_t> data);
    EcsStatus ecs_destroy_resource(EngineView &engine, ResourceId key);
    bool ecs_has_resource(const EngineView &engine, ResourceId key) noexcept;
    EcsStatus ecs_get_resource(EngineView &engine, ResourceId key, rust::Slice<std::uint8_t> out);
    EcsStatus ecs_set_resource(EngineView &engine, ResourceId key, rust::Slice<const std::uint8_t> data);

    QuerySnapshot ecs_query(EngineView &engine, rust::Slice<const std::uint32_t> component_ids);

    ResourceLookup ecs_create_event_channel(EngineView &engine, rust::Str name, std::uint32_t element_size);
    EcsStatus ecs_destroy_event_channel(EngineView &engine, ResourceId key);
    EcsStatus ecs_send_event(EngineView &engine, ResourceId key, rust::Slice<const std::uint8_t> data);
    EventBatch ecs_read_events(EngineView &engine, ResourceId key);
    EcsStatus ecs_clear_event_channel(EngineView &engine, ResourceId key);

} // namespace sturdy_rs::ecs
