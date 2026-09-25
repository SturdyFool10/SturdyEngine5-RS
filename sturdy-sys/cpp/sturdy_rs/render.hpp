// Render-graph settings, multi-window/surface, offscreen render target and presentation/HDR shim.
// Everything here calls the C++ API directly (no C ABI); shared structs/enums come from the
// cxx-generated header for src/render.rs.
#pragma once

#include <cstdint>

#include <rust/cxx.h>

#include <Engine/RenderGraph.hpp>

#include "sturdy_rs/shim.hpp"

namespace sturdy_rs {
    // Forward declaration only: the root bridge's own generated type (`src/lib.rs`'s `WindowMode`),
    // reused here (via `#[namespace = "sturdy_rs"] type WindowMode = crate::ffi::WindowMode;` in
    // src/render.rs) the same way render.cpp already reuses it unqualified in its own anonymous
    // namespace's `map(WindowMode)` helper -- this header just needs it visible for
    // `window_set_mode`'s declaration.
    enum class WindowMode : std::uint8_t;
} // namespace sturdy_rs

namespace sturdy_rs::render {

    struct SurfaceHandle;
    struct WindowDesc;
    struct WindowOpenResult;
    struct TextInputArea;
    enum class CursorIcon : std::uint8_t;
    enum class WindowEffectKind : std::uint8_t;
    struct OffscreenTargetDesc;
    struct OffscreenTargetCreateResult;
    struct PresentationSettings;
    struct Status;
    struct PresentationResolution;
    struct HdrCapabilityQuery;
    struct HdrContentLightLevelUpdate;
    struct RenderGraphSettings;

    // -- windows --

    WindowOpenResult window_open(EngineView &engine, const WindowDesc &desc);
    WindowOpenResult window_recreate(EngineView &engine, SurfaceHandle old_surface, const WindowDesc &desc);
    void window_close(EngineView &engine, SurfaceHandle surface) noexcept;
    void window_notify_resize(EngineView &engine, SurfaceHandle surface, std::uint32_t width, std::uint32_t height) noexcept;

    // -- window runtime mutation (queued via Engine::window_requests(), enqueue-and-forget) --

    void window_set_cursor_icon(EngineView &engine, SurfaceHandle surface, CursorIcon icon) noexcept;
    void window_set_cursor_grabbed(EngineView &engine, SurfaceHandle surface, bool grabbed) noexcept;
    void window_set_mode(EngineView &engine, SurfaceHandle surface, ::sturdy_rs::WindowMode mode) noexcept;
    void window_set_decorated(EngineView &engine, SurfaceHandle surface, bool decorated) noexcept;
    void window_set_transparent(EngineView &engine, SurfaceHandle surface, bool transparent) noexcept;
    void window_set_relative_mouse_mode(EngineView &engine, SurfaceHandle surface, bool enabled) noexcept;
    void window_set_mouse_locked(EngineView &engine, SurfaceHandle surface, bool locked) noexcept;
    void window_set_text_input_active(EngineView &engine, SurfaceHandle surface, bool active) noexcept;
    void window_set_text_input_area(EngineView &engine, SurfaceHandle surface, TextInputArea area) noexcept;
    void window_set_effect(EngineView &engine, SurfaceHandle surface, WindowEffectKind kind, bool enabled) noexcept;

    // -- offscreen render targets --

    OffscreenTargetCreateResult offscreen_target_create(EngineView &engine, const OffscreenTargetDesc &desc);
    void offscreen_target_destroy(EngineView &engine, std::uint64_t handle) noexcept;
    bool offscreen_target_description(const EngineView &engine, std::uint64_t handle, OffscreenTargetDesc &out);
    std::uint64_t offscreen_target_texture(const EngineView &engine, std::uint64_t handle) noexcept;

    // -- presentation --

    PresentationSettings presentation_settings_get(const EngineView &engine, SurfaceHandle surface) noexcept;
    Status presentation_settings_set(EngineView &engine, SurfaceHandle surface, const PresentationSettings &settings);
    PresentationResolution presentation_resolution_query(const EngineView &engine, SurfaceHandle surface) noexcept;

    // -- HDR --

    HdrCapabilityQuery hdr_capabilities_query(const EngineView &engine, SurfaceHandle surface);
    Status hdr_update_content_light_level(EngineView &engine, SurfaceHandle surface, const HdrContentLightLevelUpdate &update);

    // -- render-graph settings --

    Status render_graph_settings_validate(const RenderGraphSettings &settings);

    /// Bridged half of the pending-render-graph side channel; see `src/render.rs`'s doc comment on
    /// `set_pending_render_graph` for why it exists instead of a `FrameRequest` field.
    void set_pending_render_graph(const RenderGraphSettings &settings);

    /// Converts a Rust-supplied `RenderGraphSettings` into a real `SFT::Engine::RenderGraph`.
    /// Exported (unlike the equivalent conversion buried in `render_graph_settings_validate`'s
    /// anonymous-namespace `to_engine` helper) so it's reusable from `set_pending_render_graph`.
    SFT::Engine::RenderGraph render_graph_from_settings(const RenderGraphSettings &settings) noexcept;

    /// Non-bridged: called from `shim.cpp`'s `RustGameLogic::request_render_frame`, never from
    /// Rust directly. Returns whatever `set_pending_render_graph` (the bridged half of this pair,
    /// implemented alongside it in render.cpp) most recently stored, or default-constructed engine
    /// settings if nothing was ever set this run. See `set_pending_render_graph`'s doc comment in
    /// `src/render.rs` for why this is a side channel instead of a `FrameRequest` field.
    SFT::Engine::RenderGraph consume_pending_render_graph() noexcept;

} // namespace sturdy_rs::render
