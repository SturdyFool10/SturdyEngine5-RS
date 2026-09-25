//! Loading and querying engine-owned assets: textures, shaders, sounds, raw files, and glTF
//! scenes built out of them.
//!
//! Every entry point here is synchronous — the engine's own `AssetManager` API is (`std::expected`
//! all the way down, no future/handle to poll), so there is nothing to make async on the Rust
//! side. The one exception is streamed textures: `load_texture_streamed` returns immediately with
//! a usable (if not yet fully resident) asset, and residency only advances when you call
//! [`Assets::pump_texture_streaming`] — reflecting the engine's own "call this every tick" shape
//! rather than inventing a polling API that does not exist on the C++ side.

use core::pin::Pin;

use sturdy_sys::assets::ffi as sys;
use sturdy_sys::EngineView;

/// A lightweight, `Copy` reference to an engine-owned asset (texture, model, shader, sound, or raw
/// file). Cheap to store and pass around; the engine, not Rust, owns the actual resource.
///
/// A default-constructed handle never resolves to a real asset (mirrors `Asset::is_valid()` on the
/// C++ side), which is what [`Asset::is_valid`] reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Asset {
    pub(crate) inner: sys::AssetHandle,
}

impl Asset {
    pub(crate) fn from_ffi(inner: sys::AssetHandle) -> Self {
        Self { inner }
    }

    /// A handle that never refers to a real asset (a "not set yet" placeholder). Engine calls that
    /// take an asset reject it with [`AssetErrorKind::InvalidAsset`].
    pub const fn invalid() -> Self {
        Self { inner: sys::AssetHandle { owner: 0, id: 0, generation: 0, kind: sys::AssetKind::Invalid } }
    }

    pub fn kind(&self) -> AssetKind {
        AssetKind::from_ffi(self.inner.kind)
    }

    /// Whether this handle could possibly refer to a live asset. Does not check the asset manager
    /// — use [`Assets::contains`] for that.
    pub fn is_valid(&self) -> bool {
        self.inner.owner != 0 && self.inner.id != 0 && self.inner.generation != 0
            && self.inner.kind != sys::AssetKind::Invalid
    }
}

impl Default for Asset {
    fn default() -> Self {
        Self::invalid()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetKind {
    Invalid,
    Model,
    Shader,
    Sound,
    Texture,
    File,
}

impl AssetKind {
    pub(crate) fn from_ffi(kind: sys::AssetKind) -> Self {
        match kind {
            sys::AssetKind::Model => Self::Model,
            sys::AssetKind::Shader => Self::Shader,
            sys::AssetKind::Sound => Self::Sound,
            sys::AssetKind::Texture => Self::Texture,
            sys::AssetKind::File => Self::File,
            _ => Self::Invalid,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureColorSpace {
    Linear,
    Srgb,
}

impl From<TextureColorSpace> for sys::TextureColorSpace {
    fn from(v: TextureColorSpace) -> Self {
        match v {
            TextureColorSpace::Linear => Self::Linear,
            TextureColorSpace::Srgb => Self::Srgb,
        }
    }
}

/// What a texture is used for; drives compression and channel-packing choices in the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureKind {
    ColorAlpha,
    ColorOpaque,
    Mask,
    NormalMap,
    MetallicRoughness,
}

impl From<TextureKind> for sys::TextureKind {
    fn from(v: TextureKind) -> Self {
        match v {
            TextureKind::ColorAlpha => Self::ColorAlpha,
            TextureKind::ColorOpaque => Self::ColorOpaque,
            TextureKind::Mask => Self::Mask,
            TextureKind::NormalMap => Self::NormalMap,
            TextureKind::MetallicRoughness => Self::MetallicRoughness,
        }
    }
}

/// Whether a texture load should preserve the source's high dynamic range. See
/// `TextureDynamicRange` in the vendored engine for the VRAM/compression tradeoffs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TextureDynamicRange {
    Sdr,
    Hdr,
}

impl From<TextureDynamicRange> for sys::TextureDynamicRange {
    fn from(v: TextureDynamicRange) -> Self {
        match v {
            TextureDynamicRange::Sdr => Self::Sdr,
            TextureDynamicRange::Hdr => Self::Hdr,
        }
    }
}

/// The sample layout of a raw pixel buffer passed to [`Assets::create_texture`]. A much smaller
/// set than an image decoder can produce, on purpose: these are the two formats the GPU texture
/// path actually supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TexturePixelFormat {
    /// Four 8-bit unsigned-normalized channels, tightly packed (`width * height * 4` bytes).
    Rgba8,
    /// Four IEEE binary16 channels holding scene-linear light (`width * height * 8` bytes). Must
    /// be paired with [`TextureColorSpace::Linear`].
    Rgba16Float,
}

impl From<TexturePixelFormat> for sys::TexturePixelFormat {
    fn from(v: TexturePixelFormat) -> Self {
        match v {
            TexturePixelFormat::Rgba8 => Self::Rgba8,
            TexturePixelFormat::Rgba16Float => Self::Rgba16Float,
        }
    }
}

/// Why an asset operation failed. Mirrors the engine's `AssetErrorCode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AssetErrorKind {
    InvalidAsset,
    WrongType,
    NotFound,
    IoFailure,
    DecodeFailure,
    InvalidDescription,
    BackendFailure,
    InUse,
    Unsupported,
}

impl AssetErrorKind {
    fn from_ffi(code: sys::AssetErrorCode) -> Self {
        match code {
            sys::AssetErrorCode::InvalidAsset => Self::InvalidAsset,
            sys::AssetErrorCode::WrongType => Self::WrongType,
            sys::AssetErrorCode::NotFound => Self::NotFound,
            sys::AssetErrorCode::IoFailure => Self::IoFailure,
            sys::AssetErrorCode::DecodeFailure => Self::DecodeFailure,
            sys::AssetErrorCode::InvalidDescription => Self::InvalidDescription,
            sys::AssetErrorCode::InUse => Self::InUse,
            sys::AssetErrorCode::Unsupported => Self::Unsupported,
            // `None` only appears on the success path, which never reaches this conversion; a
            // C++ exception caught at the boundary also lands here as the closest fit.
            _ => Self::BackendFailure,
        }
    }
}

/// An asset operation failed. Carries the engine's own diagnostic message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetError {
    pub kind: AssetErrorKind,
    pub message: String,
}

