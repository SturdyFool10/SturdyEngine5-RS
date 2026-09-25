//! The ECS components that actually put something on screen.
//!
//! Rendering in SturdyEngine 5 is ECS-driven, not called directly: an entity carrying
//! [`WorldTransform`] + [`ModelRenderer`] is drawn every frame the render-frame-requests system
//! runs; one carrying [`WorldTransform`] + a light component ([`DirectionalLightRenderer`],
//! [`SpotLightRenderer`], [`PointLightRenderer`]) lights the scene (see
//! `Engine/src/Engine/EcsRendering.hpp`'s `RenderFrameRequests::submit`/`ExtractedLights` on the
//! vendored engine side). [`LightGizmoRenderer`] draws an editor-style gizmo mesh for a light
//! entity, independent of whether that entity also casts light.
//!
//! These are plain [`crate::Component`]s like any other — `world.spawn_builder().with(WorldTransform
//! ::identity()).with(ModelRenderer::new(model_asset)).build()` — registered under the same name
//! the engine's own `SFT_ECS_COMPONENT` macro already registered them under
//! (`"sturdy.engine.world_transform"`, `"sturdy.engine.model_renderer"`, ...), so
//! [`crate::ecs::World::register`] just confirms the layout below matches rather than creating a
//! new component from scratch.
//!
//! ## Layout notes
//!
//! Each type here is `#[repr(C)]` `bytemuck::Pod`, byte-for-byte the same layout as its C++
//! counterpart on this build (x86-64 Linux, no forced GLM alignment — see this module's own doc
//! comment in the crate source for the size/align derivation if the engine's struct ever changes):
//! - `bool` fields (`visible`, `casts_shadows`) are private `u8` (`0`/`1`) — `bytemuck::Pod` cannot
//!   be implemented for `bool` because not every byte pattern is a valid `bool`, so the wire type
//!   is `u8`; use each type's accessor/setter methods (e.g. [`ModelRenderer::visible`]/
//!   [`ModelRenderer::set_visible`]) instead of the raw field.
//! - [`WorldTransform::matrix`] is a `glam::Mat4`, column-major (`glam::Mat4`'s own convention),
//!   matching `glm::mat4`'s in-memory layout exactly. This crate is built with glam's
//!   `scalar-math` feature (see `sturdy/Cargo.toml`), which keeps `Mat4`/`Quat`/`Vec4` at 4-byte
//!   alignment instead of glam's default 16-byte SIMD alignment — without it, `WorldTransform`
//!   would be over-aligned relative to the engine's actual (non-GLM-aligned) `glm::mat4` and the
//!   layout guards at the bottom of this file would fail.
//! - [`ModelRenderer`]/[`LightGizmoRenderer`]'s `model` field embeds an [`AssetRef`], a Pod mirror
//!   of `Engine::Asset` (itself just `{ owner, id, generation, kind }`) with explicit padding bytes
//!   (`bytemuck`'s `Pod` derive rejects implicit padding) — convert with [`AssetRef::from`]`(`[`
//!   crate::assets::Asset`]`)`/[`AssetRef::asset`].

use crate::assets::Asset;

/// Pod mirror of `Engine::Asset` (`owner`/`id`/`generation`/`kind`, `u64`+`u64`+`u32`+`u8`, padded
/// to the 8-byte alignment its two `u64` fields require) for embedding in components that hold an
/// asset reference, e.g. [`ModelRenderer::model`]. See this module's doc comment for why the extra
/// padding field exists.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, bytemuck::Pod, bytemuck::Zeroable)]
pub struct AssetRef {
    pub owner: u64,
    pub id: u64,
    pub generation: u32,
    /// Raw [`crate::assets::AssetKind`] discriminant, in the engine's own declaration order
    /// (`Invalid = 0, Model, Shader, Sound, Texture, File`).
    pub kind: u8,
    _pad: [u8; 3],
}

