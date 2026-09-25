//! Raw `cxx` bindings for the RHI (buffers, textures, pipelines, command encoding) and ray
//! tracing subsystems.
//!
//! Scope: this is the largest subsystem in the engine (`RhiResources.cpp` alone is 2600+ lines in
//! the reference FFI). This bridge covers the operations a typical game needs every frame — buffer
//! /texture/sampler/bind-group/pipeline creation, a render+compute command encoder with the common
//! draw/dispatch/copy/barrier commands, buffer mapping, fill/update buffer, blit, texture clears,
//! query sets (occlusion/timestamp/pipeline-statistics), indirect draws/dispatch, debug groups, and
//! a minimal ray tracing path (BLAS/TLAS build + trace rays), plus (added in a later pass) device
//! feature/property limits, acceleration structure copy/compaction, CPU-driven multi-draw-indirect,
//! depth-bounds test, per-face stencil ops, custom MSAA sample locations, and the blend-constant/
//! stencil-reference dynamic state that turned out to be missing alongside those last two; render
//! bundles (`rb_*`), mesh/task shader draws, variable-rate shading, queue-ownership transfers,
//! timeline semaphores/fences with a full-control `rhi_submit_with`, opacity micromaps, and
//! procedural (AABB) BLAS geometry.
//!
//! Not bound, by design: swapchain/surface/present and swapchain-level HDR. The engine owns its
//! swapchains and presentation loop, so a Rust caller never holds a `SwapchainHandle` to pass
//! these; surface-level HDR is bound in `render.rs`, which forwards to the same RHI calls for the
//! engine's own swapchain.
//!
//! Every RHI call that can fail reports it through an out-param `RhiStatus` rather than a cxx
//! `Result` — none of these operations throw a C++ exception (they return `std::expected`), so
//! nothing here relies on cxx's exception-to-`Result` translation.

#[cxx::bridge(namespace = "sturdy_rs::rhi")]
pub mod ffi {
    // ─── Errors ──────────────────────────────────────────────────────────────────

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RhiErrorCode {
        Unsupported,
        OperationFailed,
        OutOfMemory,
        DeviceLost,
        SurfaceLost,
        FullScreenExclusiveLost,
        NotReady,
        InvalidArgument,
    }

    #[derive(Debug, Clone)]
    struct RhiStatus {
        ok: bool,
        code: RhiErrorCode,
        message: String,
    }

    // ─── Handles (Copy, cheap to hold) ──────────────────────────────────────────

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct BufferHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct TextureHandle {
        value: u64,
    }
    /// See [`renderer_texture_resources`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct RendererTextureResources {
        texture: TextureHandle,
        view: TextureViewHandle,
        sampler: SamplerHandle,
        width: u32,
        height: u32,
        mip_levels: u32,
        format: Format,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct TextureViewHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct SamplerHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct ShaderModuleHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct BindGroupLayoutHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct BindGroupHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct PipelineLayoutHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct RenderPipelineHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct ComputePipelineHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct RayTracingPipelineHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct AccelerationStructureHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct CommandBufferHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct QuerySetHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct OpacityMicromapHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct RenderBundleHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct SemaphoreHandle {
        value: u64,
    }
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct FenceHandle {
        value: u64,
    }