impl core::fmt::Display for AssetError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for AssetError {}

pub type AssetResult<T> = Result<T, AssetError>;

fn outcome_error(ok: bool, error: sys::AssetErrorCode, message: String) -> Option<AssetError> {
    (!ok).then(|| AssetError { kind: AssetErrorKind::from_ffi(error), message })
}

/// Everything the engine knows about a loaded asset, independent of its concrete type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetInfo {
    pub asset: Asset,
    pub label: String,
    pub source: String,
    pub memory_bytes: u64,
    pub loaded: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureInfo {
    pub size: glam::UVec2,
    pub color_space: TextureColorSpace,
}

/// `SFT::Renderer::TextureHandle` (a Renderer-registry index) for a texture asset that has already
/// been loaded through [`Assets`]. *Not* the RHI-level [`crate::rhi::TextureHandle`] that a
/// `RhiDevice` operates on: the Renderer keeps its own texture registry, distinct from the RHI's
/// raw GPU resource table, and this handle only indexes into the former. Despite both being a
/// bare `{ value: u64 }` index, passing one where the other is expected is a bug, not a supported
/// conversion — use [`Assets::rhi_texture`] to resolve one to the RHI texture/view/sampler behind
/// it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct RendererTextureHandle(pub(crate) u64);

impl RendererTextureHandle {
    /// Whether this is a real handle (`Renderer::Handle::is_valid`'s Rust mirror: a
    /// default/zero-valued handle never refers to a live texture).
    pub fn is_valid(&self) -> bool {
        self.0 != 0
    }

