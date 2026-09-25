//! Render-graph settings, multi-window/surface management, offscreen render targets, and
//! presentation/HDR control.
//!
//! ## Render-graph settings: how this attaches to a frame
//!
//! [`RenderGraphSettings`] (and its per-category pieces: [`SceneRenderSettings`],
//! [`ShadowSettings`], [`AmbientOcclusionSettings`], [`AntiAliasingSettings`], [`BloomSettings`],
//! [`ToneMappingSettings`], [`DebugOverlayRenderSettings`], [`RestirGiSettings`],
//! [`MotionBlurSettings`]) is wired into
//! [`crate::FrameDescription::render_graph`]: `FrameDescription::to_ffi`-equivalent plumbing (in
//! `sturdy/src/runtime.rs`) calls `sturdy_sys::render::ffi::set_pending_render_graph` with the
//! settings' `to_ffi()` output right before building the frame's `FrameRequest`, and `shim.cpp`'s
//! `RustGameLogic::request_render_frame` consumes that pending value
//! (`sturdy_rs::render::consume_pending_render_graph`) to fill in
//! `Engine::RenderFrameParameters::render_graph`. [`validate_render_graph_settings`] exercises the
//! same conversion path (`sturdy_rs::render::render_graph_settings_validate` in `sturdy-sys`, which
//! builds a real `SFT::Engine::RenderGraphDescription` and runs `RenderGraph::validate()` on it)
//! without attaching it to any frame.
//!
//! Out of scope here: anything that requires a live `RenderGraph`/frame handle rather than the
//! description alone (custom passes, `RenderGraphModule` registration).
//!
//! Scene ambient lighting (`Engine::RenderFrameParameters::lighting`) is a separate, much simpler
//! field and is bound directly as [`crate::SceneLighting`]/[`crate::FrameDescription::lighting`]
//! rather than through this module — see `sturdy/src/engine.rs`.

use sturdy_sys::render::ffi;

use crate::engine::Engine;
use crate::{LatencyMode, PresentationPreference, VSync, VariableRefresh, WindowMode};
use crate::Error;

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

/// Defines a plain mirror of an `ffi` enum with a one-way `From<Self> for ffi::$ffi` conversion.
/// These render-graph-settings enums are write-only (settings are never read back from the
/// engine), so no reverse conversion is generated; add one by hand (see [`HdrColorSpace`] for the
/// pattern) if a future category needs to read a settings enum back.
macro_rules! mirrored_enum {
    ($(#[$m:meta])* $name:ident => $ffi:ident { $($variant:ident),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }

        impl From<$name> for ffi::$ffi {
            fn from(value: $name) -> Self {
                match value { $($name::$variant => ffi::$ffi::$variant),+ }
            }
        }
    };
}

// ---------------------------------------------------------------------------------------------
// Windows / surfaces
// ---------------------------------------------------------------------------------------------

/// A render surface: the primary window, or one opened with [`Engine::open_window`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SurfaceHandle {
    pub(crate) window_id: u64,
}

impl SurfaceHandle {
    /// Wraps a raw window ID (e.g. [`crate::Frame::window_id`]) as a surface handle.
    pub const fn from_window_id(window_id: u64) -> Self {
        Self { window_id }
    }

    pub const fn window_id(self) -> u64 {
        self.window_id
    }

    pub const fn is_valid(self) -> bool {
        self.window_id != u64::MAX
    }

    fn to_ffi(self) -> ffi::SurfaceHandle {
        ffi::SurfaceHandle { window_id: self.window_id }
    }

    fn from_ffi(handle: ffi::SurfaceHandle) -> Self {
        Self { window_id: handle.window_id }
    }
}

/// Describes an additional OS window to open with [`Engine::open_window`].
#[derive(Debug, Clone, PartialEq)]
pub struct WindowDesc {
    pub title: String,
    pub size: glam::UVec2,
    /// Ignored unless `use_default_position` is `false`.
    pub position: glam::IVec2,
    pub use_default_position: bool,
    pub visible: bool,
    pub resizable: bool,
    pub decorated: bool,
    pub high_dpi: bool,
    pub transparent: bool,
    pub window_mode: WindowMode,
    pub desired_frames_in_flight: u32,
}

impl WindowDesc {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            size: glam::uvec2(1280, 720),
            position: glam::IVec2::ZERO,
            use_default_position: true,
            visible: true,
            resizable: true,
            decorated: true,
            high_dpi: true,
            transparent: false,
            window_mode: WindowMode::Windowed,
            desired_frames_in_flight: 2,
        }
    }

    setters! {
        size: glam::UVec2,
        position: glam::IVec2,
        use_default_position: bool,
        visible: bool,
        resizable: bool,
        decorated: bool,
        high_dpi: bool,
        transparent: bool,
        window_mode: WindowMode,
        desired_frames_in_flight: u32,
    }

    fn to_ffi(&self) -> ffi::WindowDesc {
        ffi::WindowDesc {
            title: self.title.clone(),
            width: self.size.x,
            height: self.size.y,
            x: self.position.x,
            y: self.position.y,
            use_default_position: self.use_default_position,
            visible: self.visible,
            resizable: self.resizable,
            decorated: self.decorated,
            high_dpi: self.high_dpi,
            transparent: self.transparent,
            window_mode: self.window_mode.into(),
            desired_frames_in_flight: self.desired_frames_in_flight,
        }
    }
}

