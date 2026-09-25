//! Per-frame UI overlay: a Rust closure that builds one frame of the engine's Clay-style
//! immediate-mode UI, registered once with [`install_draw_hook`].
//!
//! ```no_run
//! use sturdy::ui::{self, Direction, ElementDesc, Sizing, TextStyle};
//!
//! ui::install_draw_hook(|ui| {
//!     let _root = ui.element(
//!         &ElementDesc::new().width(Sizing::Grow).height(Sizing::Grow).direction(Direction::TopToBottom),
//!     );
//!     ui.text("Hello, UI!", &TextStyle::default());
//! });
//! ```
//!
//! This module deliberately does not expose anything tied to `Renderer::UiOverlayHooks` GPU
//! plumbing (`prepare`/`draw` render callbacks) -- see `sturdy-sys/src/ui.rs`'s module doc comment
//! for why that surface isn't meant for a foreign caller at all. What is bound here is the
//! declarative element tree, 1:1 with `sturdy-sys`'s primitives: elements, text, images, svg,
//! scroll containers, focus, hover/click queries, and pointer/text-input state. Custom shaders, raw
//! stroke/fill/sector draws, and floating/docking are still out of scope -- see `sturdy-sys/src/ui.rs`'s
//! module doc comment for what a fuller binding would need.
//!
//! `install_draw_hook`'s closure runs once per frame automatically: `sturdy-sys`'s `shim.cpp`
//! calls `ui_run_frame` (which invokes it) and attaches the result to that frame's render
//! parameters from inside `RustGameLogic::request_render_frame`, so nothing further is required
//! beyond installing a hook here.

use core::pin::Pin;

use sturdy_sys::ui::{ffi, EngineView, UiDrawHandle};

use crate::assets::RendererTextureHandle;
use cxx::UniquePtr;

macro_rules! setters {
    ($($(#[$m:meta])* $field:ident: $ty:ty),+ $(,)?) => {
        $(
            $(#[$m])*
            #[must_use]
            pub fn $field(mut self, value: $ty) -> Self {
                self.$field = value;
                self
            }
        )+
    };
}

/// How one axis of an [`ElementDesc`] is sized. Mirrors `SFT::UI::SizingAxis`'s discriminant (the
/// `min`/`max` clamp range `SizingAxis` also carries is left at the engine's defaults here -- add a
/// `min`/`max` field to `ElementDesc` in a follow-up pass if that turns out to matter).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum Sizing {
    /// Shrinks to fit its content.
    #[default]
    Fit,
    /// Expands to fill whatever space its parent gives it.
    Grow,
    /// Exactly this many pixels.
    Fixed(f32),
    /// This fraction (0.0-1.0) of the parent's own size along this axis.
    Percent(f32),
}

impl Sizing {
    fn kind(self) -> ffi::UiSizing {
        match self {
            Self::Fit => ffi::UiSizing::Fit,
            Self::Grow => ffi::UiSizing::Grow,
            Self::Fixed(_) => ffi::UiSizing::Fixed,
            Self::Percent(_) => ffi::UiSizing::Percent,
        }
    }

    fn value(self) -> f32 {
        match self {
            Self::Fixed(v) | Self::Percent(v) => v,
            Self::Fit | Self::Grow => 0.0,
        }
    }
}

/// Layout direction for an [`ElementDesc`]'s children. Mirrors `SFT::UI::LayoutDirection`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Direction {
    #[default]
    LeftToRight,
    TopToBottom,
}

impl From<Direction> for ffi::UiDirection {
    fn from(value: Direction) -> Self {
        match value {
            Direction::LeftToRight => ffi::UiDirection::LeftToRight,
            Direction::TopToBottom => ffi::UiDirection::TopToBottom,
        }
    }
}

/// Padding on each side of an [`ElementDesc`], in logical UI pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Padding {
    pub left: u16,
    pub right: u16,
    pub top: u16,
    pub bottom: u16,
}

impl Padding {
    pub const fn all(value: u16) -> Self {
        Self { left: value, right: value, top: value, bottom: value }
    }

    pub const fn symmetric(horizontal: u16, vertical: u16) -> Self {
        Self { left: horizontal, right: horizontal, top: vertical, bottom: vertical }
    }
}

/// Which axes an [`ElementDesc`] clips its overflowing content on -- also what makes it scrollable
/// along that axis (a clipped axis with content bigger than the element accepts pointer-wheel
/// scroll and [`Ui::set_scroll_offset`]). Mirrors `SFT::UI::ClipConfig`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Clip {
    pub horizontal: bool,
    pub vertical: bool,
}

impl Clip {
    /// Clips (and makes scrollable) both axes -- the common "scroll container" shape.
    pub const fn both() -> Self {
        Self { horizontal: true, vertical: true }
    }

    pub const fn vertical() -> Self {
        Self { horizontal: false, vertical: true }
    }

    pub const fn horizontal() -> Self {
        Self { horizontal: true, vertical: false }
    }
}

/// Corner radii of an [`ElementDesc`]'s background, in logical UI pixels, one value per corner.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CornerRadius {
    pub top_left: f32,
    pub top_right: f32,
    pub bottom_left: f32,
    pub bottom_right: f32,
}

impl CornerRadius {
    pub const fn all(radius: f32) -> Self {
        Self { top_left: radius, top_right: radius, bottom_left: radius, bottom_right: radius }
    }

    fn to_array(self) -> [f32; 4] {
        [self.top_left, self.top_right, self.bottom_left, self.bottom_right]
    }
}