impl AssetRef {
    /// An [`AssetRef`] that never refers to a real asset, mirroring [`Asset::invalid`].
    pub const INVALID: Self = Self { owner: 0, id: 0, generation: 0, kind: 0, _pad: [0; 3] };

    /// The [`Asset`] handle this reference embeds.
    pub fn asset(self) -> Asset {
        Asset::from_ffi(sturdy_sys::assets::ffi::AssetHandle {
            owner: self.owner,
            id: self.id,
            generation: self.generation,
            kind: kind_from_u8(self.kind),
        })
    }
}

impl Default for AssetRef {
    fn default() -> Self {
        Self::INVALID
    }
}

impl From<Asset> for AssetRef {
    fn from(asset: Asset) -> Self {
        let inner = asset.inner;
        Self { owner: inner.owner, id: inner.id, generation: inner.generation, kind: kind_to_u8(inner.kind), _pad: [0; 3] }
    }
}

fn kind_to_u8(kind: sturdy_sys::assets::ffi::AssetKind) -> u8 {
    use sturdy_sys::assets::ffi::AssetKind as K;
    match kind {
        K::Model => 1,
        K::Shader => 2,
        K::Sound => 3,
        K::Texture => 4,
        K::File => 5,
        _ => 0,
    }
}

fn kind_from_u8(kind: u8) -> sturdy_sys::assets::ffi::AssetKind {
    use sturdy_sys::assets::ffi::AssetKind as K;
    match kind {
        1 => K::Model,
        2 => K::Shader,
        3 => K::Sound,
        4 => K::Texture,
        5 => K::File,
        _ => K::Invalid,
    }
}

/// An entity's world-space transform. Mirrors `Engine::WorldTransform`
/// (`"sturdy.engine.world_transform"`) — a bare `glm::mat4`, column-major.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct WorldTransform {
    /// Column-major — see this module's doc comment.
    pub matrix: glam::Mat4,
}

impl WorldTransform {
    pub const IDENTITY: Self = Self { matrix: glam::Mat4::IDENTITY };

    pub const fn identity() -> Self {
        Self::IDENTITY
    }

    /// Builds a transform from an already-composed matrix.
    pub const fn from_matrix(matrix: glam::Mat4) -> Self {
        Self { matrix }
    }

    /// A translation-only transform.
    pub fn from_translation(position: impl Into<glam::Vec3>) -> Self {
        Self { matrix: glam::Mat4::from_translation(position.into()) }
    }

    /// A transform built from translation, rotation, and (uniform-or-not) scale, in that
    /// composition order (`T * R * S`) — the same convention `glam::Mat4::from_scale_rotation_translation`
    /// uses.
    pub fn from_scale_rotation_translation(
        scale: impl Into<glam::Vec3>,
        rotation: glam::Quat,
        translation: impl Into<glam::Vec3>,
    ) -> Self {
        Self { matrix: glam::Mat4::from_scale_rotation_translation(scale.into(), rotation, translation.into()) }
    }

    /// The transform's translation component (the matrix's fourth column).
    pub fn translation(&self) -> glam::Vec3 {
        self.matrix.w_axis.truncate()
    }

    /// Overwrites just the translation component, leaving rotation/scale untouched.
    pub fn set_translation(&mut self, translation: impl Into<glam::Vec3>) {
        let translation = translation.into();
        self.matrix.w_axis = glam::vec4(translation.x, translation.y, translation.z, self.matrix.w_axis.w);
    }

    /// Decomposes into `(scale, rotation, translation)`. Meaningless (but does not panic) for a
    /// non-affine matrix (e.g. one with a zero or non-uniform, skewed scale it can't cleanly
    /// factor); see `glam::Mat4::to_scale_rotation_translation`'s own doc comment.
    pub fn to_scale_rotation_translation(&self) -> (glam::Vec3, glam::Quat, glam::Vec3) {
        self.matrix.to_scale_rotation_translation()
    }
}