impl Engine<'_> {
    /// Opens an additional OS window with its own render surface.
    pub fn open_window(&mut self, desc: &WindowDesc) -> Result<SurfaceHandle, Error> {
        let result = ffi::window_open(self.view_mut(), &desc.to_ffi());
        if result.ok { Ok(SurfaceHandle::from_ffi(result.surface)) } else { Err(Error::new(result.message)) }
    }

    /// Tears down `old_surface`'s window and opens a replacement, e.g. to change its graphics
    /// backend affinity or recover from a lost surface.
    pub fn recreate_window(&mut self, old_surface: SurfaceHandle, desc: &WindowDesc) -> Result<SurfaceHandle, Error> {
        let result = ffi::window_recreate(self.view_mut(), old_surface.to_ffi(), &desc.to_ffi());
        if result.ok { Ok(SurfaceHandle::from_ffi(result.surface)) } else { Err(Error::new(result.message)) }
    }

    /// Closes a window previously opened with [`Engine::open_window`]. A no-op for a surface this
    /// `Engine` did not open (e.g. the primary window).
    pub fn close_window(&mut self, surface: SurfaceHandle) {
        ffi::window_close(self.view_mut(), surface.to_ffi());
    }

    /// Notifies the engine that `surface` must be resized, e.g. after an externally driven resize.
    pub fn notify_surface_resize(&mut self, surface: SurfaceHandle, size: impl Into<glam::UVec2>) {
        let size = size.into();
        ffi::window_notify_resize(self.view_mut(), surface.to_ffi(), size.x, size.y);
    }

    // -- Window runtime mutation -----------------------------------------------------------------
    //
    // All queued via the engine's own `WindowRequests` and applied on its next update, same as
    // `sturdy-sys`'s doc comment on these describes — enqueue-and-forget, no result to check (a
    // request naming a surface that no longer exists is silently dropped by the engine itself).

    /// Requests the OS cursor icon change while hovering `surface`'s window.
    pub fn set_cursor_icon(&mut self, surface: SurfaceHandle, icon: CursorIcon) {
        ffi::window_set_cursor_icon(self.view_mut(), surface.to_ffi(), icon.into());
    }

    /// Requests the OS mouse cursor be confined to (`true`) or released from (`false`) `surface`'s
    /// window bounds.
    pub fn set_cursor_grabbed(&mut self, surface: SurfaceHandle, grabbed: bool) {
        ffi::window_set_cursor_grabbed(self.view_mut(), surface.to_ffi(), grabbed);
    }

    /// Requests a fullscreen/windowed mode change at runtime, as opposed to
    /// [`WindowDesc::window_mode`], which only sets the window's *initial* mode at creation.
    pub fn set_window_mode(&mut self, surface: SurfaceHandle, mode: WindowMode) {
        ffi::window_set_mode(self.view_mut(), surface.to_ffi(), mode.into());
    }

    /// Requests the window's OS-drawn title bar/border be shown (`true`) or hidden (`false`).
    pub fn set_decorated(&mut self, surface: SurfaceHandle, decorated: bool) {
        ffi::window_set_decorated(self.view_mut(), surface.to_ffi(), decorated);
    }

    /// Requests the window's background become see-through (`true`) or opaque (`false`) --
    /// platform/compositor support permitting.
    pub fn set_transparent(&mut self, surface: SurfaceHandle, transparent: bool) {
        ffi::window_set_transparent(self.view_mut(), surface.to_ffi(), transparent);
    }

    /// Requests relative (delta-based, cursor-hidden) mouse motion mode for `surface`'s window --
    /// the shape most first-person camera controls want.
    pub fn set_relative_mouse_mode(&mut self, surface: SurfaceHandle, enabled: bool) {
        ffi::window_set_relative_mouse_mode(self.view_mut(), surface.to_ffi(), enabled);
    }

    /// Requests the OS mouse cursor be locked in place (still visible, unlike
    /// [`Engine::set_relative_mouse_mode`]) while over `surface`'s window.
    pub fn set_mouse_locked(&mut self, surface: SurfaceHandle, locked: bool) {
        ffi::window_set_mouse_locked(self.view_mut(), surface.to_ffi(), locked);
    }

    /// Requests starting (`true`) or stopping (`false`) IME text composition for `surface`'s
    /// window.
    pub fn set_text_input_active(&mut self, surface: SurfaceHandle, active: bool) {
        ffi::window_set_text_input_active(self.view_mut(), surface.to_ffi(), active);
    }

    /// Requests the IME composition UI be anchored at `area` (window-client-area-relative) for
    /// `surface`'s window -- has no visible effect unless text input is also active (see
    /// [`Engine::set_text_input_active`]).
    pub fn set_text_input_area(&mut self, surface: SurfaceHandle, area: TextInputArea) {
        ffi::window_set_text_input_area(self.view_mut(), surface.to_ffi(), area.to_ffi());
    }

    /// Requests a blur/vibrancy-style window effect (`kind`) be enabled or disabled for
    /// `surface`'s window -- platform/compositor support permitting; a request for an unsupported
    /// `kind` is silently ignored by the engine.
    pub fn set_window_effect(&mut self, surface: SurfaceHandle, kind: WindowEffectKind, enabled: bool) {
        ffi::window_set_effect(self.view_mut(), surface.to_ffi(), kind.into(), enabled);
    }
}

mirrored_enum!(
    /// The OS cursor icon while hovering a window. See [`Engine::set_cursor_icon`].
    CursorIcon => CursorIcon {
        Default, Pointer, Text, Grab, Grabbing, ResizeHorizontal, ResizeVertical, ResizeNwse,
        ResizeNesw, NotAllowed,
    }
);

mirrored_enum!(
    /// A blur/vibrancy-style window effect. See [`Engine::set_window_effect`].
    WindowEffectKind => WindowEffectKind {
        Blur, Acrylic, Mica, MicaAlt, Tabbed, DarkMode, BorderColor, CaptionColor, TextColor,
        Transparent,
    }
);

/// The on-screen rectangle (window-client-area-relative) an IME should anchor its composition UI
/// to, plus where within it the text caret sits. See [`Engine::set_text_input_area`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct TextInputArea {
    pub position: glam::Vec2,
    pub size: glam::Vec2,
    pub cursor_offset_x: f32,
}

