// Rust <-> SturdyEngine 5 UI overlay shim: builds one frame of the engine's Clay-style immediate
// mode UI (`SFT::UI::Context`, via `Engine::UiContext`) from a registered Rust closure, plus
// read-only accessors for pointer/text-input state. See ui.rs's module doc comment for scope and
// for why this binds the element/text primitives rather than `Renderer::UiOverlayHooks` directly.
#pragma once

#include <array>
#include <cstdint>
#include <memory>
#include <optional>

#include <rust/cxx.h>

#include <Renderer/UI/Docking/DockWorkspace.hpp>

#include <Renderer/Scene.hpp>

#include "sturdy_rs/shim.hpp"

namespace sturdy_rs::ui {

    /// `EngineView` re-exported into this bridge's own namespace: `sturdy_rs::ui`'s generated
    /// `ui_bridge.h` refers to it fully-qualified as `::sturdy_rs::ui::EngineView` (matching this
    /// bridge's `namespace = "sturdy_rs::ui"`), not `::sturdy_rs::EngineView` -- unqualified lookup
    /// from inside this namespace would find the latter, but the generated header needs the former
    /// to actually name something.
    using EngineView = sturdy_rs::EngineView;

    struct UiDrawHandle;
    struct UiElementDesc;
    struct UiTextStyle;
    struct UiTexture;
    struct UiSvgOutcome;
    struct UiScrollMetrics;
    struct UiPointerSnapshot;
    struct UiTextInputSnapshot;
    struct UiStrokePath;
    struct UiFillQuad;
    struct UiSector;
    struct UiCustomShader;
    struct UiGraphDesc;
    struct UiDockPlacement;
    struct UiDockEvents;

    /// Owned by Rust via `UniquePtr`; see src/ui.rs.
    using DockWorkspace = SFT::UI::Docking::DockWorkspace;

    void ui_set_draw_hook(rust::Box<UiDrawHandle> hook);
    void ui_clear_draw_hook() noexcept;

    bool ui_register_font(EngineView &engine, rust::Str path, std::uint16_t font_id);

    bool ui_run_frame(EngineView &engine, std::uint32_t framebuffer_width,
                      std::uint32_t framebuffer_height, double delta_seconds);

    bool ui_begin_element(const UiElementDesc &desc);
    bool ui_end_element();
    bool ui_text(rust::Str text, const UiTextStyle &style);

    bool ui_image(const UiElementDesc &desc, UiTexture texture);
    UiSvgOutcome ui_load_svg(EngineView &engine, rust::Str path, float target_px);
    bool ui_svg(const UiElementDesc &desc, UiTexture texture);

    bool ui_hovered(rust::Str id);
    bool ui_clicked(rust::Str id);
    std::array<float, 2> ui_pointer_position() noexcept;
    bool ui_pointer_is_down() noexcept;

    UiPointerSnapshot ui_pointer_state(const EngineView &engine) noexcept;
    UiTextInputSnapshot ui_text_input_state(const EngineView &engine);

    void ui_focus(rust::Str id);
    bool ui_has_focus(rust::Str id);
    bool ui_focused();
    void ui_clear_focus(rust::Str id);

    bool ui_stroke_paths(const UiElementDesc &desc, rust::Slice<const UiStrokePath> paths);
    bool ui_fill_quads(const UiElementDesc &desc, rust::Slice<const UiFillQuad> quads);
    bool ui_fill_sectors(const UiElementDesc &desc, rust::Slice<const UiSector> sectors);
    bool ui_begin_custom_element(const UiElementDesc &desc, const UiCustomShader &shader);
    bool ui_stroke_custom(const UiElementDesc &desc, rust::Slice<const float> points, float half_width, float feather_px,
                          const UiCustomShader &shader);
    bool ui_graph(const UiElementDesc &desc, const UiGraphDesc &graph);

    std::unique_ptr<DockWorkspace> ui_dock_workspace_new(rust::Str id_prefix);
    bool ui_dock_add_panel(DockWorkspace &workspace, rust::Str id, rust::Str title, bool closable,
                           const UiDockPlacement &placement);
    void ui_dock_remove_panel(DockWorkspace &workspace, rust::Str id);
    bool ui_dock_has_panel(const DockWorkspace &workspace, rust::Str id);
    bool ui_dock_is_empty(const DockWorkspace &workspace) noexcept;
    std::int64_t ui_dock_focused_leaf(const DockWorkspace &workspace) noexcept;
    void ui_dock_set_content_background(DockWorkspace &workspace, std::array<float, 4> color) noexcept;
    bool ui_dock_begin_frame(DockWorkspace &workspace, std::array<float, 4> rect, float delta_seconds);
    bool ui_dock_begin_panel_content(const DockWorkspace &workspace, rust::Str id);
    UiDockEvents ui_dock_end_frame(DockWorkspace &workspace);

    UiScrollMetrics ui_scroll_metrics(rust::Str container_id);
    bool ui_set_scroll_offset(rust::Str container_id, std::array<float, 2> offset);

    /// Non-bridged escape hatch: hands over whatever `Renderer::UiOverlayHooks` the most recent
    /// `ui_run_frame` call finished building, if any (and if it has not already been taken). Called
    /// from `shim.cpp`'s `RustGameLogic::request_render_frame`, right after `ui_run_frame`, to
    /// attach the result into that frame's `Engine::RenderFrameParameters::ui_overlay`. See ui.rs's
    /// module doc comment.
    [[nodiscard]] std::optional<SFT::Renderer::UiOverlayHooks> ui_take_pending_overlay_hooks() noexcept;

} // namespace sturdy_rs::ui
