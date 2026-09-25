//! Render-graph settings, multi-window/surface management, offscreen render targets, and
//! presentation/HDR bindings, layered onto the `EngineView` bridged in `src/lib.rs`.
//!
//! The render-graph settings structs here are attached to a frame via a pending-value side
//! channel (`set_pending_render_graph`/`consume_pending_render_graph`) rather than as a
//! `FrameRequest` field, to avoid a circular `#include` between this bridge and the root one; see
//! `set_pending_render_graph`'s doc comment below and the `sturdy` crate's `render.rs` module doc
//! comment for the full attachment path.

#[cxx::bridge(namespace = "sturdy_rs::render")]
pub mod ffi {
    // ---- window mode / presentation enums, reused from the root bridge -------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum HdrColorSpaceMode {
        Hdr10St2084,
        ScrgbLinear,
        Hdr10Hlg,
        DolbyVision,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum HdrTransferFunction {
        Unknown,
        Sdr,
        PqSt2084,
        Hlg,
        LinearExtended,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum HdrColorGamut {
        Unknown,
        Rec709,
        DisplayP3,
        Rec2020,
    }

    // ---- windows / surfaces ----------------------------------------------------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct SurfaceHandle {
        /// `SFT::WindowManager::WindowId` widened to `u64`; `u64::MAX` is the invalid sentinel
        /// (`WindowManager::invalid_window_id`).
        window_id: u64,
    }

    #[derive(Debug, Clone)]
    struct WindowDesc {
        title: String,
        width: u32,
        height: u32,
        x: i32,
        y: i32,
        use_default_position: bool,
        visible: bool,
        resizable: bool,
        decorated: bool,
        high_dpi: bool,
        transparent: bool,
        window_mode: WindowMode,
        desired_frames_in_flight: u32,
    }

    #[derive(Debug, Clone)]
    struct WindowOpenResult {
        ok: bool,
        message: String,
        surface: SurfaceHandle,
    }

    /// Mirrors `SFT::WindowManager::CursorIcon`. See [`window_set_cursor_icon`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CursorIcon {
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