impl TextInputArea {
    fn to_ffi(self) -> ffi::TextInputArea {
        ffi::TextInputArea {
            x: self.position.x,
            y: self.position.y,
            width: self.size.x,
            height: self.size.y,
            cursor_offset_x: self.cursor_offset_x,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Offscreen render targets
// ---------------------------------------------------------------------------------------------

/// Handle to an offscreen render target created with [`Engine::create_offscreen_target`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct OffscreenTargetHandle {
    pub(crate) value: u64,
}

impl OffscreenTargetHandle {
    pub const fn is_valid(self) -> bool {
        self.value != 0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffscreenTargetDesc {
    pub size: glam::UVec2,
    pub label: String,
}

impl OffscreenTargetDesc {
    pub fn new(size: impl Into<glam::UVec2>) -> Self {
        Self { size: size.into(), label: String::new() }
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }
}

impl Engine<'_> {
    pub fn create_offscreen_target(&mut self, desc: &OffscreenTargetDesc) -> Result<OffscreenTargetHandle, Error> {
        let ffi_desc = ffi::OffscreenTargetDesc { width: desc.size.x, height: desc.size.y, label: desc.label.clone() };
        let result = ffi::offscreen_target_create(self.view_mut(), &ffi_desc);
        if result.ok { Ok(OffscreenTargetHandle { value: result.handle }) } else { Err(Error::new(result.message)) }
    }

    pub fn destroy_offscreen_target(&mut self, handle: OffscreenTargetHandle) {
        ffi::offscreen_target_destroy(self.view_mut(), handle.value);
    }

    pub fn offscreen_target_description(&self, handle: OffscreenTargetHandle) -> Option<OffscreenTargetDesc> {
        let mut out = ffi::OffscreenTargetDesc { width: 0, height: 0, label: String::new() };
        ffi::offscreen_target_description(self.view(), handle.value, &mut out)
            .then(|| OffscreenTargetDesc { size: glam::uvec2(out.width, out.height), label: out.label })
    }

    /// The target's backing texture, as a raw `Renderer::TextureHandle` value (`0` if unknown).
    /// Opaque until a material/texture-binding API exists in `sturdy`; kept as a raw handle so
    /// downstream code that reaches for the RHI directly (or a future `sturdy` texture API) has
    /// something to key off today.
    pub fn offscreen_target_texture(&self, handle: OffscreenTargetHandle) -> u64 {
        ffi::offscreen_target_texture(self.view(), handle.value)
    }
}

// ---------------------------------------------------------------------------------------------
// Presentation
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HdrColorSpace {
    Hdr10St2084,
    ScrgbLinear,
    Hdr10Hlg,
    DolbyVision,
}

impl From<HdrColorSpace> for ffi::HdrColorSpaceMode {
    fn from(value: HdrColorSpace) -> Self {
        match value {
            HdrColorSpace::Hdr10St2084 => ffi::HdrColorSpaceMode::Hdr10St2084,
            HdrColorSpace::ScrgbLinear => ffi::HdrColorSpaceMode::ScrgbLinear,
            HdrColorSpace::Hdr10Hlg => ffi::HdrColorSpaceMode::Hdr10Hlg,
            HdrColorSpace::DolbyVision => ffi::HdrColorSpaceMode::DolbyVision,
        }
    }
}

impl From<ffi::HdrColorSpaceMode> for HdrColorSpace {
    fn from(value: ffi::HdrColorSpaceMode) -> Self {
        match value {
            ffi::HdrColorSpaceMode::ScrgbLinear => HdrColorSpace::ScrgbLinear,
            ffi::HdrColorSpaceMode::Hdr10Hlg => HdrColorSpace::Hdr10Hlg,
            ffi::HdrColorSpaceMode::DolbyVision => HdrColorSpace::DolbyVision,
            _ => HdrColorSpace::Hdr10St2084,
        }
    }
}

/// A surface's vsync/HDR/composition policy. See [`Engine::presentation_settings`] /
/// [`Engine::set_presentation_settings`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentationSettings {
    pub vsync: VSync,
    pub variable_refresh: VariableRefresh,
    pub latency: LatencyMode,
    pub preference: PresentationPreference,
    pub hdr_enabled: bool,
    pub hdr_color_space: HdrColorSpace,
    pub transparent_composition: bool,
    /// `0` lets the engine choose the swapchain image count.
    pub swapchain_image_count: u32,
    pub allow_present_from_compute: bool,
}

impl Default for PresentationSettings {
    fn default() -> Self {
        Self {
            vsync: VSync::On,
            variable_refresh: VariableRefresh::Disabled,
            latency: LatencyMode::Normal,
            preference: PresentationPreference::Automatic,
            hdr_enabled: false,
            hdr_color_space: HdrColorSpace::Hdr10St2084,
            transparent_composition: false,
            swapchain_image_count: 0,
            allow_present_from_compute: false,
        }
    }
}

impl PresentationSettings {
    setters! {
        vsync: VSync,
        variable_refresh: VariableRefresh,
        latency: LatencyMode,
        preference: PresentationPreference,
        hdr_enabled: bool,
        hdr_color_space: HdrColorSpace,
        transparent_composition: bool,
        swapchain_image_count: u32,
        allow_present_from_compute: bool,
    }

    fn to_ffi(self) -> ffi::PresentationSettings {
        ffi::PresentationSettings {
            vsync: self.vsync.into(),
            variable_refresh: self.variable_refresh.into(),
            latency: self.latency.into(),
            preference: self.preference.into(),
            hdr_enabled: self.hdr_enabled,
            hdr_color_space: self.hdr_color_space.into(),
            transparent_composition: self.transparent_composition,
            swapchain_image_count: self.swapchain_image_count,
            allow_present_from_compute: self.allow_present_from_compute,
        }
    }

    fn from_ffi(settings: ffi::PresentationSettings) -> Self {
        Self {
            vsync: settings.vsync.into(),
            variable_refresh: settings.variable_refresh.into(),
            latency: settings.latency.into(),
            preference: settings.preference.into(),
            hdr_enabled: settings.hdr_enabled,
            hdr_color_space: settings.hdr_color_space.into(),
            transparent_composition: settings.transparent_composition,
            swapchain_image_count: settings.swapchain_image_count,
            allow_present_from_compute: settings.allow_present_from_compute,
        }
    }
}

/// What the engine actually resolved a presentation request to; informational (e.g. for
/// diagnostics/UI), not something you construct yourself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresentationResolution {
    pub strategy: String,
    pub effective_mode: String,
    pub degraded: bool,
    pub present_queue_is_compute: bool,
    pub effective_composite_alpha: String,
    pub composite_alpha_degraded: bool,
    pub via_composition_present: bool,
    pub supports_completion_fence: bool,
    pub full_screen_exclusive_active: bool,
    /// Raw `RHI::Format` ordinal (no full mirror of that enum exists in `sturdy` yet).
    pub effective_format: u32,
    /// Raw `RHI::ColorSpace` ordinal.
    pub effective_color_space: u32,
}

impl Engine<'_> {
    pub fn presentation_settings(&self, surface: SurfaceHandle) -> PresentationSettings {
        PresentationSettings::from_ffi(ffi::presentation_settings_get(self.view(), surface.to_ffi()))
    }

    pub fn set_presentation_settings(&mut self, surface: SurfaceHandle, settings: PresentationSettings) -> Result<(), Error> {
        let result = ffi::presentation_settings_set(self.view_mut(), surface.to_ffi(), &settings.to_ffi());
        if result.ok { Ok(()) } else { Err(Error::new(result.message)) }
    }

