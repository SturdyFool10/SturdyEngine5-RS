//! Per-frame UI overlay bridge: registers a Rust closure that builds one frame of the engine's
//! Clay-style immediate-mode UI (`SFT::UI::Context`, driven through `Engine::UiContext`), plus
//! read-only accessors for pointer/text-input state (`Engine::UiPointerState`/`UiTextInputState`).
//!
//! ## Why this shape, not a `UiOverlayHooks` binding
//!
//! `SFT::Renderer::UiOverlayHooks` (`Renderer/Scene.hpp`) is a pair of `std::function`s --
//! `prepare`/`draw` -- that the engine's own UI renderer fills in from a finished
//! `UI::FrameSnapshot`. Their signatures operate directly on raw RHI types (`RhiDevice`,
//! `CommandEncoder`, `RenderGraph`, `RenderPassEncoder`, buffer/bind-group/texture handles, a
//! render-graph-relative resource list, ...): gpu-resource plumbing only the renderer itself knows
//! how to do, not something a foreign caller is meant to implement. `FFI/src/FFI/Ui.cpp` -- the
//! existing, shipped foreign-caller surface for this subsystem -- confirms this: it never exposes
//! `prepare`/`draw` to its caller either. What a foreign caller actually drives is the declarative
//! element tree that *produces* the `UI::FrameSnapshot` those hooks render from -- `UI::Context`'s
//! `element`/`text`/`hovered`/`clicked`/... calls, sequenced through a begin/end session exactly like
//! `sturdy_ui_begin`/`sturdy_ui_begin_element`/`sturdy_ui_text`/`sturdy_ui_end` in that file.
//!
//! So that is what this module binds, 1:1 with that reference surface: element/text primitives,
//! hover/click/pointer queries, and the pointer/text-input snapshots -- plus images, svg, scroll
//! containers, and focus (this module's own extension beyond the original element/text-only pass;
//! see each item below for the `UI::Context` entry point it wraps). Also bound: the raw
//! stroke/fill/sector draws (`ui_stroke_paths`/`ui_fill_quads`/`ui_fill_sectors`) and
//! custom-shader elements/strokes (`ui_begin_custom_element`/`ui_stroke_custom`), floating/border/
//! alignment/z/cursor element config, and the chart widget (`ui_graph`; the reference FFI's
//! `sturdy_ui_graph_series_*` ring buffer isn't mirrored -- it's just a `VecDeque` in Rust), and
//! editor-style docking (`DockWorkspace`, `ui_dock_*`).
//!
//! ## The draw hook and frame wiring
//!
//! [`ffi::ui_run_frame`] builds one frame: it readies the UI renderer, opens a layout sized to the
//! framebuffer, invokes the registered [`UiDrawHandle`] closure (which calls the element/text
//! primitives below to build the tree), and finishes the layout into a `UI::FrameSnapshot`, handing
//! the result to `UiContext::build_overlay_hooks`. The resulting `Renderer::UiOverlayHooks` is
//! stashed C++-side (`ui_take_pending_overlay_hooks` in `ui.hpp`) until `shim.cpp`'s
//! `RustGameLogic::request_render_frame` collects it via that function and attaches it to the
//! frame's `RenderFrameParameters::ui_overlay` -- that wiring now lives in `shim.cpp`, right after
//! it builds `params` from the Rust `FrameRequest`, so `ui_run_frame` runs and its overlay attaches
//! every frame without any action from `sturdy`-crate callers beyond installing a draw hook.

#[cxx::bridge(namespace = "sturdy_rs::ui")]
pub mod ffi {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum UiSizing {
        /// Shrinks to its content, within `[min, max]`.
        Fit,
        /// Expands to fill the space its parent gives it, within `[min, max]`.
        Grow,
        /// Exactly `value` pixels.
        Fixed,
        /// `value` as a fraction (0.0-1.0) of the parent's own size along this axis.
        Percent,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum UiDirection {
        LeftToRight,
        TopToBottom,
    }