/// Declares one layout box. Mirrors `SFT::UI::ElementDecl` (the subset `sturdy-sys` binds -- see
/// this module's doc comment for what's left out).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ElementDesc {
    /// Empty means "anonymous" -- [`Ui::hovered`]/[`Ui::clicked`] cannot target an anonymous
    /// element.
    pub id: String,
    pub width: Sizing,
    pub height: Sizing,
    pub padding: Padding,
    /// Gap between children along `direction`.
    pub child_gap: u16,
    pub direction: Direction,
    /// Straight-alpha sRGB.
    pub background: glam::Vec4,
    pub corner_radius: CornerRadius,
    /// Which axes to clip (and thereby make scrollable). See [`Ui::scroll_metrics`]/
    /// [`Ui::set_scroll_offset`] to read/drive the resulting scroll position -- both are keyed by
    /// this element's `id`, so a scrollable element needs one.
    pub clip: Clip,
    /// How children are aligned within this element's box.
    pub align: ChildAlignment,
    pub border: Border,
    /// `Some` positions this element out of normal flow (tooltips, dropdowns, popovers).
    pub floating: Option<Floating>,
    /// Paint-order tiebreaker among overlapping elements; higher draws on top.
    pub z: i32,
    /// Cursor shown while hovering this element.
    pub cursor: Cursor,
    /// Shown in the engine's UI debug views; no effect on layout.
    pub debug_label: String,
}

pub use ffi::UiAlignX as AlignX;
pub use ffi::UiAlignY as AlignY;
pub use ffi::UiAttachPoint as AttachPoint;
pub use ffi::UiCursor as Cursor;

/// Where an [`ElementDesc`]'s children sit inside it. Mirrors `SFT::UI::ChildAlignment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChildAlignment {
    pub x: AlignX,
    pub y: AlignY,
}

impl ChildAlignment {
    pub const fn new(x: AlignX, y: AlignY) -> Self {
        Self { x, y }
    }

    pub const fn center() -> Self {
        Self { x: AlignX::Center, y: AlignY::Center }
    }
}

/// An [`ElementDesc`]'s border. Mirrors `SFT::UI::BorderStyle`; the default (zero width,
/// transparent) draws nothing.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Border {
    /// Straight-alpha sRGB.
    pub color: glam::Vec4,
    pub left: u16,
    pub right: u16,
    pub top: u16,
    pub bottom: u16,
    /// Divider lines drawn between child elements.
    pub between_children: u16,
}

impl Border {
    /// The same `width` on all four sides, no dividers.
    pub fn all(color: impl Into<glam::Vec4>, width: u16) -> Self {
        let color = color.into();
        Self { color, left: width, right: width, top: width, bottom: width, between_children: 0 }
    }
}

/// What a floating [`ElementDesc`] attaches to. Mirrors `SFT::UI::FloatingAttachTo` minus `None`
/// (expressed as `ElementDesc::floating: None` instead).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FloatTarget {
    /// The element it's declared inside.
    Parent,
    /// Any other element, by id (e.g. a dropdown anchored to its button).
    Element(String),
    /// The layout root (screen-space overlays).
    Root,
}

/// Out-of-flow positioning for an [`ElementDesc`]. Mirrors `SFT::UI::FloatingConfig`: this
/// element's `element_attach_point` is placed on the target's `parent_attach_point`, then shifted
/// by `offset`.
#[derive(Debug, Clone, PartialEq)]
pub struct Floating {
    pub attach_to: FloatTarget,
    pub element_attach_point: AttachPoint,
    pub parent_attach_point: AttachPoint,
    pub offset: glam::Vec2,
    pub z_index: i16,
    /// Whether it blocks pointer input to what's underneath.
    pub capture_pointer: bool,
    /// Clip to the target element's bounds instead of drawing over everything.
    pub clip_to_target: bool,
}

impl Floating {
    /// Defaults match `SFT::UI::FloatingConfig`: this element's top-left on the target's
    /// bottom-left (the "dropdown below its button" layout), capturing pointer input, unclipped.
    pub fn new(attach_to: FloatTarget) -> Self {
        Self {
            attach_to,
            element_attach_point: AttachPoint::LeftTop,
            parent_attach_point: AttachPoint::LeftBottom,
            offset: glam::Vec2::ZERO,
            z_index: 0,
            capture_pointer: true,
            clip_to_target: false,
        }
    }

    fn to_ffi(floating: Option<&Self>) -> ffi::UiFloating {
        let Some(f) = floating else {
            return ffi::UiFloating::default();
        };
        let (attach_to, parent_id) = match &f.attach_to {
            FloatTarget::Parent => (ffi::UiFloatingAttachTo::Parent, String::new()),
            FloatTarget::Element(id) => (ffi::UiFloatingAttachTo::ElementWithId, id.clone()),
            FloatTarget::Root => (ffi::UiFloatingAttachTo::Root, String::new()),
        };
        ffi::UiFloating {
            attach_to,
            parent_id,
            element_attach_point: f.element_attach_point,
            parent_attach_point: f.parent_attach_point,
            offset: f.offset.into(),
            z_index: f.z_index,
            capture_pointer: f.capture_pointer,
            clip_to_attached_parent: f.clip_to_target,
        }
    }
}

impl ElementDesc {
    pub fn new() -> Self {
        Self::default()
    }