    pub fn presentation_resolution(&self, surface: SurfaceHandle) -> PresentationResolution {
        let r = ffi::presentation_resolution_query(self.view(), surface.to_ffi());
        PresentationResolution {
            strategy: r.strategy,
            effective_mode: r.effective_mode,
            degraded: r.degraded,
            present_queue_is_compute: r.present_queue_is_compute,
            effective_composite_alpha: r.effective_composite_alpha,
            composite_alpha_degraded: r.composite_alpha_degraded,
            via_composition_present: r.via_composition_present,
            supports_completion_fence: r.supports_completion_fence,
            full_screen_exclusive_active: r.full_screen_exclusive_active,
            effective_format: r.effective_format,
            effective_color_space: r.effective_color_space,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// HDR
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HdrTransferFunction {
    Unknown,
    Sdr,
    PqSt2084,
    Hlg,
    LinearExtended,
}

impl From<ffi::HdrTransferFunction> for HdrTransferFunction {
    fn from(value: ffi::HdrTransferFunction) -> Self {
        match value {
            ffi::HdrTransferFunction::Sdr => HdrTransferFunction::Sdr,
            ffi::HdrTransferFunction::PqSt2084 => HdrTransferFunction::PqSt2084,
            ffi::HdrTransferFunction::Hlg => HdrTransferFunction::Hlg,
            ffi::HdrTransferFunction::LinearExtended => HdrTransferFunction::LinearExtended,
            _ => HdrTransferFunction::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HdrColorGamut {
    Unknown,
    Rec709,
    DisplayP3,
    Rec2020,
}

impl From<ffi::HdrColorGamut> for HdrColorGamut {
    fn from(value: ffi::HdrColorGamut) -> Self {
        match value {
            ffi::HdrColorGamut::Rec709 => HdrColorGamut::Rec709,
            ffi::HdrColorGamut::DisplayP3 => HdrColorGamut::DisplayP3,
            ffi::HdrColorGamut::Rec2020 => HdrColorGamut::Rec2020,
            _ => HdrColorGamut::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrPresentationMode {
    pub transfer: HdrTransferFunction,
    pub gamut: HdrColorGamut,
    pub requires_os_hdr_mode: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrDisplayMetadata {
    pub red_primary: glam::Vec2,
    pub green_primary: glam::Vec2,
    pub blue_primary: glam::Vec2,
    pub white_point: glam::Vec2,
    pub min_luminance_nits: f32,
    pub max_luminance_nits: f32,
    pub max_full_frame_luminance_nits: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct HdrCapabilities {
    pub hdr_supported: bool,
    pub hdr_enabled_by_os: bool,
    pub hdr_metadata_output_supported: bool,
    pub supported_modes: Vec<HdrPresentationMode>,
    pub display_metadata: Option<HdrDisplayMetadata>,
    pub sdr_white_nits: f32,
    pub edr_headroom: f32,
    pub max_edr_headroom: f32,
}

impl Engine<'_> {
    /// Queries the display's actual HDR capabilities (support, supported transfer/gamut modes,
    /// EDR headroom, ...) rather than what was last requested via [`Engine::set_presentation_settings`].
    pub fn hdr_capabilities(&self, surface: SurfaceHandle) -> Result<HdrCapabilities, Error> {
        let q = ffi::hdr_capabilities_query(self.view(), surface.to_ffi());
        if !q.ok {
            return Err(Error::new(q.message));
        }
        let display_metadata = q.has_display_metadata.then(|| HdrDisplayMetadata {
            red_primary: glam::vec2(q.display_metadata.red_primary_x, q.display_metadata.red_primary_y),
            green_primary: glam::vec2(q.display_metadata.green_primary_x, q.display_metadata.green_primary_y),
            blue_primary: glam::vec2(q.display_metadata.blue_primary_x, q.display_metadata.blue_primary_y),
            white_point: glam::vec2(q.display_metadata.white_point_x, q.display_metadata.white_point_y),
            min_luminance_nits: q.display_metadata.min_luminance_nits,
            max_luminance_nits: q.display_metadata.max_luminance_nits,
            max_full_frame_luminance_nits: q.display_metadata.max_full_frame_luminance_nits,
        });
        Ok(HdrCapabilities {
            hdr_supported: q.hdr_supported,
            hdr_enabled_by_os: q.hdr_enabled_by_os,
            hdr_metadata_output_supported: q.hdr_metadata_output_supported,
            supported_modes: q
                .supported_modes
                .into_iter()
                .map(|m| HdrPresentationMode {
                    transfer: m.transfer.into(),
                    gamut: m.gamut.into(),
                    requires_os_hdr_mode: m.requires_os_hdr_mode,
                })
                .collect(),
            display_metadata,
            sdr_white_nits: q.sdr_white_nits,
            edr_headroom: q.edr_headroom,
            max_edr_headroom: q.max_edr_headroom,
        })
    }

    /// Updates the HDR content light level metadata (`MaxCLL`/`MaxFALL`-style) reported to the
    /// display for `surface`.
    pub fn update_hdr_content_light_level(
        &mut self,
        surface: SurfaceHandle,
        max_content_light_level_nits: f32,
        max_frame_average_light_level_nits: f32,
    ) -> Result<(), Error> {
        let update = ffi::HdrContentLightLevelUpdate { max_content_light_level_nits, max_frame_average_light_level_nits };
        let result = ffi::hdr_update_content_light_level(self.view_mut(), surface.to_ffi(), &update);
        if result.ok { Ok(()) } else { Err(Error::new(result.message)) }
    }
}

// ---------------------------------------------------------------------------------------------
// Render-graph settings
// ---------------------------------------------------------------------------------------------

mirrored_enum!(
    AmbientOcclusionQuality => AmbientOcclusionQuality { Low, Medium, High, Ultra }
);
mirrored_enum!(
    PostProcessAntiAliasing => PostProcessAntiAliasing { None, Fxaa, ConservativeMorphological }
);
mirrored_enum!(
    ToneMappingOperator => ToneMappingOperator {
        None, Reinhard, Exponential, Agx, HermiteSpline, PsychoV
    }
);
mirrored_enum!(
    AgxLook => AgxLook { None, Punchy, Golden }
);
mirrored_enum!(
    RestirGiQuality => RestirGiQuality { Low, Medium, High }
);
mirrored_enum!(
    RestirGiDenoiser => RestirGiDenoiser { None, Svgf, DlssRayReconstruction, FsrRedstone }
);
mirrored_enum!(
    /// Single-frame shadow debug visualization; see `sturdy-sys`' `render::ffi::ShadowDebugView`
    /// (mirroring the engine's `Engine::ShadowDebugView`) for what each value shows.
    ShadowDebugView => ShadowDebugView {
        None,
        CascadeIndex,
        CascadeFade,
        ShadowTexelGrid,
        ShadowUv,
        ReceiverDepth,
        AtlasDepth,
        DepthDelta,
        NormalBias,
        ReceiverPlaneGradient,
        HardComparison,
        Pcf,
        DirectionalCsm,
        ContactShadow,
        CombinedSunVisibility,
        GbufferDepth,
        WorldPosition,
        GbufferNormal,
        GbufferAlbedo,
        GbufferRoughness,
        GbufferMetallic,
        MaterialAmbientOcclusion,
        AmbientLighting,
        SunNdotL,
        UnshadowedSunLighting,
        ScreenSpaceAmbientOcclusion,
    }
);
mirrored_enum!(
    RenderGraphExecutionMode => RenderGraphExecutionMode { FireAndForget, WaitForCompletion }
);

/// Cascaded-shadow-map settings. Defaults match `SFT::Engine::ShadowSettings`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShadowSettings {
    pub enabled: bool,
    pub atlas_size: u32,
    pub cascade_count: u32,
    pub max_distance: f32,
    pub cascade_split_lambda: f32,
    pub cascade_blend: f32,
    pub depth_bias: f32,
    pub slope_bias: f32,
    /// Per-cascade shadow-map edge resolution, near cascade first.
    pub cascade_resolutions: [u32; 4],
    pub filter_radius_texels: f32,
    pub normal_bias: f32,
    pub debug_view: ShadowDebugView,
    pub max_shadowed_spot_lights: u32,
    pub max_shadowed_point_lights: u32,
    pub contact_hardening: bool,
    pub contact_shadows: bool,
    pub contact_shadow_distance: f32,
    pub contact_shadow_thickness: f32,
    pub contact_shadow_steps: u32,
    pub contact_shadow_intensity: f32,
    pub contact_shadow_fade_distance: f32,
}

impl Default for ShadowSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            atlas_size: 4096,
            cascade_count: 4,
            max_distance: 250.0,
            cascade_split_lambda: 0.65,
            cascade_blend: 0.10,
            depth_bias: 0.75,
            slope_bias: 1.0,
            cascade_resolutions: [2048, 1024, 1024, 1024],
            filter_radius_texels: 2.0,
            normal_bias: 0.75,
            debug_view: ShadowDebugView::None,
            max_shadowed_spot_lights: 8,
            max_shadowed_point_lights: 4,
            contact_hardening: false,
            contact_shadows: true,
            contact_shadow_distance: 0.5,
            contact_shadow_thickness: 0.05,
            contact_shadow_steps: 8,
            contact_shadow_intensity: 0.85,
            contact_shadow_fade_distance: 40.0,
        }
    }
}

impl ShadowSettings {
    setters! {
        enabled: bool,
        atlas_size: u32,
        cascade_count: u32,
        max_distance: f32,
        cascade_split_lambda: f32,
        cascade_blend: f32,
        depth_bias: f32,
        slope_bias: f32,
        cascade_resolutions: [u32; 4],
        filter_radius_texels: f32,
        normal_bias: f32,
        debug_view: ShadowDebugView,
        max_shadowed_spot_lights: u32,
        max_shadowed_point_lights: u32,
        contact_hardening: bool,
        contact_shadows: bool,
        contact_shadow_distance: f32,
        contact_shadow_thickness: f32,
        contact_shadow_steps: u32,
        contact_shadow_intensity: f32,
        contact_shadow_fade_distance: f32,
    }

    fn to_ffi(self) -> ffi::ShadowSettings {
        ffi::ShadowSettings {
            enabled: self.enabled,
            atlas_size: self.atlas_size,
            cascade_count: self.cascade_count,
            max_distance: self.max_distance,
            cascade_split_lambda: self.cascade_split_lambda,
            cascade_blend: self.cascade_blend,
            depth_bias: self.depth_bias,
            slope_bias: self.slope_bias,
            cascade_resolutions: self.cascade_resolutions,
            filter_radius_texels: self.filter_radius_texels,
            normal_bias: self.normal_bias,
            debug_view: self.debug_view.into(),
            max_shadowed_spot_lights: self.max_shadowed_spot_lights,
            max_shadowed_point_lights: self.max_shadowed_point_lights,
            contact_hardening: self.contact_hardening,
            contact_shadows: self.contact_shadows,
            contact_shadow_distance: self.contact_shadow_distance,
            contact_shadow_thickness: self.contact_shadow_thickness,
            contact_shadow_steps: self.contact_shadow_steps,
            contact_shadow_intensity: self.contact_shadow_intensity,
            contact_shadow_fade_distance: self.contact_shadow_fade_distance,
        }
    }
}

/// Screen-space ambient occlusion settings. Defaults match `SFT::Engine::AmbientOcclusionSettings`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AmbientOcclusionSettings {
    pub enabled: bool,
    pub radius: f32,
    pub quality: AmbientOcclusionQuality,
    pub intensity: f32,
    pub falloff_range: f32,
    pub thin_occluder_compensation: f32,
    pub final_value_power: f32,
    pub sample_distribution_power: f32,
    pub denoise: bool,
}

impl Default for AmbientOcclusionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            radius: 1.0,
            quality: AmbientOcclusionQuality::High,
            intensity: 1.0,
            falloff_range: 0.615,
            thin_occluder_compensation: 0.0,
            final_value_power: 2.2,
            sample_distribution_power: 2.0,
            denoise: true,
        }
    }
}

impl AmbientOcclusionSettings {
    setters! {
        enabled: bool,
        radius: f32,
        quality: AmbientOcclusionQuality,
        intensity: f32,
        falloff_range: f32,
        thin_occluder_compensation: f32,
        final_value_power: f32,
        sample_distribution_power: f32,
        denoise: bool,
    }

    fn to_ffi(self) -> ffi::AmbientOcclusionSettings {
        ffi::AmbientOcclusionSettings {
            enabled: self.enabled,
            radius: self.radius,
            quality: self.quality.into(),
            intensity: self.intensity,
            falloff_range: self.falloff_range,
            thin_occluder_compensation: self.thin_occluder_compensation,
            final_value_power: self.final_value_power,
            sample_distribution_power: self.sample_distribution_power,
            denoise: self.denoise,
        }
    }
}

/// Defaults match `SFT::Engine::AntiAliasingSettings`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AntiAliasingSettings {
    pub msaa_samples: u32,
    pub post_process: PostProcessAntiAliasing,
    pub subpixel_quality: f32,
    pub edge_threshold: f32,
}

impl Default for AntiAliasingSettings {
    fn default() -> Self {
        Self {
            msaa_samples: 1,
            post_process: PostProcessAntiAliasing::Fxaa,
            subpixel_quality: 0.75,
            edge_threshold: 0.125,
        }
    }
}

impl AntiAliasingSettings {
    setters! {
        msaa_samples: u32,
        post_process: PostProcessAntiAliasing,
        subpixel_quality: f32,
        edge_threshold: f32,
    }