    /// The raw registry index, for logging/debugging. Not meaningful as an RHI-level handle — see
    /// this type's doc comment.
    pub fn raw(&self) -> u64 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelInfo {
    pub primitive_count: u64,
    pub vertex_count: u64,
    pub index_count: u64,
    pub triangle_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SoundInfo {
    pub channels: u32,
    pub sample_rate: u32,
    pub frame_count: u64,
    pub duration_seconds: f64,
}

/// One node's mesh instance out of an imported glTF scene.
#[derive(Debug, Clone, PartialEq)]
pub struct GltfInstance {
    pub name: String,
    pub model: Asset,
    /// Column-major 4x4 world transform, as the engine computed it (parent transforms already
    /// applied).
    pub world_transform: glam::Mat4,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GltfLightKind {
    Directional,
    Point,
    Spot,
}

impl GltfLightKind {
    fn from_ffi(kind: sys::GltfLightKind) -> Self {
        match kind {
            sys::GltfLightKind::Directional => Self::Directional,
            sys::GltfLightKind::Spot => Self::Spot,
            _ => Self::Point,
        }
    }
}

/// One light out of an imported glTF scene. Not spawned into any world by this crate — that is a
/// higher-level (ECS) concern; this is just what the importer found.
#[derive(Debug, Clone, PartialEq)]
pub struct GltfLight {
    pub name: String,
    pub kind: GltfLightKind,
    pub radiance: glam::Vec3,
    pub range: f32,
    pub inner_cone_degrees: f32,
    pub outer_cone_degrees: f32,
    pub world_transform: glam::Mat4,
}

/// Everything a glTF file produced: the model assets it created plus the scene graph of instances
/// and lights that reference them. Returned by value — the engine hands back a flat value type
/// here, not an opaque scene object, so there is nothing further to own or release.
#[derive(Debug, Clone, PartialEq)]
pub struct GltfScene {
    pub models: Vec<Asset>,
    pub instances: Vec<GltfInstance>,
    pub lights: Vec<GltfLight>,
}

impl GltfScene {
    /// Spawns every model instance and light this scene contains as its own ECS entity — a
    /// `WorldTransform` + [`crate::scene::ModelRenderer`] per [`GltfInstance`], a `WorldTransform`
    /// + the matching light-renderer component per [`GltfLight`]. Not a hard gap this replaces
    ///   (every field needed is already on [`GltfInstance`]/[`GltfLight`], so a caller could always
    ///   spawn these by hand via [`crate::World::spawn_builder`]) — just the one-call convenience
    ///   the reference C API offers, since that hand-walk is exactly the same every time.
    ///
    /// Returns every spawned entity, instances first (in [`GltfScene::instances`] order), then
    /// lights (in [`GltfScene::lights`] order). Fails on the first spawn that does, leaving
    /// whatever spawned before it in `world` (matching every other partial-batch operation in this
    /// crate, e.g. [`crate::ecs::SpawnBuilder`]'s own doc comment).
    ///
    /// [`GltfLight`]'s cone angles are in degrees (matching glTF's own `KHR_lights_punctual`
    /// convention); [`crate::scene::SpotLightRenderer`] wants their cosines, so this converts.
    /// [`crate::scene::DirectionalLightRenderer::angular_radius_degrees`] has no glTF counterpart
    /// and is left at its engine default.
    pub fn spawn_all(&self, world: &mut crate::World<'_>) -> Result<Vec<crate::Entity>, crate::EcsError> {
        use crate::scene::{DirectionalLightRenderer, ModelRenderer, PointLightRenderer, SpotLightRenderer, WorldTransform};

        let mut entities = Vec::with_capacity(self.instances.len() + self.lights.len());
        for instance in &self.instances {
            let entity = world
                .spawn_builder()
                .with(WorldTransform::from_matrix(instance.world_transform))
                .with(ModelRenderer::new(instance.model))
                .build()?;
            entities.push(entity);
        }
        for light in &self.lights {
            let transform = WorldTransform::from_matrix(light.world_transform);
            let entity = match light.kind {
                GltfLightKind::Directional => {
                    let mut component = DirectionalLightRenderer::default();
                    component.radiance = light.radiance;
                    world.spawn_builder().with(transform).with(component).build()?
                }
                GltfLightKind::Point => {
                    let mut component = PointLightRenderer::default();
                    component.radiance = light.radiance;
                    component.range = light.range;
                    world.spawn_builder().with(transform).with(component).build()?
                }
                GltfLightKind::Spot => {
                    let mut component = SpotLightRenderer::default();
                    component.radiance = light.radiance;
                    component.range = light.range;
                    component.inner_cone_cos = light.inner_cone_degrees.to_radians().cos();
                    component.outer_cone_cos = light.outer_cone_degrees.to_radians().cos();
                    world.spawn_builder().with(transform).with(component).build()?
                }
            };
            entities.push(entity);
        }
        Ok(entities)
    }
}

/// One vertex of a procedurally-authored model primitive. Mirrors
/// `SFT::Renderer::GeometryVertex`'s field layout exactly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelVertex {
    pub position: glam::Vec3,
    pub normal: glam::Vec3,
    pub uv: glam::Vec2,
    pub color: glam::Vec4,
    /// `xyz` the tangent direction, `w` the bitangent sign (+1/-1).
    pub tangent: glam::Vec4,
}

impl Default for ModelVertex {
    fn default() -> Self {
        Self {
            position: glam::Vec3::ZERO,
            normal: glam::Vec3::ZERO,
            uv: glam::Vec2::ZERO,
            color: glam::Vec4::ONE,
            tangent: glam::vec4(1.0, 0.0, 0.0, 1.0),
        }
    }
}

/// Axis a [`Shape::Plane`]'s normal (or a [`Shape::Cylinder`]/[`Shape::Cone`]'s height) runs
/// along.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShapeAxis {
    X,
    #[default]
    Y,
    Z,
}

/// One of the engine's built-in primitive meshes (`Renderer::Mesh::cube`/`uv_sphere`/...). Turn
/// it into geometry with [`Shape::generate`] or straight into a model primitive with
/// [`ModelPrimitive::shape`]. Segment counts below each shape's minimum are clamped up by the
/// engine.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Cube { size: f32 },
    /// An axis-aligned box with full edge lengths `extents`.
    Box { extents: glam::Vec3 },
    UvSphere { radius: f32, rings: u32, segments: u32 },
    IcoSphere { radius: f32, subdivisions: u32 },
    /// `size.x`/`size.y` are the plane's width/depth (span along the axes perpendicular to `axis`).
    Plane { size: glam::Vec2, width_segments: u32, depth_segments: u32, axis: ShapeAxis },
    Cylinder { radius: f32, height: f32, radial_segments: u32, axis: ShapeAxis, capped: bool },
    Cone { radius: f32, height: f32, radial_segments: u32, axis: ShapeAxis, capped: bool },
    Torus { major_radius: f32, minor_radius: f32, major_segments: u32, minor_segments: u32 },
    Tetrahedron { size: f32 },
}