    setters! {
        id: String,
        width: Sizing,
        height: Sizing,
        padding: Padding,
        child_gap: u16,
        direction: Direction,
        background: glam::Vec4,
        corner_radius: CornerRadius,
        clip: Clip,
        align: ChildAlignment,
        border: Border,
        floating: Option<Floating>,
        z: i32,
        cursor: Cursor,
        debug_label: String,
    }

    fn to_ffi(&self) -> ffi::UiElementDesc {
        ffi::UiElementDesc {
            id: self.id.clone(),
            width_kind: self.width.kind(),
            width_value: self.width.value(),
            height_kind: self.height.kind(),
            height_value: self.height.value(),
            padding_left: self.padding.left,
            padding_right: self.padding.right,
            padding_top: self.padding.top,
            padding_bottom: self.padding.bottom,
            child_gap: self.child_gap,
            direction: self.direction.into(),
            background: self.background.into(),
            corner_radius: [
                self.corner_radius.top_left,
                self.corner_radius.top_right,
                self.corner_radius.bottom_left,
                self.corner_radius.bottom_right,
            ],
            clip_horizontal: self.clip.horizontal,
            clip_vertical: self.clip.vertical,
            align_x: self.align.x,
            align_y: self.align.y,
            border_color: self.border.color.into(),
            border_width: [
                self.border.left,
                self.border.right,
                self.border.top,
                self.border.bottom,
                self.border.between_children,
            ],
            floating: Floating::to_ffi(self.floating.as_ref()),
            z: self.z,
            cursor: self.cursor,
            debug_label: self.debug_label.clone(),
        }
    }
}

/// A text leaf's appearance. Mirrors `SFT::UI::TextStyle` (the subset `sturdy-sys` binds).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TextStyle {
    /// Straight-alpha sRGB.
    pub color: glam::Vec4,
    /// A font id previously registered with [`Ui::register_font`].
    pub font_id: u16,
    pub font_size: u16,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self { color: glam::Vec4::ONE, font_id: 0, font_size: 16 }
    }
}

impl TextStyle {
    fn to_ffi(self) -> ffi::UiTextStyle {
        ffi::UiTextStyle { color: self.color.into(), font_id: self.font_id, font_size: self.font_size }
    }
}

/// Read-only snapshot of the engine's UI pointer state for this tick, independent of whether a UI
/// frame is currently open. Mirrors `Engine::UiPointerState`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct PointerState {
    pub position: glam::Vec2,
    pub down: bool,
    pub pressed_this_frame: bool,
    pub released_this_frame: bool,
    pub cancelled_this_frame: bool,
    /// Where the pointer went down, if a press is in progress (cleared on release/cancel).
    pub press_position: Option<glam::Vec2>,
    pub scroll_delta: glam::Vec2,
    /// Something in the UI tree claimed the pointer this frame -- a game should not also react to
    /// it as world input.
    pub consumed: bool,
}

impl From<ffi::UiPointerSnapshot> for PointerState {
    fn from(value: ffi::UiPointerSnapshot) -> Self {
        Self {
            position: value.position.into(),
            down: value.down,
            pressed_this_frame: value.pressed_this_frame,
            released_this_frame: value.released_this_frame,
            cancelled_this_frame: value.cancelled_this_frame,
            press_position: value.has_press_position.then_some(value.press_position.into()),
            scroll_delta: value.scroll_delta.into(),
            consumed: value.consumed,
        }
    }
}

/// Read-only snapshot of text typed / IME composition this tick. Mirrors `Engine::UiTextInputState`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TextInputState {
    /// Committed UTF-8 text typed this tick (after IME composition finishes, if any).
    pub typed_text: String,
    pub composing: bool,
    /// In-progress IME composition text; meaningful only while `composing`.
    pub composition_text: String,
}

impl From<ffi::UiTextInputSnapshot> for TextInputState {
    fn from(value: ffi::UiTextInputSnapshot) -> Self {
        Self { typed_text: value.typed_text, composing: value.composing, composition_text: value.composition_text }
    }
}

/// Handed to the closure installed with [`install_draw_hook`]; the only place the element/text
/// primitives below are meaningful (there is no UI layout open otherwise).
pub struct Ui<'a> {
    view: Pin<&'a mut EngineView>,
}

impl<'a> Ui<'a> {
    pub(crate) fn new(view: Pin<&'a mut EngineView>) -> Self {
        Self { view }
    }

    fn view_ref(&self) -> &EngineView {
        &self.view
    }

    /// Loads a font file from `path` and registers it under `font_id`, for later use as
    /// [`TextStyle::font_id`]. Safe to call every frame (cheap once already registered) or just
    /// once at startup; returns `false` on read/parse failure.
    pub fn register_font(&mut self, path: &str, font_id: u16) -> bool {
        ffi::ui_register_font(self.view.as_mut(), path, font_id)
    }

    /// Opens a new element as a child of whichever element is currently open (the layout root, if
    /// none is). The returned [`ElementScope`] closes it again when dropped -- nest by keeping the
    /// scope alive across the calls that build its children.
    #[must_use = "dropping this immediately closes the element it just opened"]
    pub fn element(&mut self, desc: &ElementDesc) -> ElementScope {
        ElementScope { open: ffi::ui_begin_element(&desc.to_ffi()) }
    }

    /// Adds a text leaf as a child of the currently open element.
    pub fn text(&mut self, text: &str, style: &TextStyle) {
        ffi::ui_text(text, &style.to_ffi());
    }