    fn to_ffi(self) -> ffi::AntiAliasingSettings {
        ffi::AntiAliasingSettings {
            msaa_samples: self.msaa_samples,
            post_process: self.post_process.into(),
            subpixel_quality: self.subpixel_quality,
            edge_threshold: self.edge_threshold,
        }
    }
}

/// Defaults match `SFT::Engine::BloomSettings`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BloomSettings {
    pub enabled: bool,
    pub threshold: f32,
    pub soft_knee: f32,
    pub intensity: f32,
    pub scatter: f32,
    pub downsample_ratio: f32,
    pub max_levels: u32,
}

impl Default for BloomSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold: 0.0,
            soft_knee: 0.5,
            intensity: 0.04,
            scatter: 0.7,
            downsample_ratio: 1.618_034,
            max_levels: 12,
        }
    }
}

impl BloomSettings {
    setters! {
        enabled: bool,
        threshold: f32,
        soft_knee: f32,
        intensity: f32,
        scatter: f32,
        downsample_ratio: f32,
        max_levels: u32,
    }

    fn to_ffi(self) -> ffi::BloomSettings {
        ffi::BloomSettings {
            enabled: self.enabled,
            threshold: self.threshold,
            soft_knee: self.soft_knee,
            intensity: self.intensity,
            scatter: self.scatter,
            downsample_ratio: self.downsample_ratio,
            max_levels: self.max_levels,
        }
    }
}

