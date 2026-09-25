//! Raw `cxx` bindings between Rust and the statically linked SturdyEngine 5 C++ runtime.
//!
//! This crate is the unsafe-ish plumbing; the ergonomic API lives in the `sturdy` crate. Nothing
//! here goes through the engine's C ABI: the shim in `cpp/sturdy_rs` calls the C++ API directly.
//!
//! One `#[cxx::bridge]` module per engine subsystem, each its own file — `build.rs` discovers and
//! compiles all of them automatically (see its module doc comment). Every bridge but this root one
//! reuses `EngineView` rather than redeclaring it: `type EngineView = crate::ffi::EngineView;`
//! under `#[namespace = "sturdy_rs"]` (required — see any of those modules' own comment on it, e.g.
//! `ecs::ffi`, for why the override matters).

pub mod assets;
pub mod diagnostics;
pub mod ecs;
pub mod reflection;
pub mod render;
pub mod rhi;
pub mod schedule;
pub mod shader_compiler;
pub mod ui;

#[cxx::bridge(namespace = "sturdy_rs")]
pub mod ffi {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum VSync {
        Off,
        On,
        Adaptive,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum VariableRefresh {
        Disabled,
        Automatic,
        Preferred,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LatencyMode {
        Normal,
        Low,
        Ultra,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PresentationPreference {
        Automatic,
        LowestLatency,
        Smoothest,
        PowerEfficient,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum WindowMode {
        Windowed,
        BorderlessFullscreen,
        ExclusiveFullscreen,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CameraProjection {
        Perspective,
        Orthographic,
    }

    #[derive(Debug, Clone)]
    struct RuntimeOptions {
        window_title: String,
        app_name: String,
        shaders_directory: String,
        width: u32,
        height: u32,
        resizable: bool,
        decorated: bool,
        high_dpi: bool,
        window_mode: WindowMode,
        raytracing: bool,
        vsync: VSync,
        variable_refresh: VariableRefresh,
        latency: LatencyMode,
        preference: PresentationPreference,
        /// Seconds between window-title refreshes; `<= 0` disables.
        title_update_interval_seconds: f64,
        runtime_window_management: bool,
    }

    #[derive(Debug, Clone)]
    struct Status {
        ok: bool,
        message: String,
    }

    #[derive(Debug, Clone, Copy)]
    struct FrameInfo {
        delta_seconds: f64,
        frame_index: u64,
        framebuffer_width: u32,
        framebuffer_height: u32,
        live_resize: bool,
        window_id: u64,
    }

    #[derive(Debug, Clone, Copy)]
    struct CameraDesc {
        projection: CameraProjection,
        position: [f32; 3],
        euler_degrees: [f32; 3],
        vertical_fov_degrees: f32,
        orthographic_size: f32,
        aspect_ratio: f32,
        near_clip: f32,
        far_clip: f32,
    }

    #[derive(Debug, Clone, Copy)]
    struct SceneLightingDesc {
        ambient_radiance: [f32; 3],
        exposure: f32,
    }

    #[derive(Debug, Clone)]
    struct FrameRequest {
        present: bool,
        camera: CameraDesc,
        lighting: SceneLightingDesc,
        debug_label: String,
    }

    #[derive(Debug, Clone, Default)]
    struct GpuDescription {
        name: String,
        vendor: String,
        driver_version: String,
        api_version: String,
        device_type: String,
        vendor_id: u32,
        device_id: u32,
    }

    extern "Rust" {
        type LogicHandle;

        fn logic_on_init(logic: &mut LogicHandle, engine: Pin<&mut EngineView>) -> Status;
        fn logic_request_frame(
            logic: &mut LogicHandle,
            engine: Pin<&mut EngineView>,
            frame: &FrameInfo,
        ) -> FrameRequest;
        fn logic_on_shutdown(logic: &mut LogicHandle, engine: Pin<&mut EngineView>);
    }

    unsafe extern "C++" {
        include!("sturdy_rs/shim.hpp");

        /// `SFT::Engine::Engine`, only ever handled by reference.
        type EngineView;

        fn run_runtime(options: &RuntimeOptions, args: &Vec<String>, logic: Box<LogicHandle>) -> i32;

        fn engine_delta_seconds(engine: &EngineView) -> f64;
        fn engine_unscaled_delta_seconds(engine: &EngineView) -> f64;
        fn engine_tick_index(engine: &EngineView) -> u64;
        fn engine_time_scale(engine: &EngineView) -> f64;
        fn engine_set_time_scale(engine: Pin<&mut EngineView>, scale: f64);
        fn engine_gpu_description(engine: &EngineView, out: &mut GpuDescription) -> bool;

        fn input_key_down(engine: &EngineView, key: i32) -> bool;
        fn input_key_just_pressed(engine: &EngineView, key: i32) -> bool;
        fn input_key_just_released(engine: &EngineView, key: i32) -> bool;
        fn input_mouse_down(engine: &EngineView, button: u8) -> bool;
        fn input_mouse_just_pressed(engine: &EngineView, button: u8) -> bool;
        fn input_mouse_just_released(engine: &EngineView, button: u8) -> bool;
        fn input_mouse_position(engine: &EngineView) -> [f32; 2];
        fn input_mouse_delta(engine: &EngineView) -> [f32; 2];
        fn input_wheel_delta(engine: &EngineView) -> [f32; 2];
        fn input_text_this_tick(engine: &EngineView) -> String;
    }
}

use core::pin::Pin;

pub use ffi::EngineView;

/// See [`LogicHandle::on_init`].
pub type OnInitFn = Box<dyn FnMut(Pin<&mut EngineView>) -> ffi::Status + Send>;
/// See [`LogicHandle::request_frame`].
pub type RequestFrameFn = Box<dyn FnMut(Pin<&mut EngineView>, &ffi::FrameInfo) -> ffi::FrameRequest + Send>;
/// See [`LogicHandle::on_shutdown`].
pub type OnShutdownFn = Box<dyn FnMut(Pin<&mut EngineView>) + Send>;

/// Trampoline target for the C++ side: holds the user's boxed game logic. The `sturdy` crate
/// installs the vtable-like function table below so this crate stays free of user-facing traits.
pub struct LogicHandle {
    pub on_init: OnInitFn,
    pub request_frame: RequestFrameFn,
    pub on_shutdown: OnShutdownFn,
}

fn logic_on_init(logic: &mut LogicHandle, engine: Pin<&mut EngineView>) -> ffi::Status {
    (logic.on_init)(engine)
}

fn logic_request_frame(
    logic: &mut LogicHandle,
    engine: Pin<&mut EngineView>,
    frame: &ffi::FrameInfo,
) -> ffi::FrameRequest {
    (logic.request_frame)(engine, frame)
}

fn logic_on_shutdown(logic: &mut LogicHandle, engine: Pin<&mut EngineView>) {
    (logic.on_shutdown)(engine)
}