    /// Mirrors `SFT::WindowManager::WindowEffectKind`. See [`window_set_effect`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum WindowEffectKind {
        Blur,
        Acrylic,
        Mica,
        MicaAlt,
        Tabbed,
        DarkMode,
        BorderColor,
        CaptionColor,
        TextColor,
        Transparent,
    }

    /// Mirrors `SFT::WindowManager::TextInputArea` -- the on-screen rectangle (window-client-area
    /// relative) an IME should anchor its composition UI to, plus where within it the text caret
    /// sits. See [`window_set_text_input_area`].
    #[derive(Debug, Clone, Copy, PartialEq, Default)]
    struct TextInputArea {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        cursor_offset_x: f32,
    }

    // ---- offscreen render targets -----------------------------------------------------------

    #[derive(Debug, Clone)]
    struct OffscreenTargetDesc {
        width: u32,
        height: u32,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct OffscreenTargetCreateResult {
        ok: bool,
        message: String,
        /// `SFT::Engine::RenderTargetHandle::value`; `0` is invalid.
        handle: u64,
    }

    // ---- presentation / HDR -------------------------------------------------------------------

    #[derive(Debug, Clone, Copy)]
    struct PresentationSettings {
        vsync: VSync,
        variable_refresh: VariableRefresh,
        latency: LatencyMode,
        preference: PresentationPreference,
        hdr_enabled: bool,
        hdr_color_space: HdrColorSpaceMode,
        transparent_composition: bool,
        /// `0` lets the engine choose the swapchain image count.
        swapchain_image_count: u32,
        allow_present_from_compute: bool,
    }

    #[derive(Debug, Clone)]
    struct Status {
        ok: bool,
        message: String,
    }

    #[derive(Debug, Clone)]
    struct PresentationResolution {
        /// Human-readable name from `RHI::present_strategy_name`.
        strategy: String,
        /// Human-readable name from `RHI::present_mode_name`.
        effective_mode: String,
        degraded: bool,
        present_queue_is_compute: bool,
        /// Human-readable name from `RHI::composite_alpha_mode_name`.
        effective_composite_alpha: String,
        composite_alpha_degraded: bool,
        via_composition_present: bool,
        supports_completion_fence: bool,
        full_screen_exclusive_active: bool,
        /// Raw `RHI::Format` ordinal.
        effective_format: u32,
        /// Raw `RHI::ColorSpace` ordinal.
        effective_color_space: u32,
    }

    #[derive(Debug, Clone, Copy)]
    struct HdrPresentationMode {
        transfer: HdrTransferFunction,
        gamut: HdrColorGamut,
        requires_os_hdr_mode: bool,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct HdrDisplayMetadataInfo {
        red_primary_x: f32,
        red_primary_y: f32,
        green_primary_x: f32,
        green_primary_y: f32,
        blue_primary_x: f32,
        blue_primary_y: f32,
        white_point_x: f32,
        white_point_y: f32,
        min_luminance_nits: f32,
        max_luminance_nits: f32,
        max_full_frame_luminance_nits: f32,
    }

    #[derive(Debug, Clone)]
    struct HdrCapabilityQuery {
        /// `false` if the platform query itself failed (see `message`); the capability fields are
        /// then default-valued and should not be trusted.
        ok: bool,
        message: String,
        hdr_supported: bool,
        hdr_enabled_by_os: bool,
        hdr_metadata_output_supported: bool,
        supported_modes: Vec<HdrPresentationMode>,
        has_display_metadata: bool,
        display_metadata: HdrDisplayMetadataInfo,
        sdr_white_nits: f32,
        edr_headroom: f32,
        max_edr_headroom: f32,
    }

    #[derive(Debug, Clone, Copy)]
    struct HdrContentLightLevelUpdate {
        max_content_light_level_nits: f32,
        max_frame_average_light_level_nits: f32,
    }

    // ---- render-graph settings ---------------------------------------------------------------

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AmbientOcclusionQuality {
        Low,
        Medium,
        High,
        Ultra,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PostProcessAntiAliasing {
        None,
        Fxaa,
        ConservativeMorphological,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ToneMappingOperator {
        None,
        Reinhard,
        Exponential,
        Agx,
        HermiteSpline,
        PsychoV,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AgxLook {
        None,
        Punchy,
        Golden,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RestirGiQuality {
        Low,
        Medium,
        High,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RestirGiDenoiser {
        None,
        Svgf,
        DlssRayReconstruction,
        FsrRedstone,
    }

    /// Mirrors `SFT::Engine::ShadowDebugView`; single-frame shadow debug visualizations.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ShadowDebugView {
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

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct ShadowSettings {
        enabled: bool,
        atlas_size: u32,
        cascade_count: u32,
        max_distance: f32,
        cascade_split_lambda: f32,
        cascade_blend: f32,
        depth_bias: f32,
        slope_bias: f32,
        /// Per-cascade shadow-map edge resolution, near cascade first. Always 4 entries.
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

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct AmbientOcclusionSettings {
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

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct AntiAliasingSettings {
        msaa_samples: u32,
        post_process: PostProcessAntiAliasing,
        subpixel_quality: f32,
        edge_threshold: f32,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct BloomSettings {
        enabled: bool,
        threshold: f32,
        soft_knee: f32,
        intensity: f32,
        scatter: f32,
        downsample_ratio: f32,
        max_levels: u32,
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct ToneMappingSettings {
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
        psychov_adapted_gray_bt709: [f32; 3],
        psychov_background_gray_bt709: [f32; 3],
    }

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct RestirGiSettings {
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

    #[derive(Debug, Clone, Copy, PartialEq)]
    struct MotionBlurSettings {
        enabled: bool,
        intensity: f32,
        shutter_angle_degrees: f32,
        tile_size_px: u32,
        sample_count: u32,
        max_blur_radius_px: f32,
        background_foreground_weight_bias: f32,
        camera_motion_only: bool,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RenderGraphExecutionMode {
        FireAndForget,
        WaitForCompletion,
    }

    /// Mirrors `SFT::Engine::SceneIntegrator`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SceneIntegrator {
        RasterDeferred,
        ShadowOnly,
        ReflectionOnly,
        AmbientOcclusionOnly,
        ShadowAndTransmission,
        FullPathTracing,
    }

    /// Mirrors `SFT::Engine::SceneRenderSettings`. `background_color` is a
    /// `std::optional<glm::vec4>` on the C++ side; `has_background_color` carries the optional's
    /// engaged-ness (cxx has no `Option<[f32; 4]>` for shared structs).
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct SceneRenderSettings {
        enabled: bool,
        integrator: SceneIntegrator,
        path_samples_per_pixel: u32,
        path_max_bounces: u32,
        path_russian_roulette_start_bounce: u32,
        caustic_photon_count: u32,
        caustic_gather_radius: f32,
        wavelength_min_nm: f32,
        wavelength_max_nm: f32,
        has_background_color: bool,
        background_color: [f32; 4],
        background_intensity: f32,
    }

    /// Mirrors `SFT::Engine::DebugOverlayRenderSettings`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct DebugOverlayRenderSettings {
        enabled: bool,
        draw_text: bool,
    }

    /// Every render-graph settings category (all nine of `SFT::Engine::RenderGraphDescription`'s),
    /// plus the two graph-wide knobs (`execution_mode`, `resolution_scale`).
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct RenderGraphSettings {
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

    unsafe extern "C++" {
        include!("sturdy_rs/shim.hpp");
        // The root bridge's generated header: needed for the `VSync`/`WindowMode`/etc. shared
        // enums reused below (their full C++ definitions live here, not in shim.hpp).
        include!("sturdy_rs/bridge.h");
        include!("sturdy_rs/render.hpp");

        /// `SFT::Engine::Engine`, reused from the root bridge. `#[namespace]` overrides this
        /// bridge's default `sturdy_rs::render` so cxx qualifies these as `sturdy_rs::...`
        /// (where the root bridge in `src/lib.rs` actually defines them) instead of looking for
        /// (nonexistent) `sturdy_rs::render::...` copies.
        #[namespace = "sturdy_rs"]
        type EngineView = crate::ffi::EngineView;
        #[namespace = "sturdy_rs"]
        type WindowMode = crate::ffi::WindowMode;
        #[namespace = "sturdy_rs"]
        type VSync = crate::ffi::VSync;
        #[namespace = "sturdy_rs"]
        type VariableRefresh = crate::ffi::VariableRefresh;
        #[namespace = "sturdy_rs"]
        type LatencyMode = crate::ffi::LatencyMode;
        #[namespace = "sturdy_rs"]
        type PresentationPreference = crate::ffi::PresentationPreference;

        // -- windows --

        /// Opens an additional OS window backed by its own render surface. The window itself is
        /// owned by this shim (keyed by the returned surface's `window_id`) until `window_close`.
        fn window_open(engine: Pin<&mut EngineView>, desc: &WindowDesc) -> WindowOpenResult;
        /// Tears down `old_surface`'s window and opens a replacement, mirroring
        /// `Engine::recreate_window`.
        fn window_recreate(
            engine: Pin<&mut EngineView>,
            old_surface: SurfaceHandle,
            desc: &WindowDesc,
        ) -> WindowOpenResult;
        /// Removes the surface from the engine and destroys the backing window. A no-op if
        /// `surface` is not one this shim opened.
        fn window_close(engine: Pin<&mut EngineView>, surface: SurfaceHandle);
        /// Notifies the engine that `surface`'s backing surface must be resized to `width` x
        /// `height` (e.g. after an external/OS-driven resize of a window this shim does not own).
        fn window_notify_resize(engine: Pin<&mut EngineView>, surface: SurfaceHandle, width: u32, height: u32);

        // -- window runtime mutation --
        //
        // All queued via `Engine::window_requests()` (`SFT::Engine::WindowRequests`), the same
        // async request queue `window_open`/`window_recreate`/`window_close` are conceptually
        // built on, and applied by the engine's own per-frame `Application::process_window_requests`
        // loop -- not applied synchronously by this call. Unlike the three above, none of these
        // carry a `WindowRequestCompletion` (`WindowRequestKind` only has `Spawn`/`Close`/
        // `RecreatePrimary` variants), so there is nothing to drain/poll for: enqueue-and-forget,
        // matching the engine's own fire-and-forget contract for them. All are no-ops if `surface`
        // does not (or no longer) names a window this shim's engine knows about.

        /// Requests the OS cursor icon change while hovering `surface`'s window.
        fn window_set_cursor_icon(engine: Pin<&mut EngineView>, surface: SurfaceHandle, icon: CursorIcon);
        /// Requests the OS mouse cursor be confined to (`true`) or released from (`false`)
        /// `surface`'s window bounds.
        fn window_set_cursor_grabbed(engine: Pin<&mut EngineView>, surface: SurfaceHandle, grabbed: bool);
        /// Requests a fullscreen/windowed mode change at runtime (as opposed to `WindowDesc::
        /// window_mode`, which only sets the *initial* mode at creation).
        fn window_set_mode(engine: Pin<&mut EngineView>, surface: SurfaceHandle, mode: WindowMode);
        /// Requests the window's OS-drawn title bar/border be shown (`true`) or hidden (`false`).
        fn window_set_decorated(engine: Pin<&mut EngineView>, surface: SurfaceHandle, decorated: bool);
        /// Requests the window's background become see-through (`true`) or opaque (`false`) --
        /// platform/compositor support permitting.
        fn window_set_transparent(engine: Pin<&mut EngineView>, surface: SurfaceHandle, transparent: bool);
        /// Requests relative (delta-based, cursor-hidden) mouse motion mode for `surface`'s
        /// window, the shape most first-person camera controls want.
        fn window_set_relative_mouse_mode(engine: Pin<&mut EngineView>, surface: SurfaceHandle, enabled: bool);
        /// Requests the OS mouse cursor be locked in place (still visible, unlike relative mouse
        /// mode) while over `surface`'s window.
        fn window_set_mouse_locked(engine: Pin<&mut EngineView>, surface: SurfaceHandle, locked: bool);
        /// Requests starting (`true`) or stopping (`false`) IME text composition for `surface`'s
        /// window.
        fn window_set_text_input_active(engine: Pin<&mut EngineView>, surface: SurfaceHandle, active: bool);
        /// Requests the IME composition UI be anchored at `area` (window-client-area-relative)
        /// for `surface`'s window -- has no visible effect unless text input is also active (see
        /// [`window_set_text_input_active`]).
        fn window_set_text_input_area(engine: Pin<&mut EngineView>, surface: SurfaceHandle, area: TextInputArea);
        /// Requests a blur/vibrancy-style window effect (`kind`) be enabled or disabled for
        /// `surface`'s window -- platform/compositor support permitting; a request for an
        /// unsupported `kind` is silently ignored the same way the engine's own `Window::
        /// set_effect` documents.
        fn window_set_effect(engine: Pin<&mut EngineView>, surface: SurfaceHandle, kind: WindowEffectKind, enabled: bool);

        // -- offscreen render targets --

        fn offscreen_target_create(
            engine: Pin<&mut EngineView>,
            desc: &OffscreenTargetDesc,
        ) -> OffscreenTargetCreateResult;
        fn offscreen_target_destroy(engine: Pin<&mut EngineView>, handle: u64);
        fn offscreen_target_description(engine: &EngineView, handle: u64, out: &mut OffscreenTargetDesc) -> bool;
        /// Returns the target's backing `Renderer::TextureHandle::value`; `0` if `handle` is
        /// unknown.
        fn offscreen_target_texture(engine: &EngineView, handle: u64) -> u64;

        // -- presentation --

        fn presentation_settings_get(engine: &EngineView, surface: SurfaceHandle) -> PresentationSettings;
        fn presentation_settings_set(
            engine: Pin<&mut EngineView>,
            surface: SurfaceHandle,
            settings: &PresentationSettings,
        ) -> Status;
        fn presentation_resolution_query(engine: &EngineView, surface: SurfaceHandle) -> PresentationResolution;

        // -- HDR --

        fn hdr_capabilities_query(engine: &EngineView, surface: SurfaceHandle) -> HdrCapabilityQuery;
        fn hdr_update_content_light_level(
            engine: Pin<&mut EngineView>,
            surface: SurfaceHandle,
            update: &HdrContentLightLevelUpdate,
        ) -> Status;

        // -- render-graph settings --

        /// Validates `settings` (plus default scene/debug-overlay settings) the same way the
        /// engine validates a `RenderGraphDescription` before use, without attaching it to any
        /// frame.
        fn render_graph_settings_validate(settings: &RenderGraphSettings) -> Status;

        /// Stores `settings` as the render graph the *next* `request_render_frame` call attaches
        /// to its `RenderFrameParameters`.
        ///
        /// A side channel rather than a `FrameRequest` field: `FrameRequest` lives in the root
        /// bridge (`src/lib.rs`), which would need `RenderGraphSettings`'s full definition to hold
        /// it by value -- but the root bridge's own generated header is exactly what this bridge's
        /// generated header already `#include`s (for `EngineView`/`VSync`/`WindowMode`/...), so the
        /// reverse `#include` back from root would be circular and neither header would compile
        /// (tried; see git history / the orchestrator's notes if this comment predates one of you
        /// reading it). A single global slot sidesteps that: only one engine ever runs per process
        /// (see `sturdy::run`'s docs), and this is always set and consumed on the same thread
        /// within the same call to `request_render_frame`, so no synchronization beyond what
        /// `set`/`consume`'s own implementation does internally is needed.
        fn set_pending_render_graph(settings: &RenderGraphSettings);
    }
}