    /// Whether the pointer was over the element with this id, as of the last finished frame.
    pub fn hovered(&self, id: &str) -> bool {
        ffi::ui_hovered(id)
    }

    /// Whether the element with this id was clicked this frame.
    pub fn clicked(&self, id: &str) -> bool {
        ffi::ui_clicked(id)
    }

    /// Pointer position in the current layout's coordinate space.
    pub fn pointer_position(&self) -> glam::Vec2 {
        ffi::ui_pointer_position().into()
    }

    /// Whether the pointer is currently held down.
    pub fn pointer_down(&self) -> bool {
        ffi::ui_pointer_is_down()
    }

    /// Read-only snapshot of the engine's UI pointer state for this tick.
    pub fn pointer_state(&self) -> PointerState {
        ffi::ui_pointer_state(self.view_ref()).into()
    }

    /// Read-only snapshot of text typed / IME composition this tick.
    pub fn text_input(&self) -> TextInputState {
        ffi::ui_text_input_state(self.view_ref()).into()
    }

    /// Adds an image leaf as a child of the currently open element, sized/positioned by `desc`
    /// same as [`Ui::element`]. `texture` comes from
    /// [`crate::assets::Assets::texture_handle`] on a texture asset already loaded through
    /// [`crate::assets::Assets`].
    pub fn image(&mut self, desc: &ElementDesc, texture: RendererTextureHandle) {
        ffi::ui_image(&desc.to_ffi(), ffi::UiTexture { value: texture.0 });
    }

    /// Loads (or returns the cached rasterization of) an SVG file, rendered at `target_px` (its
    /// largest dimension, in logical UI pixels). Unlike texture assets this does not go through
    /// [`crate::assets::Assets`] at all -- see `sturdy-sys::ui::ffi::ui_load_svg`'s doc comment for
    /// why. Pass the result to [`Ui::svg`] to draw it.
    pub fn load_svg(&mut self, path: &str, target_px: f32) -> Result<RendererTextureHandle, String> {
        let out = ffi::ui_load_svg(self.view.as_mut(), path, target_px);
        if out.ok {
            Ok(RendererTextureHandle(out.texture.value))
        } else {
            Err(out.message)
        }
    }

    /// Adds an SVG leaf (from [`Ui::load_svg`]) as a child of the currently open element, same
    /// shape as [`Ui::image`].
    pub fn svg(&mut self, desc: &ElementDesc, texture: RendererTextureHandle) {
        ffi::ui_svg(&desc.to_ffi(), ffi::UiTexture { value: texture.0 });
    }

    /// Gives keyboard/text-input focus to the element with this id (it need not be open right now
    /// -- focus is looked up by id again each frame).
    pub fn focus(&mut self, id: &str) {
        ffi::ui_focus(id);
    }

    /// Whether the element with this id currently has focus.
    pub fn has_focus(&self, id: &str) -> bool {
        ffi::ui_has_focus(id)
    }

    /// Whether *any* element currently has focus.
    pub fn focused(&self) -> bool {
        ffi::ui_focused()
    }

    /// Clears focus if the element with this id currently has it (a no-op otherwise).
    pub fn clear_focus(&mut self, id: &str) {
        ffi::ui_clear_focus(id);
    }

    /// Scroll state of the clipped ([`Clip::horizontal`]/[`Clip::vertical`]) element with this id,
    /// as of the last finished frame. `None` if no such element was part of that frame.
    pub fn scroll_metrics(&self, container_id: &str) -> Option<ScrollMetrics> {
        let metrics = ffi::ui_scroll_metrics(container_id);
        metrics.found.then(|| ScrollMetrics {
            offset: metrics.offset.into(),
            content_size: metrics.content_size.into(),
            container_size: metrics.container_size.into(),
            horizontal: metrics.horizontal,
            vertical: metrics.vertical,
        })
    }

    /// Sets the scroll offset of the clipped element with this id, clamped to its content size
    /// same as a real scroll gesture would be. Returns `false` if no such element was part of the
    /// last finished frame. Takes effect on the *next* finished frame's `scroll_metrics`/rendering.
    pub fn set_scroll_offset(&mut self, container_id: &str, offset: impl Into<glam::Vec2>) -> bool {
        ffi::ui_set_scroll_offset(container_id, offset.into().into())
    }

    /// Draws anti-aliased polylines as one leaf element sized/positioned by `desc`. Every path's
    /// points are element-local (relative to its top-left) and share its bounding box, scissor,
    /// and paint order -- e.g. a chart's axes, gridlines, and every data series in one call.
    pub fn stroke_paths(&mut self, desc: &ElementDesc, paths: &[StrokePath]) -> bool {
        let converted: Vec<ffi::UiStrokePath> = paths.iter().map(StrokePath::to_ffi).collect();
        ffi::ui_stroke_paths(&desc.to_ffi(), &converted)
    }

    /// Convenience over [`Ui::stroke_paths`] for a single polyline.
    pub fn stroke_polyline(&mut self, desc: &ElementDesc, points: &[glam::Vec2], style: StrokeStyle) -> bool {
        self.stroke_paths(desc, &[StrokePath { points: points.to_vec(), style }])
    }

    /// Draws filled (optionally rounded) rects as one leaf element; positions are element-local.
    pub fn fill_quads(&mut self, desc: &ElementDesc, quads: &[FillQuad]) -> bool {
        let converted: Vec<ffi::UiFillQuad> = quads.iter().map(|q| q.to_ffi()).collect();
        ffi::ui_fill_quads(&desc.to_ffi(), &converted)
    }

