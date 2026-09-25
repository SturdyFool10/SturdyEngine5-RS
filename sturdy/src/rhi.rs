//! Ergonomic wrapper over the RHI (buffers, textures, samplers, bind groups, pipelines, command
//! encoding) and ray tracing bridge in `sturdy-sys`.
//!
//! # Scope
//! This covers what a typical game needs every frame: buffer/texture/sampler creation and
//! upload, bind-group-layout/bind-group/pipeline-layout/pipeline creation, a command encoder with
//! render+compute passes (draw/dispatch/copy/barrier), buffer mapping, fill/update buffer, blit,
//! texture clears, query sets, indirect draws/dispatch, debug groups, and a minimal ray tracing
//! path (BLAS/TLAS build + trace rays), plus device feature/property limits
//! ([`Rhi::feature_properties`]), acceleration structure copy/compaction
//! ([`CommandEncoder::copy_acceleration_structure`]), CPU-driven multi-draw-indirect
//! ([`RenderPass::draw_indirect_multi`]/`draw_indexed_indirect_multi`), and per-pipeline
//! depth-bounds test/per-face stencil ops/custom MSAA sample locations (extra
//! [`DepthStencilState`]/[`MultisampleState`] fields plus [`RenderPass::set_depth_bounds`]/
//! `set_stencil_reference`/`set_sample_locations`/`set_blend_constant`), render bundles
//! ([`RenderBundleEncoder`], [`RenderPass::execute_bundles`]), mesh/task shader draws,
//! variable-rate shading ([`RenderPass::set_shading_rate`]), queue-ownership transfers
//! ([`QueueOwnershipTransfer`] on both barrier types), timeline semaphores/fences plus
//! [`Rhi::submit_with`], opacity micromaps, and procedural (AABB) BLAS geometry.
//!
//! Deliberately not here: swapchain/surface/present and swapchain-level HDR. The engine owns its
//! swapchains and presentation loop (Rust never holds a `SwapchainHandle`); surface-level HDR
//! is bound one level up, in [`crate::render`], which forwards to the same RHI calls for the
//! engine's own swapchain.
//!
//! # Design
//! Descriptor structs (`BufferDesc`, `RenderPipelineDesc`, ...) and simple value enums (`Format`,
//! `CompareOp`, ...) are the bridge's own types, re-exported here: they are already plain
//! `Vec`/`String`/primitive data with no unsafety for a caller to touch, so a parallel builder
//! type per descriptor would only add ceremony without adding safety. What this module *does* add
//! on top:
//! - [`RhiError`]/`Result` instead of the bridge's out-param `RhiStatus`.
//! - [`HandleExt::is_valid`] on every resource handle.
//! - Real bitflag types ([`BufferUsage`], [`ShaderStage`], ...) instead of raw `u32`/`u64`.
//! - [`CommandEncoder`]/[`RenderPass`]/[`ComputePass`] as RAII wrappers: a render/compute pass
//!   borrows its encoder mutably, so the borrow checker — not a runtime check — refuses any
//!   attempt to touch the encoder while a pass is open, and the pass is ended automatically when
//!   it goes out of scope.

use core::marker::PhantomData;
use core::pin::Pin;
use std::fmt;

use cxx::UniquePtr;
use sturdy_sys::rhi::ffi as sys;

// ─── Re-exported value enums ────────────────────────────────────────────────────
//
// These are already real Rust enums (cxx generates them with Debug/Clone/Copy/PartialEq/Eq); a
// parallel copy of each would be pure duplication.
pub use sys::{
    AccelerationStructureCopyMode, AccelerationStructureGeometryType, AccelerationStructureType,
    OpacityMicromapFormat,
    AddressMode, BindGroupLifetime, BindingType, BlendFactor, BlendOp, BorderColor, CompareOp,
    CullMode, Filter, Format, FrontFace, IndexFormat, LoadOp, MemoryLocation, MipmapMode,
    PolygonMode, PrimitiveTopology, QueryType, QueueClass, RayTracingShaderGroupType,
    RhiErrorCode, SampleCount, ShaderLanguage, ShadingRate, ShadingRateCombiner, StencilOp,
    StorageTextureAccess, StoreOp, TextureDimension, TextureLayout, TextureViewType, VertexFormat,
    VertexStepMode,
};

// ─── Re-exported resource handles ───────────────────────────────────────────────
pub use sys::{
    AccelerationStructureHandle, BindGroupHandle, BindGroupLayoutHandle, BufferHandle,
    CommandBufferHandle, ComputePipelineHandle, FenceHandle, OpacityMicromapHandle,
    PipelineLayoutHandle, QuerySetHandle,
    RayTracingPipelineHandle, RenderBundleHandle, RenderPipelineHandle, SamplerHandle,
    SemaphoreHandle, ShaderModuleHandle, TextureHandle, TextureViewHandle,
};
/// The RHI resources behind a loaded texture asset, from
/// [`Assets::rhi_texture`](crate::assets::Assets::rhi_texture).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RendererTextureResources {
    pub texture: TextureHandle,
    pub view: TextureViewHandle,
    pub sampler: SamplerHandle,
    pub size: glam::UVec2,
    pub mip_levels: u32,
    pub format: Format,
}

impl RendererTextureResources {
    pub(crate) fn from_ffi(value: sys::RendererTextureResources) -> Self {
        Self {
            texture: value.texture,
            view: value.view,
            sampler: value.sampler,
            size: glam::uvec2(value.width, value.height),
            mip_levels: value.mip_levels,
            format: value.format,
        }
    }
}
pub use sys::{DeviceLimits, ExtensionInfo, QueueInfo};

/// Every RHI handle carries `value == 0` for "invalid"; this is the one thing that isn't obvious
/// from the bridge's plain `{ value: u64 }` structs.
pub trait HandleExt {
    fn is_valid(&self) -> bool;
}

macro_rules! impl_handle_ext {
    ($($t:ty),+ $(,)?) => {
        $(impl HandleExt for $t {
            fn is_valid(&self) -> bool {
                self.value != 0
            }
        })+
    };
}
impl_handle_ext!(
    BufferHandle,
    TextureHandle,
    TextureViewHandle,
    SamplerHandle,
    ShaderModuleHandle,
    BindGroupLayoutHandle,
    BindGroupHandle,
    PipelineLayoutHandle,
    RenderPipelineHandle,
    ComputePipelineHandle,
    RayTracingPipelineHandle,
    AccelerationStructureHandle,
    CommandBufferHandle,
    QuerySetHandle,
    RenderBundleHandle,
    SemaphoreHandle,
    FenceHandle,
    OpacityMicromapHandle,
);

