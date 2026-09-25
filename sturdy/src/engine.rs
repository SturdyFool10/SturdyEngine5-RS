use core::marker::PhantomData;
use core::pin::Pin;

use sturdy_sys::{ffi, EngineView};

use crate::assets::Assets;
use crate::camera::Camera;
use crate::diagnostics::Diagnostics;
use crate::ecs::World;
use crate::input::Input;
use crate::rhi::Rhi;
use crate::ui::Ui;

/// Borrowed access to the live engine.
///
/// It only exists inside a [`GameLogic`](crate::GameLogic) callback and borrows from it, so the
/// "stored the handle past its scope" bug class the C API has to detect at runtime cannot compile.
pub struct Engine<'a> {
    view: Pin<&'a mut EngineView>,
    // Pin<&mut _> is already !Send/!Sync-agnostic; this documents that Engine is callback-scoped.
    _scope: PhantomData<&'a mut ()>,
}

impl<'a> Engine<'a> {
    pub(crate) fn new(view: Pin<&'a mut EngineView>) -> Self {
        Self { view, _scope: PhantomData }
    }

    // `pub(crate)`, not private: every subsystem module (ecs.rs, assets.rs, rhi.rs, ui.rs,
    // diagnostics.rs, render.rs) extends `Engine` with its own `impl Engine<'_>` block or builds a
    // borrowed wrapper type from a reborrow of `self.view`, so each needs to reach it from outside
    // this file. `Engine` staying the single owner of the real `Pin<&mut EngineView>` — every
    // subsystem only ever sees a reborrow scoped to the call that asked for it — is what keeps the
    // "stored a handle past its scope" bug class described on the struct itself impossible here too.
    pub(crate) fn view(&self) -> &EngineView {
        &self.view
    }

    pub(crate) fn view_mut(&mut self) -> Pin<&mut EngineView> {
        self.view.as_mut()
    }

    /// Seconds since the previous tick, after time scaling.
    pub fn delta_seconds(&self) -> f64 {
        ffi::engine_delta_seconds(self.view())
    }

    /// Seconds since the previous tick, ignoring the time scale.
    pub fn unscaled_delta_seconds(&self) -> f64 {
        ffi::engine_unscaled_delta_seconds(self.view())
    }

    pub fn tick_index(&self) -> u64 {
        ffi::engine_tick_index(self.view())
    }

    pub fn time_scale(&self) -> f64 {
        ffi::engine_time_scale(self.view())
    }

    pub fn set_time_scale(&mut self, scale: f64) {
        ffi::engine_set_time_scale(self.view.as_mut(), scale);
    }