    /// Draws annular sectors ("pie slices") as one leaf element; centers are element-local.
    pub fn fill_sectors(&mut self, desc: &ElementDesc, sectors: &[Sector]) -> bool {
        let converted: Vec<ffi::UiSector> = sectors.iter().map(|s| s.to_ffi()).collect();
        ffi::ui_fill_sectors(&desc.to_ffi(), &converted)
    }

    /// Opens an element whose background is drawn by a caller-supplied Slang shader instead of
    /// the built-in rect pipeline. Children may be added; the returned scope closes it on drop,
    /// same as [`Ui::element`]. See [`CustomShader`] for the shader contract.
    #[must_use = "dropping this immediately closes the element it just opened"]
    pub fn custom_element(&mut self, desc: &ElementDesc, shader: &CustomShader) -> ElementScope {
        ElementScope { open: ffi::ui_begin_custom_element(&desc.to_ffi(), &shader.to_ffi()) }
    }

    /// Draws a chart (line/area/bar/scatter/pie, with axes, gridlines, ticks, and a legend) as one
    /// leaf element sized/positioned by `desc`. For a rolling series, keep a `VecDeque` and copy it
    /// into [`GraphSeries::x`]/`y` each frame.
    ///
    /// `desc.id` must be non-empty and stable across frames: the widget sizes its plot from the
    /// element's bounds in the *previous* finished frame (`SFT::UI::Context::element_bounds`), so it
    /// returns `false` and draws nothing for an anonymous element, and on the first frame a given
    /// id appears.
    pub fn graph(&mut self, desc: &ElementDesc, graph: &GraphDesc) -> bool {
        ffi::ui_graph(&desc.to_ffi(), &graph.to_ffi())
    }

    /// Draws a polyline (element-local points) through a caller-supplied shader instead of the
    /// built-in stroke pipeline. One draw per segment, so meant for a handful of stylized lines,
    /// not bulk charting (use [`Ui::stroke_paths`] for that).
    pub fn stroke_custom(
        &mut self,
        desc: &ElementDesc,
        points: &[glam::Vec2],
        half_width: f32,
        feather_px: f32,
        shader: &CustomShader,
    ) -> bool {
        let flat: Vec<f32> = points.iter().flat_map(|p| [p.x, p.y]).collect();
        ffi::ui_stroke_custom(&desc.to_ffi(), &flat, half_width, feather_px, &shader.to_ffi())
    }
}

/// Chart kind for [`GraphDesc::graph_type`].
pub use ffi::UiGraphType as GraphType;
/// How multiple bar series share a category, for [`GraphDesc::bar_stack_mode`].
pub use ffi::UiBarStackMode as BarStackMode;
/// Axis scale for [`GraphAxis::scale`].
pub use ffi::UiScaleKind as ScaleKind;

/// One chart axis. Defaults match `SFT::UI::AxisConfig` (linear, autoscaled with 5% padding, ~6
/// ticks, gridlines on). Custom scale transforms and tick-label formatters (C++ `std::function`s)
/// aren't available through this binding.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphAxis {
    pub scale: ScaleKind,
    pub log_base: f64,
    pub symlog_linear_threshold: f64,
    /// `None` autoscales from the data.
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub autoscale_padding_percent: f32,
    pub target_tick_count: u32,
    pub show_gridlines: bool,
    pub show_minor_gridlines: bool,
    pub axis_color: glam::Vec4,
    pub gridline_color: glam::Vec4,
    pub minor_gridline_color: glam::Vec4,
    pub title: String,
    /// Categorical axes label ticks with `categories` instead of numbers (bar charts).
    pub is_categorical: bool,
    pub categories: Vec<String>,
}

impl Default for GraphAxis {
    fn default() -> Self {
        Self {
            scale: ScaleKind::Linear,
            log_base: 10.0,
            symlog_linear_threshold: 1.0,
            min: None,
            max: None,
            autoscale_padding_percent: 0.05,
            target_tick_count: 6,
            show_gridlines: true,
            show_minor_gridlines: false,
            axis_color: glam::vec4(0.45, 0.47, 0.53, 1.0),
            gridline_color: glam::vec4(1.0, 1.0, 1.0, 0.06),
            minor_gridline_color: glam::vec4(1.0, 1.0, 1.0, 0.03),
            title: String::new(),
            is_categorical: false,
            categories: Vec::new(),
        }
    }
}

impl GraphAxis {
    fn to_ffi(&self) -> ffi::UiGraphAxis {
        ffi::UiGraphAxis {
            scale: self.scale,
            log_base: self.log_base,
            symlog_linear_threshold: self.symlog_linear_threshold,
            has_min: self.min.is_some(),
            min: self.min.unwrap_or(0.0),
            has_max: self.max.is_some(),
            max: self.max.unwrap_or(0.0),
            autoscale_padding_percent: self.autoscale_padding_percent,
            target_tick_count: self.target_tick_count,
            show_gridlines: self.show_gridlines,
            show_minor_gridlines: self.show_minor_gridlines,
            axis_color: self.axis_color.into(),
            gridline_color: self.gridline_color.into(),
            minor_gridline_color: self.minor_gridline_color.into(),
            title: self.title.clone(),
            is_categorical: self.is_categorical,
            categories: self.categories.clone(),
        }
    }
}