// ─── Re-exported descriptor structs ─────────────────────────────────────────────
//
// Plain data (Vec/String/primitive fields); see the module doc for why these aren't re-wrapped.
pub use sys::{
    AccelerationStructureBuildDesc, AccelerationStructureBuildRangeInfo,
    AccelerationStructureBuildSizes, AccelerationStructureDesc, AccelerationStructureGeometryDesc,
    AccelerationStructureInstance, AccelerationStructureInstancesDesc,
    AccelerationStructureTrianglesDesc, BindGroupDesc, BindGroupEntry, BindGroupLayoutDesc,
    BindGroupLayoutEntry, BlendComponent, BufferBarrier, BufferCopy, BufferDesc,
    BufferTextureCopy, ClearColor, ColorAttachment, ColorTargetState, CommandEncoderDesc,
    ComputePipelineDesc, DepthStencilAttachment, DepthStencilState, DescriptorIndexingProperties,
    DrawArgs, DrawIndexedArgs, FeatureProperties, GlobalBarrier, MeshShaderProperties,
    MultisampleState, PipelineLayoutDesc, PushConstantRange, RasterizationState,
    RayTracingPipelineDesc, RayTracingProperties, RayTracingShaderGroupDesc, Rect2D,
    RenderPassDesc, RenderPipelineDesc, SampleLocation, SamplerDesc, ShaderBindingTableRegion,
    ShaderEntry, ShaderModuleDesc, SparseResourceProperties, StencilFaceState,
    SubgroupProperties, TextureBarrier, TextureBlit, TextureDesc, TextureSubresourceLayers,
    TextureSubresourceRange, TextureViewDesc, TraceRaysDesc, VariableRateShadingProperties,
    Viewport, VertexAttribute, VertexBufferLayout,
};
pub use sys::{ClearDepthStencilValue, QuerySetDesc};
pub use sys::{
    DrawMeshTasksArgs, FenceDesc, QueueLane, QueueOwnershipTransfer, QueueSemaphoreOp,
    RenderBundleDesc, SemaphoreDesc, SubmitDesc,
};
pub use sys::{
    AccelerationStructureAabbsDesc, OpacityMicromapBuildDesc, OpacityMicromapBuildSizes,
    OpacityMicromapDesc, OpacityMicromapUsageCount,
};

/// Timeout value meaning "block until the condition is met", for [`Rhi::wait_semaphore`]/
/// [`Rhi::wait_fences`]. Mirrors `SFT::RHI::wait_forever`.
pub const WAIT_FOREVER: u64 = u64::MAX;

// ─── Bitflags ────────────────────────────────────────────────────────────────────

macro_rules! bitflags {
    ($name:ident : $repr:ty { $($flag:ident = $value:expr),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
        pub struct $name(pub $repr);

        impl $name {
            pub const NONE: Self = Self(0);
            $(pub const $flag: Self = Self($value);)+

            #[must_use]
            pub const fn bits(self) -> $repr {
                self.0
            }

            #[must_use]
            pub const fn contains(self, other: Self) -> bool {
                (self.0 & other.0) == other.0
            }
        }

        impl core::ops::BitOr for $name {
            type Output = Self;
            fn bitor(self, rhs: Self) -> Self {
                Self(self.0 | rhs.0)
            }
        }
        impl core::ops::BitOrAssign for $name {
            fn bitor_assign(&mut self, rhs: Self) {
                self.0 |= rhs.0;
            }
        }
        impl core::ops::BitAnd for $name {
            type Output = Self;
            fn bitand(self, rhs: Self) -> Self {
                Self(self.0 & rhs.0)
            }
        }
    };
}

bitflags!(BufferUsage: u32 {
    TRANSFER_SRC = 1 << 0,
    TRANSFER_DST = 1 << 1,
    VERTEX = 1 << 2,
    INDEX = 1 << 3,
    UNIFORM = 1 << 4,
    STORAGE = 1 << 5,
    INDIRECT = 1 << 6,
    SHADER_BINDING_TABLE = 1 << 7,
    ACCELERATION_STRUCTURE = 1 << 8,
    ACCELERATION_STRUCTURE_INPUT = 1 << 9,
    ACCELERATION_STRUCTURE_SCRATCH = 1 << 10,
});

bitflags!(TextureUsage: u32 {
    TRANSFER_SRC = 1 << 0,
    TRANSFER_DST = 1 << 1,
    SAMPLED = 1 << 2,
    STORAGE = 1 << 3,
    COLOR_ATTACHMENT = 1 << 4,
    DEPTH_STENCIL_ATTACHMENT = 1 << 5,
    TRANSIENT_ATTACHMENT = 1 << 6,
});

bitflags!(ShaderStage: u32 {
    VERTEX = 1 << 0,
    FRAGMENT = 1 << 1,
    COMPUTE = 1 << 2,
    GEOMETRY = 1 << 3,
    TESS_CONTROL = 1 << 4,
    TESS_EVAL = 1 << 5,
    TASK = 1 << 6,
    MESH = 1 << 7,
    RAY_GENERATION = 1 << 8,
    ANY_HIT = 1 << 9,
    CLOSEST_HIT = 1 << 10,
    MISS = 1 << 11,
    INTERSECTION = 1 << 12,
    CALLABLE = 1 << 13,
});

bitflags!(BindingFlags: u32 {
    PARTIALLY_BOUND = 1 << 0,
    UPDATE_AFTER_BIND = 1 << 1,
    VARIABLE_DESCRIPTOR_COUNT = 1 << 2,
});

bitflags!(ColorWriteMask: u32 {
    RED = 1 << 0,
    GREEN = 1 << 1,
    BLUE = 1 << 2,
    ALPHA = 1 << 3,
});
impl ColorWriteMask {
    pub const ALL: Self = Self(Self::RED.0 | Self::GREEN.0 | Self::BLUE.0 | Self::ALPHA.0);
}

bitflags!(AccelerationStructureBuildFlags: u32 {
    ALLOW_UPDATE = 1 << 0,
    ALLOW_COMPACTION = 1 << 1,
    PREFER_FAST_TRACE = 1 << 2,
    PREFER_FAST_BUILD = 1 << 3,
    MINIMIZE_MEMORY = 1 << 4,
});

bitflags!(AccelerationStructureGeometryFlags: u32 {
    OPAQUE = 1 << 0,
    NO_DUPLICATE_ANY_HIT_INVOCATION = 1 << 1,
});

bitflags!(PipelineStage: u64 {
    DRAW_INDIRECT = 1 << 0,
    VERTEX_INPUT = 1 << 1,
    VERTEX_SHADER = 1 << 2,
    TESS_CONTROL_SHADER = 1 << 3,
    TESS_EVAL_SHADER = 1 << 4,
    GEOMETRY_SHADER = 1 << 5,
    FRAGMENT_SHADER = 1 << 6,
    EARLY_FRAGMENT_TESTS = 1 << 7,
    LATE_FRAGMENT_TESTS = 1 << 8,
    COLOR_ATTACHMENT_OUTPUT = 1 << 9,
    COMPUTE_SHADER = 1 << 10,
    TRANSFER = 1 << 11,
    HOST = 1 << 12,
    TASK_SHADER = 1 << 13,
    MESH_SHADER = 1 << 14,
    RAY_TRACING_SHADER = 1 << 15,
    ACCELERATION_STRUCTURE_BUILD = 1 << 16,
    ALL_COMMANDS = u64::MAX,
});

bitflags!(AccessFlags: u64 {
    INDIRECT_COMMAND_READ = 1 << 0,
    INDEX_READ = 1 << 1,
    VERTEX_ATTRIBUTE_READ = 1 << 2,
    UNIFORM_READ = 1 << 3,
    SHADER_READ = 1 << 4,
    SHADER_WRITE = 1 << 5,
    COLOR_ATTACHMENT_READ = 1 << 6,
    COLOR_ATTACHMENT_WRITE = 1 << 7,
    DEPTH_STENCIL_ATTACHMENT_READ = 1 << 8,
    DEPTH_STENCIL_ATTACHMENT_WRITE = 1 << 9,
    TRANSFER_READ = 1 << 10,
    TRANSFER_WRITE = 1 << 11,
    HOST_READ = 1 << 12,
    HOST_WRITE = 1 << 13,
    ACCELERATION_STRUCTURE_READ = 1 << 14,
    ACCELERATION_STRUCTURE_WRITE = 1 << 15,
    MEMORY_READ = 1 << 16,
    MEMORY_WRITE = 1 << 17,
});

bitflags!(QueryResultFlags: u32 {
    RESULT_64_BIT = 1 << 0,
    WAIT = 1 << 1,
    WITH_AVAILABILITY = 1 << 2,
    PARTIAL = 1 << 3,
});

bitflags!(PipelineStatistic: u32 {
    INPUT_ASSEMBLY_VERTICES = 1 << 0,
    INPUT_ASSEMBLY_PRIMITIVES = 1 << 1,
    VERTEX_SHADER_INVOCATIONS = 1 << 2,
    GEOMETRY_SHADER_INVOCATIONS = 1 << 3,
    GEOMETRY_SHADER_PRIMITIVES = 1 << 4,
    CLIPPING_INVOCATIONS = 1 << 5,
    CLIPPING_PRIMITIVES = 1 << 6,
    FRAGMENT_SHADER_INVOCATIONS = 1 << 7,
    TESS_CONTROL_SHADER_PATCHES = 1 << 8,
    TESS_EVALUATION_SHADER_INVOCATIONS = 1 << 9,
    COMPUTE_SHADER_INVOCATIONS = 1 << 10,
    TASK_SHADER_INVOCATIONS = 1 << 11,
    MESH_SHADER_INVOCATIONS = 1 << 12,
});

// ─── Errors ──────────────────────────────────────────────────────────────────────

/// An RHI operation failed; `code` is coarse (see `RhiErrorCode`), `message` is backend-specific
/// detail meant for logs, not for branching on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RhiError {
    pub code: RhiErrorCode,
    pub message: String,
}