impl Default for WorldTransform {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl crate::Component for WorldTransform {
    const NAME: &'static str = "sturdy.engine.world_transform";
}

/// Draws `model` at the entity's [`WorldTransform`] every frame. Mirrors `Engine::ModelRenderer`
/// (`Engine::MeshRenderer` is a same-layout alias on the C++ side) under
/// `"sturdy.engine.model_renderer"`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ModelRenderer {
    pub model: AssetRef,
    /// Visible to a camera/light only where `visibility_mask & camera_mask != 0`.
    pub visibility_mask: u32,
    /// Draw-order tiebreaker within a pass; lower sorts first.
    pub sort_key: u32,
    visible: u8,
    _pad: [u8; 7],
}

impl ModelRenderer {
    pub fn new(model: Asset) -> Self {
        Self { model: model.into(), visibility_mask: !0, sort_key: 0, visible: 1, _pad: [0; 7] }
    }

    pub fn visible(&self) -> bool {
        self.visible != 0
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible as u8;
    }

    #[must_use]
    pub fn with_visible(mut self, visible: bool) -> Self {
        self.set_visible(visible);
        self
    }

    #[must_use]
    pub fn with_visibility_mask(mut self, mask: u32) -> Self {
        self.visibility_mask = mask;
        self
    }

    #[must_use]
    pub fn with_sort_key(mut self, sort_key: u32) -> Self {
        self.sort_key = sort_key;
        self
    }
}

impl crate::Component for ModelRenderer {
    const NAME: &'static str = "sturdy.engine.model_renderer";
}

/// Draws an editor-style gizmo mesh for a light entity at its [`WorldTransform`], independent of
/// whether that entity also carries a light component. Mirrors `Engine::LightGizmoRenderer` under
/// `"sturdy.engine.light_gizmo_renderer"`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, bytemuck::Pod, bytemuck::Zeroable)]
pub struct LightGizmoRenderer {
    pub model: AssetRef,
    visible: u8,
    _pad: [u8; 7],
}

impl LightGizmoRenderer {
    pub fn new(model: Asset) -> Self {
        Self { model: model.into(), visible: 1, _pad: [0; 7] }
    }

    pub fn visible(&self) -> bool {
        self.visible != 0
    }

    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible as u8;
    }

    #[must_use]
    pub fn with_visible(mut self, visible: bool) -> Self {
        self.set_visible(visible);
        self
    }
}

impl crate::Component for LightGizmoRenderer {
    const NAME: &'static str = "sturdy.engine.light_gizmo_renderer";
}

/// A directional (sun-like) light. Mirrors `Engine::DirectionalLightRenderer` under
/// `"sturdy.engine.directional_light_renderer"`; defaults match the engine's own defaults.
/// Direction comes from the entity's [`WorldTransform`] rotation, not a field here.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct DirectionalLightRenderer {
    pub radiance: glam::Vec3,
    pub angular_radius_degrees: f32,
    casts_shadows: u8,
    _pad: [u8; 3],
}

impl DirectionalLightRenderer {
    pub fn casts_shadows(&self) -> bool {
        self.casts_shadows != 0
    }

    pub fn set_casts_shadows(&mut self, casts_shadows: bool) {
        self.casts_shadows = casts_shadows as u8;
    }

    #[must_use]
    pub fn with_casts_shadows(mut self, casts_shadows: bool) -> Self {
        self.set_casts_shadows(casts_shadows);
        self
    }
}

impl Default for DirectionalLightRenderer {
    fn default() -> Self {
        Self { radiance: glam::vec3(4.0, 3.75, 3.35), angular_radius_degrees: 0.27, casts_shadows: 1, _pad: [0; 3] }
    }
}

impl crate::Component for DirectionalLightRenderer {
    const NAME: &'static str = "sturdy.engine.directional_light_renderer";
}