    // ─── Shared enums ────────────────────────────────────────────────────────────

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum MemoryLocation {
        DeviceLocal,
        HostUpload,
        HostReadback,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TextureDimension {
        Dim1D,
        Dim2D,
        Dim3D,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum TextureViewType {
        View1D,
        View2D,
        View2DArray,
        ViewCube,
        ViewCubeArray,
        View3D,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Filter {
        Nearest,
        Linear,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum MipmapMode {
        Nearest,
        Linear,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AddressMode {
        Repeat,
        MirroredRepeat,
        ClampToEdge,
        ClampToBorder,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BorderColor {
        TransparentBlack,
        OpaqueBlack,
        OpaqueWhite,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CompareOp {
        Never,
        Less,
        Equal,
        LessEqual,
        Greater,
        NotEqual,
        GreaterEqual,
        Always,
    }

    /// Mirrors `SFT::RHI::Format`; see that header for the exact bit layout each variant implies.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum Format {
        #[default]
        Undefined,
        R8Unorm,
        R8Snorm,
        R8Uint,
        R8Sint,
        RG8Unorm,
        RG8Snorm,
        RG8Uint,
        RG8Sint,
        RGBA8Unorm,
        RGBA8UnormSrgb,
        RGBA8Snorm,
        RGBA8Uint,
        RGBA8Sint,
        BGRA8Unorm,
        BGRA8UnormSrgb,
        RGB10A2Unorm,
        RG11B10Float,
        R16Uint,
        R16Sint,
        R16Float,
        RG16Uint,
        RG16Sint,
        RG16Float,
        RGBA16Uint,
        RGBA16Sint,
        RGBA16Float,
        R32Uint,
        R32Sint,
        R32Float,
        RG32Uint,
        RG32Sint,
        RG32Float,
        RGBA32Uint,
        RGBA32Sint,
        RGBA32Float,
        D16Unorm,
        D24UnormS8Uint,
        D32Float,
        D32FloatS8Uint,
        BC1Unorm,
        BC1UnormSrgb,
        BC3Unorm,
        BC3UnormSrgb,
        BC4Unorm,
        BC5Unorm,
        BC7Unorm,
        BC7UnormSrgb,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum SampleCount {
        X1 = 1,
        X2 = 2,
        X4 = 4,
        X8 = 8,
        X16 = 16,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum IndexFormat {
        Uint16,
        Uint32,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ShaderLanguage {
        SpirV,
        Dxil,
        Msl,
        Wgsl,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum VertexFormat {
        Float32,
        Float32x2,
        Float32x3,
        Float32x4,
        Uint32,
        Uint32x2,
        Uint32x3,
        Uint32x4,
        Sint32,
        Sint32x2,
        Sint32x3,
        Sint32x4,
        Uint8x4Unorm,
        Sint8x4Norm,
        Uint16x2Unorm,
        Uint16x4Unorm,
        Float16x2,
        Float16x4,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum VertexStepMode {
        Vertex,
        Instance,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BindingType {
        UniformBuffer,
        StorageBuffer,
        ReadOnlyStorageBuffer,
        SampledTexture,
        StorageTexture,
        Sampler,
        CombinedImageSampler,
        AccelerationStructure,
        InputAttachment,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StorageTextureAccess {
        WriteOnly,
        ReadOnly,
        ReadWrite,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BindGroupLifetime {
        Persistent,
        FrameTransient,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PrimitiveTopology {
        PointList,
        LineList,
        LineStrip,
        TriangleList,
        TriangleStrip,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum PolygonMode {
        Fill,
        Line,
        Point,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum CullMode {
        None,
        Front,
        Back,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FrontFace {
        CounterClockwise,
        Clockwise,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BlendFactor {
        Zero,
        One,
        SrcColor,
        OneMinusSrcColor,
        DstColor,
        OneMinusDstColor,
        SrcAlpha,
        OneMinusSrcAlpha,
        DstAlpha,
        OneMinusDstAlpha,
        ConstantColor,
        OneMinusConstantColor,
        SrcAlphaSaturated,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum BlendOp {
        Add,
        Subtract,
        ReverseSubtract,
        Min,
        Max,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LoadOp {
        Load,
        Clear,
        DontCare,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StoreOp {
        Store,
        DontCare,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum QueueClass {
        #[default]
        Graphics,
        Compute,
        Transfer,
        Sparse,
        VideoDecode,
        VideoEncode,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    enum TextureLayout {
        #[default]
        Undefined,
        General,
        ColorAttachment,
        DepthStencilAttachment,
        DepthStencilReadOnly,
        ShaderReadOnly,
        TransferSrc,
        TransferDst,
        Present,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AccelerationStructureType {
        BottomLevel,
        TopLevel,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AccelerationStructureGeometryType {
        Triangles,
        Instances,
        /// Procedural geometry: axis-aligned boxes hit-tested by an intersection shader
        /// (see [`AccelerationStructureGeometryDesc::aabbs`]). Declared last (not in the engine's
        /// own `Triangles, Aabbs, Instances` order) so existing discriminants don't shift; mapped
        /// explicitly in `rhi.cpp`.
        Aabbs,
    }

    /// Mirrors `SFT::RHI::OpacityMicromapFormat`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum OpacityMicromapFormat {
        TwoState,
        FourState,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RayTracingShaderGroupType {
        General,
        TrianglesHitGroup,
        ProceduralHitGroup,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum QueryType {
        Occlusion,
        Timestamp,
        PipelineStatistics,
    }

    /// Mirrors `SFT::RHI::StencilOp`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum StencilOp {
        Keep,
        Zero,
        Replace,
        IncrementClamp,
        DecrementClamp,
        Invert,
        IncrementWrap,
        DecrementWrap,
    }

    /// Mirrors `SFT::RHI::AccelerationStructureCopyMode`. See [`ce_copy_acceleration_structure`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum AccelerationStructureCopyMode {
        /// A verbatim copy — a second, independent acceleration structure with identical contents.
        Clone,
        /// Copies into a smaller destination sized to the geometry actually built, freeing the
        /// bounding-box slack a from-scratch build reserves. `dst` must already be created at the
        /// (backend-specific, not queried through this binding) compacted size.
        Compact,
        /// Serializes into a buffer-backed, backend-specific portable format (for caching to disk
        /// across runs), rather than another acceleration structure.
        Serialize,
        /// Reverses `Serialize`.
        Deserialize,
    }

    /// Mirrors `SFT::RHI::ShadingRate` (coarse pixel size, width x height). See
    /// [`rp_set_shading_rate`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ShadingRate {
        X1x1,
        X1x2,
        X2x1,
        X2x2,
        X2x4,
        X4x2,
        X4x4,
    }

    /// Mirrors `SFT::RHI::ShadingRateCombiner`. See [`rp_set_shading_rate`].
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum ShadingRateCombiner {
        Passthrough,
        Override,
        Min,
        Max,
        Combine,
    }

    // ─── Resource descriptors ────────────────────────────────────────────────────

    #[derive(Debug, Clone)]
    struct BufferDesc {
        size: u64,
        /// Bitmask of `sturdy::rhi::BufferUsage` flags.
        usage: u32,
        memory: MemoryLocation,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct TextureDesc {
        dimension: TextureDimension,
        format: Format,
        width: u32,
        height: u32,
        depth_or_layers: u32,
        mip_levels: u32,
        samples: SampleCount,
        /// Bitmask of `sturdy::rhi::TextureUsage` flags.
        usage: u32,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct TextureViewDesc {
        texture: TextureHandle,
        view_type: TextureViewType,
        format: Format,
        base_mip_level: u32,
        /// `u32::MAX` means "all remaining", matching `RHI::all_remaining`.
        mip_level_count: u32,
        base_array_layer: u32,
        /// `u32::MAX` means "all remaining".
        array_layer_count: u32,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct SamplerDesc {
        min_filter: Filter,
        mag_filter: Filter,
        mipmap_mode: MipmapMode,
        address_u: AddressMode,
        address_v: AddressMode,
        address_w: AddressMode,
        mip_lod_bias: f32,
        min_lod: f32,
        max_lod: f32,
        max_anisotropy: f32,
        compare_enable: bool,
        compare: CompareOp,
        border_color: BorderColor,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct ShaderModuleDesc {
        language: ShaderLanguage,
        code: Vec<u8>,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct ShaderEntry {
        module: ShaderModuleHandle,
        entry_point: String,
        /// Exactly one `sturdy::rhi::ShaderStage` bit.
        stage: u32,
    }

    #[derive(Debug, Clone)]
    struct BindGroupLayoutEntry {
        binding: u32,
        /// D3D register override; `u32::MAX` (the default) means "unused".
        shader_register: u32,
        kind: BindingType,
        /// Bitmask of `sturdy::rhi::ShaderStage` flags.
        visibility: u32,
        count: u32,
        has_dynamic_offset: bool,
        /// Bitmask of `sturdy::rhi::BindingFlags` flags.
        flags: u32,
        /// Only meaningful when `kind == StorageTexture`.
        storage_format: Format,
        storage_access: StorageTextureAccess,
        /// Only meaningful when `kind == Sampler`.
        sampler_is_comparison: bool,
        /// Only meaningful when `kind == SampledTexture`.
        sampled_texture_is_depth: bool,
        sampled_texture_is_multisampled: bool,
    }

    #[derive(Debug, Clone)]
    struct BindGroupLayoutDesc {
        entries: Vec<BindGroupLayoutEntry>,
        label: String,
    }

    #[derive(Debug, Clone, Default)]
    struct BindGroupEntry {
        binding: u32,
        array_element: u32,
        buffer: BufferHandle,
        offset: u64,
        size: u64,
        structure_stride: u32,
        texture_view: TextureViewHandle,
        sampler: SamplerHandle,
        acceleration_structure: AccelerationStructureHandle,
    }

    #[derive(Debug, Clone)]
    struct BindGroupDesc {
        layout: BindGroupLayoutHandle,
        entries: Vec<BindGroupEntry>,
        variable_descriptor_count: u32,
        lifetime: BindGroupLifetime,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct PushConstantRange {
        /// Bitmask of `sturdy::rhi::ShaderStage` flags.
        stages: u32,
        offset: u32,
        size: u32,
        shader_register: u32,
        register_space: u32,
    }

    #[derive(Debug, Clone)]
    struct PipelineLayoutDesc {
        bind_group_layouts: Vec<BindGroupLayoutHandle>,
        push_constant_ranges: Vec<PushConstantRange>,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct VertexAttribute {
        format: VertexFormat,
        offset: u32,
        shader_location: u32,
        semantic_name: String,
        semantic_index: u32,
    }

    #[derive(Debug, Clone)]
    struct VertexBufferLayout {
        stride: u64,
        step_mode: VertexStepMode,
        attributes: Vec<VertexAttribute>,
    }

    #[derive(Debug, Clone)]
    struct RasterizationState {
        polygon_mode: PolygonMode,
        cull_mode: CullMode,
        front_face: FrontFace,
        depth_bias_constant: f32,
        depth_bias_slope_scale: f32,
        depth_bias_clamp: f32,
        line_width: f32,
    }

    /// Mirrors `SFT::RHI::StencilFaceState` — the stencil ops for one polygon face, set
    /// independently for front/back via [`DepthStencilState::stencil_front`]/`stencil_back`.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct StencilFaceState {
        fail_op: StencilOp,
        depth_fail_op: StencilOp,
        pass_op: StencilOp,
        compare: CompareOp,
    }

    #[derive(Debug, Clone)]
    struct DepthStencilState {
        format: Format,
        depth_test_enable: bool,
        depth_write_enable: bool,
        depth_compare: CompareOp,
        stencil_test_enable: bool,
        stencil_front: StencilFaceState,
        stencil_back: StencilFaceState,
        stencil_read_mask: u8,
        stencil_write_mask: u8,
        /// Requires `Feature::DepthBoundsTest`; opts this pipeline into
        /// `RenderPassEncoder::set_depth_bounds` being called against draws that use it (see
        /// [`rp_set_depth_bounds`]).
        depth_bounds_test_enable: bool,
    }

    #[derive(Debug, Clone)]
    struct MultisampleState {
        samples: SampleCount,
        sample_mask: u32,
        alpha_to_coverage_enable: bool,
        /// Requires `Feature::SampleLocations`; opts this pipeline into
        /// `RenderPassEncoder::set_sample_locations` being called against draws that use it (see
        /// [`rp_set_sample_locations`]).
        sample_locations_enable: bool,
    }

    /// Mirrors `SFT::RHI::SampleLocation` — a custom MSAA sample position within a pixel,
    /// normalized `[0, 1]` in both axes (`(0.5, 0.5)` is the pixel center). See
    /// [`rp_set_sample_locations`].
    #[derive(Debug, Clone, Copy, PartialEq)]
    struct SampleLocation {
        x: f32,
        y: f32,
    }

    #[derive(Debug, Clone)]
    struct BlendComponent {
        src_factor: BlendFactor,
        dst_factor: BlendFactor,
        op: BlendOp,
    }

    #[derive(Debug, Clone)]
    struct ColorTargetState {
        format: Format,
        blend_enable: bool,
        color: BlendComponent,
        alpha: BlendComponent,
        /// Bitmask of `sturdy::rhi::ColorWriteMask` flags.
        write_mask: u32,
    }

    #[derive(Debug, Clone)]
    struct RenderPipelineDesc {
        layout: PipelineLayoutHandle,
        vertex: ShaderEntry,
        fragment: ShaderEntry,
        vertex_buffers: Vec<VertexBufferLayout>,
        topology: PrimitiveTopology,
        rasterization: RasterizationState,
        multisample: MultisampleState,
        /// `depth_stencil.format == Format::Undefined` means no depth/stencil attachment.
        depth_stencil: DepthStencilState,
        color_targets: Vec<ColorTargetState>,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct ComputePipelineDesc {
        layout: PipelineLayoutHandle,
        compute: ShaderEntry,
        label: String,
    }

    // ─── Command encoding ────────────────────────────────────────────────────────

    #[derive(Debug, Clone)]
    struct CommandEncoderDesc {
        queue: QueueClass,
        label: String,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct ClearColor {
        r: f32,
        g: f32,
        b: f32,
        a: f32,
    }

    #[derive(Debug, Clone)]
    struct ColorAttachment {
        view: TextureViewHandle,
        resolve_view: TextureViewHandle,
        load_op: LoadOp,
        store_op: StoreOp,
        clear_color: ClearColor,
    }

    #[derive(Debug, Clone)]
    struct DepthStencilAttachment {
        view: TextureViewHandle,
        depth_load_op: LoadOp,
        depth_store_op: StoreOp,
        stencil_load_op: LoadOp,
        stencil_store_op: StoreOp,
        clear_depth: f32,
        clear_stencil: u32,
    }

    #[derive(Debug, Clone, Default)]
    struct Rect2D {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    }

    #[derive(Debug, Clone)]
    struct RenderPassDesc {
        color_attachments: Vec<ColorAttachment>,
        has_depth_stencil: bool,
        depth_stencil: DepthStencilAttachment,
        render_area: Rect2D,
        label: String,
    }

    #[derive(Debug, Clone, Default)]
    struct Viewport {
        x: f32,
        y: f32,
        width: f32,
        height: f32,
        min_depth: f32,
        max_depth: f32,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct BufferCopy {
        src_offset: u64,
        dst_offset: u64,
        size: u64,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct BufferTextureCopy {
        buffer_offset: u64,
        buffer_row_length: u32,
        buffer_image_height: u32,
        mip_level: u32,
        base_array_layer: u32,
        array_layer_count: u32,
        offset_x: i32,
        offset_y: i32,
        offset_z: i32,
        extent_width: u32,
        extent_height: u32,
        extent_depth_or_layers: u32,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct DrawArgs {
        vertex_count: u32,
        instance_count: u32,
        first_vertex: u32,
        first_instance: u32,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct DrawIndexedArgs {
        index_count: u32,
        instance_count: u32,
        first_index: u32,
        base_vertex: i32,
        first_instance: u32,
    }

    /// Mirrors `SFT::RHI::DrawMeshTasksArgs` — task/mesh shader workgroup counts. See
    /// [`rp_draw_mesh_tasks`].
    #[derive(Debug, Clone, Copy, Default)]
    struct DrawMeshTasksArgs {
        group_count_x: u32,
        group_count_y: u32,
        group_count_z: u32,
    }

    /// Mirrors `SFT::RHI::RenderBundleDesc` — the attachment formats a render bundle will be
    /// replayed into; must match the render pass it's executed in (see [`rp_execute_bundles`]).
    #[derive(Debug, Clone)]
    struct RenderBundleDesc {
        color_formats: Vec<Format>,
        depth_stencil_format: Format,
        samples: SampleCount,
        view_mask: u32,
        label: String,
    }

    /// Mirrors `SFT::RHI::SemaphoreDesc` — a timeline semaphore (monotonically increasing `u64`
    /// counter, waited/signaled by value).
    #[derive(Debug, Clone)]
    struct SemaphoreDesc {
        initial_value: u64,
        label: String,
    }

    /// Mirrors `SFT::RHI::FenceDesc`.
    #[derive(Debug, Clone)]
    struct FenceDesc {
        signaled: bool,
        label: String,
    }

    /// Mirrors `SFT::RHI::QueueSemaphoreWait`/`QueueSemaphoreSignal` (identical shape): the
    /// submission waits for (or signals) `semaphore` reaching `value` at `stages` (a
    /// `sturdy::rhi::PipelineStage` bitmask).
    #[derive(Debug, Clone, Copy, Default)]
    struct QueueSemaphoreOp {
        semaphore: SemaphoreHandle,
        value: u64,
        stages: u64,
    }

    /// Mirrors `SFT::RHI::SubmitDesc`, minus `presented_textures` (swapchain presentation is
    /// owned by `sturdy::render`'s surface API — see this module's doc comment).
    #[derive(Debug, Clone)]
    struct SubmitDesc {
        queue: QueueLane,
        command_buffers: Vec<CommandBufferHandle>,
        waits: Vec<QueueSemaphoreOp>,
        signals: Vec<QueueSemaphoreOp>,
        /// Invalid (`value == 0`) for no fence.
        fence: FenceHandle,
        /// `SubmitFlags::OneShot`: the command buffers are released after this submission.
        one_shot: bool,
        label: String,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TextureSubresourceLayers {
        mip_level: u32,
        base_array_layer: u32,
        array_layer_count: u32,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TextureBlit {
        src_subresource: TextureSubresourceLayers,
        src_min_x: i32,
        src_min_y: i32,
        src_min_z: i32,
        src_max_x: i32,
        src_max_y: i32,
        src_max_z: i32,
        dst_subresource: TextureSubresourceLayers,
        dst_min_x: i32,
        dst_min_y: i32,
        dst_min_z: i32,
        dst_max_x: i32,
        dst_max_y: i32,
        dst_max_z: i32,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct ClearDepthStencilValue {
        depth: f32,
        stencil: u32,
    }

    // ─── Query sets ──────────────────────────────────────────────────────────────

    #[derive(Debug, Clone)]
    struct QuerySetDesc {
        query_type: QueryType,
        count: u32,
        /// Bitmask of `sturdy::rhi::PipelineStatistic` flags; only meaningful when `query_type ==
        /// PipelineStatistics`.
        statistics: u32,
        label: String,
    }

    // ─── Barriers ────────────────────────────────────────────────────────────────

    #[derive(Debug, Clone, Copy, Default)]
    struct GlobalBarrier {
        /// Bitmask of `sturdy::rhi::PipelineStage` flags.
        src_stage: u64,
        /// Bitmask of `sturdy::rhi::AccessFlags` flags.
        src_access: u64,
        dst_stage: u64,
        dst_access: u64,
    }

    /// Mirrors `SFT::RHI::QueueLane` — one queue of a given class (`index` distinguishes multiple
    /// queues of the same class, when the device exposes more than one).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct QueueLane {
        queue: QueueClass,
        index: u32,
    }

    /// Mirrors `SFT::RHI::QueueOwnershipTransfer`. When `enabled`, the barrier it's attached to is
    /// half of a queue-family ownership transfer from `src` to `dst` (record the release half on
    /// `src`'s encoder and the acquire half on `dst`'s, same barrier on both); `enabled == false`
    /// (the default) is an ordinary same-queue barrier.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    struct QueueOwnershipTransfer {
        src: QueueLane,
        dst: QueueLane,
        enabled: bool,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct BufferBarrier {
        buffer: BufferHandle,
        src_stage: u64,
        src_access: u64,
        dst_stage: u64,
        dst_access: u64,
        ownership: QueueOwnershipTransfer,
        offset: u64,
        size: u64,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TextureSubresourceRange {
        base_mip_level: u32,
        /// `u32::MAX` means "all remaining".
        mip_level_count: u32,
        base_array_layer: u32,
        array_layer_count: u32,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TextureBarrier {
        texture: TextureHandle,
        src_stage: u64,
        src_access: u64,
        dst_stage: u64,
        dst_access: u64,
        ownership: QueueOwnershipTransfer,
        old_layout: TextureLayout,
        new_layout: TextureLayout,
        range: TextureSubresourceRange,
    }

    // ─── Feature/device properties ────────────────────────────────────────────────

    /// Mirrors `SFT::RHI::RayTracingProperties`. See [`rhi_feature_properties`].
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct RayTracingProperties {
        max_ray_recursion_depth: u32,
        /// Byte size of one shader group handle, e.g. as written by
        /// [`rhi_write_ray_tracing_shader_group_handles`].
        shader_group_handle_size: u32,
        /// Required alignment of each region's base address within a shader binding table buffer.
        shader_group_base_alignment: u32,
        max_ray_hit_attribute_size: u32,
        max_acceleration_structure_geometry_count: u32,
        max_acceleration_structure_instance_count: u32,
        min_acceleration_structure_scratch_offset_alignment: u64,
    }

    /// Mirrors `SFT::RHI::MeshShaderProperties`. See [`rhi_feature_properties`]. Descriptor-only —
    /// mesh/task shader stages and their draw calls are not otherwise bound yet (see this module's
    /// doc comment), but the limits are cheap to expose regardless of whether a caller can act on
    /// them through this binding yet.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct MeshShaderProperties {
        max_task_work_group_invocations: u32,
        max_mesh_work_group_invocations: u32,
        max_mesh_output_vertices: u32,
        max_mesh_output_primitives: u32,
        max_mesh_multiview_view_count: u32,
        max_mesh_payload_size: u32,
    }

    /// Mirrors `SFT::RHI::VariableRateShadingProperties`. See [`rhi_feature_properties`].
    /// Descriptor-only, same caveat as [`MeshShaderProperties`].
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct VariableRateShadingProperties {
        min_tile_width: u32,
        min_tile_height: u32,
        max_tile_width: u32,
        max_tile_height: u32,
    }

    /// Mirrors `SFT::RHI::SubgroupProperties`. See [`rhi_feature_properties`].
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct SubgroupProperties {
        min_subgroup_size: u32,
        max_subgroup_size: u32,
        /// Bitmask of `sturdy::rhi::ShaderStage` flags the backend supports subgroup operations in.
        supported_stage_mask: u32,
        /// Backend-specific bitmask of supported subgroup operation categories (unlike every other
        /// bitmask in this bridge, this one has no matching `sturdy::rhi` flag type yet, since the
        /// bit meanings aren't otherwise consumed by this binding — treat as opaque/passthrough).
        supported_operation_mask: u32,
    }

    /// Mirrors `SFT::RHI::DescriptorIndexingProperties`. See [`rhi_feature_properties`].
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct DescriptorIndexingProperties {
        max_update_after_bind_descriptors: u32,
        max_variable_descriptor_count: u32,
    }

    /// Mirrors `SFT::RHI::SparseResourceProperties`. See [`rhi_feature_properties`].
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct SparseResourceProperties {
        residency_standard_2d_block_shape: bool,
        residency_standard_3d_block_shape: bool,
        residency_aligned_mip_size: bool,
        residency_non_resident_strict: bool,
    }

    /// Mirrors `SFT::RHI::FeatureProperties` — every numeric device limit tied to an optional
    /// `Feature`, gathered up front at device creation. Every field is present regardless of
    /// whether the corresponding feature is actually enabled/supported (in which case it reads as
    /// its zero value) — check `Diagnostics::gpu_capabilities`
    /// ([`sturdy_sys::diagnostics::ffi::gpu_capabilities`]) or the relevant `Feature` first if the
    /// distinction between "unsupported" and "supported with a zero limit" matters.
    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct FeatureProperties {
        ray_tracing: RayTracingProperties,
        mesh_shader: MeshShaderProperties,
        variable_rate_shading: VariableRateShadingProperties,
        subgroup: SubgroupProperties,
        descriptor_indexing: DescriptorIndexingProperties,
        sparse_resources: SparseResourceProperties,
    }

    // ─── Ray tracing ─────────────────────────────────────────────────────────────

    #[derive(Debug, Clone, Copy, Default)]
    struct AccelerationStructureBuildSizes {
        acceleration_structure_size: u64,
        build_scratch_size: u64,
        update_scratch_size: u64,
    }

    #[derive(Debug, Clone)]
    struct AccelerationStructureDesc {
        kind: AccelerationStructureType,
        size: u64,
        label: String,
    }

    #[derive(Debug, Clone)]
    struct AccelerationStructureTrianglesDesc {
        vertex_buffer: BufferHandle,
        vertex_offset: u64,
        vertex_format: VertexFormat,
        vertex_stride: u64,
        max_vertex: u32,
        index_buffer: BufferHandle,
        index_offset: u64,
        index_format: IndexFormat,
        transform_buffer: BufferHandle,
        transform_offset: u64,
        /// Requires `Feature::OpacityMicromap`. Invalid (`value == 0`, the default) for geometry
        /// without one; see [`rhi_create_opacity_micromap`].
        opacity_micromap: OpacityMicromapHandle,
        /// One index per triangle into `opacity_micromap`'s packed regions; required whenever
        /// `opacity_micromap` is valid.
        opacity_micromap_index_buffer: BufferHandle,
        opacity_micromap_index_offset: u64,
        opacity_micromap_index_format: IndexFormat,
    }

    /// Mirrors `SFT::RHI::AccelerationStructureAabbsDesc` — a buffer of `stride`-spaced
    /// `{min_xyz, max_xyz}` `f32` boxes for procedural geometry.
    #[derive(Debug, Clone, Copy, Default)]
    struct AccelerationStructureAabbsDesc {
        buffer: BufferHandle,
        offset: u64,
        stride: u64,
    }

    /// Mirrors `SFT::RHI::OpacityMicromapUsageCount`.
    #[derive(Debug, Clone, Copy)]
    struct OpacityMicromapUsageCount {
        count: u32,
        subdivision_level: u32,
        format: OpacityMicromapFormat,
    }

    /// Mirrors `SFT::RHI::OpacityMicromapDesc`.
    #[derive(Debug, Clone)]
    struct OpacityMicromapDesc {
        usage_counts: Vec<OpacityMicromapUsageCount>,
        label: String,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct OpacityMicromapBuildSizes {
        micromap_size: u64,
        build_scratch_size: u64,
    }

    /// Mirrors `SFT::RHI::OpacityMicromapBuildDesc`. `data_buffer` holds packed 1 (TwoState) or 2
    /// (FourState) bit-per-microtriangle opacity data in `VK_EXT_opacity_micromap` encoding.
    #[derive(Debug, Clone)]
    struct OpacityMicromapBuildDesc {
        dst: OpacityMicromapHandle,
        scratch_buffer: BufferHandle,
        scratch_offset: u64,
        data_buffer: BufferHandle,
        data_buffer_offset: u64,
        usage_counts: Vec<OpacityMicromapUsageCount>,
    }

    #[derive(Debug, Clone, Default)]
    struct AccelerationStructureInstancesDesc {
        buffer: BufferHandle,
        offset: u64,
        array_of_pointers: bool,
    }

    #[derive(Debug, Clone)]
    struct AccelerationStructureGeometryDesc {
        geometry_type: AccelerationStructureGeometryType,
        /// Bitmask of `sturdy::rhi::AccelerationStructureGeometryFlags` flags.
        flags: u32,
        triangles: AccelerationStructureTrianglesDesc,
        instances: AccelerationStructureInstancesDesc,
        /// Only meaningful when `geometry_type == Aabbs`.
        aabbs: AccelerationStructureAabbsDesc,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct AccelerationStructureBuildRangeInfo {
        primitive_count: u32,
        primitive_offset: u32,
        first_vertex: u32,
        transform_offset: u32,
    }

    #[derive(Debug, Clone)]
    struct AccelerationStructureBuildDesc {
        kind: AccelerationStructureType,
        /// Bitmask of `sturdy::rhi::AccelerationStructureBuildFlags` flags.
        flags: u32,
        dst: AccelerationStructureHandle,
        /// Invalid (`value == 0`) for a from-scratch build; set for an in-place update.
        src: AccelerationStructureHandle,
        scratch_buffer: BufferHandle,
        scratch_offset: u64,
        geometries: Vec<AccelerationStructureGeometryDesc>,
        ranges: Vec<AccelerationStructureBuildRangeInfo>,
    }

    #[derive(Debug, Clone)]
    struct RayTracingShaderGroupDesc {
        group_type: RayTracingShaderGroupType,
        general: ShaderEntry,
        closest_hit: ShaderEntry,
        any_hit: ShaderEntry,
        intersection: ShaderEntry,
    }

    #[derive(Debug, Clone)]
    struct RayTracingPipelineDesc {
        layout: PipelineLayoutHandle,
        groups: Vec<RayTracingShaderGroupDesc>,
        max_ray_recursion_depth: u32,
        label: String,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct ShaderBindingTableRegion {
        buffer: BufferHandle,
        offset: u64,
        size: u64,
        stride: u64,
    }

    #[derive(Debug, Clone, Copy, Default)]
    struct TraceRaysDesc {
        raygen: ShaderBindingTableRegion,
        miss: ShaderBindingTableRegion,
        hit: ShaderBindingTableRegion,
        callable: ShaderBindingTableRegion,
        width: u32,
        height: u32,
        depth: u32,
    }

    /// Mirrors `SFT::RHI::DeviceLimits`.
    #[derive(Debug, Clone, Copy, Default, PartialEq)]
    struct DeviceLimits {
        max_texture_dimension_2d: u32,
        max_texture_array_layers: u32,
        max_bind_groups: u32,
        max_push_constants_size: u32,
        max_vertex_buffers: u32,
        max_vertex_attributes: u32,
        max_color_attachments: u32,
        max_framebuffer_sample_count: u32,
        /// Bitmask of supported sample counts (bit N set = 2^N samples).
        framebuffer_sample_counts: u32,
        supports_minimum_depth_resolve: bool,
        supports_bc_texture_compression: bool,
        max_compute_workgroup_size_x: u32,
        max_compute_workgroup_size_y: u32,
        max_compute_workgroup_size_z: u32,
        min_uniform_buffer_offset_alignment: u64,
        min_storage_buffer_offset_alignment: u64,
        /// Nanoseconds per timestamp-query tick.
        timestamp_period_ns: f32,
        timestamp_valid_bits: u32,
    }

    /// Mirrors `SFT::RHI::ExtensionId`.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct ExtensionInfo {
        name_space: String,
        name: String,
        version: u32,
    }

    /// Mirrors `SFT::RHI::QueueInfo`.
    #[derive(Debug, Clone, Default, PartialEq, Eq)]
    struct QueueInfo {
        queue: QueueClass,
        /// `RHI::QueueCapability` bits: graphics 1, compute 2, transfer 4, present 8, sparse
        /// binding 16, video decode 32, video encode 64.
        capabilities: u32,
        lane_count: u32,
        physical_group: u32,
        likely_parallel_with_graphics: bool,
        dedicated: bool,
        label: String,
    }

    /// One TLAS instance, unpacked (the C++ side packs it into the backend's 64-byte layout via
    /// `AccelerationStructureInstance::set_*`, so this bridge never depends on that bit layout).
    #[derive(Debug, Clone, Copy, Default)]
    struct AccelerationStructureInstance {
        transform: [f32; 12],
        custom_index: u32,
        mask: u8,
        shader_binding_table_offset: u32,
        flags: u8,
        blas_device_address: u64,
    }

    unsafe extern "C++" {
        include!("sturdy_rs/shim.hpp");
        include!("sturdy_rs/rhi.hpp");

        // `#[namespace = "sturdy_rs"]` is required here: without it cxx computes this alias's
        // `ExternType::Id` from *this* bridge's namespace ("sturdy_rs::rhi::EngineView"), which
        // does not match the root bridge's actual identity ("sturdy_rs::EngineView") and fails to
        // typecheck.
        #[namespace = "sturdy_rs"]
        type EngineView = crate::ffi::EngineView;

        /// `RHI::RhiDevice`, only ever handled by pointer/reference; owned by the engine.
        type RhiDevice;
        /// `RHI::CommandEncoder`, owned by whichever `UniquePtr` currently holds it.
        type CommandEncoder;
        type RenderPassEncoder;
        type ComputePassEncoder;
        /// `RHI::RenderBundleEncoder` — records a reusable draw sequence, replayed inside any
        /// compatible render pass via [`rp_execute_bundles`].
        type RenderBundleEncoder;

        fn engine_rhi_device(engine: Pin<&mut EngineView>) -> *mut RhiDevice;

        /// Resolves a Renderer-registry texture (`sturdy::assets::RendererTextureHandle`'s raw
        /// value) to the RHI resources behind it (`Renderer::texture()`'s `TextureResource`).
        /// `false` when there's no renderer yet or the handle isn't a live texture. The RHI
        /// handles stay owned by the Renderer: valid only while the texture asset is loaded, and
        /// never to be destroyed by the caller.
        fn renderer_texture_resources(
            engine: Pin<&mut EngineView>,
            renderer_texture: u64,
            out: &mut RendererTextureResources,
        ) -> bool;

        /// Every numeric device limit gated behind an optional `Feature` (ray tracing, mesh
        /// shaders, variable-rate shading, subgroups, descriptor indexing, sparse resources),
        /// gathered once at device creation (`RhiDevice::feature_properties`). Cheap, read-only —
        /// safe to call every frame, though the values never change for a given device.
        fn rhi_feature_properties(device: &RhiDevice) -> FeatureProperties;
        /// Core device limits (`RhiDevice::limits`).
        fn rhi_device_limits(device: &RhiDevice) -> DeviceLimits;
        /// Human-readable names of every optional feature the device enabled
        /// (`RhiDevice::enabled_features`, named by `RHI::feature_name`).
        fn rhi_enabled_features(device: &RhiDevice) -> Vec<String>;
        /// Every feature name this engine build knows about, in `RHI::Feature` order.
        fn rhi_all_feature_names() -> Vec<String>;
        /// Backend extensions the device enabled (`RhiDevice::enabled_extensions`).
        fn rhi_enabled_extensions(device: &RhiDevice) -> Vec<ExtensionInfo>;
        /// Every queue family the device exposes (`RhiDevice::queue_infos`).
        fn rhi_queue_infos(device: &RhiDevice) -> Vec<QueueInfo>;

        // ─── Resources ───────────────────────────────────────────────────────────
        fn rhi_create_buffer(device: Pin<&mut RhiDevice>, desc: &BufferDesc, status: &mut RhiStatus) -> BufferHandle;
        fn rhi_destroy_buffer(device: Pin<&mut RhiDevice>, handle: BufferHandle);
        fn rhi_write_buffer(
            device: Pin<&mut RhiDevice>,
            buffer: BufferHandle,
            offset: u64,
            data: &[u8],
            status: &mut RhiStatus,
        );
        fn rhi_buffer_device_address(device: &RhiDevice, buffer: BufferHandle, status: &mut RhiStatus) -> u64;
        /// Maps `buffer` for CPU access; `len` receives the mapped range's length in bytes. Returns
        /// null (and an error `status`) on failure. The returned pointer stays valid until
        /// `rhi_unmap_buffer` is called for the same buffer.
        fn rhi_map_buffer(device: Pin<&mut RhiDevice>, buffer: BufferHandle, len: &mut u64, status: &mut RhiStatus) -> *mut u8;
        fn rhi_unmap_buffer(device: Pin<&mut RhiDevice>, buffer: BufferHandle);

        fn rhi_create_texture(device: Pin<&mut RhiDevice>, desc: &TextureDesc, status: &mut RhiStatus) -> TextureHandle;
        fn rhi_destroy_texture(device: Pin<&mut RhiDevice>, handle: TextureHandle);
        fn rhi_create_texture_view(
            device: Pin<&mut RhiDevice>,
            desc: &TextureViewDesc,
            status: &mut RhiStatus,
        ) -> TextureViewHandle;
        fn rhi_destroy_texture_view(device: Pin<&mut RhiDevice>, handle: TextureViewHandle);

        fn rhi_create_sampler(device: Pin<&mut RhiDevice>, desc: &SamplerDesc, status: &mut RhiStatus) -> SamplerHandle;
        fn rhi_destroy_sampler(device: Pin<&mut RhiDevice>, handle: SamplerHandle);

        fn rhi_create_shader_module(
            device: Pin<&mut RhiDevice>,
            desc: &ShaderModuleDesc,
            status: &mut RhiStatus,
        ) -> ShaderModuleHandle;
        fn rhi_destroy_shader_module(device: Pin<&mut RhiDevice>, handle: ShaderModuleHandle);

        fn rhi_create_bind_group_layout(
            device: Pin<&mut RhiDevice>,
            desc: &BindGroupLayoutDesc,
            status: &mut RhiStatus,
        ) -> BindGroupLayoutHandle;
        fn rhi_destroy_bind_group_layout(device: Pin<&mut RhiDevice>, handle: BindGroupLayoutHandle);

        fn rhi_create_bind_group(
            device: Pin<&mut RhiDevice>,
            desc: &BindGroupDesc,
            status: &mut RhiStatus,
        ) -> BindGroupHandle;
        fn rhi_destroy_bind_group(device: Pin<&mut RhiDevice>, handle: BindGroupHandle);

        fn rhi_create_pipeline_layout(
            device: Pin<&mut RhiDevice>,
            desc: &PipelineLayoutDesc,
            status: &mut RhiStatus,
        ) -> PipelineLayoutHandle;
        fn rhi_destroy_pipeline_layout(device: Pin<&mut RhiDevice>, handle: PipelineLayoutHandle);

        fn rhi_create_render_pipeline(
            device: Pin<&mut RhiDevice>,
            desc: &RenderPipelineDesc,
            status: &mut RhiStatus,
        ) -> RenderPipelineHandle;
        fn rhi_destroy_render_pipeline(device: Pin<&mut RhiDevice>, handle: RenderPipelineHandle);

        fn rhi_create_compute_pipeline(
            device: Pin<&mut RhiDevice>,
            desc: &ComputePipelineDesc,
            status: &mut RhiStatus,
        ) -> ComputePipelineHandle;
        fn rhi_destroy_compute_pipeline(device: Pin<&mut RhiDevice>, handle: ComputePipelineHandle);

        // ─── Query sets ──────────────────────────────────────────────────────────
        fn rhi_create_query_set(device: Pin<&mut RhiDevice>, desc: &QuerySetDesc, status: &mut RhiStatus) -> QuerySetHandle;
        fn rhi_destroy_query_set(device: Pin<&mut RhiDevice>, handle: QuerySetHandle);
        /// Reads back `count` query results starting at `first` into `dst` (each result `stride`
        /// bytes apart); `flags` is a bitmask of `sturdy::rhi::QueryResultFlags`.
        #[allow(clippy::too_many_arguments)]
        fn rhi_get_query_set_results(
            device: Pin<&mut RhiDevice>,
            query_set: QuerySetHandle,
            first: u32,
            count: u32,
            dst: &mut [u8],
            stride: u64,
            flags: u32,
            status: &mut RhiStatus,
        );
        fn rhi_reset_query_set(device: Pin<&mut RhiDevice>, query_set: QuerySetHandle, first: u32, count: u32);

        // ─── Ray tracing ─────────────────────────────────────────────────────────
        fn rhi_acceleration_structure_build_sizes(
            device: &RhiDevice,
            desc: &AccelerationStructureBuildDesc,
            status: &mut RhiStatus,
        ) -> AccelerationStructureBuildSizes;
        fn rhi_create_acceleration_structure(
            device: Pin<&mut RhiDevice>,
            desc: &AccelerationStructureDesc,
            status: &mut RhiStatus,
        ) -> AccelerationStructureHandle;
        fn rhi_destroy_acceleration_structure(device: Pin<&mut RhiDevice>, handle: AccelerationStructureHandle);
        fn rhi_acceleration_structure_device_address(
            device: &RhiDevice,
            handle: AccelerationStructureHandle,
            status: &mut RhiStatus,
        ) -> u64;

        /// Storage/scratch sizes for an opacity micromap. Requires `Feature::OpacityMicromap`.
        fn rhi_opacity_micromap_build_sizes(
            device: &RhiDevice,
            desc: &OpacityMicromapDesc,
            status: &mut RhiStatus,
        ) -> OpacityMicromapBuildSizes;
        /// `size` is `OpacityMicromapBuildSizes::micromap_size` from
        /// [`rhi_opacity_micromap_build_sizes`].
        fn rhi_create_opacity_micromap(
            device: Pin<&mut RhiDevice>,
            desc: &OpacityMicromapDesc,
            size: u64,
            status: &mut RhiStatus,
        ) -> OpacityMicromapHandle;
        fn rhi_destroy_opacity_micromap(device: Pin<&mut RhiDevice>, handle: OpacityMicromapHandle);

        fn rhi_create_ray_tracing_pipeline(
            device: Pin<&mut RhiDevice>,
            desc: &RayTracingPipelineDesc,
            status: &mut RhiStatus,
        ) -> RayTracingPipelineHandle;
        fn rhi_destroy_ray_tracing_pipeline(device: Pin<&mut RhiDevice>, handle: RayTracingPipelineHandle);
        fn rhi_write_ray_tracing_shader_group_handles(
            device: Pin<&mut RhiDevice>,
            pipeline: RayTracingPipelineHandle,
            first_group: u32,
            group_count: u32,
            dst: &mut [u8],
            status: &mut RhiStatus,
        );

        /// Packs unpacked TLAS instances into the backend's native
        /// `AccelerationStructureInstance` byte layout (64 bytes each), ready to upload into an
        /// instance buffer via `rhi_write_buffer`.
        fn rhi_pack_acceleration_structure_instances(instances: &[AccelerationStructureInstance]) -> Vec<u8>;

        // ─── Command encoding ────────────────────────────────────────────────────
        fn rhi_create_command_encoder(
            device: Pin<&mut RhiDevice>,
            desc: &CommandEncoderDesc,
            status: &mut RhiStatus,
        ) -> UniquePtr<CommandEncoder>;
        fn rhi_submit(device: Pin<&mut RhiDevice>, command_buffers: &[CommandBufferHandle], status: &mut RhiStatus);
        /// Blocks until all submitted GPU work has completed. The blunt instrument — prefer a
        /// fence ([`rhi_submit_with`] + [`rhi_wait_fences`]) or timeline semaphore when only one
        /// submission's completion matters.
        fn rhi_wait_idle(device: Pin<&mut RhiDevice>);
        /// Full-control submission: target queue lane, semaphore waits/signals, and an optional
        /// fence signaled on completion.
        fn rhi_submit_with(device: Pin<&mut RhiDevice>, desc: &SubmitDesc, status: &mut RhiStatus);

        // ─── Synchronization ─────────────────────────────────────────────────────
        fn rhi_create_semaphore(device: Pin<&mut RhiDevice>, desc: &SemaphoreDesc, status: &mut RhiStatus) -> SemaphoreHandle;
        fn rhi_destroy_semaphore(device: Pin<&mut RhiDevice>, handle: SemaphoreHandle);
        /// The semaphore's current counter value.
        fn rhi_semaphore_value(device: &RhiDevice, handle: SemaphoreHandle, status: &mut RhiStatus) -> u64;
        /// Blocks the calling thread until the semaphore reaches `value` or `timeout_ns` elapses
        /// (`u64::MAX` waits forever); a timeout reports `RhiErrorCode::NotReady`.
        fn rhi_wait_semaphore(
            device: Pin<&mut RhiDevice>,
            handle: SemaphoreHandle,
            value: u64,
            timeout_ns: u64,
            status: &mut RhiStatus,
        );
        /// Signals the semaphore to `value` from the CPU.
        fn rhi_signal_semaphore(device: Pin<&mut RhiDevice>, handle: SemaphoreHandle, value: u64, status: &mut RhiStatus);
        fn rhi_create_fence(device: Pin<&mut RhiDevice>, desc: &FenceDesc, status: &mut RhiStatus) -> FenceHandle;
        fn rhi_destroy_fence(device: Pin<&mut RhiDevice>, handle: FenceHandle);
        /// Blocks until all (`wait_all`) or any of `fences` are signaled, or `timeout_ns` elapses
        /// (`u64::MAX` waits forever). Returns whether the wait condition was met (`false` = timed
        /// out).
        fn rhi_wait_fences(
            device: Pin<&mut RhiDevice>,
            fences: &[FenceHandle],
            wait_all: bool,
            timeout_ns: u64,
            status: &mut RhiStatus,
        ) -> bool;
        fn rhi_reset_fences(device: Pin<&mut RhiDevice>, fences: &[FenceHandle], status: &mut RhiStatus);

        // ─── Render bundles ──────────────────────────────────────────────────────
        fn rhi_create_render_bundle_encoder(
            device: Pin<&mut RhiDevice>,
            desc: &RenderBundleDesc,
            status: &mut RhiStatus,
        ) -> UniquePtr<RenderBundleEncoder>;
        fn rhi_destroy_render_bundle(device: Pin<&mut RhiDevice>, handle: RenderBundleHandle);

        fn rb_set_pipeline(rb: Pin<&mut RenderBundleEncoder>, pipeline: RenderPipelineHandle);
        fn rb_set_bind_group(rb: Pin<&mut RenderBundleEncoder>, index: u32, bind_group: BindGroupHandle, dynamic_offsets: &[u32]);
        fn rb_set_vertex_buffer(rb: Pin<&mut RenderBundleEncoder>, slot: u32, buffer: BufferHandle, offset: u64);
        fn rb_set_index_buffer(rb: Pin<&mut RenderBundleEncoder>, buffer: BufferHandle, format: IndexFormat, offset: u64);
        fn rb_set_push_constants(rb: Pin<&mut RenderBundleEncoder>, stages: u32, offset: u32, data: &[u8]);
        fn rb_set_viewport(rb: Pin<&mut RenderBundleEncoder>, viewport: &Viewport);
        fn rb_set_scissor(rb: Pin<&mut RenderBundleEncoder>, scissor: &Rect2D);
        fn rb_set_blend_constant(rb: Pin<&mut RenderBundleEncoder>, color: &ClearColor);
        fn rb_set_stencil_reference(rb: Pin<&mut RenderBundleEncoder>, reference: u32);
        fn rb_set_depth_bounds(rb: Pin<&mut RenderBundleEncoder>, min_depth: f32, max_depth: f32);
        fn rb_set_sample_locations(
            rb: Pin<&mut RenderBundleEncoder>,
            samples_per_pixel: u32,
            grid_width: u32,
            grid_height: u32,
            locations: &[SampleLocation],
        );
        fn rb_draw(rb: Pin<&mut RenderBundleEncoder>, args: &DrawArgs);
        fn rb_draw_indexed(rb: Pin<&mut RenderBundleEncoder>, args: &DrawIndexedArgs);
        fn rb_draw_mesh_tasks(rb: Pin<&mut RenderBundleEncoder>, args: &DrawMeshTasksArgs);
        fn rb_draw_indirect(rb: Pin<&mut RenderBundleEncoder>, indirect_buffer: BufferHandle, offset: u64);
        fn rb_draw_indexed_indirect(rb: Pin<&mut RenderBundleEncoder>, indirect_buffer: BufferHandle, offset: u64);
        fn rb_draw_indirect_multi(
            rb: Pin<&mut RenderBundleEncoder>,
            indirect_buffer: BufferHandle,
            offset: u64,
            draw_count: u32,
            stride: u32,
        );
        fn rb_draw_indexed_indirect_multi(
            rb: Pin<&mut RenderBundleEncoder>,
            indirect_buffer: BufferHandle,
            offset: u64,
            draw_count: u32,
            stride: u32,
        );
        fn rb_draw_indirect_count(
            rb: Pin<&mut RenderBundleEncoder>,
            indirect_buffer: BufferHandle,
            indirect_offset: u64,
            count_buffer: BufferHandle,
            count_offset: u64,
            max_draws: u32,
            stride: u32,
        );
        fn rb_draw_indexed_indirect_count(
            rb: Pin<&mut RenderBundleEncoder>,
            indirect_buffer: BufferHandle,
            indirect_offset: u64,
            count_buffer: BufferHandle,
            count_offset: u64,
            max_draws: u32,
            stride: u32,
        );
        fn rb_draw_mesh_tasks_indirect(rb: Pin<&mut RenderBundleEncoder>, indirect_buffer: BufferHandle, offset: u64);
        fn rb_draw_mesh_tasks_indirect_count(
            rb: Pin<&mut RenderBundleEncoder>,
            indirect_buffer: BufferHandle,
            indirect_offset: u64,
            count_buffer: BufferHandle,
            count_offset: u64,
            max_draws: u32,
            stride: u32,
        );
        /// Ends recording and returns the replayable bundle.
        fn rb_finish(rb: Pin<&mut RenderBundleEncoder>, status: &mut RhiStatus) -> RenderBundleHandle;

        fn ce_begin_render_pass(
            encoder: Pin<&mut CommandEncoder>,
            desc: &RenderPassDesc,
            status: &mut RhiStatus,
        ) -> UniquePtr<RenderPassEncoder>;
        fn ce_begin_compute_pass(
            encoder: Pin<&mut CommandEncoder>,
            label: &str,
            status: &mut RhiStatus,
        ) -> UniquePtr<ComputePassEncoder>;

        fn ce_copy_buffer_to_buffer(encoder: Pin<&mut CommandEncoder>, src: BufferHandle, dst: BufferHandle, region: &BufferCopy);
        fn ce_copy_buffer_to_texture(
            encoder: Pin<&mut CommandEncoder>,
            src: BufferHandle,
            dst: TextureHandle,
            region: &BufferTextureCopy,
        );
        fn ce_copy_texture_to_buffer(
            encoder: Pin<&mut CommandEncoder>,
            src: TextureHandle,
            dst: BufferHandle,
            region: &BufferTextureCopy,
        );

        fn ce_barrier(
            encoder: Pin<&mut CommandEncoder>,
            global_barriers: &[GlobalBarrier],
            buffer_barriers: &[BufferBarrier],
            texture_barriers: &[TextureBarrier],
        );

        fn ce_fill_buffer(encoder: Pin<&mut CommandEncoder>, buffer: BufferHandle, offset: u64, size: u64, value: u32);
        fn ce_update_buffer(encoder: Pin<&mut CommandEncoder>, buffer: BufferHandle, offset: u64, data: &[u8]);
        fn ce_blit_texture(
            encoder: Pin<&mut CommandEncoder>,
            src: TextureHandle,
            dst: TextureHandle,
            region: &TextureBlit,
            filter: Filter,
        );
        fn ce_clear_color_texture(
            encoder: Pin<&mut CommandEncoder>,
            texture: TextureHandle,
            color: &ClearColor,
            range: &TextureSubresourceRange,
        );
        fn ce_clear_depth_stencil_texture(
            encoder: Pin<&mut CommandEncoder>,
            texture: TextureHandle,
            value: &ClearDepthStencilValue,
            range: &TextureSubresourceRange,
        );

        fn ce_build_acceleration_structures(encoder: Pin<&mut CommandEncoder>, builds: &[AccelerationStructureBuildDesc]);
        /// Requires `Feature::OpacityMicromap`.
        fn ce_build_opacity_micromaps(encoder: Pin<&mut CommandEncoder>, builds: &[OpacityMicromapBuildDesc]);
        /// Copies/compacts/(de)serializes `src` into `dst` per `mode`; see
        /// [`AccelerationStructureCopyMode`] for what each mode requires of `dst`.
        fn ce_copy_acceleration_structure(
            encoder: Pin<&mut CommandEncoder>,
            src: AccelerationStructureHandle,
            dst: AccelerationStructureHandle,
            mode: AccelerationStructureCopyMode,
        );
        fn ce_set_ray_tracing_pipeline(encoder: Pin<&mut CommandEncoder>, pipeline: RayTracingPipelineHandle);
        fn ce_trace_rays(encoder: Pin<&mut CommandEncoder>, desc: &TraceRaysDesc);

        fn ce_reset_query_set(encoder: Pin<&mut CommandEncoder>, query_set: QuerySetHandle, first: u32, count: u32);
        /// `stage` is a single `sturdy::rhi::PipelineStage` bit.
        fn ce_write_timestamp(encoder: Pin<&mut CommandEncoder>, stage: u64, query_set: QuerySetHandle, index: u32);
        fn ce_begin_pipeline_statistics_query(encoder: Pin<&mut CommandEncoder>, query_set: QuerySetHandle, index: u32);
        fn ce_end_pipeline_statistics_query(encoder: Pin<&mut CommandEncoder>);
        /// `flags` is a bitmask of `sturdy::rhi::QueryResultFlags`.
        #[allow(clippy::too_many_arguments)]
        fn ce_resolve_query_set(
            encoder: Pin<&mut CommandEncoder>,
            query_set: QuerySetHandle,
            first: u32,
            count: u32,
            dst: BufferHandle,
            dst_offset: u64,
            stride: u64,
            flags: u32,
        );

        fn ce_push_debug_group(encoder: Pin<&mut CommandEncoder>, label: &str);
        fn ce_pop_debug_group(encoder: Pin<&mut CommandEncoder>);

        fn ce_finish(encoder: Pin<&mut CommandEncoder>, status: &mut RhiStatus) -> CommandBufferHandle;

        fn rp_set_pipeline(rp: Pin<&mut RenderPassEncoder>, pipeline: RenderPipelineHandle);
        fn rp_set_bind_group(rp: Pin<&mut RenderPassEncoder>, index: u32, bind_group: BindGroupHandle, dynamic_offsets: &[u32]);
        fn rp_set_vertex_buffer(rp: Pin<&mut RenderPassEncoder>, slot: u32, buffer: BufferHandle, offset: u64);
        fn rp_set_index_buffer(rp: Pin<&mut RenderPassEncoder>, buffer: BufferHandle, format: IndexFormat, offset: u64);
        fn rp_set_push_constants(rp: Pin<&mut RenderPassEncoder>, stages: u32, offset: u32, data: &[u8]);
        fn rp_set_viewport(rp: Pin<&mut RenderPassEncoder>, viewport: &Viewport);
        fn rp_set_scissor(rp: Pin<&mut RenderPassEncoder>, scissor: &Rect2D);
        /// Sets the constant color blended against via [`crate::rhi::ffi::BlendFactor::ConstantColor`]/
        /// `OneMinusConstantColor`.
        fn rp_set_blend_constant(rp: Pin<&mut RenderPassEncoder>, color: &ClearColor);
        /// Sets the stencil reference value `CompareOp`s in the bound pipeline's
        /// [`DepthStencilState::stencil_front`]/`stencil_back` compare against.
        fn rp_set_stencil_reference(rp: Pin<&mut RenderPassEncoder>, reference: u32);
        /// Sets the depth range that passes the depth-bounds test, in `[0, 1]`. Requires the bound
        /// pipeline to have set [`DepthStencilState::depth_bounds_test_enable`] at creation time.
        fn rp_set_depth_bounds(rp: Pin<&mut RenderPassEncoder>, min_depth: f32, max_depth: f32);
        /// Sets custom per-sample MSAA positions for subsequent draws. `locations` must have
        /// exactly `samples_per_pixel * grid_width * grid_height` entries. Requires the bound
        /// pipeline to have set [`MultisampleState::sample_locations_enable`] at creation time.
        fn rp_set_sample_locations(
            rp: Pin<&mut RenderPassEncoder>,
            samples_per_pixel: u32,
            grid_width: u32,
            grid_height: u32,
            locations: &[SampleLocation],
        );
        /// Sets the base shading rate plus how it combines with a draw's per-primitive rate and a
        /// shading-rate attachment's per-tile rate. Requires `Feature::VariableRateShading`; not
        /// available on render bundles (D3D12 bundle-legality, per the engine's own doc comment).
        fn rp_set_shading_rate(
            rp: Pin<&mut RenderPassEncoder>,
            rate: ShadingRate,
            primitive_combiner: ShadingRateCombiner,
            attachment_combiner: ShadingRateCombiner,
        );
        fn rp_draw(rp: Pin<&mut RenderPassEncoder>, args: &DrawArgs);
        fn rp_draw_indexed(rp: Pin<&mut RenderPassEncoder>, args: &DrawIndexedArgs);
        /// Dispatches task/mesh shader workgroups (requires a mesh-shader pipeline and
        /// `Feature::MeshShader`).
        fn rp_draw_mesh_tasks(rp: Pin<&mut RenderPassEncoder>, args: &DrawMeshTasksArgs);
        fn rp_draw_mesh_tasks_indirect(rp: Pin<&mut RenderPassEncoder>, indirect_buffer: BufferHandle, offset: u64);
        fn rp_draw_mesh_tasks_indirect_count(
            rp: Pin<&mut RenderPassEncoder>,
            indirect_buffer: BufferHandle,
            indirect_offset: u64,
            count_buffer: BufferHandle,
            count_offset: u64,
            max_draws: u32,
            stride: u32,
        );
        /// Replays previously recorded render bundles inside this pass. Their
        /// [`RenderBundleDesc`] formats must match this pass's attachments.
        fn rp_execute_bundles(rp: Pin<&mut RenderPassEncoder>, bundles: &[RenderBundleHandle]);
        fn rp_draw_indirect(rp: Pin<&mut RenderPassEncoder>, indirect_buffer: BufferHandle, offset: u64);
        fn rp_draw_indexed_indirect(rp: Pin<&mut RenderPassEncoder>, indirect_buffer: BufferHandle, offset: u64);
        /// Issues `draw_count` draws read from consecutive `DrawArgs`-shaped records in
        /// `indirect_buffer` (`stride` bytes apart, starting at `offset`) in one call — unlike
        /// [`rp_draw_indirect_count`], `draw_count` is a CPU-known constant baked into the command
        /// itself rather than read from a separate count buffer, so it needs no
        /// `multi_draw_indirect_count`-equivalent feature, only `Feature::MultiDrawIndirect`.
        fn rp_draw_indirect_multi(
            rp: Pin<&mut RenderPassEncoder>,
            indirect_buffer: BufferHandle,
            offset: u64,
            draw_count: u32,
            stride: u32,
        );
        /// Same as [`rp_draw_indirect_multi`], reading `DrawIndexedArgs`-shaped records instead.
        fn rp_draw_indexed_indirect_multi(
            rp: Pin<&mut RenderPassEncoder>,
            indirect_buffer: BufferHandle,
            offset: u64,
            draw_count: u32,
            stride: u32,
        );
        fn rp_draw_indirect_count(
            rp: Pin<&mut RenderPassEncoder>,
            indirect_buffer: BufferHandle,
            indirect_offset: u64,
            count_buffer: BufferHandle,
            count_offset: u64,
            max_draws: u32,
            stride: u32,
        );
        fn rp_draw_indexed_indirect_count(
            rp: Pin<&mut RenderPassEncoder>,
            indirect_buffer: BufferHandle,
            indirect_offset: u64,
            count_buffer: BufferHandle,
            count_offset: u64,
            max_draws: u32,
            stride: u32,
        );
        fn rp_begin_occlusion_query(rp: Pin<&mut RenderPassEncoder>, query_set: QuerySetHandle, index: u32);
        fn rp_end_occlusion_query(rp: Pin<&mut RenderPassEncoder>);
        fn rp_end(rp: Pin<&mut RenderPassEncoder>);

        fn cp_set_pipeline(cp: Pin<&mut ComputePassEncoder>, pipeline: ComputePipelineHandle);
        fn cp_set_bind_group(cp: Pin<&mut ComputePassEncoder>, index: u32, bind_group: BindGroupHandle, dynamic_offsets: &[u32]);
        fn cp_set_push_constants(cp: Pin<&mut ComputePassEncoder>, stages: u32, offset: u32, data: &[u8]);
        fn cp_dispatch(cp: Pin<&mut ComputePassEncoder>, group_count_x: u32, group_count_y: u32, group_count_z: u32);
        fn cp_dispatch_indirect(cp: Pin<&mut ComputePassEncoder>, indirect_buffer: BufferHandle, offset: u64);
        fn cp_end(cp: Pin<&mut ComputePassEncoder>);
    }
}