impl Shape {
    /// A unit cube (the engine's `CubeParams` default).
    pub const CUBE: Shape = Shape::Cube { size: 1.0 };
    /// A 0.5-radius UV sphere with the engine's default tessellation.
    pub const SPHERE: Shape = Shape::UvSphere { radius: 0.5, rings: 16, segments: 32 };
    /// A 1x1 single-quad plane facing +Y.
    pub const PLANE: Shape = Shape::Plane { size: glam::Vec2::ONE, width_segments: 1, depth_segments: 1, axis: ShapeAxis::Y };

    fn to_ffi(self) -> (sys::ShapeKind, sys::ShapeParams) {
        let axis_of = |axis: ShapeAxis| match axis {
            ShapeAxis::X => sys::ShapeAxis::X,
            ShapeAxis::Y => sys::ShapeAxis::Y,
            ShapeAxis::Z => sys::ShapeAxis::Z,
        };
        let mut p = sys::ShapeParams::default();
        let kind = match self {
            Shape::Cube { size } => {
                p.size = size;
                sys::ShapeKind::Cube
            }
            Shape::Box { extents } => {
                p.extents = extents.into();
                sys::ShapeKind::Box
            }
            Shape::UvSphere { radius, rings, segments } => {
                (p.radius, p.rings, p.segments) = (radius, rings, segments);
                sys::ShapeKind::UvSphere
            }
            Shape::IcoSphere { radius, subdivisions } => {
                (p.radius, p.subdivisions) = (radius, subdivisions);
                sys::ShapeKind::IcoSphere
            }
            Shape::Plane { size, width_segments, depth_segments, axis } => {
                (p.width, p.depth, p.width_segments, p.depth_segments, p.axis) =
                    (size.x, size.y, width_segments, depth_segments, axis_of(axis));
                sys::ShapeKind::Plane
            }
            Shape::Cylinder { radius, height, radial_segments, axis, capped } => {
                (p.radius, p.height, p.radial_segments, p.axis, p.capped) =
                    (radius, height, radial_segments, axis_of(axis), capped);
                sys::ShapeKind::Cylinder
            }
            Shape::Cone { radius, height, radial_segments, axis, capped } => {
                (p.radius, p.height, p.radial_segments, p.axis, p.capped) =
                    (radius, height, radial_segments, axis_of(axis), capped);
                sys::ShapeKind::Cone
            }
            Shape::Torus { major_radius, minor_radius, major_segments, minor_segments } => {
                (p.major_radius, p.minor_radius, p.major_segments, p.minor_segments) =
                    (major_radius, minor_radius, major_segments, minor_segments);
                sys::ShapeKind::Torus
            }
            Shape::Tetrahedron { size } => {
                p.size = size;
                sys::ShapeKind::Tetrahedron
            }
        };
        (kind, p)
    }

    /// Generates this shape's vertices and triangle-list indices. `None` if a dimension is
    /// non-finite.
    pub fn generate(self) -> Option<(Vec<ModelVertex>, Vec<u32>)> {
        let (kind, params) = self.to_ffi();
        let mut vertices = Vec::new();
        let mut indices = Vec::new();
        if !sys::assets_generate_shape(kind, &params, &mut vertices, &mut indices) {
            return None;
        }
        let vertices = vertices
            .into_iter()
            .map(|v| ModelVertex {
                position: v.position.into(),
                normal: v.normal.into(),
                uv: v.uv.into(),
                color: v.color.into(),
                tangent: v.tangent.into(),
            })
            .collect();
        Some((vertices, indices))
    }
}

/// One primitive of a procedurally-authored model: geometry (a raw vertex/index buffer, turned
/// into a `Mesh` on the C++ side via `Mesh::from_vertices`) plus the shader/material state
/// `ModelPrimitiveDesc` carries. Build one with [`ModelPrimitive::new`], add it to a
/// [`ModelBuilder`], then [`Assets::create_model`] it.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelPrimitive {
    pub vertices: Vec<ModelVertex>,
    pub indices: Vec<u32>,
    pub shader: Asset,
    pub vertex_color: Option<glam::Vec4>,
    /// `(slot name, texture asset)` bindings, e.g. `("albedo", texture)`.
    pub textures: Vec<(String, Asset)>,
    pub double_sided: bool,
}