impl fmt::Display for RhiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.code, self.message)
    }
}

impl std::error::Error for RhiError {}

pub type Result<T> = core::result::Result<T, RhiError>;

fn finish<T>(value: T, status: sys::RhiStatus) -> Result<T> {
    if status.ok {
        Ok(value)
    } else {
        Err(RhiError { code: status.code, message: status.message })
    }
}

fn finish_unit(status: sys::RhiStatus) -> Result<()> {
    finish((), status)
}

/// A placeholder `RhiStatus` for the bridge's out-param calling convention; every bridged
/// function overwrites this before returning (see `sturdy_sys::rhi`'s module doc on why it's an
/// out-param rather than a cxx `Result`).
fn new_status() -> sys::RhiStatus {
    sys::RhiStatus { ok: false, code: sys::RhiErrorCode::OperationFailed, message: String::new() }
}

// ─── Device entry point ──────────────────────────────────────────────────────────

/// Borrowed access to `RHI::RhiDevice`. Obtained from [`crate::Engine::rhi`]; scoped exactly like
/// [`crate::Engine`] itself so a handle to the device can never outlive the callback it came from.
pub struct Rhi<'a> {
    device: *mut sys::RhiDevice,
    _scope: PhantomData<&'a mut ()>,
}

impl<'a> Rhi<'a> {
    /// # Safety-relevant invariant
    /// `device` must come from `EngineView::rhi_device()` (never null while the engine's renderer
    /// is initialized) and must not be used past the lifetime `'a` it is tied to here.
    pub(crate) fn new(device: *mut sys::RhiDevice) -> Self {
        Self { device, _scope: PhantomData }
    }

    fn device_mut(&mut self) -> Pin<&mut sys::RhiDevice> {
        // SAFETY: `device` is non-null and live for `'a` per the constructor's invariant; `RhiDevice`
        // is never relocated by the engine while alive, so pinning is sound.
        unsafe { Pin::new_unchecked(&mut *self.device) }
    }

    fn device_ref(&self) -> &sys::RhiDevice {
        // SAFETY: see `device_mut`.
        unsafe { &*self.device }
    }

    /// Every numeric device limit gated behind an optional feature (ray tracing, mesh shaders,
    /// variable-rate shading, subgroups, descriptor indexing, sparse resources). Cheap and
    /// read-only — the values never change for a given device, but nothing here caches them, so
    /// call it once and hold onto the result if you need it repeatedly. A field reads as its zero
    /// value when the corresponding feature isn't supported; check
    /// [`crate::Diagnostics::gpu_capabilities`] first if that distinction matters.
    pub fn feature_properties(&self) -> FeatureProperties {
        sys::rhi_feature_properties(self.device_ref())
    }

    /// Core device limits (texture/bind-group/push-constant/workgroup maxima, buffer offset
    /// alignments, timestamp period).
    pub fn limits(&self) -> DeviceLimits {
        sys::rhi_device_limits(self.device_ref())
    }

    /// Names of the optional features the device enabled; see [`all_feature_names`] for the
    /// full vocabulary.
    pub fn enabled_features(&self) -> Vec<String> {
        sys::rhi_enabled_features(self.device_ref())
    }

    /// Backend extensions the device enabled.
    pub fn enabled_extensions(&self) -> Vec<ExtensionInfo> {
        sys::rhi_enabled_extensions(self.device_ref())
    }

    /// Every queue family the device exposes.
    pub fn queue_infos(&self) -> Vec<QueueInfo> {
        sys::rhi_queue_infos(self.device_ref())
    }