/// One data series. `x` and `y` must be the same length. Defaults match `SFT::UI::SeriesRef`.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphSeries {
    pub name: String,
    pub x: Vec<f64>,
    pub y: Vec<f64>,
    pub color: glam::Vec4,
    pub line_width: f32,
    pub feather_px: f32,
    pub area_fill_opacity: f32,
    /// `> 0` draws a dot at each point (and is the dot size for scatter plots).
    pub marker_radius: f32,
    pub glow_intensity: f32,
}

impl Default for GraphSeries {
    fn default() -> Self {
        Self {
            name: String::new(),
            x: Vec::new(),
            y: Vec::new(),
            color: glam::vec4(0.4, 0.7, 1.0, 1.0),
            line_width: 2.0,
            feather_px: 1.0,
            area_fill_opacity: 0.35,
            marker_radius: 0.0,
            glow_intensity: 0.0,
        }
    }
}

/// One slice of a [`GraphType::Pie`] chart.
#[derive(Debug, Clone, PartialEq)]
pub struct PieSlice {
    pub name: String,
    pub value: f64,
    pub color: glam::Vec4,
}

/// A chart for [`Ui::graph`]. Defaults match `SFT::UI::GraphDesc`.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphDesc {
    pub graph_type: GraphType,
    pub x_axis: GraphAxis,
    pub y_axis: GraphAxis,
    pub series: Vec<GraphSeries>,
    pub bar_stack_mode: BarStackMode,
    pub bar_group_gap_fraction: f32,
    pub bar_series_gap_fraction: f32,
    pub pie_slices: Vec<PieSlice>,
    /// `0` is a full pie; `0.5` a donut with a hole half the radius.
    pub pie_hole_ratio: f32,
    pub pie_start_angle_degrees: f32,
    pub pie_gap_degrees: f32,
    pub pie_feather_px: f32,
    pub background: glam::Vec4,
    pub corner_radius: CornerRadius,
    pub axis_margin_left: f32,
    pub axis_margin_bottom: f32,
    pub axis_margin_top: f32,
    pub axis_margin_right: f32,
    pub font_id: u16,
    pub label_font_size: u16,
    pub title_font_size: u16,
    pub show_legend: bool,
}

impl Default for GraphDesc {
    fn default() -> Self {
        Self {
            graph_type: GraphType::Line,
            x_axis: GraphAxis::default(),
            y_axis: GraphAxis::default(),
            series: Vec::new(),
            bar_stack_mode: BarStackMode::Grouped,
            bar_group_gap_fraction: 0.2,
            bar_series_gap_fraction: 0.08,
            pie_slices: Vec::new(),
            pie_hole_ratio: 0.0,
            pie_start_angle_degrees: -90.0,
            pie_gap_degrees: 0.0,
            pie_feather_px: 1.0,
            background: glam::vec4(0.07, 0.08, 0.1, 1.0),
            corner_radius: CornerRadius::all(6.0),
            axis_margin_left: 48.0,
            axis_margin_bottom: 24.0,
            axis_margin_top: 10.0,
            axis_margin_right: 10.0,
            font_id: 0,
            label_font_size: 11,
            title_font_size: 12,
            show_legend: true,
        }
    }
}

impl GraphDesc {
    fn to_ffi(&self) -> ffi::UiGraphDesc {
        ffi::UiGraphDesc {
            graph_type: self.graph_type,
            x_axis: self.x_axis.to_ffi(),
            y_axis: self.y_axis.to_ffi(),
            series: self
                .series
                .iter()
                .map(|s| ffi::UiGraphSeries {
                    name: s.name.clone(),
                    x: s.x.clone(),
                    y: s.y.clone(),
                    color: s.color.into(),
                    line_width: s.line_width,
                    feather_px: s.feather_px,
                    area_fill_opacity: s.area_fill_opacity,
                    marker_radius: s.marker_radius,
                    glow_intensity: s.glow_intensity,
                })
                .collect(),
            bar_stack_mode: self.bar_stack_mode,
            bar_group_gap_fraction: self.bar_group_gap_fraction,
            bar_series_gap_fraction: self.bar_series_gap_fraction,
            pie_slices: self
                .pie_slices
                .iter()
                .map(|p| ffi::UiPieSlice { name: p.name.clone(), value: p.value, color: p.color.into() })
                .collect(),
            pie_hole_ratio: self.pie_hole_ratio,
            pie_start_angle_degrees: self.pie_start_angle_degrees,
            pie_gap_degrees: self.pie_gap_degrees,
            pie_feather_px: self.pie_feather_px,
            background: self.background.into(),
            corner_radius: self.corner_radius.to_array(),
            axis_margins: [self.axis_margin_left, self.axis_margin_bottom, self.axis_margin_top, self.axis_margin_right],
            font_id: self.font_id,
            label_font_size: self.label_font_size,
            title_font_size: self.title_font_size,
            show_legend: self.show_legend,
        }
    }
}

/// Line-end shape for [`StrokeStyle::cap`].
pub use ffi::UiStrokeCap as StrokeCap;
/// Corner shape for [`StrokeStyle::join`].
pub use ffi::UiStrokeJoin as StrokeJoin;

/// How a [`StrokePath`] is drawn. Defaults match `SFT::UI::StrokeStyle` (1 px solid black, round
/// caps/joins). `dash_length == 0` means solid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokeStyle {
    /// Straight-alpha sRGB.
    pub color: glam::Vec4,
    pub width: f32,
    pub feather_px: f32,
    pub dash_length: f32,
    pub dash_gap: f32,
    pub cap: StrokeCap,
    pub join: StrokeJoin,
    pub snap_to_pixel_grid: bool,
    pub glow_intensity: f32,
}