/// Tone-mapping settings, including the per-operator AgX/Hermite-spline/PsychoV knobs. Defaults
/// match `SFT::Engine::ToneMappingSettings`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ToneMappingSettings {
    pub enabled: bool,
    pub operation: ToneMappingOperator,
    pub exposure: f32,
    pub white_point: f32,
    pub saturation: f32,
    pub hdr_paper_white_nits: f32,
    pub hdr_peak_nits: f32,
    /// Only read when `operation == ToneMappingOperator::Agx`.
    pub agx_look: AgxLook,
    /// Only read when `operation == ToneMappingOperator::HermiteSpline`.
    pub hermite_toe_strength: f32,
    pub hermite_toe_length: f32,
    pub hermite_shoulder_strength: f32,
    pub hermite_shoulder_length: f32,
    pub hermite_shoulder_angle: f32,
    /// Only read when `operation == ToneMappingOperator::PsychoV`.
    pub psychov_highlights: f32,
    pub psychov_shadows: f32,
    pub psychov_contrast: f32,
    pub psychov_purity_scale: f32,
    pub psychov_gamut_compression: f32,
    pub psychov_gamut_compression_use_bt2020: bool,
    pub psychov_compression: f32,
    pub psychov_adapted_gray_bt709: glam::Vec3,
    pub psychov_background_gray_bt709: glam::Vec3,
}

impl Default for ToneMappingSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            operation: ToneMappingOperator::Agx,
            exposure: 1.0,
            white_point: 1.0,
            saturation: 1.0,
            hdr_paper_white_nits: 203.0,
            hdr_peak_nits: 1000.0,
            agx_look: AgxLook::None,
            hermite_toe_strength: 0.5,
            hermite_toe_length: 0.5,
            hermite_shoulder_strength: 2.0,
            hermite_shoulder_length: 0.5,
            hermite_shoulder_angle: 1.0,
            psychov_highlights: 1.0,
            psychov_shadows: 1.0,
            psychov_contrast: 1.0,
            psychov_purity_scale: 1.0,
            psychov_gamut_compression: 1.0,
            psychov_gamut_compression_use_bt2020: true,
            psychov_compression: 0.0,
            psychov_adapted_gray_bt709: glam::Vec3::splat(0.18),
            psychov_background_gray_bt709: glam::Vec3::splat(0.18),
        }
    }
}

impl ToneMappingSettings {
    setters! {
        enabled: bool,
        operation: ToneMappingOperator,
        exposure: f32,
        white_point: f32,
        saturation: f32,
        hdr_paper_white_nits: f32,
        hdr_peak_nits: f32,
        agx_look: AgxLook,
        hermite_toe_strength: f32,
        hermite_toe_length: f32,
        hermite_shoulder_strength: f32,
        hermite_shoulder_length: f32,
        hermite_shoulder_angle: f32,
        psychov_highlights: f32,
        psychov_shadows: f32,
        psychov_contrast: f32,
        psychov_purity_scale: f32,
        psychov_gamut_compression: f32,
        psychov_gamut_compression_use_bt2020: bool,
        psychov_compression: f32,
        psychov_adapted_gray_bt709: glam::Vec3,
        psychov_background_gray_bt709: glam::Vec3,
    }

    fn to_ffi(self) -> ffi::ToneMappingSettings {
        ffi::ToneMappingSettings {
            enabled: self.enabled,
            operation: self.operation.into(),
            exposure: self.exposure,
            white_point: self.white_point,
            saturation: self.saturation,
            hdr_paper_white_nits: self.hdr_paper_white_nits,
            hdr_peak_nits: self.hdr_peak_nits,
            agx_look: self.agx_look.into(),
            hermite_toe_strength: self.hermite_toe_strength,
            hermite_toe_length: self.hermite_toe_length,
            hermite_shoulder_strength: self.hermite_shoulder_strength,
            hermite_shoulder_length: self.hermite_shoulder_length,
            hermite_shoulder_angle: self.hermite_shoulder_angle,
            psychov_highlights: self.psychov_highlights,
            psychov_shadows: self.psychov_shadows,
            psychov_contrast: self.psychov_contrast,
            psychov_purity_scale: self.psychov_purity_scale,
            psychov_gamut_compression: self.psychov_gamut_compression,
            psychov_gamut_compression_use_bt2020: self.psychov_gamut_compression_use_bt2020,
            psychov_compression: self.psychov_compression,
            psychov_adapted_gray_bt709: self.psychov_adapted_gray_bt709.into(),
            psychov_background_gray_bt709: self.psychov_background_gray_bt709.into(),
        }
    }
}

/// ReSTIR GI (screen-space ray-traced indirect diffuse) settings. Defaults match
/// `SFT::Engine::RestirGiSettings`; disabled by default (it is a raytracing feature).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RestirGiSettings {
    pub enabled: bool,
    pub quality: RestirGiQuality,
    pub spatial_reuse_samples: u32,
    pub spatial_reuse_radius_px: f32,
    pub temporal_history_max: u32,
    pub max_ray_distance: f32,
    pub multi_bounce_feedback: f32,
    pub intensity: f32,
    pub denoiser: RestirGiDenoiser,
    /// Only read when `denoiser == RestirGiDenoiser::Svgf`.
    pub svgf_atrous_iterations: u32,
    pub svgf_temporal_alpha: f32,
    pub svgf_phi_normal: f32,
    pub svgf_phi_depth: f32,
    pub svgf_phi_luminance: f32,
    pub show_debug_reservoirs: bool,
}

