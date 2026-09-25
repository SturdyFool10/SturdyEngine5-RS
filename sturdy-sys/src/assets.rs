//! Raw `cxx` bindings for `SFT::Engine::AssetManager` and glTF import.
//!
//! Every `AssetManager` entry point is synchronous (it returns `std::expected` on the C++ side,
//! never a future/handle to poll); `load_texture_streamed` is the one exception, and even that is
//! "call once, then call `pump_texture_streaming` from your own tick" rather than an async result,
//! so nothing here needs a poll-able status. No C++ exception crosses into Rust: every entry point
//! below is a plain data-returning function, and the shim catches anything the engine throws and
//! folds it into the `ok`/`error`/`message` fields instead.

#[cxx::bridge(namespace = "sturdy_rs::assets")]
pub mod ffi {
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    enum AssetKind {
        Invalid,
        Model,
        Shader,
        Sound,
        Texture,
        File,
    }

    /// Mirrors `SFT::Engine::AssetErrorCode`. `None` is used for the synthetic "ok" case since cxx
    /// enums cannot carry `Option`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AssetErrorCode {
        None,
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

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TextureColorSpace {
        Linear,
        Srgb,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TextureKind {
        ColorAlpha,
        ColorOpaque,
        Mask,
        NormalMap,
        MetallicRoughness,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TextureDynamicRange {
        Sdr,
        Hdr,
    }

    /// Mirrors `SFT::Engine::TexturePixelFormat`: the sample layout of the raw pixel buffer passed
    /// to `create_texture`. A much smaller set than an image decoder can produce on purpose — these
    /// are the two formats the GPU texture path actually supports.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TexturePixelFormat {
        /// Four 8-bit unsigned-normalized channels, tightly packed (`width * height * 4` bytes).
        Rgba8,
        /// Four IEEE binary16 channels holding scene-linear light (`width * height * 8` bytes).
        /// Must be paired with `TextureColorSpace::Linear`.
        Rgba16Float,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum GltfLightKind {
        Directional,
        Point,
        Spot,
    }

    /// Opaque, trivially-copyable engine asset reference (`SFT::Engine::Asset`, bit-copied). A
    /// default-constructed handle (`kind == Invalid`, all indices zero) never refers to a real
    /// asset, matching `Asset::is_valid()` on the C++ side.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    struct AssetHandle {
        owner: u64,
        id: u64,
        generation: u32,
        kind: AssetKind,
    }

    #[derive(Debug, Clone)]
    struct AssetOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
        asset: AssetHandle,
    }