impl Default for StrokeStyle {
    fn default() -> Self {
        Self {
            color: glam::vec4(0.0, 0.0, 0.0, 1.0),
            width: 1.0,
            feather_px: 0.0,
            dash_length: 0.0,
            dash_gap: 0.0,
            cap: StrokeCap::Round,
            join: StrokeJoin::Round,
            snap_to_pixel_grid: false,
            glow_intensity: 0.0,
        }
    }
}

impl StrokeStyle {
    fn to_ffi(self) -> ffi::UiStrokeStyle {
        ffi::UiStrokeStyle {
            color: self.color.into(),
            width: self.width,
            feather_px: self.feather_px,
            dash_length: self.dash_length,
            dash_gap: self.dash_gap,
            cap: self.cap,
            join: self.join,
            snap_to_pixel_grid: self.snap_to_pixel_grid,
            glow_intensity: self.glow_intensity,
        }
    }
}

/// One polyline for [`Ui::stroke_paths`]; `points` are element-local.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StrokePath {
    pub points: Vec<glam::Vec2>,
    pub style: StrokeStyle,
}

impl StrokePath {
    fn to_ffi(&self) -> ffi::UiStrokePath {
        ffi::UiStrokePath { points: self.points.iter().flat_map(|p| [p.x, p.y]).collect(), style: self.style.to_ffi() }
    }
}

/// One filled rect for [`Ui::fill_quads`]; `position`/`size` are element-local.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct FillQuad {
    pub position: glam::Vec2,
    pub size: glam::Vec2,
    /// Straight-alpha sRGB.
    pub color: glam::Vec4,
    pub corner_radius: CornerRadius,
}

impl FillQuad {
    fn to_ffi(self) -> ffi::UiFillQuad {
        ffi::UiFillQuad {
            position: self.position.into(),
            size: self.size.into(),
            color: self.color.into(),
            corner_radius: self.corner_radius.to_array(),
        }
    }
}

/// One annular sector ("pie slice") for [`Ui::fill_sectors`]; `center` is element-local, angles in
/// radians. `inner_radius = 0` gives a plain pie wedge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sector {
    pub center: glam::Vec2,
    pub inner_radius: f32,
    pub outer_radius: f32,
    pub start_angle: f32,
    pub end_angle: f32,
    /// Straight-alpha sRGB.
    pub color: glam::Vec4,
    pub feather_px: f32,
}

impl Default for Sector {
    fn default() -> Self {
        Self {
            center: glam::Vec2::ZERO,
            inner_radius: 0.0,
            outer_radius: 1.0,
            start_angle: 0.0,
            end_angle: 0.0,
            color: glam::Vec4::ONE,
            feather_px: 1.0,
        }
    }
}

impl Sector {
    fn to_ffi(self) -> ffi::UiSector {
        ffi::UiSector {
            center: self.center.into(),
            inner_radius: self.inner_radius,
            outer_radius: self.outer_radius,
            start_angle: self.start_angle,
            end_angle: self.end_angle,
            color: self.color.into(),
            feather_px: self.feather_px,
        }
    }
}

/// A Slang shader for [`Ui::custom_element`]/[`Ui::stroke_custom`] (`SFT::UI::CustomShaderRef`).
///
/// Both entry points live in `shader_path`. `vertexMain` must build its geometry through
/// `Shaders/sturdy_common.slang`'s `uiQuadClipPosition()` (custom elements) or
/// `uiStrokeSegmentClipPosition()` (custom strokes), reading a `[[push_constant]]` struct whose
/// prefix the engine writes (32 bytes for elements, 48 for strokes); `push_constants` is appended
/// right after it, and the total must equal the shader's reflected push-constant size (Slang packs
/// HLSL-cbuffer-style, so a `float4` aligns to 16 bytes). Push constants only -- no texture or
/// buffer bindings.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct CustomShader {
    pub shader_path: String,
    pub module_name: String,
    /// `None` uses the engine default, `"fragmentMain"`.
    pub fragment_entry_point: Option<String>,
    pub push_constants: Vec<u8>,
}

impl CustomShader {
    fn to_ffi(&self) -> ffi::UiCustomShader {
        ffi::UiCustomShader {
            shader_path: self.shader_path.clone(),
            module_name: self.module_name.clone(),
            fragment_entry_point: self.fragment_entry_point.clone().unwrap_or_default(),
            push_constants: self.push_constants.clone(),
        }
    }
}

/// Scroll position/extent of a clipped [`ElementDesc`], as reported by [`Ui::scroll_metrics`].
/// Mirrors `SFT::UI::Context::ScrollMetrics`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollMetrics {
    pub offset: glam::Vec2,
    pub content_size: glam::Vec2,
    pub container_size: glam::Vec2,
    /// Whether the container actually scrolls (has overflow) along each axis.
    pub horizontal: bool,
    pub vertical: bool,
}

/// Closes the [`ElementDesc`] opened by [`Ui::element`] when dropped.
#[must_use]
pub struct ElementScope {
    open: bool,
}

impl Drop for ElementScope {
    fn drop(&mut self) {
        if self.open {
            ffi::ui_end_element();
        }
    }
}