impl Default for RestirGiSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            quality: RestirGiQuality::Medium,
            spatial_reuse_samples: 4,
            spatial_reuse_radius_px: 24.0,
            temporal_history_max: 20,
            max_ray_distance: 60.0,
            multi_bounce_feedback: 0.5,
            intensity: 1.0,
            denoiser: RestirGiDenoiser::Svgf,
            svgf_atrous_iterations: 5,
            svgf_temporal_alpha: 0.2,
            svgf_phi_normal: 128.0,
            svgf_phi_depth: 1.0,
            svgf_phi_luminance: 4.0,
            show_debug_reservoirs: false,
        }
    }
}

impl RestirGiSettings {
    setters! {
        enabled: bool,
        quality: RestirGiQuality,
        spatial_reuse_samples: u32,
        spatial_reuse_radius_px: f32,
        temporal_history_max: u32,
        max_ray_distance: f32,
        multi_bounce_feedback: f32,
        intensity: f32,
        denoiser: RestirGiDenoiser,
        svgf_atrous_iterations: u32,
        svgf_temporal_alpha: f32,
        svgf_phi_normal: f32,
        svgf_phi_depth: f32,
        svgf_phi_luminance: f32,
        show_debug_reservoirs: bool,
    }

    fn to_ffi(self) -> ffi::RestirGiSettings {
        ffi::RestirGiSettings {
            enabled: self.enabled,
            quality: self.quality.into(),
            spatial_reuse_samples: self.spatial_reuse_samples,
            spatial_reuse_radius_px: self.spatial_reuse_radius_px,
            temporal_history_max: self.temporal_history_max,
            max_ray_distance: self.max_ray_distance,
            multi_bounce_feedback: self.multi_bounce_feedback,
            intensity: self.intensity,
            denoiser: self.denoiser.into(),
            svgf_atrous_iterations: self.svgf_atrous_iterations,
            svgf_temporal_alpha: self.svgf_temporal_alpha,
            svgf_phi_normal: self.svgf_phi_normal,
            svgf_phi_depth: self.svgf_phi_depth,
            svgf_phi_luminance: self.svgf_phi_luminance,
            show_debug_reservoirs: self.show_debug_reservoirs,
        }
    }
}

/// Defaults match `SFT::Engine::MotionBlurSettings`; disabled by default.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MotionBlurSettings {
    pub enabled: bool,
    pub intensity: f32,
    pub shutter_angle_degrees: f32,
    pub tile_size_px: u32,
    pub sample_count: u32,
    pub max_blur_radius_px: f32,
    pub background_foreground_weight_bias: f32,
    pub camera_motion_only: bool,
}

impl Default for MotionBlurSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            intensity: 1.0,
            shutter_angle_degrees: 180.0,
            tile_size_px: 20,
            sample_count: 8,
            max_blur_radius_px: 32.0,
            background_foreground_weight_bias: 0.5,
            camera_motion_only: false,
        }
    }
}

impl MotionBlurSettings {
    setters! {
        enabled: bool,
        intensity: f32,
        shutter_angle_degrees: f32,
        tile_size_px: u32,
        sample_count: u32,
        max_blur_radius_px: f32,
        background_foreground_weight_bias: f32,
        camera_motion_only: bool,
    }

    fn to_ffi(self) -> ffi::MotionBlurSettings {
        ffi::MotionBlurSettings {
            enabled: self.enabled,
            intensity: self.intensity,
            shutter_angle_degrees: self.shutter_angle_degrees,
            tile_size_px: self.tile_size_px,
            sample_count: self.sample_count,
            max_blur_radius_px: self.max_blur_radius_px,
            background_foreground_weight_bias: self.background_foreground_weight_bias,
            camera_motion_only: self.camera_motion_only,
        }
    }
}

mirrored_enum!(
    /// How the scene itself is rendered. `RasterDeferred` is the engine's default real-time path;
    /// the other variants are debug/visualization or offline-quality integrators (see
    /// `SFT::Engine::SceneIntegrator`). `FullPathTracing` uses [`SceneRenderSettings`]'s `path_*`,
    /// `caustic_*`, and `wavelength_*` knobs; the raster integrators ignore them.
    SceneIntegrator => SceneIntegrator {
        RasterDeferred, ShadowOnly, ReflectionOnly, AmbientOcclusionOnly, ShadowAndTransmission,
        FullPathTracing,
    }
);

/// Scene composition: whether the scene is drawn at all, which integrator draws it, the
/// spectral path tracer's knobs, and an optional solid background override. Defaults match
/// `SFT::Engine::SceneRenderSettings`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneRenderSettings {
    pub enabled: bool,
    pub integrator: SceneIntegrator,
    pub path_samples_per_pixel: u32,
    pub path_max_bounces: u32,
    pub path_russian_roulette_start_bounce: u32,
    pub caustic_photon_count: u32,
    pub caustic_gather_radius: f32,
    pub wavelength_min_nm: f32,
    pub wavelength_max_nm: f32,
    /// Linear RGBA. `None` keeps the engine's own sky/environment background.
    pub background_color: Option<glam::Vec4>,
    pub background_intensity: f32,
}

impl Default for SceneRenderSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            integrator: SceneIntegrator::RasterDeferred,
            path_samples_per_pixel: 1,
            path_max_bounces: 8,
            path_russian_roulette_start_bounce: 3,
            caustic_photon_count: 262_144,
            caustic_gather_radius: 0.075,
            wavelength_min_nm: 380.0,
            wavelength_max_nm: 780.0,
            background_color: None,
            background_intensity: 1.0,
        }
    }
}

impl SceneRenderSettings {
    setters! {
        enabled: bool,
        integrator: SceneIntegrator,
        path_samples_per_pixel: u32,
        path_max_bounces: u32,
        path_russian_roulette_start_bounce: u32,
        caustic_photon_count: u32,
        caustic_gather_radius: f32,
        wavelength_min_nm: f32,
        wavelength_max_nm: f32,
        background_color: Option<glam::Vec4>,
        background_intensity: f32,
    }

