//! Ergonomic Rust API for SturdyEngine 5.
//!
//! The engine is compiled into your executable as static archives (see `sturdy-sys`); nothing but
//! a GPU driver is needed on the target machine. Game logic is a plain Rust type:
//!
//! ```no_run
//! use sturdy::prelude::*;
//!
//! struct Game { yaw: f32 }
//!
//! impl GameLogic for Game {
//!     fn frame(&mut self, engine: &mut Engine<'_>, frame: &Frame) -> Option<FrameDescription> {
//!         self.yaw += 45.0 * frame.delta_seconds as f32;
//!         if engine.input().key_just_pressed(Key::Escape) { /* ... */ }
//!         Some(FrameDescription::new(
//!             Camera::perspective().at(sturdy::glam::vec3(0.0, 1.0, 4.0)).euler_degrees(sturdy::glam::vec3(0.0, self.yaw, 0.0)),
//!         ))
//!     }
//! }
//!
//! fn main() -> std::process::ExitCode {
//!     sturdy::run(RuntimeConfig::new("My game"), Game { yaw: 0.0 })
//! }
//! ```

mod camera;
mod config;
mod engine;
mod input;
mod keys;
mod runtime;

// The rest of the engine surface: sizable enough (RHI alone has dozens of descriptor types) that
// flattening it all into the crate root would bury the small core API above. Reach it as
// `sturdy::ecs::World`, `sturdy::rhi::BufferDesc`, etc. — each accessible from `Engine` via the
// matching method (`Engine::ecs()`, `Engine::rhi()`, ...).
pub mod assets;
pub mod diagnostics;
pub mod ecs;
pub mod reflection;
pub mod render;
pub mod rhi;
pub mod schedule;
pub mod scene;
pub mod select;
pub mod shader_compiler;
pub mod sync;
pub mod task;
pub mod time;
pub mod ui;

// Re-exported so implementing `ecs::Component` (which requires `bytemuck::Pod` +
// `bytemuck::Zeroable`, usually via `#[derive(bytemuck::Pod, bytemuck::Zeroable)]`) doesn't force
// every downstream crate to also depend on `bytemuck` directly and keep its version in lockstep.
pub use bytemuck;
// Re-exported for the same reason: positions, rotations, scales, colors, and directions
// throughout this crate's public API (`Camera::position`, `scene::WorldTransform::matrix`,
// `ui::ElementDesc::background`, ...) are glam types, not raw `[f32; N]` arrays, so a caller
// building/reading them needs this crate's exact `glam` version too. See `sturdy/Cargo.toml` for
// why the `scalar-math` feature is required (byte-layout parity with the engine's C++ structs).
pub use glam;

pub use camera::{Camera, Projection};
pub use config::{
    LatencyMode, PresentationPreference, RuntimeConfig, VSync, VariableRefresh, WindowMode,
};
pub use ecs::{Component, ComponentId, EcsError, Entity, World};
pub use engine::{Engine, Frame, FrameDescription, GpuInfo, SceneLighting};
pub use input::{Input, MouseButton};
pub use keys::Key;
pub use runtime::{run, run_with_args, Error, GameLogic};

// Bundle/Event/Resource: typed ECS ergonomics layered on top of the core `ecs` re-exports above
// (kept as their own line rather than folded into the existing one, so the two lists' history
// stays easy to read independently).
pub use ecs::{Bundle, Event, EventChannelId, QueryTuple, Resource, ResourceId};
// Real, engine-scheduler-driven ECS systems (as opposed to the ad-hoc `World` access above); see
// `schedule`'s module doc for how this differs from `ecs`.
pub use schedule::{Access, Commands, ScheduleTarget, R, W};
// The `#[derive(Bundle)]` proc macro; see `ecs::Bundle`'s doc comment for what it generates.
pub use sturdy_macros::Bundle;

pub mod prelude {
    pub use crate::{
        run, Bundle, Camera, Component, EcsError, Engine, Entity, Error, Event, Frame,
        FrameDescription, GameLogic, Input, Key, MouseButton, Resource, RuntimeConfig, VSync,
        World,
    };
}

#[cfg(test)]
mod glam_layout_tests {
    #[test]
    fn glam_types_match_plain_array_layout() {
        assert_eq!((size_of::<glam::Vec2>(), align_of::<glam::Vec2>()), (size_of::<[f32; 2]>(), align_of::<[f32; 2]>()));
        assert_eq!((size_of::<glam::Vec3>(), align_of::<glam::Vec3>()), (size_of::<[f32; 3]>(), align_of::<[f32; 3]>()));
        assert_eq!((size_of::<glam::Vec4>(), align_of::<glam::Vec4>()), (size_of::<[f32; 4]>(), align_of::<[f32; 4]>()));
        assert_eq!((size_of::<glam::Quat>(), align_of::<glam::Quat>()), (size_of::<[f32; 4]>(), align_of::<[f32; 4]>()));
        assert_eq!((size_of::<glam::Mat3>(), align_of::<glam::Mat3>()), (size_of::<[f32; 9]>(), align_of::<[f32; 9]>()));
        assert_eq!((size_of::<glam::Mat4>(), align_of::<glam::Mat4>()), (size_of::<[f32; 16]>(), align_of::<[f32; 16]>()));
        assert_eq!((size_of::<glam::IVec2>(), align_of::<glam::IVec2>()), (size_of::<[i32; 2]>(), align_of::<[i32; 2]>()));
        // bytemuck::Pod is implemented (compile-time check).
        fn assert_pod<T: bytemuck::Pod>() {}
        assert_pod::<glam::Vec3>();
        assert_pod::<glam::Vec4>();
        assert_pod::<glam::Quat>();
        assert_pod::<glam::Mat4>();
    }
}

#[cfg(test)]
mod glam_array_conversion_tests {
    #[test]
    fn arrays_convert_into_glam_types() {
        let v: glam::Vec3 = [1.0f32, 2.0, 3.0].into();
        assert_eq!(v, glam::vec3(1.0, 2.0, 3.0));
        let v2: glam::Vec2 = [1.0f32, 2.0].into();
        assert_eq!(v2, glam::vec2(1.0, 2.0));
        let v4: glam::Vec4 = [1.0f32, 2.0, 3.0, 4.0].into();
        assert_eq!(v4, glam::vec4(1.0, 2.0, 3.0, 4.0));
        let arr: [f32; 3] = v.into();
        assert_eq!(arr, [1.0, 2.0, 3.0]);
        let radiance: glam::Vec3 = [1.0f32, 1.0, 1.0].into();
        assert_eq!(radiance, glam::Vec3::ONE);
    }
}