    /// Mirrors `SFT::UI::AlignX`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum UiAlignX {
        #[default]
        Left,
        Center,
        Right,
    }

    /// Mirrors `SFT::UI::AlignY`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum UiAlignY {
        #[default]
        Top,
        Center,
        Bottom,
    }

    /// Mirrors `SFT::UI::CursorIcon` (the cursor shown while hovering an element; `Auto` lets the
    /// engine decide).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum UiCursor {
        #[default]
        Auto,
        Default,
        Pointer,
        Text,
        Grab,
        Grabbing,
        ResizeHorizontal,
        ResizeVertical,
        ResizeNwse,
        ResizeNesw,
        NotAllowed,
    }

    /// Mirrors `SFT::UI::FloatingAttachTo`. `None` means the element is an ordinary in-flow box.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum UiFloatingAttachTo {
        #[default]
        None,
        Parent,
        ElementWithId,
        Root,
    }

    /// Mirrors `SFT::UI::FloatingAttachPoint`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum UiAttachPoint {
        #[default]
        LeftTop,
        LeftCenter,
        LeftBottom,
        CenterTop,
        CenterCenter,
        CenterBottom,
        RightTop,
        RightCenter,
        RightBottom,
    }

    /// Mirrors `SFT::UI::FloatingConfig` — an element positioned out of normal flow (tooltips,
    /// dropdowns, popovers, drag previews), attached by `element_attach_point` to
    /// `parent_attach_point` of its target plus `offset`.
    #[derive(Debug, Clone, Default)]
    struct UiFloating {
        attach_to: UiFloatingAttachTo,
        /// Only meaningful for `attach_to == ElementWithId`.
        parent_id: String,
        element_attach_point: UiAttachPoint,
        parent_attach_point: UiAttachPoint,
        offset: [f32; 2],
        z_index: i16,
        /// Whether the floating element blocks pointer input to what's underneath.
        capture_pointer: bool,
        /// `FloatingClipTo::AttachedParent` when `true`, `None` otherwise.
        clip_to_attached_parent: bool,
    }

    /// Declares one element (a layout box, optionally with a background/border/corner radius).
    /// Mirrors `SturdyUiElement`/`SFT::UI::ElementDecl`.
    #[derive(Debug, Clone)]
    struct UiElementDesc {
        /// Empty means "anonymous" -- `ui_hovered`/`ui_clicked` cannot target an anonymous element.
        id: String,
        width_kind: UiSizing,
        /// Meaning depends on `width_kind`: unused for `Fit`/`Grow`, pixels for `Fixed`, a 0.0-1.0
        /// fraction for `Percent`.
        width_value: f32,
        height_kind: UiSizing,
        height_value: f32,
        padding_left: u16,
        padding_right: u16,
        padding_top: u16,
        padding_bottom: u16,
        /// Gap between children along `direction`.
        child_gap: u16,
        direction: UiDirection,
        /// Straight-alpha sRGB.
        background: [f32; 4],
        /// Top-left, top-right, bottom-left, bottom-right, in that order.
        corner_radius: [f32; 4],
        /// Clips (and, combined with `clip_vertical`, makes scrollable) this element's overflow
        /// along the horizontal axis. Mirrors `SFT::UI::ClipConfig::horizontal`.
        clip_horizontal: bool,
        /// Same as `clip_horizontal`, vertical axis.
        clip_vertical: bool,
        align_x: UiAlignX,
        align_y: UiAlignY,
        /// Straight-alpha sRGB; a zero-alpha color (the default) draws no border.
        border_color: [f32; 4],
        /// Left, right, top, bottom, then `between_children` (dividers between child elements).
        border_width: [u16; 5],
        floating: UiFloating,
        /// Paint-order tiebreaker among overlapping elements; higher draws on top.
        z: i32,
        cursor: UiCursor,
        /// Shown in the engine's UI debug views; no effect on layout.
        debug_label: String,
    }

    /// `SFT::Renderer::TextureHandle` (a Renderer-registry index, `{ value: u64 }`), as accepted by
    /// [`ui_image`]/[`ui_svg`]. Get one from `sturdy_rs::assets::assets_texture_handle` (a texture
    /// asset already loaded through `AssetManager`) or from [`ui_load_svg`] (a rasterized SVG,
    /// which is not an `AssetManager` entry at all -- see that function's doc comment).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct UiTexture {
        value: u64,
    }

    /// Outcome of [`ui_load_svg`]. No structured error code (unlike `sturdy_rs::assets`'s
    /// `AssetErrorCode` outcomes) -- `UiSvgCache::resolve` reports failure as a generic
    /// `AssetError` and the one case its own doc comment calls out by name
    /// (`AssetErrorCode::DecodeFailure`) is already obvious from context, so a message string
    /// covers this without pulling `sturdy_rs::assets`'s enum into this bridge.
    #[derive(Debug, Clone, Default)]
    struct UiSvgOutcome {
        ok: bool,
        message: String,
        texture: UiTexture,
    }

    /// Mirrors `SFT::UI::Context::ScrollMetrics`, as reported by [`ui_scroll_metrics`].
    #[derive(Debug, Clone, Copy, PartialEq, Default)]
    struct UiScrollMetrics {
        /// Whether a clipped element with this id was part of the last finished frame -- every
        /// other field is meaningless (zeroed) when this is `false`.
        found: bool,
        offset: [f32; 2],
        content_size: [f32; 2],
        container_size: [f32; 2],
        /// Whether the container actually scrolls (has overflow) along each axis.
        horizontal: bool,
        vertical: bool,
    }

    /// Mirrors `SFT::UI::StrokeCapKind`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum UiStrokeCap {
        Round,
        Butt,
        Square,
    }

    /// Mirrors `SFT::UI::StrokeJoinKind`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum UiStrokeJoin {
        Round,
        Miter,
        Bevel,
    }

    /// Mirrors `SFT::UI::StrokeStyle`. `dash_length == 0` means a solid line.
    #[derive(Debug, Clone, Copy)]
    struct UiStrokeStyle {
        /// Straight-alpha sRGB.
        color: [f32; 4],
        width: f32,
        feather_px: f32,
        dash_length: f32,
        dash_gap: f32,
        cap: UiStrokeCap,
        join: UiStrokeJoin,
        snap_to_pixel_grid: bool,
        glow_intensity: f32,
    }

    /// One polyline for [`ui_stroke_paths`]. `points` is flattened `x, y` pairs in the stroked
    /// element's own local space (relative to its top-left), resolved to absolute pixels when the
    /// frame's layout finishes.
    #[derive(Debug, Clone)]
    struct UiStrokePath {
        points: Vec<f32>,
        style: UiStrokeStyle,
    }

    /// Mirrors `SFT::UI::FillQuad` (element-local `position`/`size`).
    #[derive(Debug, Clone, Copy)]
    struct UiFillQuad {
        position: [f32; 2],
        size: [f32; 2],
        color: [f32; 4],
        /// Top-left, top-right, bottom-left, bottom-right.
        corner_radius: [f32; 4],
    }

    /// Mirrors `SFT::UI::Sector` — an annular "pie slice". Angles in radians; element-local
    /// `center`.
    #[derive(Debug, Clone, Copy)]
    struct UiSector {
        center: [f32; 2],
        inner_radius: f32,
        outer_radius: f32,
        start_angle: f32,
        end_angle: f32,
        color: [f32; 4],
        feather_px: f32,
    }

    /// Mirrors `SFT::UI::CustomShaderRef` — a Slang file supplying `vertexMain` (built via
    /// `Shaders/sturdy_common.slang`'s `uiQuadClipPosition()` for [`ui_begin_custom_element`], or
    /// `uiStrokeSegmentClipPosition()` for [`ui_stroke_custom`]) and a fragment entry point.
    /// `push_constants` is appended after the engine-written prefix (`UiElementConstants`, 32
    /// bytes / `CustomStrokeElementConstants`, 48 bytes); the total must equal the shader's own
    /// reflected push-constant size, or the UI renderer refuses to draw it.
    #[derive(Debug, Clone)]
    struct UiCustomShader {
        shader_path: String,
        module_name: String,
        /// Empty means the engine default, `"fragmentMain"`.
        fragment_entry_point: String,
        push_constants: Vec<u8>,
    }

    /// Mirrors `SFT::UI::GraphType`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum UiGraphType {
        #[default]
        Line,
        Area,
        Bar,
        Scatter,
        Pie,
    }

    /// Mirrors `SFT::UI::BarStackMode`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum UiBarStackMode {
        #[default]
        Grouped,
        Stacked,
    }

    /// Mirrors `SFT::UI::ScaleKind` minus `Custom` (its forward/inverse are `std::function`s,
    /// which don't cross this bridge).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum UiScaleKind {
        #[default]
        Linear,
        Log,
        Symlog,
    }

    /// Mirrors `SFT::UI::AxisConfig` minus its `std::function` hooks (custom scale transform,
    /// tick-label formatter). `has_min`/`has_max` carry the C++ `optional`s (unset = autoscale).
    #[derive(Debug, Clone)]
    struct UiGraphAxis {
        scale: UiScaleKind,
        log_base: f64,
        symlog_linear_threshold: f64,
        has_min: bool,
        min: f64,
        has_max: bool,
        max: f64,
        autoscale_padding_percent: f32,
        target_tick_count: u32,
        show_gridlines: bool,
        show_minor_gridlines: bool,
        axis_color: [f32; 4],
        gridline_color: [f32; 4],
        minor_gridline_color: [f32; 4],
        title: String,
        is_categorical: bool,
        categories: Vec<String>,
    }

    /// Mirrors `SFT::UI::SeriesRef` (owning its data here; the C++ side borrows it for the call).
    #[derive(Debug, Clone)]
    struct UiGraphSeries {
        name: String,
        x: Vec<f64>,
        y: Vec<f64>,
        color: [f32; 4],
        line_width: f32,
        feather_px: f32,
        area_fill_opacity: f32,
        marker_radius: f32,
        glow_intensity: f32,
    }

    /// Mirrors `SFT::UI::PieSlice`.
    #[derive(Debug, Clone)]
    struct UiPieSlice {
        name: String,
        value: f64,
        color: [f32; 4],
    }

    /// Mirrors `SFT::UI::GraphDesc` (`pie_style` flattened into `pie_*`).
    #[derive(Debug, Clone)]
    struct UiGraphDesc {
        graph_type: UiGraphType,
        x_axis: UiGraphAxis,
        y_axis: UiGraphAxis,
        series: Vec<UiGraphSeries>,
        bar_stack_mode: UiBarStackMode,
        bar_group_gap_fraction: f32,
        bar_series_gap_fraction: f32,
        pie_slices: Vec<UiPieSlice>,
        pie_hole_ratio: f32,
        pie_start_angle_degrees: f32,
        pie_gap_degrees: f32,
        pie_feather_px: f32,
        background: [f32; 4],
        corner_radius: [f32; 4],
        /// Left, bottom, top, right.
        axis_margins: [f32; 4],
        font_id: u16,
        label_font_size: u16,
        title_font_size: u16,
        show_legend: bool,
    }

    /// Mirrors `SFT::UI::Docking::DockDropZone`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum UiDockZone {
        #[default]
        Center,
        Left,
        Right,
        Top,
        Bottom,
    }

    /// Mirrors `optional<SFT::UI::Docking::DockPlacement>`: `has_placement == false` lets the
    /// workspace pick (the focused leaf, else the root).
    #[derive(Debug, Clone, Copy, Default)]
    struct UiDockPlacement {
        has_placement: bool,
        /// A `DockNodeId`, e.g. from [`ui_dock_focused_leaf`].
        target_node: u32,
        zone: UiDockZone,
    }

    /// Mirrors `SFT::UI::Docking::DockWorkspaceEvents` from one [`ui_dock_end_frame`]. The
    /// workspace doesn't act on these itself -- a tear-off request means "the user dragged this
    /// panel out", a close request "the user clicked its close button"; the caller decides.
    #[derive(Debug, Clone, Default)]
    struct UiDockEvents {
        tear_off_panels: Vec<String>,
        /// Workspace-local drop position of each tear-off, flattened `x, y` pairs parallel to
        /// `tear_off_panels`.
        tear_off_positions: Vec<f32>,
        close_requests: Vec<String>,
    }

    /// Mirrors `SturdyUiTextStyle`/`SFT::UI::TextStyle`.
    #[derive(Debug, Clone, Copy)]
    struct UiTextStyle {
        /// Straight-alpha sRGB.
        color: [f32; 4],
        font_id: u16,
        font_size: u16,
    }

    /// Read-only snapshot of `Engine::UiPointerState` (`UI::PointerState` plus the `consumed` flag),
    /// independent of whether a UI frame is currently open.
    #[derive(Debug, Clone, Copy, Default)]
    struct UiPointerSnapshot {
        position: [f32; 2],
        down: bool,
        pressed_this_frame: bool,
        released_this_frame: bool,
        cancelled_this_frame: bool,
        /// Whether `press_position` holds a meaningful value (set when `pressed_this_frame` most
        /// recently became true and cleared on release/cancel).
        has_press_position: bool,
        press_position: [f32; 2],
        scroll_delta: [f32; 2],
        /// Something in the UI tree claimed the pointer this frame -- a game should not also react
        /// to it as world input.
        consumed: bool,
    }

    /// Read-only snapshot of `Engine::UiTextInputState` for this tick.
    #[derive(Debug, Clone, Default)]
    struct UiTextInputSnapshot {
        /// Committed UTF-8 text typed this tick (after IME composition finishes, if any).
        typed_text: String,
        composing: bool,
        /// In-progress IME composition text; meaningful only while `composing`.
        composition_text: String,
    }

    extern "Rust" {
        type UiDrawHandle;

        /// Builds one frame of UI by calling the element/text primitives below. Invoked from inside
        /// `ui_run_frame`, between the layout being opened and finished -- calling any of the
        /// `ui_begin_element`/`ui_end_element`/`ui_text`/`ui_hovered`/`ui_clicked`/`ui_pointer_*`
        /// functions outside of this callback has no open session to act on.
        fn ui_invoke_draw_hook(handle: &mut UiDrawHandle, engine: Pin<&mut EngineView>);
    }

    unsafe extern "C++" {
        include!("sturdy_rs/ui.hpp");

        /// `SFT::Engine::Engine`; reused from the root bridge rather than redeclared. The
        /// `#[namespace]` override matches this bridge's own type to the root bridge's actual
        /// identity (`sturdy_rs::EngineView`, not `sturdy_rs::ui::EngineView`) so
        /// `verify_extern_type` accepts it at Rust compile time.
        #[namespace = "sturdy_rs"]
        type EngineView = crate::ffi::EngineView;

        /// Installs (or replaces) the process-wide per-frame UI draw hook.
        fn ui_set_draw_hook(hook: Box<UiDrawHandle>);
        /// Removes whatever hook is currently registered; `ui_run_frame` becomes a no-op again.
        fn ui_clear_draw_hook();

        /// Loads a font file from `path` and registers it under `font_id`, for later use as
        /// `UiTextStyle::font_id`. Safe to call before the UI renderer exists -- the registration is
        /// replayed automatically once it does. Returns `false` on read/parse failure.
        fn ui_register_font(engine: Pin<&mut EngineView>, path: &str, font_id: u16) -> bool;

        /// Builds one overlay UI frame: readies the UI renderer, opens a layout sized to
        /// `framebuffer_width` x `framebuffer_height`, invokes the registered draw hook (if any),
        /// and finishes the layout. Returns `false` (nothing drawn) when the UI renderer isn't ready
        /// yet or no hook is registered -- calling this unconditionally every frame is fine.
        fn ui_run_frame(
            engine: Pin<&mut EngineView>,
            framebuffer_width: u32,
            framebuffer_height: u32,
            delta_seconds: f64,
        ) -> bool;

        /// Opens a new element scope as a child of whichever element is currently open (the layout
        /// root, if none is). Must be balanced by a matching `ui_end_element` before the frame ends.
        /// Only meaningful from inside the draw hook. Returns `false` if there is no open frame.
        fn ui_begin_element(desc: &UiElementDesc) -> bool;
        /// Closes the innermost element opened by `ui_begin_element`. Returns `false` if there is no
        /// open frame or no open element to close.
        fn ui_end_element() -> bool;
        /// Adds a text leaf as a child of the currently open element. Returns `false` if there is no
        /// open frame.
        fn ui_text(text: &str, style: &UiTextStyle) -> bool;

        /// Adds an image leaf (a texture already loaded via `AssetManager`, resolved to a
        /// `Renderer::TextureHandle` with `sturdy_rs::assets::assets_texture_handle`) as a child of
        /// the currently open element, sized/positioned by `desc` same as `ui_begin_element`.
        /// Mirrors `SFT::UI::Context::image`. Returns `false` if there is no open frame.
        fn ui_image(desc: &UiElementDesc, texture: UiTexture) -> bool;

        /// Loads (or returns the cached rasterization of) an SVG file at `path`, rendered at
        /// `target_px` (the SVG's largest dimension, in logical UI pixels -- re-rasterized at a new
        /// size if a previous call cached a different `target_px` for the same path). Mirrors
        /// `Engine::UiSvgCache::resolve`. Unlike texture assets, this does not go through
        /// `AssetManager`/`Asset` at all -- the cache is keyed and owned entirely inside
        /// `Engine::UiSvgCache`, so there is no asset handle to unload later; the cache simply holds
        /// the rasterized texture for the process lifetime (or until `Engine::UiSvgCache::clear`,
        /// not yet bound -- see this module's doc comment for what's out of scope).
        fn ui_load_svg(engine: Pin<&mut EngineView>, path: &str, target_px: f32) -> UiSvgOutcome;

        /// Adds an SVG leaf (from [`ui_load_svg`]) as a child of the currently open element, same
        /// shape as `ui_image`. Mirrors `SFT::UI::Context::svg`. Returns `false` if there is no open
        /// frame.
        fn ui_svg(desc: &UiElementDesc, texture: UiTexture) -> bool;

        /// Whether the pointer was over the element with this id, as of the last finished frame.
        fn ui_hovered(id: &str) -> bool;
        /// Whether the element with this id was clicked this frame.
        fn ui_clicked(id: &str) -> bool;
        /// Pointer position in the current layout's coordinate space. Only meaningful inside the
        /// draw hook (zero outside of one).
        fn ui_pointer_position() -> [f32; 2];
        /// Whether the pointer is currently held down. Only meaningful inside the draw hook.
        fn ui_pointer_is_down() -> bool;

        /// Read-only snapshot of `Engine::UiPointerState`.
        fn ui_pointer_state(engine: &EngineView) -> UiPointerSnapshot;
        /// Read-only snapshot of `Engine::UiTextInputState` for this tick.
        fn ui_text_input_state(engine: &EngineView) -> UiTextInputSnapshot;

        /// Gives keyboard/text-input focus to the element with this id (it need not be open right
        /// now -- focus is looked up by id again each frame). Mirrors `SFT::UI::Context::focus`.
        /// Only meaningful inside the draw hook; a no-op if there is no open frame.
        fn ui_focus(id: &str);
        /// Whether the element with this id currently has focus. Mirrors
        /// `SFT::UI::Context::has_focus`.
        fn ui_has_focus(id: &str) -> bool;
        /// Whether *any* element currently has focus. Mirrors `SFT::UI::Context::focused`.
        fn ui_focused() -> bool;
        /// Clears focus if the element with this id currently has it (a no-op otherwise -- clearing
        /// focus while a different element holds it does nothing, matching
        /// `SFT::UI::Context::clear_focus`).
        fn ui_clear_focus(id: &str);

        /// Scroll state of the clipped (`clip_horizontal`/`clip_vertical`) element with this id, as
        /// of the last finished frame. `UiScrollMetrics::found` is `false` (every other field
        /// zeroed) if no such element was part of that frame. Mirrors
        /// `SFT::UI::Context::scroll_metrics`.
        fn ui_scroll_metrics(container_id: &str) -> UiScrollMetrics;
        /// Sets the scroll offset of the clipped element with this id, clamped to its content size
        /// same as a real scroll gesture would be. Returns `false` if no such element was part of
        /// the last finished frame (there is nothing to clamp against yet). Mirrors
        /// `SFT::UI::Context::set_scroll_offset`. Takes effect on the *next* finished frame's
        /// `ui_scroll_metrics`/rendering, same lag as a driven pointer scroll would have.
        fn ui_set_scroll_offset(container_id: &str, offset: [f32; 2]) -> bool;

        /// Draws anti-aliased polylines as one leaf element sized/positioned by `desc`
        /// (`SFT::UI::Context::stroke_paths`). All paths share the element's bounding box,
        /// scissor, and paint order — e.g. a chart's axes, gridlines, and every series in one call.
        fn ui_stroke_paths(desc: &UiElementDesc, paths: &[UiStrokePath]) -> bool;
        /// Draws filled (optionally rounded) rects as one leaf element
        /// (`SFT::UI::Context::fill_quads`).
        fn ui_fill_quads(desc: &UiElementDesc, quads: &[UiFillQuad]) -> bool;
        /// Draws annular sectors as one leaf element (`SFT::UI::Context::fill_sectors`).
        fn ui_fill_sectors(desc: &UiElementDesc, sectors: &[UiSector]) -> bool;
        /// Opens an element whose background is drawn by `shader` instead of the built-in rect
        /// pipeline (`SFT::UI::Context::custom_element`). Children may be added; close it with
        /// [`ui_end_element`] like any other element.
        fn ui_begin_custom_element(desc: &UiElementDesc, shader: &UiCustomShader) -> bool;
        /// Draws a polyline (flattened element-local `x, y` pairs) through `shader` instead of the
        /// built-in stroke pipeline (`SFT::UI::Context::stroke_custom`). One draw per segment; meant
        /// for a handful of stylized lines, not bulk charting.
        fn ui_stroke_custom(
            desc: &UiElementDesc,
            points: &[f32],
            half_width: f32,
            feather_px: f32,
            shader: &UiCustomShader,
        ) -> bool;
        /// Draws a line/area/bar/scatter/pie chart (axes, gridlines, ticks, legend) as one leaf
        /// element (`SFT::UI::graph`). Returns whether anything was drawn -- always `false` for an
        /// anonymous `desc` and on the first frame an id appears, since the widget sizes itself
        /// from that id's bounds in the previous finished frame.
        fn ui_graph(desc: &UiElementDesc, graph: &UiGraphDesc) -> bool;

        /// `SFT::UI::Docking::DockWorkspace`: a tabbed, splittable, drag-to-rearrange panel
        /// layout (editor-style docking). Owned by Rust through a `UniquePtr`; persists across
        /// frames and is driven once per frame from inside the UI draw hook.
        type DockWorkspace;

        /// Creates an empty workspace. `id_prefix` namespaces the element ids its chrome (tabs,
        /// dividers, close buttons) generates -- distinct per workspace.
        fn ui_dock_workspace_new(id_prefix: &str) -> UniquePtr<DockWorkspace>;
        /// Adds a panel (a tab) at `placement`. Returns `false` if `id` is already present or the
        /// placement is invalid.
        fn ui_dock_add_panel(
            workspace: Pin<&mut DockWorkspace>,
            id: &str,
            title: &str,
            closable: bool,
            placement: &UiDockPlacement,
        ) -> bool;
        fn ui_dock_remove_panel(workspace: Pin<&mut DockWorkspace>, id: &str);
        fn ui_dock_has_panel(workspace: &DockWorkspace, id: &str) -> bool;
        fn ui_dock_is_empty(workspace: &DockWorkspace) -> bool;
        /// The focused leaf's `DockNodeId`, or `-1` if none.
        fn ui_dock_focused_leaf(workspace: &DockWorkspace) -> i64;
        fn ui_dock_set_content_background(workspace: Pin<&mut DockWorkspace>, color: [f32; 4]);
        /// Lays out and draws the workspace's chrome into `rect` (`x, y, width, height`, in the UI
        /// layout's coordinate space) and processes this frame's tab/divider drags. Must be
        /// called inside a UI session, before any [`ui_dock_begin_panel_content`].
        fn ui_dock_begin_frame(workspace: Pin<&mut DockWorkspace>, rect: [f32; 4], delta_seconds: f32) -> bool;
        /// Opens an element covering panel `id`'s content region (only if it's the active tab of
        /// its leaf this frame); add its content as children and close it with
        /// [`ui_end_element`]. Returns `false` (opening nothing) for a hidden or unknown panel.
        fn ui_dock_begin_panel_content(workspace: &DockWorkspace, id: &str) -> bool;
        /// Finishes the frame (drop guides, deferred drops) and drains its events.
        fn ui_dock_end_frame(workspace: Pin<&mut DockWorkspace>) -> UiDockEvents;
    }
}