    fn to_ffi(self) -> ffi::SceneRenderSettings {
        ffi::SceneRenderSettings {
            enabled: self.enabled,
            integrator: self.integrator.into(),
            path_samples_per_pixel: self.path_samples_per_pixel,
            path_max_bounces: self.path_max_bounces,
            path_russian_roulette_start_bounce: self.path_russian_roulette_start_bounce,
            caustic_photon_count: self.caustic_photon_count,
            caustic_gather_radius: self.caustic_gather_radius,
            wavelength_min_nm: self.wavelength_min_nm,
            wavelength_max_nm: self.wavelength_max_nm,
            has_background_color: self.background_color.is_some(),
            background_color: self.background_color.unwrap_or(glam::Vec4::ZERO).into(),
            background_intensity: self.background_intensity,
        }
    }
}

/// The engine's built-in debug overlay (frame stats text etc.). Disabled by default, matching
/// `SFT::Engine::DebugOverlayRenderSettings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebugOverlayRenderSettings {
    pub enabled: bool,
    pub draw_text: bool,
}

impl Default for DebugOverlayRenderSettings {
    fn default() -> Self {
        Self { enabled: false, draw_text: true }
    }
}

impl DebugOverlayRenderSettings {
    setters! {
        enabled: bool,
        draw_text: bool,
    }

    fn to_ffi(self) -> ffi::DebugOverlayRenderSettings {
        ffi::DebugOverlayRenderSettings { enabled: self.enabled, draw_text: self.draw_text }
    }
}

/// Every render-graph settings category (all nine of `SFT::Engine::RenderGraphDescription`'s),
/// plus the two graph-wide knobs. Build it, tweak the pieces you care about, then
/// [`validate_render_graph_settings`] before handing it off (see the module doc comment for the
/// full attachment plan).
///
/// ```
/// # use sturdy::render::*;
/// let settings = RenderGraphSettings::default()
///     .bloom(BloomSettings::default().intensity(0.08))
///     .tone_mapping(ToneMappingSettings::default().exposure(1.2));
/// assert!(validate_render_graph_settings(&settings).is_ok());
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderGraphSettings {
    pub scene: SceneRenderSettings,
    pub debug_overlay: DebugOverlayRenderSettings,
    pub shadows: ShadowSettings,
    pub ambient_occlusion: AmbientOcclusionSettings,
    pub anti_aliasing: AntiAliasingSettings,
    pub bloom: BloomSettings,
    pub tone_mapping: ToneMappingSettings,
    pub restir_gi: RestirGiSettings,
    pub motion_blur: MotionBlurSettings,
    pub execution_mode: RenderGraphExecutionMode,
    pub resolution_scale: f32,
}

impl Default for RenderGraphSettings {
    fn default() -> Self {
        Self {
            scene: SceneRenderSettings::default(),
            debug_overlay: DebugOverlayRenderSettings::default(),
            shadows: ShadowSettings::default(),
            ambient_occlusion: AmbientOcclusionSettings::default(),
            anti_aliasing: AntiAliasingSettings::default(),
            bloom: BloomSettings::default(),
            tone_mapping: ToneMappingSettings::default(),
            restir_gi: RestirGiSettings::default(),
            motion_blur: MotionBlurSettings::default(),
            execution_mode: RenderGraphExecutionMode::FireAndForget,
            resolution_scale: 1.0,
        }
    }
}

impl RenderGraphSettings {
    setters! {
        scene: SceneRenderSettings,
        debug_overlay: DebugOverlayRenderSettings,
        shadows: ShadowSettings,
        ambient_occlusion: AmbientOcclusionSettings,
        anti_aliasing: AntiAliasingSettings,
        bloom: BloomSettings,
        tone_mapping: ToneMappingSettings,
        restir_gi: RestirGiSettings,
        motion_blur: MotionBlurSettings,
        execution_mode: RenderGraphExecutionMode,
        resolution_scale: f32,
    }

    /// `pub(crate)`: `runtime.rs` converts this into the render-graph settings field of every
    /// `ffi::FrameRequest`, attaching it to the frame the same way `Camera` already is.
    pub(crate) fn to_ffi(self) -> ffi::RenderGraphSettings {
        ffi::RenderGraphSettings {
            scene: self.scene.to_ffi(),
            debug_overlay: self.debug_overlay.to_ffi(),
            shadows: self.shadows.to_ffi(),
            ambient_occlusion: self.ambient_occlusion.to_ffi(),
            anti_aliasing: self.anti_aliasing.to_ffi(),
            bloom: self.bloom.to_ffi(),
            tone_mapping: self.tone_mapping.to_ffi(),
            restir_gi: self.restir_gi.to_ffi(),
            motion_blur: self.motion_blur.to_ffi(),
            execution_mode: self.execution_mode.into(),
            resolution_scale: self.resolution_scale,
        }
    }
}

/// Runs the same validation the engine performs on a `RenderGraphDescription` before use (see the
/// module doc comment), without attaching `settings` to any frame.
pub fn validate_render_graph_settings(settings: &RenderGraphSettings) -> Result<(), Error> {
    let result = ffi::render_graph_settings_validate(&settings.to_ffi());
    if result.ok { Ok(()) } else { Err(Error::new(result.message)) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_settings_validate() {
        assert!(validate_render_graph_settings(&RenderGraphSettings::default()).is_ok());
    }

    #[test]
    fn path_tracing_scene_settings_reach_engine_validation() {
        let valid = RenderGraphSettings::default().scene(
            SceneRenderSettings::default()
                .integrator(SceneIntegrator::FullPathTracing)
                .path_samples_per_pixel(4)
                .background_color(Some(glam::vec4(0.1, 0.2, 0.3, 1.0))),
        );
        assert!(validate_render_graph_settings(&valid).is_ok());

        // Inverted wavelength interval: only rejected if `scene` actually reaches the engine.
        let inverted = RenderGraphSettings::default().scene(
            SceneRenderSettings::default()
                .integrator(SceneIntegrator::FullPathTracing)
                .wavelength_min_nm(700.0)
                .wavelength_max_nm(400.0),
        );
        assert!(validate_render_graph_settings(&inverted).is_err());

        // `background_color: Some(NaN)` must reach the engine's optional, not be dropped.
        let nan_background = RenderGraphSettings::default()
            .scene(SceneRenderSettings::default().background_color(Some(glam::vec4(f32::NAN, 0.0, 0.0, 1.0))));
        assert!(validate_render_graph_settings(&nan_background).is_err());
    }

    #[test]
    fn debug_overlay_round_trips() {
        let settings = RenderGraphSettings::default()
            .debug_overlay(DebugOverlayRenderSettings::default().enabled(true).draw_text(false));
        assert!(validate_render_graph_settings(&settings).is_ok());
    }
}
