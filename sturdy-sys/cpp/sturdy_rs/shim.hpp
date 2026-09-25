// Rust <-> SturdyEngine 5 shim. Everything here calls the C++ API directly (no C ABI in between);
// it exists only where cxx cannot express the engine's types itself (UString, std::expected,
// std::optional, virtual GameLogic, ...). Shared structs/enums come from the cxx-generated header.
#pragma once

#include <array>
#include <cstddef>
#include <cstdint>

#include <rust/cxx.h>

#include <Engine/Engine.hpp>

namespace sturdy_rs {

    /// The engine the Rust side sees. An alias: cxx only ever handles it by reference.
    using EngineView = SFT::Engine::Engine;

    struct RuntimeOptions;
    struct Status;
    struct FrameInfo;
    struct FrameRequest;
    struct GpuDescription;
    struct LogicHandle;

    /// Runs the whole application on the calling thread; returns the process exit status.
    std::int32_t run_runtime(const RuntimeOptions &options,
                             const rust::Vec<rust::String> &args,
                             rust::Box<LogicHandle> logic);

    double engine_delta_seconds(const EngineView &engine) noexcept;
    double engine_unscaled_delta_seconds(const EngineView &engine) noexcept;
    std::uint64_t engine_tick_index(const EngineView &engine) noexcept;
    double engine_time_scale(const EngineView &engine) noexcept;
    void engine_set_time_scale(EngineView &engine, double scale) noexcept;
    bool engine_gpu_description(const EngineView &engine, GpuDescription &out);

    bool input_key_down(const EngineView &engine, std::int32_t key) noexcept;
    bool input_key_just_pressed(const EngineView &engine, std::int32_t key) noexcept;
    bool input_key_just_released(const EngineView &engine, std::int32_t key) noexcept;
    bool input_mouse_down(const EngineView &engine, std::uint8_t button) noexcept;
    bool input_mouse_just_pressed(const EngineView &engine, std::uint8_t button) noexcept;
    bool input_mouse_just_released(const EngineView &engine, std::uint8_t button) noexcept;
    std::array<float, 2> input_mouse_position(const EngineView &engine) noexcept;
    std::array<float, 2> input_mouse_delta(const EngineView &engine) noexcept;
    std::array<float, 2> input_wheel_delta(const EngineView &engine) noexcept;
    rust::String input_text_this_tick(const EngineView &engine);

} // namespace sturdy_rs