use core::pin::Pin;
use std::panic::{catch_unwind, AssertUnwindSafe};

pub use ffi::EngineView;

/// Trampoline target: holds the user's boxed per-frame UI draw closure. Mirrors
/// [`sturdy_sys::LogicHandle`](crate::LogicHandle) -- the `sturdy` crate installs the closure here so
/// this crate stays free of user-facing traits.
/// See [`UiDrawHandle::draw`].
pub type UiDrawFn = Box<dyn FnMut(Pin<&mut EngineView>) + Send>;

pub struct UiDrawHandle {
    pub draw: UiDrawFn,
}

/// A panic must never unwind into the C++ frame that invoked this callback (undefined behavior),
/// so it is reported and the process aborts instead. Duplicated per bridge module rather than
/// shared (it's a private fn in both `diagnostics.rs` and the `sturdy` crate's `runtime.rs`) since
/// each crosses the C++ boundary independently.
fn guard<R>(what: &str, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(value) => value,
        Err(_) => {
            eprintln!("sturdy: `{what}` callback panicked; aborting (cannot unwind through C++)");
            std::process::abort();
        }
    }
}

fn ui_invoke_draw_hook(handle: &mut UiDrawHandle, engine: Pin<&mut EngineView>) {
    guard("ui draw hook", move || (handle.draw)(engine))
}

// SAFETY: `SFT::UI::Docking::DockWorkspace` is plain owned data (maps, vectors, strings) with no
// thread affinity or interior pointers into thread-local state; every use goes through `&mut`/`&`
// on whichever single thread runs the UI draw hook, so moving ownership across threads (the draw
// hook closure must be `Send`) is sound.
unsafe impl Send for ffi::DockWorkspace {}