impl ModelPrimitive {
    pub fn new(vertices: Vec<ModelVertex>, indices: Vec<u32>, shader: Asset) -> Self {
        Self { vertices, indices, shader, vertex_color: None, textures: Vec::new(), double_sided: false }
    }

    /// A primitive with a built-in [`Shape`]'s geometry. `None` if a dimension is non-finite.
    pub fn shape(shape: Shape, shader: Asset) -> Option<Self> {
        let (vertices, indices) = shape.generate()?;
        Some(Self::new(vertices, indices, shader))
    }

    #[must_use]
    pub fn with_vertex_color(mut self, color: impl Into<glam::Vec4>) -> Self {
        self.vertex_color = Some(color.into());
        self
    }

    #[must_use]
    pub fn with_texture(mut self, slot: impl Into<String>, texture: Asset) -> Self {
        self.textures.push((slot.into(), texture));
        self
    }

    #[must_use]
    pub fn with_double_sided(mut self, double_sided: bool) -> Self {
        self.double_sided = double_sided;
        self
    }

    fn into_ffi(self) -> sys::ModelPrimitiveInput {
        sys::ModelPrimitiveInput {
            vertices: self
                .vertices
                .into_iter()
                .map(|v| sys::ModelVertex {
                    position: v.position.into(),
                    normal: v.normal.into(),
                    uv: v.uv.into(),
                    color: v.color.into(),
                    tangent: v.tangent.into(),
                })
                .collect(),
            indices: self.indices,
            shader: self.shader.inner,
            has_vertex_color: self.vertex_color.is_some(),
            vertex_color: self.vertex_color.unwrap_or(glam::Vec4::ZERO).into(),
            textures: self
                .textures
                .into_iter()
                .map(|(slot, texture)| sys::ModelTextureBinding { slot, texture: texture.inner })
                .collect(),
            double_sided: self.double_sided,
        }
    }
}

/// Builds a procedurally-authored model out of one or more [`ModelPrimitive`]s, ready for
/// [`Assets::create_model`]. Mirrors `ModelAssetDesc { label, primitives }` on the C++ side.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelBuilder {
    pub label: String,
    pub primitives: Vec<ModelPrimitive>,
}

impl ModelBuilder {
    pub fn new(label: impl Into<String>) -> Self {
        Self { label: label.into(), primitives: Vec::new() }
    }

    #[must_use]
    pub fn with_primitive(mut self, primitive: ModelPrimitive) -> Self {
        self.primitives.push(primitive);
        self
    }
}

/// Loads, queries and releases engine-owned assets.
pub struct Assets<'a> {
    view: Pin<&'a mut EngineView>,
}

impl<'a> Assets<'a> {
    pub(crate) fn new(view: Pin<&'a mut EngineView>) -> Self {
        Self { view }
    }

    fn view(&self) -> &EngineView {
        &self.view
    }