    #[derive(Debug, Clone)]
    struct StatusOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
    }

    #[derive(Debug, Clone)]
    struct AssetInfoOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
        asset: AssetHandle,
        label: String,
        source: String,
        memory_bytes: u64,
        loaded: bool,
    }

    #[derive(Debug, Clone)]
    struct TextureInfoOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
        width: u32,
        height: u32,
        color_space: TextureColorSpace,
    }

    #[derive(Debug, Clone)]
    struct ModelInfoOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
        primitive_count: u64,
        vertex_count: u64,
        index_count: u64,
        triangle_count: u64,
    }

    #[derive(Debug, Clone)]
    struct SoundInfoOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
        channels: u32,
        sample_rate: u32,
        frame_count: u64,
        duration_seconds: f64,
    }

    /// Which built-in mesh [`assets_generate_shape`] builds (`SturdyShape` in the C API).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ShapeKind {
        Cube,
        Box,
        UvSphere,
        IcoSphere,
        Plane,
        Cylinder,
        Cone,
        Torus,
        Tetrahedron,
    }

    /// `Renderer::Axis`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum ShapeAxis {
        X,
        #[default]
        Y,
        Z,
    }

    /// Union of every shape's parameters (the `*Params` structs in `Renderer/Mesh.hpp`).
    #[derive(Debug, Clone, Copy, Default)]
    struct ShapeParams {
        size: f32,
        extents: [f32; 3],
        radius: f32,
        height: f32,
        width: f32,
        depth: f32,
        major_radius: f32,
        minor_radius: f32,
        rings: u32,
        segments: u32,
        subdivisions: u32,
        width_segments: u32,
        depth_segments: u32,
        radial_segments: u32,
        major_segments: u32,
        minor_segments: u32,
        axis: ShapeAxis,
        capped: bool,
    }

    /// One vertex of a procedurally-authored model primitive. Mirrors
    /// `SFT::Renderer::GeometryVertex`'s field layout exactly.
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct ModelVertex {
        position: [f32; 3],
        normal: [f32; 3],
        uv: [f32; 2],
        color: [f32; 4],
        tangent: [f32; 4],
    }

    /// One `slot -> texture` binding for a model primitive (mirrors `ModelTextureBinding`).
    #[derive(Debug, Clone)]
    struct ModelTextureBinding {
        slot: String,
        texture: AssetHandle,
    }

    /// One primitive of a procedurally-authored model (mirrors `ModelPrimitiveDesc`, minus `mesh`
    /// which is built from `vertices`/`indices` on the C++ side via `Mesh::from_vertices`).
    #[derive(Debug, Clone)]
    struct ModelPrimitiveInput {
        vertices: Vec<ModelVertex>,
        indices: Vec<u32>,
        shader: AssetHandle,
        /// Whether `vertex_color` should be forwarded as `ModelPrimitiveDesc::vertex_color`
        /// (cxx has no `Option<[f32; 4]>`, so this stands in for the engine's `std::optional`).
        has_vertex_color: bool,
        vertex_color: [f32; 4],
        textures: Vec<ModelTextureBinding>,
        double_sided: bool,
    }

    #[derive(Debug, Clone)]
    struct BytesOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
        data: Vec<u8>,
    }

    #[derive(Debug, Clone)]
    struct SamplesOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
        data: Vec<f32>,
    }

    /// `SFT::Renderer::TextureHandle` (a Renderer-registry index), *not* the RHI-level
    /// `SFT::RHI::TextureHandle` that `sturdy_rs::rhi`/`sturdy::rhi`'s `RhiDevice` operations take.
    /// See the `Assets::texture_handle` doc comment on the Rust side for why these two handle
    /// spaces are not interchangeable despite an identical `{ value: u64 }` layout.
    #[derive(Debug, Clone)]
    struct RendererTextureHandleOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
        value: u64,
    }

    #[derive(Debug, Clone)]
    struct GltfInstance {
        name: String,
        model: AssetHandle,
        /// Column-major 4x4 world transform.
        world_transform: [f32; 16],
    }

    #[derive(Debug, Clone)]
    struct GltfLight {
        name: String,
        kind: GltfLightKind,
        radiance: [f32; 3],
        range: f32,
        inner_cone_degrees: f32,
        outer_cone_degrees: f32,
        /// Column-major 4x4 world transform.
        world_transform: [f32; 16],
    }

    #[derive(Debug, Clone)]
    struct GltfImportOutcome {
        ok: bool,
        error: AssetErrorCode,
        message: String,
        models: Vec<AssetHandle>,
        instances: Vec<GltfInstance>,
        lights: Vec<GltfLight>,
    }

    unsafe extern "C++" {
        include!("sturdy_rs/assets.hpp");

        /// Reuses the engine view already bridged in `sturdy_rs::ffi`.
        #[namespace = "sturdy_rs"]
        type EngineView = crate::ffi::EngineView;

        fn assets_load_texture(
            engine: Pin<&mut EngineView>,
            source: &str,
            color_space: TextureColorSpace,
            kind: TextureKind,
            label: &str,
            dynamic_range: TextureDynamicRange,
        ) -> AssetOutcome;

        /// Decodes an in-memory image (PNG/JPEG/etc., whatever the engine's image decoder accepts)
        /// into a texture asset, for callers that already have the bytes rather than a path.
        fn assets_create_texture_from_bytes(
            engine: Pin<&mut EngineView>,
            encoded: &[u8],
            color_space: TextureColorSpace,
            kind: TextureKind,
            label: &str,
            dynamic_range: TextureDynamicRange,
        ) -> AssetOutcome;

        /// Kicks off a block-compressed, progressively-resident texture load. The asset exists
        /// (and is safe to bind/query) immediately, but its mip residency only advances when
        /// `pump_texture_streaming` is called; there is no separate "is it done" query beyond
        /// `texture_info`/`info`, because streaming never truly finishes (residency tracks the
        /// VRAM budget for as long as the asset is alive).
        fn assets_load_texture_streamed(
            engine: Pin<&mut EngineView>,
            source: &str,
            color_space: TextureColorSpace,
            kind: TextureKind,
            label: &str,
        ) -> AssetOutcome;

        /// Advances streamed-texture residency by whatever budget the engine allots this call.
        /// Call once per tick if any streamed textures are loaded.
        fn assets_pump_texture_streaming(engine: Pin<&mut EngineView>);

        /// Builds a texture directly from a raw pixel buffer (`width * height * bytes-per-pixel`
        /// of `format`), for procedurally-generated textures that never touch an image decoder.
        /// Mirrors `AssetManager::create_texture(TextureAssetDesc)`.
        // Mirrors `AssetManager::create_texture`'s descriptor 1:1; no natural sub-group to bundle.
        #[allow(clippy::too_many_arguments)]
        fn assets_create_texture(
            engine: Pin<&mut EngineView>,
            pixels: &[u8],
            width: u32,
            height: u32,
            format: TexturePixelFormat,
            color_space: TextureColorSpace,
            kind: TextureKind,
            label: &str,
            allow_compression: bool,
            generate_mipmaps: bool,
        ) -> AssetOutcome;

        /// Packs a separate occlusion (RGBA8) buffer and metallic-roughness (RGBA8) buffer into a
        /// single ORM (Occlusion/Roughness/Metallic) texture asset. Both buffers must be
        /// `width * height * 4` bytes; only the channels the engine actually reads from each are
        /// used. Mirrors `AssetManager::create_orm_texture`.
        fn assets_create_orm_texture(
            engine: Pin<&mut EngineView>,
            occlusion_rgba8: &[u8],
            metallic_roughness_rgba8: &[u8],
            width: u32,
            height: u32,
            label: &str,
        ) -> AssetOutcome;

        fn assets_load_shader(engine: Pin<&mut EngineView>, source: &str, label: &str) -> AssetOutcome;
        fn assets_load_sound(engine: Pin<&mut EngineView>, source: &str, label: &str) -> AssetOutcome;
        fn assets_load_file(engine: Pin<&mut EngineView>, source: &str, label: &str) -> AssetOutcome;

        /// Generates one of the engine's built-in primitive meshes (`Renderer::Mesh::cube`/
        /// `uv_sphere`/...) and copies its vertices/indices out, ready to become a
        /// [`ModelPrimitiveInput`]. Only the `params` fields the shape uses are read. `false` for
        /// non-finite or degenerate dimensions (nothing written).
        fn assets_generate_shape(
            shape: ShapeKind,
            params: &ShapeParams,
            out_vertices: &mut Vec<ModelVertex>,
            out_indices: &mut Vec<u32>,
        ) -> bool;

        /// Builds a model asset out of raw primitive geometry — each primitive's `vertices`/
        /// `indices` are turned into a `Mesh` via `Mesh::from_vertices` on the C++ side, then
        /// assembled into a `ModelAssetDesc`. Mirrors `AssetManager::create_model(ModelAssetDesc)`.
        fn assets_create_model(
            engine: Pin<&mut EngineView>,
            label: &str,
            primitives: &[ModelPrimitiveInput],
        ) -> AssetOutcome;

        /// Sets a named `float` shader parameter on one primitive of a procedurally-authored
        /// model. Mirrors `AssetManager::set_model_float`.
        fn assets_set_model_float(
            engine: Pin<&mut EngineView>,
            model: AssetHandle,
            primitive: u64,
            name: &str,
            value: f32,
        ) -> StatusOutcome;

        /// Sets a named `vec4` shader parameter on one primitive of a procedurally-authored model.
        /// Mirrors `AssetManager::set_model_vec4`.
        fn assets_set_model_vec4(
            engine: Pin<&mut EngineView>,
            model: AssetHandle,
            primitive: u64,
            name: &str,
            value: [f32; 4],
        ) -> StatusOutcome;

        /// Binds a texture asset into a named slot on one primitive of a procedurally-authored
        /// model. Mirrors `AssetManager::set_model_texture`.
        fn assets_set_model_texture(
            engine: Pin<&mut EngineView>,
            model: AssetHandle,
            primitive: u64,
            slot: &str,
            texture: AssetHandle,
        ) -> StatusOutcome;

        fn assets_unload(engine: Pin<&mut EngineView>, asset: AssetHandle) -> StatusOutcome;
        fn assets_clear(engine: Pin<&mut EngineView>);

        fn assets_contains(engine: &EngineView, asset: AssetHandle) -> bool;
        fn assets_size(engine: &EngineView) -> u64;
        fn assets_info(engine: &EngineView, asset: AssetHandle) -> AssetInfoOutcome;
        fn assets_texture_info(engine: &EngineView, asset: AssetHandle) -> TextureInfoOutcome;
        fn assets_model_info(engine: &EngineView, asset: AssetHandle) -> ModelInfoOutcome;
        fn assets_sound_info(engine: &EngineView, asset: AssetHandle) -> SoundInfoOutcome;

        /// The Renderer-level texture handle for a texture asset. See
        /// `RendererTextureHandleOutcome`'s doc comment for why this is not directly interchangeable
        /// with `sturdy_rs::rhi`'s `TextureHandle`. Mirrors `AssetManager::texture_handle`.
        fn assets_texture_handle(engine: &EngineView, asset: AssetHandle) -> RendererTextureHandleOutcome;

        /// Raw bytes of a `File`-kind asset, read back into Rust. Mirrors
        /// `AssetManager::file_bytes`.
        fn assets_file_bytes(engine: &EngineView, asset: AssetHandle) -> BytesOutcome;

        /// Decoded interleaved sample buffer of a `Sound`-kind asset, read back into Rust. Mirrors
        /// `AssetManager::sound_samples`.
        fn assets_sound_samples(engine: &EngineView, asset: AssetHandle) -> SamplesOutcome;

        /// Imports every model, node instance and light out of a glTF file in one call. The result
        /// is returned by value (models/instances/lights, exactly what `GltfImportResult` holds on
        /// the C++ side) rather than as a handle to poll piecemeal: the engine already hands back a
        /// flat value type here, not an opaque scene object, so there is nothing to wrap.
        fn gltf_import(engine: Pin<&mut EngineView>, source: &str, shader: AssetHandle) -> GltfImportOutcome;
    }
}
