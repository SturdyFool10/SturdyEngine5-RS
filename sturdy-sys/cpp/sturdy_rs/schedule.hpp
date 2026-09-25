// Rust <-> SturdyEngine 5 ECS *scheduler* shim: registers Rust closures as real systems in the
// engine's own `SFT::Ecs::Schedule` (see `Ecs/src/Ecs/System.hpp`), run automatically every tick.
// See src/schedule.rs for the bridge these prototypes are generated against and the ownership/
// panic-safety model.
#pragma once

#include <cstdint>

#include <rust/cxx.h>

#include "sturdy_rs/ecs.hpp"
#include "sturdy_rs/shim.hpp"

namespace SFT::Ecs {
    // Forward declaration only; the full definition (`Ecs/Commands.hpp`, pulled in transitively via
    // `Ecs/System.hpp`) is only needed where `Commands`'s members are actually used, i.e.
    // schedule.cpp — not here, where it's just named in a type alias and pointer signatures.
    class Commands;
} // namespace SFT::Ecs

namespace sturdy_rs::schedule {

    /// Reuses the engine view the root bridge already defined; see shim.hpp.
    using EngineView = sturdy_rs::EngineView;

    // `EntityId`/`EcsStatus` are *not* redeclared here: unlike `SystemAccessSpec` below (owned by
    // this bridge), they are namespace-qualified reuses of `sturdy_rs::ecs`'s own types (see
    // src/schedule.rs's `#[namespace = "sturdy_rs::ecs"] type EcsStatus = ...`), whose forward
    // declarations already live in ecs.hpp (included above) — declaring a second, distinct
    // `sturdy_rs::schedule::EcsStatus` here would silently create an incompatible type instead of
    // referring to the same one cxx actually generates. `ResourceId` is not reused at all (see
    // `SystemAccessSpec`'s doc comment in src/schedule.rs for why); resource keys cross this
    // bridge as flat `(high, low)` `u64` pairs instead.
    using EntityId = sturdy_rs::ecs::EntityId;
    using EcsStatus = sturdy_rs::ecs::EcsStatus;

    // `Commands` is owned by this bridge (declared with `type Commands;` in src/schedule.rs, not
    // reused from elsewhere): it's `SFT::Ecs::Commands` itself, forward-declared here and given its
    // full definition via `Ecs/System.hpp`'s include chain in schedule.cpp.
    using Commands = SFT::Ecs::Commands;

    // Forward declarations for the types cxx generates into schedule_bridge.h that *are* owned by
    // this bridge.
    struct SystemAccessSpec;
    enum class ScheduleTarget : std::uint8_t;
    struct RustEntitySystem;
    struct RustGlobalSystem;

    EcsStatus schedule_add_entity_system(EngineView &engine,
                                         ScheduleTarget target,
                                         const SystemAccessSpec &access,
                                         rust::Slice<const std::uint32_t> component_ids,
                                         RustEntitySystem *system);

    EcsStatus schedule_add_global_system(EngineView &engine,
                                         ScheduleTarget target,
                                         const SystemAccessSpec &access,
                                         RustGlobalSystem *system);

    // Deferred-mutation operations on this dispatch's `Commands` queue — see src/schedule.rs's
    // bridge doc comments for the exact contract (all four silently no-op on malformed input,
    // matching `SFT::Ecs::Commands`'s own erased methods).
    void commands_spawn(Commands *commands,
                        rust::Slice<const std::uint32_t> component_ids,
                        rust::Slice<const std::uint32_t> sizes,
                        rust::Slice<const std::uint8_t> data);
    void commands_add_component(Commands *commands, EntityId entity, std::uint32_t component,
                                rust::Slice<const std::uint8_t> data);
    void commands_remove_component(Commands *commands, EntityId entity, std::uint32_t component);
    void commands_destroy(Commands *commands, EntityId entity);

} // namespace sturdy_rs::schedule