/// Installs (or replaces) the process-wide per-frame UI draw closure. Called once per frame once
/// the engine-loop wiring described in this module's doc comment exists; `draw` builds the whole
/// UI tree for that frame through the [`Ui`] it's given.
pub fn install_draw_hook<F>(mut draw: F)
where
    F: FnMut(&mut Ui<'_>) + Send + 'static,
{
    let handle = UiDrawHandle {
        draw: Box::new(move |view: Pin<&mut EngineView>| {
            let mut ui = Ui::new(view);
            draw(&mut ui);
        }),
    };
    ffi::ui_set_draw_hook(Box::new(handle));
}

/// Removes whatever draw hook is currently installed.
pub fn clear_draw_hook() {
    ffi::ui_clear_draw_hook();
}

// ---------------------------------------------------------------------------------------------
// Docking
// ---------------------------------------------------------------------------------------------

/// Which part of a target leaf a docked panel lands in: `Center` adds it as another tab,
/// the edges split the leaf. Mirrors `SFT::UI::Docking::DockDropZone`.
pub use ffi::UiDockZone as DockZone;

/// Where to insert a panel in [`DockWorkspace::add_panel`]. Mirrors
/// `SFT::UI::Docking::DockPlacement`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DockPlacement {
    /// A leaf node id, e.g. from [`DockWorkspace::focused_leaf`].
    pub target_node: u32,
    pub zone: DockZone,
}

/// What the user did to a [`DockWorkspace`] this frame. The workspace doesn't act on these
/// itself -- remove a closed panel with [`DockWorkspace::remove_panel`], or open a torn-off panel
/// in its own window, as your app sees fit.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct DockEvents {
    /// Panels dragged out of the workspace, with their workspace-local drop position.
    pub tear_offs: Vec<(String, glam::Vec2)>,
    /// Panels whose close button was clicked.
    pub close_requests: Vec<String>,
}

/// An editor-style docking layout (`SFT::UI::Docking::DockWorkspace`): panels as tabs in leaves
/// that can be split, resized by dragging dividers, reordered and re-docked by dragging tabs, and
/// torn off. Create it once and keep it (e.g. captured in the draw hook closure); each frame,
/// inside the draw hook:
///
/// ```ignore
/// workspace.begin_frame(ui, glam::vec4(0.0, 0.0, 1280.0, 720.0), dt);
/// for id in ["scene", "inspector"] {
///     if let Some(_content) = workspace.panel_content(ui, id) {
///         // build this panel's UI as children
///     }
/// }
/// let events = workspace.end_frame(ui);
/// ```
pub struct DockWorkspace {
    inner: UniquePtr<ffi::DockWorkspace>,
}

impl DockWorkspace {
    /// `id_prefix` namespaces the element ids the workspace's chrome generates; make it unique
    /// per workspace.
    pub fn new(id_prefix: &str) -> Self {
        Self { inner: ffi::ui_dock_workspace_new(id_prefix) }
    }

    /// Adds a panel (tab). `None` placement lets the workspace choose (the focused leaf, else the
    /// root). Returns `false` if `id` already exists or `placement` is invalid.
    pub fn add_panel(&mut self, id: &str, title: &str, closable: bool, placement: Option<DockPlacement>) -> bool {
        let placement = match placement {
            Some(p) => ffi::UiDockPlacement { has_placement: true, target_node: p.target_node, zone: p.zone },
            None => ffi::UiDockPlacement::default(),
        };
        ffi::ui_dock_add_panel(self.inner.pin_mut(), id, title, closable, &placement)
    }

    pub fn remove_panel(&mut self, id: &str) {
        ffi::ui_dock_remove_panel(self.inner.pin_mut(), id);
    }

    pub fn has_panel(&self, id: &str) -> bool {
        ffi::ui_dock_has_panel(&self.inner, id)
    }

    pub fn is_empty(&self) -> bool {
        ffi::ui_dock_is_empty(&self.inner)
    }

    /// The leaf the user last interacted with, as a [`DockPlacement::target_node`].
    pub fn focused_leaf(&self) -> Option<u32> {
        u32::try_from(ffi::ui_dock_focused_leaf(&self.inner)).ok()
    }

    pub fn set_content_background(&mut self, color: impl Into<glam::Vec4>) {
        ffi::ui_dock_set_content_background(self.inner.pin_mut(), color.into().into());
    }

    /// Lays out the workspace in `rect` (`x, y, width, height`) and draws its chrome for this
    /// frame. Call before any [`DockWorkspace::panel_content`].
    pub fn begin_frame(&mut self, _ui: &mut Ui<'_>, rect: glam::Vec4, delta_seconds: f32) -> bool {
        ffi::ui_dock_begin_frame(self.inner.pin_mut(), rect.into(), delta_seconds)
    }

    /// Opens an element covering panel `id`'s content area, if it's the visible tab of its leaf
    /// this frame. Build the panel's UI as children while the returned scope is alive.
    #[must_use]
    pub fn panel_content(&self, _ui: &mut Ui<'_>, id: &str) -> Option<ElementScope> {
        ffi::ui_dock_begin_panel_content(&self.inner, id).then_some(ElementScope { open: true })
    }

    /// Finishes the frame and returns what the user did to the workspace.
    pub fn end_frame(&mut self, _ui: &mut Ui<'_>) -> DockEvents {
        let events = ffi::ui_dock_end_frame(self.inner.pin_mut());
        DockEvents {
            tear_offs: events
                .tear_off_panels
                .into_iter()
                .zip(events.tear_off_positions.chunks_exact(2).map(|p| glam::vec2(p[0], p[1])))
                .collect(),
            close_requests: events.close_requests,
        }
    }
}