    /// Keyboard and mouse state for the current tick.
    pub fn input(&self) -> Input<'_> {
        Input::new(self.view())
    }

    /// The entity-component-world. See [`World`] — this pass is byte-level (`&[u8]` components),
    /// not generic over a typed `Component` trait; see its module docs for why.
    pub fn ecs(&mut self) -> World<'_> {
        World::new(self.view.as_mut())
    }

    /// Loading textures, sounds, and glTF scenes.
    pub fn assets(&mut self) -> Assets<'_> {
        Assets::new(self.view.as_mut())
    }

    /// Direct RHI access (buffers, textures, pipelines, command encoding, ray tracing) for a
    /// caller doing its own GPU work alongside the engine's renderer.
    pub fn rhi(&mut self) -> Rhi<'_> {
        let device = sturdy_sys::rhi::ffi::engine_rhi_device(self.view.as_mut());
        Rhi::new(device)
    }

    /// The immediate-mode UI overlay hook and pointer/text-input state.
    pub fn ui(&mut self) -> Ui<'_> {
        Ui::new(self.view.as_mut())
    }

    /// Native GPU handles, extended GPU inventory, window diagnostics, log sink, and the
    /// background task scheduler.
    pub fn diagnostics(&mut self) -> Diagnostics<'_> {
        Diagnostics::new(self.view.as_mut())
    }

    /// The GPU the engine selected, once the renderer is initialized.
    pub fn gpu(&self) -> Option<GpuInfo> {
        let mut out = ffi::GpuDescription::default();
        ffi::engine_gpu_description(self.view(), &mut out).then_some(GpuInfo {
            name: out.name,
            vendor: out.vendor,
            driver_version: out.driver_version,
            api_version: out.api_version,
            device_type: out.device_type,
            vendor_id: out.vendor_id,
            device_id: out.device_id,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuInfo {
    pub name: String,
    pub vendor: String,
    pub driver_version: String,
    pub api_version: String,
    pub device_type: String,
    pub vendor_id: u32,
    pub device_id: u32,
}

/// Per-frame facts handed to [`GameLogic::frame`](crate::GameLogic::frame).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub delta_seconds: f64,
    pub index: u64,
    /// Framebuffer size in pixels.
    pub size: glam::UVec2,
    /// The window is being resized right now.
    pub live_resize: bool,
    pub window_id: u64,
}

impl Frame {
    pub fn aspect_ratio(&self) -> f32 {
        if self.size.y == 0 { 1.0 } else { self.size.x as f32 / self.size.y as f32 }
    }

    pub(crate) fn from_ffi(info: &ffi::FrameInfo) -> Self {
        Self {
            delta_seconds: info.delta_seconds,
            index: info.frame_index,
            size: glam::uvec2(info.framebuffer_width, info.framebuffer_height),
            live_resize: info.live_resize,
            window_id: info.window_id,
        }
    }
}

/// Scene-wide ambient lighting for a frame. Mirrors `SFT::Engine::SceneLighting`; defaults match
/// the engine's own defaults.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SceneLighting {
    pub ambient_radiance: glam::Vec3,
    pub exposure: f32,
}

impl Default for SceneLighting {
    fn default() -> Self {
        Self { ambient_radiance: glam::Vec3::splat(0.02), exposure: 1.0 }
    }
}

impl SceneLighting {
    #[must_use]
    pub fn ambient_radiance(mut self, ambient_radiance: impl Into<glam::Vec3>) -> Self {
        self.ambient_radiance = ambient_radiance.into();
        self
    }

    #[must_use]
    pub fn exposure(mut self, exposure: f32) -> Self {
        self.exposure = exposure;
        self
    }

    pub(crate) fn to_ffi(self) -> ffi::SceneLightingDesc {
        ffi::SceneLightingDesc { ambient_radiance: self.ambient_radiance.into(), exposure: self.exposure }
    }
}

/// What to render this frame.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameDescription {
    pub camera: Camera,
    pub debug_label: String,
    /// Scene ambient lighting for this frame. Defaults to [`SceneLighting::default()`] (which
    /// matches the engine's own defaults) unless overridden with [`FrameDescription::lighting`].
    pub lighting: SceneLighting,
    /// Shadow/AO/anti-aliasing/bloom/tone-mapping/ReSTIR GI/motion-blur tuning for this frame.
    /// Defaults to [`crate::render::RenderGraphSettings::default()`] (which matches the engine's
    /// own defaults) unless overridden with [`FrameDescription::render_graph`].
    pub render_graph: crate::render::RenderGraphSettings,
}

impl FrameDescription {
    pub fn new(camera: Camera) -> Self {
        Self {
            camera,
            debug_label: String::new(),
            lighting: SceneLighting::default(),
            render_graph: crate::render::RenderGraphSettings::default(),
        }
    }

    #[must_use]
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.debug_label = label.into();
        self
    }

    #[must_use]
    pub fn lighting(mut self, lighting: SceneLighting) -> Self {
        self.lighting = lighting;
        self
    }

    #[must_use]
    pub fn render_graph(mut self, settings: crate::render::RenderGraphSettings) -> Self {
        self.render_graph = settings;
        self
    }
}