    /// Loads a texture from a file path.
    pub fn load_texture(
        &mut self,
        source: impl AsRef<str>,
        color_space: TextureColorSpace,
        kind: TextureKind,
        label: impl AsRef<str>,
        dynamic_range: TextureDynamicRange,
    ) -> AssetResult<Asset> {
        let out = sys::assets_load_texture(
            self.view.as_mut(),
            source.as_ref(),
            color_space.into(),
            kind.into(),
            label.as_ref(),
            dynamic_range.into(),
        );
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(Asset::from_ffi(out.asset)),
        }
    }

    /// Decodes an in-memory image (whatever the engine's image decoder accepts — PNG, JPEG, ...)
    /// into a texture asset, for callers that already have the bytes rather than a file path.
    pub fn create_texture_from_bytes(
        &mut self,
        encoded: &[u8],
        color_space: TextureColorSpace,
        kind: TextureKind,
        label: impl AsRef<str>,
        dynamic_range: TextureDynamicRange,
    ) -> AssetResult<Asset> {
        let out = sys::assets_create_texture_from_bytes(
            self.view.as_mut(),
            encoded,
            color_space.into(),
            kind.into(),
            label.as_ref(),
            dynamic_range.into(),
        );
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(Asset::from_ffi(out.asset)),
        }
    }

    /// Builds a texture directly from a raw pixel buffer (`width * height * bytes-per-pixel` of
    /// `format`, tightly packed) — for procedurally-generated textures that never touch an image
    /// decoder. `allow_compression`/`generate_mipmaps` mirror the engine's `TextureAssetDesc`
    /// flags of the same name.
    #[allow(clippy::too_many_arguments)]
    pub fn create_texture(
        &mut self,
        pixels: &[u8],
        width: u32,
        height: u32,
        format: TexturePixelFormat,
        color_space: TextureColorSpace,
        kind: TextureKind,
        label: impl AsRef<str>,
        allow_compression: bool,
        generate_mipmaps: bool,
    ) -> AssetResult<Asset> {
        let out = sys::assets_create_texture(
            self.view.as_mut(),
            pixels,
            width,
            height,
            format.into(),
            color_space.into(),
            kind.into(),
            label.as_ref(),
            allow_compression,
            generate_mipmaps,
        );
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(Asset::from_ffi(out.asset)),
        }
    }

    /// Packs a separate occlusion buffer and metallic-roughness buffer (both RGBA8,
    /// `width * height * 4` bytes) into a single ORM (Occlusion/Roughness/Metallic) texture asset
    /// — the packed-channel convention this engine's PBR shaders read material textures in. Only
    /// the channels the engine actually reads from each input are used.
    pub fn create_orm_texture(
        &mut self,
        occlusion_rgba8: &[u8],
        metallic_roughness_rgba8: &[u8],
        width: u32,
        height: u32,
        label: impl AsRef<str>,
    ) -> AssetResult<Asset> {
        let out = sys::assets_create_orm_texture(
            self.view.as_mut(),
            occlusion_rgba8,
            metallic_roughness_rgba8,
            width,
            height,
            label.as_ref(),
        );
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(Asset::from_ffi(out.asset)),
        }
    }

    /// Starts a block-compressed, progressively-resident texture load. The returned asset is
    /// immediately valid to bind and query, but its mip residency only advances as
    /// [`Assets::pump_texture_streaming`] is called — call that once per tick while any streamed
    /// texture is alive. There is no separate "streaming finished" signal: residency tracks the
    /// VRAM budget for as long as the asset exists, so use [`Assets::texture_info`] if you need to
    /// inspect its current state.
    ///
    /// Always decodes 8-bit sRGB (no HDR option) — use [`Assets::load_texture`] with
    /// [`TextureDynamicRange::Hdr`] for HDR sources instead.
    pub fn load_texture_streamed(
        &mut self,
        source: impl AsRef<str>,
        color_space: TextureColorSpace,
        kind: TextureKind,
        label: impl AsRef<str>,
    ) -> AssetResult<Asset> {
        let out = sys::assets_load_texture_streamed(
            self.view.as_mut(),
            source.as_ref(),
            color_space.into(),
            kind.into(),
            label.as_ref(),
        );
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(Asset::from_ffi(out.asset)),
        }
    }

    /// Advances streamed-texture residency by whatever budget the engine allots this call. A
    /// no-op if no streamed textures are loaded; cheap enough to call every tick unconditionally.
    pub fn pump_texture_streaming(&mut self) {
        sys::assets_pump_texture_streaming(self.view.as_mut());
    }

    /// Loads a shader from a file path.
    pub fn load_shader(&mut self, source: impl AsRef<str>, label: impl AsRef<str>) -> AssetResult<Asset> {
        let out = sys::assets_load_shader(self.view.as_mut(), source.as_ref(), label.as_ref());
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(Asset::from_ffi(out.asset)),
        }
    }

    /// Loads and decodes a sound from a file path.
    pub fn load_sound(&mut self, source: impl AsRef<str>, label: impl AsRef<str>) -> AssetResult<Asset> {
        let out = sys::assets_load_sound(self.view.as_mut(), source.as_ref(), label.as_ref());
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(Asset::from_ffi(out.asset)),
        }
    }

    /// Loads a file's raw bytes as an asset, with no interpretation of its contents.
    pub fn load_file(&mut self, source: impl AsRef<str>, label: impl AsRef<str>) -> AssetResult<Asset> {
        let out = sys::assets_load_file(self.view.as_mut(), source.as_ref(), label.as_ref());
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(Asset::from_ffi(out.asset)),
        }
    }

    /// Releases an asset. Fails with [`AssetErrorKind::InUse`] if something still references it.
    pub fn unload(&mut self, asset: Asset) -> AssetResult<()> {
        let out = sys::assets_unload(self.view.as_mut(), asset.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Releases every asset the manager holds.
    pub fn clear(&mut self) {
        sys::assets_clear(self.view.as_mut());
    }

    /// Whether `asset` currently resolves to a live entry in this asset manager.
    pub fn contains(&self, asset: Asset) -> bool {
        sys::assets_contains(self.view(), asset.inner)
    }

    /// How many assets this manager currently holds.
    pub fn len(&self) -> u64 {
        sys::assets_size(self.view())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Type-independent load state and bookkeeping for an asset.
    pub fn info(&self, asset: Asset) -> AssetResult<AssetInfo> {
        let out = sys::assets_info(self.view(), asset.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(AssetInfo {
                asset: Asset::from_ffi(out.asset),
                label: out.label,
                source: out.source,
                memory_bytes: out.memory_bytes,
                loaded: out.loaded,
            }),
        }
    }

    /// Dimensions and color space of a texture asset. Fails with
    /// [`AssetErrorKind::WrongType`] if `asset` is not a texture.
    pub fn texture_info(&self, asset: Asset) -> AssetResult<TextureInfo> {
        let out = sys::assets_texture_info(self.view(), asset.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(TextureInfo {
                size: glam::uvec2(out.width, out.height),
                color_space: match out.color_space {
                    sys::TextureColorSpace::Linear => TextureColorSpace::Linear,
                    _ => TextureColorSpace::Srgb,
                },
            }),
        }
    }

    /// The Renderer-level [`RendererTextureHandle`] backing a texture asset — see that type's doc
    /// comment for how (and how not) to use it. Fails with [`AssetErrorKind::WrongType`] if `asset`
    /// is not a texture.
    pub fn texture_handle(&self, asset: Asset) -> AssetResult<RendererTextureHandle> {
        let out = sys::assets_texture_handle(self.view(), asset.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(RendererTextureHandle(out.value)),
        }
    }

    /// The RHI texture, view, and sampler (plus size/mips/format) behind a Renderer texture —
    /// e.g. to bind a loaded texture asset in your own [`crate::rhi`] pipeline. `None` before the
    /// renderer exists or for a dead handle. The handles stay owned by the Renderer: valid only
    /// while the asset stays loaded, and never to be destroyed through [`crate::rhi::Rhi`].
    pub fn rhi_texture(&mut self, texture: RendererTextureHandle) -> Option<crate::rhi::RendererTextureResources> {
        let mut out = sturdy_sys::rhi::ffi::RendererTextureResources::default();
        sturdy_sys::rhi::ffi::renderer_texture_resources(self.view.as_mut(), texture.0, &mut out)
            .then(|| crate::rhi::RendererTextureResources::from_ffi(out))
    }

    /// Primitive/vertex/index/triangle counts of a model asset. Fails with
    /// [`AssetErrorKind::WrongType`] if `asset` is not a model.
    pub fn model_info(&self, asset: Asset) -> AssetResult<ModelInfo> {
        let out = sys::assets_model_info(self.view(), asset.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(ModelInfo {
                primitive_count: out.primitive_count,
                vertex_count: out.vertex_count,
                index_count: out.index_count,
                triangle_count: out.triangle_count,
            }),
        }
    }

    /// Builds a model asset out of raw primitive geometry supplied by `builder` — each
    /// primitive's vertex/index buffer becomes a `Mesh` (via `Mesh::from_vertices`) on the C++
    /// side, then the whole thing is assembled into a `ModelAssetDesc`. This is procedural model
    /// authoring, not glTF import — see [`Assets::import_gltf`] for loading modeled assets from a
    /// file instead.
    pub fn create_model(&mut self, builder: ModelBuilder) -> AssetResult<Asset> {
        let primitives: Vec<sys::ModelPrimitiveInput> =
            builder.primitives.into_iter().map(ModelPrimitive::into_ffi).collect();
        let out = sys::assets_create_model(self.view.as_mut(), builder.label.as_str(), &primitives);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(Asset::from_ffi(out.asset)),
        }
    }

    /// Sets a named `float` shader parameter on one primitive of a procedurally-authored model
    /// (`primitive` is the index into the [`ModelBuilder`]'s primitive list it was built from).
    pub fn set_model_float(&mut self, model: Asset, primitive: u64, name: &str, value: f32) -> AssetResult<()> {
        let out = sys::assets_set_model_float(self.view.as_mut(), model.inner, primitive, name, value);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Sets a named `vec4` shader parameter on one primitive of a procedurally-authored model.
    pub fn set_model_vec4(&mut self, model: Asset, primitive: u64, name: &str, value: impl Into<glam::Vec4>) -> AssetResult<()> {
        let out = sys::assets_set_model_vec4(self.view.as_mut(), model.inner, primitive, name, value.into().into());
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Binds a texture asset into a named slot on one primitive of a procedurally-authored model.
    pub fn set_model_texture(&mut self, model: Asset, primitive: u64, slot: &str, texture: Asset) -> AssetResult<()> {
        let out = sys::assets_set_model_texture(self.view.as_mut(), model.inner, primitive, slot, texture.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    /// Raw bytes of a `File`-kind asset (as loaded by [`Assets::load_file`]), read back into Rust.
    /// Fails with [`AssetErrorKind::WrongType`] if `asset` is not a file.
    pub fn file_bytes(&self, asset: Asset) -> AssetResult<Vec<u8>> {
        let out = sys::assets_file_bytes(self.view(), asset.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(out.data),
        }
    }

    /// Decoded interleaved sample buffer of a `Sound`-kind asset, read back into Rust — `channels`
    /// interleaved `f32` samples per frame, [`Assets::sound_info`] gives the channel count/sample
    /// rate/frame count needed to interpret it. Fails with [`AssetErrorKind::WrongType`] if `asset`
    /// is not a sound.
    pub fn sound_samples(&self, asset: Asset) -> AssetResult<Vec<f32>> {
        let out = sys::assets_sound_samples(self.view(), asset.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(out.data),
        }
    }

    /// Channel/sample-rate/duration info of a sound asset. Fails with
    /// [`AssetErrorKind::WrongType`] if `asset` is not a sound.
    pub fn sound_info(&self, asset: Asset) -> AssetResult<SoundInfo> {
        let out = sys::assets_sound_info(self.view(), asset.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(SoundInfo {
                channels: out.channels,
                sample_rate: out.sample_rate,
                frame_count: out.frame_count,
                duration_seconds: out.duration_seconds,
            }),
        }
    }

    /// Imports every model, node instance and light out of a glTF file. `shader` is the shader
    /// asset every imported model primitive is built with. It must be a real, loaded shader (e.g.
    /// from [`Assets::load_shader`]); [`Asset::invalid`] is rejected by the engine with
    /// `InvalidAsset` (verified live), not treated as "use a default".
    ///
    /// This does not spawn anything into an ECS world; it only loads the assets and reports what
    /// the file contained. Spawning the returned instances/lights is left to whatever scene layer
    /// sits above this crate.
    pub fn import_gltf(&mut self, source: impl AsRef<str>, shader: Asset) -> AssetResult<GltfScene> {
        let out = sys::gltf_import(self.view.as_mut(), source.as_ref(), shader.inner);
        match outcome_error(out.ok, out.error, out.message) {
            Some(err) => Err(err),
            None => Ok(GltfScene {
                models: out.models.into_iter().map(Asset::from_ffi).collect(),
                instances: out
                    .instances
                    .into_iter()
                    .map(|i| GltfInstance {
                        name: i.name,
                        model: Asset::from_ffi(i.model),
                        world_transform: glam::Mat4::from_cols_array(&i.world_transform),
                    })
                    .collect(),
                lights: out
                    .lights
                    .into_iter()
                    .map(|l| GltfLight {
                        name: l.name,
                        kind: GltfLightKind::from_ffi(l.kind),
                        radiance: l.radiance.into(),
                        range: l.range,
                        inner_cone_degrees: l.inner_cone_degrees,
                        outer_cone_degrees: l.outer_cone_degrees,
                        world_transform: glam::Mat4::from_cols_array(&l.world_transform),
                    })
                    .collect(),
            }),
        }
    }
}

#[cfg(test)]
mod shape_tests {
    use super::{Shape, ShapeAxis};

    #[test]
    fn every_shape_generates_valid_triangles() {
        let shapes = [
            Shape::CUBE,
            Shape::Box { extents: glam::vec3(1.0, 2.0, 3.0) },
            Shape::SPHERE,
            Shape::IcoSphere { radius: 1.0, subdivisions: 1 },
            Shape::PLANE,
            Shape::Cylinder { radius: 0.5, height: 2.0, radial_segments: 8, axis: ShapeAxis::Z, capped: true },
            Shape::Cone { radius: 0.5, height: 1.0, radial_segments: 8, axis: ShapeAxis::Y, capped: false },
            Shape::Torus { major_radius: 1.0, minor_radius: 0.25, major_segments: 12, minor_segments: 6 },
            Shape::Tetrahedron { size: 1.0 },
        ];
        for shape in shapes {
            let (vertices, indices) = shape.generate().unwrap_or_else(|| panic!("{shape:?} generates"));
            assert!(!vertices.is_empty() && indices.len() % 3 == 0, "{shape:?}");
            assert!(indices.iter().all(|&i| (i as usize) < vertices.len()), "{shape:?} index in range");
        }
    }

    #[test]
    fn shape_geometry_matches_parameters() {
        let (vertices, _) = Shape::UvSphere { radius: 2.0, rings: 4, segments: 6 }.generate().unwrap();
        assert_eq!(vertices.len(), 5 * 7);
        for v in &vertices {
            let length = v.position.length();
            assert!((length - 2.0).abs() < 1e-4, "sphere vertex at radius {length}");
        }
        let (vertices, _) = Shape::Box { extents: glam::vec3(2.0, 4.0, 6.0) }.generate().unwrap();
        let max = |axis: usize| vertices.iter().map(|v| v.position[axis]).fold(f32::MIN, f32::max);
        assert_eq!((max(0), max(1), max(2)), (1.0, 2.0, 3.0));
        let (vertices, _) = Shape::Plane { size: glam::Vec2::ONE, width_segments: 1, depth_segments: 1, axis: ShapeAxis::X }
            .generate()
            .unwrap();
        assert!(vertices.iter().all(|v| v.normal == glam::Vec3::X));
    }

    #[test]
    fn non_finite_shape_is_rejected() {
        assert!(Shape::Cube { size: f32::NAN }.generate().is_none());
        assert!(Shape::UvSphere { radius: f32::INFINITY, rings: 4, segments: 4 }.generate().is_none());
    }
}