    // ─── Buffers ───────────────────────────────────────────────────────────────
    pub fn create_buffer(&mut self, desc: &BufferDesc) -> Result<BufferHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_buffer(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_buffer(&mut self, handle: BufferHandle) {
        sys::rhi_destroy_buffer(self.device_mut(), handle);
    }

    /// Uploads `data` into `buffer` at `offset`. Works for both `HostUpload` buffers (a direct
    /// memcpy) and `DeviceLocal` ones (the engine stages the upload internally).
    pub fn write_buffer(&mut self, buffer: BufferHandle, offset: u64, data: &[u8]) -> Result<()> {
        let mut status = new_status();
        sys::rhi_write_buffer(self.device_mut(), buffer, offset, data, &mut status);
        finish_unit(status)
    }

    pub fn buffer_device_address(&self, buffer: BufferHandle) -> Result<u64> {
        let mut status = new_status();
        let address = sys::rhi_buffer_device_address(self.device_ref(), buffer, &mut status);
        finish(address, status)
    }

    // ─── Textures ──────────────────────────────────────────────────────────────
    pub fn create_texture(&mut self, desc: &TextureDesc) -> Result<TextureHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_texture(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_texture(&mut self, handle: TextureHandle) {
        sys::rhi_destroy_texture(self.device_mut(), handle);
    }

    pub fn create_texture_view(&mut self, desc: &TextureViewDesc) -> Result<TextureViewHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_texture_view(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_texture_view(&mut self, handle: TextureViewHandle) {
        sys::rhi_destroy_texture_view(self.device_mut(), handle);
    }

    // ─── Samplers ──────────────────────────────────────────────────────────────
    pub fn create_sampler(&mut self, desc: &SamplerDesc) -> Result<SamplerHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_sampler(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_sampler(&mut self, handle: SamplerHandle) {
        sys::rhi_destroy_sampler(self.device_mut(), handle);
    }

    // ─── Shader modules ────────────────────────────────────────────────────────
    pub fn create_shader_module(&mut self, desc: &ShaderModuleDesc) -> Result<ShaderModuleHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_shader_module(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_shader_module(&mut self, handle: ShaderModuleHandle) {
        sys::rhi_destroy_shader_module(self.device_mut(), handle);
    }

    // ─── Bind groups ───────────────────────────────────────────────────────────
    pub fn create_bind_group_layout(&mut self, desc: &BindGroupLayoutDesc) -> Result<BindGroupLayoutHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_bind_group_layout(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_bind_group_layout(&mut self, handle: BindGroupLayoutHandle) {
        sys::rhi_destroy_bind_group_layout(self.device_mut(), handle);
    }

    pub fn create_bind_group(&mut self, desc: &BindGroupDesc) -> Result<BindGroupHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_bind_group(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_bind_group(&mut self, handle: BindGroupHandle) {
        sys::rhi_destroy_bind_group(self.device_mut(), handle);
    }

    // ─── Pipeline layouts / pipelines ──────────────────────────────────────────
    pub fn create_pipeline_layout(&mut self, desc: &PipelineLayoutDesc) -> Result<PipelineLayoutHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_pipeline_layout(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_pipeline_layout(&mut self, handle: PipelineLayoutHandle) {
        sys::rhi_destroy_pipeline_layout(self.device_mut(), handle);
    }

    pub fn create_render_pipeline(&mut self, desc: &RenderPipelineDesc) -> Result<RenderPipelineHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_render_pipeline(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_render_pipeline(&mut self, handle: RenderPipelineHandle) {
        sys::rhi_destroy_render_pipeline(self.device_mut(), handle);
    }

    pub fn create_compute_pipeline(&mut self, desc: &ComputePipelineDesc) -> Result<ComputePipelineHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_compute_pipeline(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_compute_pipeline(&mut self, handle: ComputePipelineHandle) {
        sys::rhi_destroy_compute_pipeline(self.device_mut(), handle);
    }

    // ─── Ray tracing ───────────────────────────────────────────────────────────
    pub fn acceleration_structure_build_sizes(
        &self,
        desc: &AccelerationStructureBuildDesc,
    ) -> Result<AccelerationStructureBuildSizes> {
        let mut status = new_status();
        let sizes = sys::rhi_acceleration_structure_build_sizes(self.device_ref(), desc, &mut status);
        finish(sizes, status)
    }

    pub fn create_acceleration_structure(
        &mut self,
        desc: &AccelerationStructureDesc,
    ) -> Result<AccelerationStructureHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_acceleration_structure(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_acceleration_structure(&mut self, handle: AccelerationStructureHandle) {
        sys::rhi_destroy_acceleration_structure(self.device_mut(), handle);
    }

    pub fn acceleration_structure_device_address(&self, handle: AccelerationStructureHandle) -> Result<u64> {
        let mut status = new_status();
        let address = sys::rhi_acceleration_structure_device_address(self.device_ref(), handle, &mut status);
        finish(address, status)
    }

    /// Storage/scratch sizes for an opacity micromap (requires `Feature::OpacityMicromap`).
    pub fn opacity_micromap_build_sizes(&self, desc: &OpacityMicromapDesc) -> Result<OpacityMicromapBuildSizes> {
        let mut status = new_status();
        let sizes = sys::rhi_opacity_micromap_build_sizes(self.device_ref(), desc, &mut status);
        finish(sizes, status)
    }

    /// Creates an opacity micromap with `size` bytes of backing storage (from
    /// [`Rhi::opacity_micromap_build_sizes`]); fill it with
    /// [`CommandEncoder::build_opacity_micromaps`], then link it onto BLAS triangle geometry via
    /// [`AccelerationStructureTrianglesDesc::opacity_micromap`].
    pub fn create_opacity_micromap(&mut self, desc: &OpacityMicromapDesc, size: u64) -> Result<OpacityMicromapHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_opacity_micromap(self.device_mut(), desc, size, &mut status);
        finish(handle, status)
    }

    pub fn destroy_opacity_micromap(&mut self, handle: OpacityMicromapHandle) {
        sys::rhi_destroy_opacity_micromap(self.device_mut(), handle);
    }

    pub fn create_ray_tracing_pipeline(&mut self, desc: &RayTracingPipelineDesc) -> Result<RayTracingPipelineHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_ray_tracing_pipeline(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_ray_tracing_pipeline(&mut self, handle: RayTracingPipelineHandle) {
        sys::rhi_destroy_ray_tracing_pipeline(self.device_mut(), handle);
    }

    /// Fills `dst` with `group_count` shader-group handles starting at `first_group`, ready to be
    /// laid out into a shader binding table buffer per `RayTracingPipelineDesc::groups`' order.
    pub fn write_ray_tracing_shader_group_handles(
        &mut self,
        pipeline: RayTracingPipelineHandle,
        first_group: u32,
        group_count: u32,
        dst: &mut [u8],
    ) -> Result<()> {
        let mut status = new_status();
        sys::rhi_write_ray_tracing_shader_group_handles(
            self.device_mut(),
            pipeline,
            first_group,
            group_count,
            dst,
            &mut status,
        );
        finish_unit(status)
    }

    // ─── Command encoding ──────────────────────────────────────────────────────
    pub fn create_command_encoder(&mut self, desc: &CommandEncoderDesc) -> Result<CommandEncoder> {
        let mut status = new_status();
        let inner = sys::rhi_create_command_encoder(self.device_mut(), desc, &mut status);
        finish(CommandEncoder { inner }, status)
    }

    pub fn submit(&mut self, command_buffers: &[CommandBufferHandle]) -> Result<()> {
        let mut status = new_status();
        sys::rhi_submit(self.device_mut(), command_buffers, &mut status);
        finish_unit(status)
    }

    /// Blocks until all submitted GPU work has completed — the blunt instrument. Prefer
    /// [`Rhi::submit_with`] plus a fence ([`Rhi::wait_fences`]) or timeline semaphore
    /// ([`Rhi::wait_semaphore`]) when only one submission's completion matters.
    pub fn wait_idle(&mut self) {
        sys::rhi_wait_idle(self.device_mut());
    }

    /// Full-control submission: target queue lane, semaphore waits/signals, and an optional
    /// fence signaled when the GPU finishes. [`Rhi::submit`] is this with every option defaulted
    /// (graphics queue 0, no waits/signals/fence).
    pub fn submit_with(&mut self, desc: &SubmitDesc) -> Result<()> {
        let mut status = new_status();
        sys::rhi_submit_with(self.device_mut(), desc, &mut status);
        finish_unit(status)
    }

    // ─── Synchronization ───────────────────────────────────────────────────────
    /// Creates a timeline semaphore (a GPU/CPU-shared monotonically increasing `u64` counter).
    pub fn create_semaphore(&mut self, desc: &SemaphoreDesc) -> Result<SemaphoreHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_semaphore(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_semaphore(&mut self, handle: SemaphoreHandle) {
        sys::rhi_destroy_semaphore(self.device_mut(), handle);
    }

    /// The semaphore's current counter value.
    pub fn semaphore_value(&self, handle: SemaphoreHandle) -> Result<u64> {
        let mut status = new_status();
        let value = sys::rhi_semaphore_value(self.device_ref(), handle, &mut status);
        finish(value, status)
    }

    /// Blocks until the semaphore reaches `value`, or `timeout_ns` elapses ([`WAIT_FOREVER`] never
    /// times out; a timeout reports [`RhiErrorCode::NotReady`]).
    pub fn wait_semaphore(&mut self, handle: SemaphoreHandle, value: u64, timeout_ns: u64) -> Result<()> {
        let mut status = new_status();
        sys::rhi_wait_semaphore(self.device_mut(), handle, value, timeout_ns, &mut status);
        finish_unit(status)
    }

    /// Signals the semaphore to `value` from the CPU.
    pub fn signal_semaphore(&mut self, handle: SemaphoreHandle, value: u64) -> Result<()> {
        let mut status = new_status();
        sys::rhi_signal_semaphore(self.device_mut(), handle, value, &mut status);
        finish_unit(status)
    }

    pub fn create_fence(&mut self, desc: &FenceDesc) -> Result<FenceHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_fence(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_fence(&mut self, handle: FenceHandle) {
        sys::rhi_destroy_fence(self.device_mut(), handle);
    }

    /// Blocks until all (`wait_all`) or any of `fences` are signaled, or `timeout_ns` elapses.
    /// `Ok(true)` means the condition was met; `Ok(false)` means it timed out.
    pub fn wait_fences(&mut self, fences: &[FenceHandle], wait_all: bool, timeout_ns: u64) -> Result<bool> {
        let mut status = new_status();
        let met = sys::rhi_wait_fences(self.device_mut(), fences, wait_all, timeout_ns, &mut status);
        finish(met, status)
    }

    /// Returns `fences` to the unsignaled state so they can be reused for another submission.
    pub fn reset_fences(&mut self, fences: &[FenceHandle]) -> Result<()> {
        let mut status = new_status();
        sys::rhi_reset_fences(self.device_mut(), fences, &mut status);
        finish_unit(status)
    }

    // ─── Render bundles ────────────────────────────────────────────────────────
    /// Starts recording a render bundle: a reusable draw sequence replayed inside any render
    /// pass whose attachments match `desc`, via [`RenderPass::execute_bundles`]. Call
    /// [`RenderBundleEncoder::finish`] to get the replayable [`RenderBundleHandle`].
    pub fn create_render_bundle_encoder(&mut self, desc: &RenderBundleDesc) -> Result<RenderBundleEncoder> {
        let mut status = new_status();
        let inner = sys::rhi_create_render_bundle_encoder(self.device_mut(), desc, &mut status);
        finish(RenderBundleEncoder { inner }, status)
    }

    pub fn destroy_render_bundle(&mut self, handle: RenderBundleHandle) {
        sys::rhi_destroy_render_bundle(self.device_mut(), handle);
    }

    // ─── Buffer mapping ────────────────────────────────────────────────────────
    /// Maps `buffer` for direct CPU access. The returned [`MappedBuffer`] borrows this `Rhi`
    /// mutably and unmaps automatically when dropped — mirrors the [`CommandEncoder`]/[`RenderPass`]
    /// borrow-scoped-RAII pattern used elsewhere in this module.
    pub fn map_buffer(&mut self, buffer: BufferHandle) -> Result<MappedBuffer<'_>> {
        let mut status = new_status();
        let mut len: u64 = 0;
        let ptr = sys::rhi_map_buffer(self.device_mut(), buffer, &mut len, &mut status);
        if !status.ok {
            return Err(RhiError { code: status.code, message: status.message });
        }
        Ok(MappedBuffer { device: self.device, buffer, ptr, len: len as usize, _scope: PhantomData })
    }

    // ─── Query sets ────────────────────────────────────────────────────────────
    pub fn create_query_set(&mut self, desc: &QuerySetDesc) -> Result<QuerySetHandle> {
        let mut status = new_status();
        let handle = sys::rhi_create_query_set(self.device_mut(), desc, &mut status);
        finish(handle, status)
    }

    pub fn destroy_query_set(&mut self, handle: QuerySetHandle) {
        sys::rhi_destroy_query_set(self.device_mut(), handle);
    }

    /// Reads back `count` query results starting at `first` into `dst` (each result `stride`
    /// bytes apart).
    pub fn get_query_set_results(
        &mut self,
        query_set: QuerySetHandle,
        first: u32,
        count: u32,
        dst: &mut [u8],
        stride: u64,
        flags: QueryResultFlags,
    ) -> Result<()> {
        let mut status = new_status();
        sys::rhi_get_query_set_results(self.device_mut(), query_set, first, count, dst, stride, flags.bits(), &mut status);
        finish_unit(status)
    }

    /// Resets `count` queries starting at `first` immediately (outside any command encoder); see
    /// [`CommandEncoder::reset_query_set`] for the recorded form.
    pub fn reset_query_set(&mut self, query_set: QuerySetHandle, first: u32, count: u32) {
        sys::rhi_reset_query_set(self.device_mut(), query_set, first, count);
    }
}

/// A CPU-mapped view into a buffer's memory; borrows the [`Rhi`] it came from and unmaps on drop.
pub struct MappedBuffer<'a> {
    device: *mut sys::RhiDevice,
    buffer: BufferHandle,
    ptr: *mut u8,
    len: usize,
    _scope: PhantomData<&'a mut ()>,
}

impl core::ops::Deref for MappedBuffer<'_> {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        // SAFETY: `ptr`/`len` come from a successful `rhi_map_buffer` call and stay valid until
        // this `MappedBuffer` is dropped (which unmaps).
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl core::ops::DerefMut for MappedBuffer<'_> {
    fn deref_mut(&mut self) -> &mut [u8] {
        // SAFETY: see `Deref::deref`.
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

impl Drop for MappedBuffer<'_> {
    fn drop(&mut self) {
        // SAFETY: `device` is non-null and outlives this borrow per `Rhi::map_buffer`'s invariant.
        let device = unsafe { Pin::new_unchecked(&mut *self.device) };
        sys::rhi_unmap_buffer(device, self.buffer);
    }
}

/// Every optional-feature name this engine build knows, in `RHI::Feature` order (whether or not
/// the current device supports it).
#[must_use]
pub fn all_feature_names() -> Vec<String> {
    sys::rhi_all_feature_names()
}

/// Packs unpacked TLAS instances (see [`AccelerationStructureInstance`]) into the backend's native
/// 64-byte layout, ready to `Rhi::write_buffer` into an instance buffer for a TLAS build's
/// `AccelerationStructureInstancesDesc`.
#[must_use]
pub fn pack_acceleration_structure_instances(instances: &[AccelerationStructureInstance]) -> Vec<u8> {
    sys::rhi_pack_acceleration_structure_instances(instances)
}

// ─── Command encoder / passes ────────────────────────────────────────────────────

/// An open command buffer being recorded. Dropping it without calling [`CommandEncoder::finish`]
/// discards the recorded work (matches the underlying `RHI::CommandEncoder`'s own lifetime: only
/// `finish()` produces something `Rhi::submit` can use).
pub struct CommandEncoder {
    inner: UniquePtr<sys::CommandEncoder>,
}

impl CommandEncoder {
    fn pin(&mut self) -> Pin<&mut sys::CommandEncoder> {
        self.inner.pin_mut()
    }

    /// Opens a render pass. The returned [`RenderPass`] borrows this encoder mutably, so the
    /// borrow checker refuses any other encoder call until the pass is dropped (which ends it).
    pub fn begin_render_pass(&mut self, desc: &RenderPassDesc) -> Result<RenderPass<'_>> {
        let mut status = new_status();
        let inner = sys::ce_begin_render_pass(self.pin(), desc, &mut status);
        finish(RenderPass { inner, _scope: PhantomData }, status)
    }

    /// Opens a compute pass; see [`begin_render_pass`](Self::begin_render_pass) for the borrow
    /// discipline.
    pub fn begin_compute_pass(&mut self, label: &str) -> Result<ComputePass<'_>> {
        let mut status = new_status();
        let inner = sys::ce_begin_compute_pass(self.pin(), label, &mut status);
        finish(ComputePass { inner, _scope: PhantomData }, status)
    }

    pub fn copy_buffer_to_buffer(&mut self, src: BufferHandle, dst: BufferHandle, region: &BufferCopy) {
        sys::ce_copy_buffer_to_buffer(self.pin(), src, dst, region);
    }

    pub fn copy_buffer_to_texture(&mut self, src: BufferHandle, dst: TextureHandle, region: &BufferTextureCopy) {
        sys::ce_copy_buffer_to_texture(self.pin(), src, dst, region);
    }

    pub fn copy_texture_to_buffer(&mut self, src: TextureHandle, dst: BufferHandle, region: &BufferTextureCopy) {
        sys::ce_copy_texture_to_buffer(self.pin(), src, dst, region);
    }

    /// Explicit resource-state transitions. `stage`/`access` fields on the barrier structs are
    /// [`PipelineStage`]/[`AccessFlags`] bitmasks (`.bits()` into the raw `u64` the bridge expects).
    pub fn barrier(&mut self, global: &[GlobalBarrier], buffers: &[BufferBarrier], textures: &[TextureBarrier]) {
        sys::ce_barrier(self.pin(), global, buffers, textures);
    }

    pub fn fill_buffer(&mut self, buffer: BufferHandle, offset: u64, size: u64, value: u32) {
        sys::ce_fill_buffer(self.pin(), buffer, offset, size, value);
    }

    pub fn update_buffer(&mut self, buffer: BufferHandle, offset: u64, data: &[u8]) {
        sys::ce_update_buffer(self.pin(), buffer, offset, data);
    }

    pub fn blit_texture(&mut self, src: TextureHandle, dst: TextureHandle, region: &TextureBlit, filter: Filter) {
        sys::ce_blit_texture(self.pin(), src, dst, region, filter);
    }

    pub fn clear_color_texture(&mut self, texture: TextureHandle, color: &ClearColor, range: &TextureSubresourceRange) {
        sys::ce_clear_color_texture(self.pin(), texture, color, range);
    }

    pub fn clear_depth_stencil_texture(
        &mut self,
        texture: TextureHandle,
        value: &ClearDepthStencilValue,
        range: &TextureSubresourceRange,
    ) {
        sys::ce_clear_depth_stencil_texture(self.pin(), texture, value, range);
    }

    pub fn build_acceleration_structures(&mut self, builds: &[AccelerationStructureBuildDesc]) {
        sys::ce_build_acceleration_structures(self.pin(), builds);
    }

    /// Builds opacity micromaps (requires `Feature::OpacityMicromap`).
    pub fn build_opacity_micromaps(&mut self, builds: &[OpacityMicromapBuildDesc]) {
        sys::ce_build_opacity_micromaps(self.pin(), builds);
    }

    /// Copies/compacts/(de)serializes `src` into `dst`; see [`AccelerationStructureCopyMode`] for
    /// what each mode requires of `dst` (e.g. `Compact` needs `dst` already created at the
    /// compacted size — this binding does not query that size for you).
    pub fn copy_acceleration_structure(
        &mut self,
        src: AccelerationStructureHandle,
        dst: AccelerationStructureHandle,
        mode: AccelerationStructureCopyMode,
    ) {
        sys::ce_copy_acceleration_structure(self.pin(), src, dst, mode);
    }

    pub fn set_ray_tracing_pipeline(&mut self, pipeline: RayTracingPipelineHandle) {
        sys::ce_set_ray_tracing_pipeline(self.pin(), pipeline);
    }

    pub fn trace_rays(&mut self, desc: &TraceRaysDesc) {
        sys::ce_trace_rays(self.pin(), desc);
    }

    /// Records a reset of `count` queries starting at `first`; see [`Rhi::reset_query_set`] for
    /// the immediate (outside-an-encoder) form.
    pub fn reset_query_set(&mut self, query_set: QuerySetHandle, first: u32, count: u32) {
        sys::ce_reset_query_set(self.pin(), query_set, first, count);
    }

    /// Writes a GPU timestamp into `query_set[index]` once `stage` completes. `stage` should be a
    /// single [`PipelineStage`] bit.
    pub fn write_timestamp(&mut self, stage: PipelineStage, query_set: QuerySetHandle, index: u32) {
        sys::ce_write_timestamp(self.pin(), stage.bits(), query_set, index);
    }

    pub fn begin_pipeline_statistics_query(&mut self, query_set: QuerySetHandle, index: u32) {
        sys::ce_begin_pipeline_statistics_query(self.pin(), query_set, index);
    }

    pub fn end_pipeline_statistics_query(&mut self) {
        sys::ce_end_pipeline_statistics_query(self.pin());
    }

    /// Records a copy of `count` query results starting at `first` into `dst` at `dst_offset`
    /// (each result `stride` bytes apart) — the recorded counterpart of
    /// [`Rhi::get_query_set_results`], useful when the readback buffer is `HostReadback` memory
    /// mapped later rather than read back synchronously.
    // Every parameter here is independently meaningful (no natural sub-group to bundle into a
    // descriptor struct without just renaming the problem), and it mirrors `RHI::CommandEncoder::
    // resolve_query_set`'s own arity 1:1 — same tradeoff as the RHI descriptor structs elsewhere
    // in this file.
    #[allow(clippy::too_many_arguments)]
    pub fn resolve_query_set(
        &mut self,
        query_set: QuerySetHandle,
        first: u32,
        count: u32,
        dst: BufferHandle,
        dst_offset: u64,
        stride: u64,
        flags: QueryResultFlags,
    ) {
        sys::ce_resolve_query_set(self.pin(), query_set, first, count, dst, dst_offset, stride, flags.bits());
    }

    /// Pushes a labeled debug group visible in GPU-capture tools (RenderDoc, Nsight, PIX); must be
    /// balanced by [`CommandEncoder::pop_debug_group`].
    pub fn push_debug_group(&mut self, label: &str) {
        sys::ce_push_debug_group(self.pin(), label);
    }

    pub fn pop_debug_group(&mut self) {
        sys::ce_pop_debug_group(self.pin());
    }

    /// Ends recording and returns the command buffer, ready for [`Rhi::submit`]. Consumes `self`:
    /// there is nothing meaningful left to do with an encoder after this.
    pub fn finish(mut self) -> Result<CommandBufferHandle> {
        let mut status = new_status();
        let handle = sys::ce_finish(self.pin(), &mut status);
        finish(handle, status)
    }
}

/// An open render pass; borrows its [`CommandEncoder`] for as long as it's alive, and ends the
/// pass automatically on drop.
pub struct RenderPass<'a> {
    inner: UniquePtr<sys::RenderPassEncoder>,
    _scope: PhantomData<&'a mut CommandEncoder>,
}

impl RenderPass<'_> {
    fn pin(&mut self) -> Pin<&mut sys::RenderPassEncoder> {
        self.inner.pin_mut()
    }

    pub fn set_pipeline(&mut self, pipeline: RenderPipelineHandle) {
        sys::rp_set_pipeline(self.pin(), pipeline);
    }

    pub fn set_bind_group(&mut self, index: u32, bind_group: BindGroupHandle, dynamic_offsets: &[u32]) {
        sys::rp_set_bind_group(self.pin(), index, bind_group, dynamic_offsets);
    }

    pub fn set_vertex_buffer(&mut self, slot: u32, buffer: BufferHandle, offset: u64) {
        sys::rp_set_vertex_buffer(self.pin(), slot, buffer, offset);
    }

    pub fn set_index_buffer(&mut self, buffer: BufferHandle, format: IndexFormat, offset: u64) {
        sys::rp_set_index_buffer(self.pin(), buffer, format, offset);
    }

    pub fn set_push_constants(&mut self, stages: ShaderStage, offset: u32, data: &[u8]) {
        sys::rp_set_push_constants(self.pin(), stages.bits(), offset, data);
    }

    pub fn set_viewport(&mut self, viewport: &Viewport) {
        sys::rp_set_viewport(self.pin(), viewport);
    }

    pub fn set_scissor(&mut self, scissor: &Rect2D) {
        sys::rp_set_scissor(self.pin(), scissor);
    }

    /// Sets the constant color blended against via [`BlendFactor::ConstantColor`]/
    /// `OneMinusConstantColor`.
    pub fn set_blend_constant(&mut self, color: &ClearColor) {
        sys::rp_set_blend_constant(self.pin(), color);
    }

    /// Sets the stencil reference value the bound pipeline's [`DepthStencilState::stencil_front`]/
    /// `stencil_back` compare against.
    pub fn set_stencil_reference(&mut self, reference: u32) {
        sys::rp_set_stencil_reference(self.pin(), reference);
    }

    /// Sets the depth range that passes the depth-bounds test, in `[0, 1]`. Requires the bound
    /// pipeline to have set [`DepthStencilState::depth_bounds_test_enable`] at creation time.
    pub fn set_depth_bounds(&mut self, min_depth: f32, max_depth: f32) {
        sys::rp_set_depth_bounds(self.pin(), min_depth, max_depth);
    }

    /// Sets custom per-sample MSAA positions for subsequent draws. `locations` must have exactly
    /// `samples_per_pixel * grid_width * grid_height` entries. Requires the bound pipeline to have
    /// set [`MultisampleState::sample_locations_enable`] at creation time.
    pub fn set_sample_locations(
        &mut self,
        samples_per_pixel: u32,
        grid_width: u32,
        grid_height: u32,
        locations: &[SampleLocation],
    ) {
        sys::rp_set_sample_locations(self.pin(), samples_per_pixel, grid_width, grid_height, locations);
    }

    /// Sets the base shading rate plus how it combines with a draw's per-primitive rate and a
    /// shading-rate attachment's per-tile rate. Requires `Feature::VariableRateShading`; the
    /// device's tile-size range is [`FeatureProperties::variable_rate_shading`]. Not available on
    /// [`RenderBundleEncoder`].
    pub fn set_shading_rate(
        &mut self,
        rate: ShadingRate,
        primitive_combiner: ShadingRateCombiner,
        attachment_combiner: ShadingRateCombiner,
    ) {
        sys::rp_set_shading_rate(self.pin(), rate, primitive_combiner, attachment_combiner);
    }

    pub fn draw(&mut self, args: &DrawArgs) {
        sys::rp_draw(self.pin(), args);
    }

    pub fn draw_indexed(&mut self, args: &DrawIndexedArgs) {
        sys::rp_draw_indexed(self.pin(), args);
    }

    /// Dispatches task/mesh shader workgroups (needs a mesh-shader pipeline and
    /// `Feature::MeshShader`; limits in [`FeatureProperties::mesh_shader`]).
    pub fn draw_mesh_tasks(&mut self, args: &DrawMeshTasksArgs) {
        sys::rp_draw_mesh_tasks(self.pin(), args);
    }

    pub fn draw_mesh_tasks_indirect(&mut self, indirect_buffer: BufferHandle, offset: u64) {
        sys::rp_draw_mesh_tasks_indirect(self.pin(), indirect_buffer, offset);
    }

    pub fn draw_mesh_tasks_indirect_count(
        &mut self,
        indirect_buffer: BufferHandle,
        indirect_offset: u64,
        count_buffer: BufferHandle,
        count_offset: u64,
        max_draws: u32,
        stride: u32,
    ) {
        sys::rp_draw_mesh_tasks_indirect_count(
            self.pin(),
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_draws,
            stride,
        );
    }

    /// Replays previously recorded render bundles (see [`Rhi::create_render_bundle_encoder`])
    /// inside this pass. Their [`RenderBundleDesc`] formats must match this pass's attachments.
    pub fn execute_bundles(&mut self, bundles: &[RenderBundleHandle]) {
        sys::rp_execute_bundles(self.pin(), bundles);
    }

    /// Draws using `DrawArgs` read from `indirect_buffer` at `offset` (GPU-driven draw count).
    pub fn draw_indirect(&mut self, indirect_buffer: BufferHandle, offset: u64) {
        sys::rp_draw_indirect(self.pin(), indirect_buffer, offset);
    }

    pub fn draw_indexed_indirect(&mut self, indirect_buffer: BufferHandle, offset: u64) {
        sys::rp_draw_indexed_indirect(self.pin(), indirect_buffer, offset);
    }

    /// Issues `draw_count` draws read from consecutive `DrawArgs`-shaped records in
    /// `indirect_buffer` (`stride` bytes apart, starting at `offset`) in one call. Unlike
    /// [`RenderPass::draw_indirect_count`], `draw_count` is a CPU-known constant baked into the
    /// command itself rather than read from a separate count buffer, so it needs no
    /// GPU-driven-count feature — only `Feature::MultiDrawIndirect`.
    pub fn draw_indirect_multi(&mut self, indirect_buffer: BufferHandle, offset: u64, draw_count: u32, stride: u32) {
        sys::rp_draw_indirect_multi(self.pin(), indirect_buffer, offset, draw_count, stride);
    }

    /// Same as [`RenderPass::draw_indirect_multi`], reading `DrawIndexedArgs`-shaped records
    /// instead.
    pub fn draw_indexed_indirect_multi(&mut self, indirect_buffer: BufferHandle, offset: u64, draw_count: u32, stride: u32) {
        sys::rp_draw_indexed_indirect_multi(self.pin(), indirect_buffer, offset, draw_count, stride);
    }

    /// Multi-draw-indirect with the actual draw count itself read from `count_buffer` (capped at
    /// `max_draws`); requires the backend's `multi_draw_indirect_count`-equivalent feature.
    pub fn draw_indirect_count(
        &mut self,
        indirect_buffer: BufferHandle,
        indirect_offset: u64,
        count_buffer: BufferHandle,
        count_offset: u64,
        max_draws: u32,
        stride: u32,
    ) {
        sys::rp_draw_indirect_count(self.pin(), indirect_buffer, indirect_offset, count_buffer, count_offset, max_draws, stride);
    }

    pub fn draw_indexed_indirect_count(
        &mut self,
        indirect_buffer: BufferHandle,
        indirect_offset: u64,
        count_buffer: BufferHandle,
        count_offset: u64,
        max_draws: u32,
        stride: u32,
    ) {
        sys::rp_draw_indexed_indirect_count(
            self.pin(),
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_draws,
            stride,
        );
    }

    /// Begins an occlusion query at `query_set[index]`; must be matched by
    /// [`RenderPass::end_occlusion_query`] before the pass ends.
    pub fn begin_occlusion_query(&mut self, query_set: QuerySetHandle, index: u32) {
        sys::rp_begin_occlusion_query(self.pin(), query_set, index);
    }

    pub fn end_occlusion_query(&mut self) {
        sys::rp_end_occlusion_query(self.pin());
    }
}

impl Drop for RenderPass<'_> {
    fn drop(&mut self) {
        sys::rp_end(self.pin());
    }
}

/// Records a render bundle — a reusable draw sequence replayed via
/// [`RenderPass::execute_bundles`]. Created with [`Rhi::create_render_bundle_encoder`]; dropping it
/// without [`RenderBundleEncoder::finish`] discards the recording. Has the same draw/state
/// surface as [`RenderPass`] except `set_shading_rate` (not bundle-legal on D3D12) and occlusion
/// queries.
pub struct RenderBundleEncoder {
    inner: UniquePtr<sys::RenderBundleEncoder>,
}

impl RenderBundleEncoder {
    fn pin(&mut self) -> Pin<&mut sys::RenderBundleEncoder> {
        self.inner.pin_mut()
    }

    pub fn set_pipeline(&mut self, pipeline: RenderPipelineHandle) {
        sys::rb_set_pipeline(self.pin(), pipeline);
    }

    pub fn set_bind_group(&mut self, index: u32, bind_group: BindGroupHandle, dynamic_offsets: &[u32]) {
        sys::rb_set_bind_group(self.pin(), index, bind_group, dynamic_offsets);
    }

    pub fn set_vertex_buffer(&mut self, slot: u32, buffer: BufferHandle, offset: u64) {
        sys::rb_set_vertex_buffer(self.pin(), slot, buffer, offset);
    }

    pub fn set_index_buffer(&mut self, buffer: BufferHandle, format: IndexFormat, offset: u64) {
        sys::rb_set_index_buffer(self.pin(), buffer, format, offset);
    }

    pub fn set_push_constants(&mut self, stages: ShaderStage, offset: u32, data: &[u8]) {
        sys::rb_set_push_constants(self.pin(), stages.bits(), offset, data);
    }

    pub fn set_viewport(&mut self, viewport: &Viewport) {
        sys::rb_set_viewport(self.pin(), viewport);
    }

    pub fn set_scissor(&mut self, scissor: &Rect2D) {
        sys::rb_set_scissor(self.pin(), scissor);
    }

    pub fn set_blend_constant(&mut self, color: &ClearColor) {
        sys::rb_set_blend_constant(self.pin(), color);
    }

    pub fn set_stencil_reference(&mut self, reference: u32) {
        sys::rb_set_stencil_reference(self.pin(), reference);
    }

    pub fn set_depth_bounds(&mut self, min_depth: f32, max_depth: f32) {
        sys::rb_set_depth_bounds(self.pin(), min_depth, max_depth);
    }

    pub fn set_sample_locations(
        &mut self,
        samples_per_pixel: u32,
        grid_width: u32,
        grid_height: u32,
        locations: &[SampleLocation],
    ) {
        sys::rb_set_sample_locations(self.pin(), samples_per_pixel, grid_width, grid_height, locations);
    }

    pub fn draw(&mut self, args: &DrawArgs) {
        sys::rb_draw(self.pin(), args);
    }

    pub fn draw_indexed(&mut self, args: &DrawIndexedArgs) {
        sys::rb_draw_indexed(self.pin(), args);
    }

    pub fn draw_mesh_tasks(&mut self, args: &DrawMeshTasksArgs) {
        sys::rb_draw_mesh_tasks(self.pin(), args);
    }

    pub fn draw_indirect(&mut self, indirect_buffer: BufferHandle, offset: u64) {
        sys::rb_draw_indirect(self.pin(), indirect_buffer, offset);
    }

    pub fn draw_indexed_indirect(&mut self, indirect_buffer: BufferHandle, offset: u64) {
        sys::rb_draw_indexed_indirect(self.pin(), indirect_buffer, offset);
    }

    pub fn draw_indirect_multi(&mut self, indirect_buffer: BufferHandle, offset: u64, draw_count: u32, stride: u32) {
        sys::rb_draw_indirect_multi(self.pin(), indirect_buffer, offset, draw_count, stride);
    }

    pub fn draw_indexed_indirect_multi(&mut self, indirect_buffer: BufferHandle, offset: u64, draw_count: u32, stride: u32) {
        sys::rb_draw_indexed_indirect_multi(self.pin(), indirect_buffer, offset, draw_count, stride);
    }

    pub fn draw_indirect_count(
        &mut self,
        indirect_buffer: BufferHandle,
        indirect_offset: u64,
        count_buffer: BufferHandle,
        count_offset: u64,
        max_draws: u32,
        stride: u32,
    ) {
        sys::rb_draw_indirect_count(self.pin(), indirect_buffer, indirect_offset, count_buffer, count_offset, max_draws, stride);
    }

    pub fn draw_indexed_indirect_count(
        &mut self,
        indirect_buffer: BufferHandle,
        indirect_offset: u64,
        count_buffer: BufferHandle,
        count_offset: u64,
        max_draws: u32,
        stride: u32,
    ) {
        sys::rb_draw_indexed_indirect_count(
            self.pin(),
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_draws,
            stride,
        );
    }

    pub fn draw_mesh_tasks_indirect(&mut self, indirect_buffer: BufferHandle, offset: u64) {
        sys::rb_draw_mesh_tasks_indirect(self.pin(), indirect_buffer, offset);
    }

    pub fn draw_mesh_tasks_indirect_count(
        &mut self,
        indirect_buffer: BufferHandle,
        indirect_offset: u64,
        count_buffer: BufferHandle,
        count_offset: u64,
        max_draws: u32,
        stride: u32,
    ) {
        sys::rb_draw_mesh_tasks_indirect_count(
            self.pin(),
            indirect_buffer,
            indirect_offset,
            count_buffer,
            count_offset,
            max_draws,
            stride,
        );
    }

    /// Ends recording and returns the replayable bundle. Consumes `self`.
    pub fn finish(mut self) -> Result<RenderBundleHandle> {
        let mut status = new_status();
        let handle = sys::rb_finish(self.pin(), &mut status);
        finish(handle, status)
    }
}

/// An open compute pass; see [`RenderPass`] for the borrow discipline.
pub struct ComputePass<'a> {
    inner: UniquePtr<sys::ComputePassEncoder>,
    _scope: PhantomData<&'a mut CommandEncoder>,
}

impl ComputePass<'_> {
    fn pin(&mut self) -> Pin<&mut sys::ComputePassEncoder> {
        self.inner.pin_mut()
    }

    pub fn set_pipeline(&mut self, pipeline: ComputePipelineHandle) {
        sys::cp_set_pipeline(self.pin(), pipeline);
    }

    pub fn set_bind_group(&mut self, index: u32, bind_group: BindGroupHandle, dynamic_offsets: &[u32]) {
        sys::cp_set_bind_group(self.pin(), index, bind_group, dynamic_offsets);
    }

    pub fn set_push_constants(&mut self, stages: ShaderStage, offset: u32, data: &[u8]) {
        sys::cp_set_push_constants(self.pin(), stages.bits(), offset, data);
    }

    pub fn dispatch(&mut self, group_count_x: u32, group_count_y: u32, group_count_z: u32) {
        sys::cp_dispatch(self.pin(), group_count_x, group_count_y, group_count_z);
    }

    /// Dispatches using workgroup counts read from `indirect_buffer` at `offset`.
    pub fn dispatch_indirect(&mut self, indirect_buffer: BufferHandle, offset: u64) {
        sys::cp_dispatch_indirect(self.pin(), indirect_buffer, offset);
    }
}

impl Drop for ComputePass<'_> {
    fn drop(&mut self) {
        sys::cp_end(self.pin());
    }
}