/// A cone-shaped light. Mirrors `Engine::SpotLightRenderer` under
/// `"sturdy.engine.spot_light_renderer"`; defaults match the engine's own defaults. Position and
/// direction come from the entity's [`WorldTransform`], not fields here.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SpotLightRenderer {
    pub radiance: glam::Vec3,
    pub range: f32,
    /// `cos` of the inner (full-intensity) cone half-angle.
    pub inner_cone_cos: f32,
    /// `cos` of the outer (falloff) cone half-angle.
    pub outer_cone_cos: f32,
    pub source_radius: f32,
    casts_shadows: u8,
    _pad: [u8; 3],
}

impl SpotLightRenderer {
    pub fn casts_shadows(&self) -> bool {
        self.casts_shadows != 0
    }

    pub fn set_casts_shadows(&mut self, casts_shadows: bool) {
        self.casts_shadows = casts_shadows as u8;
    }

    #[must_use]
    pub fn with_casts_shadows(mut self, casts_shadows: bool) -> Self {
        self.set_casts_shadows(casts_shadows);
        self
    }
}

impl Default for SpotLightRenderer {
    fn default() -> Self {
        Self {
            radiance: glam::Vec3::ONE,
            range: 10.0,
            inner_cone_cos: 0.97,
            outer_cone_cos: 0.90,
            source_radius: 0.05,
            casts_shadows: 1,
            _pad: [0; 3],
        }
    }
}

impl crate::Component for SpotLightRenderer {
    const NAME: &'static str = "sturdy.engine.spot_light_renderer";
}

/// An omnidirectional light. Mirrors `Engine::PointLightRenderer` under
/// `"sturdy.engine.point_light_renderer"`; defaults match the engine's own defaults. Position comes
/// from the entity's [`WorldTransform`], not a field here.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PointLightRenderer {
    pub radiance: glam::Vec3,
    pub range: f32,
    pub source_radius: f32,
    casts_shadows: u8,
    _pad: [u8; 3],
}

impl PointLightRenderer {
    pub fn casts_shadows(&self) -> bool {
        self.casts_shadows != 0
    }

    pub fn set_casts_shadows(&mut self, casts_shadows: bool) {
        self.casts_shadows = casts_shadows as u8;
    }

    #[must_use]
    pub fn with_casts_shadows(mut self, casts_shadows: bool) -> Self {
        self.set_casts_shadows(casts_shadows);
        self
    }
}

impl Default for PointLightRenderer {
    fn default() -> Self {
        Self { radiance: glam::Vec3::ONE, range: 10.0, source_radius: 0.05, casts_shadows: 1, _pad: [0; 3] }
    }
}

impl crate::Component for PointLightRenderer {
    const NAME: &'static str = "sturdy.engine.point_light_renderer";
}

// Layout guards: confirmed byte-for-byte against `SFT::Engine`'s real `sizeof`/`alignof` (compiled
// a standalone translation unit against the vendored headers with this build's exact compiler
// flags: clang++, C++26, x86-64-v2, no forced GLM alignment). If the engine ever changes one of
// these types' layout, `register`/`insert`/`spawn_one` will start failing at runtime with "already
// registered with a different layout" well before these would trip, but asserting the exact
// numbers here catches it at compile time instead.
const _: () = {
    assert!(size_of::<AssetRef>() == 24 && align_of::<AssetRef>() == 8);
    assert!(size_of::<WorldTransform>() == 64 && align_of::<WorldTransform>() == 4);
    assert!(size_of::<ModelRenderer>() == 40 && align_of::<ModelRenderer>() == 8);
    assert!(size_of::<LightGizmoRenderer>() == 32 && align_of::<LightGizmoRenderer>() == 8);
    assert!(size_of::<DirectionalLightRenderer>() == 20 && align_of::<DirectionalLightRenderer>() == 4);
    assert!(size_of::<SpotLightRenderer>() == 32 && align_of::<SpotLightRenderer>() == 4);
    assert!(size_of::<PointLightRenderer>() == 24 && align_of::<PointLightRenderer>() == 4);
};
