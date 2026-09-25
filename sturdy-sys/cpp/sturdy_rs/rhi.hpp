// Rust <-> SturdyEngine 5 RHI/ray-tracing shim. See shim.hpp's top comment for the general shape
// of this split: everything here calls RHI::RhiDevice and friends directly (no C ABI), existing
// only where cxx cannot express the engine's types itself (std::expected, std::span, virtual
// encoder interfaces, ...).
//
// This header cannot include the generated "sturdy_rs/rhi_bridge.h" (that generated header itself
// includes this one, via the bridge's own `include!("sturdy_rs/rhi.hpp")`), so every bridge
// struct/enum used here is forward-declared instead; rhi.cpp includes rhi_bridge.h for the full
// definitions before implementing these functions. Enums are forward-declared with the explicit
// `std::uint8_t` underlying type cxx-gen assigns any shared enum whose discriminants all fit in a
// byte (true for every enum below) -- see cxx-gen's `DiscriminantSet` default.
#pragma once

#include <cstddef>
#include <cstdint>

#include <rust/cxx.h>

#include "sturdy_rs/shim.hpp"

#include <RHI/RHI.hpp>

namespace sturdy_rs::rhi {

    using EngineView = sturdy_rs::EngineView;

    /// `RHI::RhiDevice`, only ever handled by pointer/reference: owned by the engine.
    using RhiDevice = SFT::RHI::RhiDevice;
    /// `RHI::CommandEncoder`, owned by whichever `rust::Box`-adjacent `UniquePtr` currently holds it.
    using CommandEncoder = SFT::RHI::CommandEncoder;
    using RenderPassEncoder = SFT::RHI::RenderPassEncoder;
    using ComputePassEncoder = SFT::RHI::ComputePassEncoder;
    using RenderBundleEncoder = SFT::RHI::RenderBundleEncoder;

    // ─── Forward-declared bridge enums (see the file comment for why) ────────────
    enum class RhiErrorCode : ::std::uint8_t;
    enum class MemoryLocation : ::std::uint8_t;
    enum class TextureDimension : ::std::uint8_t;
    enum class TextureViewType : ::std::uint8_t;
    enum class Filter : ::std::uint8_t;
    enum class MipmapMode : ::std::uint8_t;
    enum class AddressMode : ::std::uint8_t;
    enum class BorderColor : ::std::uint8_t;
    enum class CompareOp : ::std::uint8_t;
    enum class Format : ::std::uint8_t;
    enum class SampleCount : ::std::uint8_t;
    enum class IndexFormat : ::std::uint8_t;
    enum class ShaderLanguage : ::std::uint8_t;
    enum class VertexFormat : ::std::uint8_t;
    enum class VertexStepMode : ::std::uint8_t;
    enum class BindingType : ::std::uint8_t;
    enum class StorageTextureAccess : ::std::uint8_t;
    enum class BindGroupLifetime : ::std::uint8_t;
    enum class PrimitiveTopology : ::std::uint8_t;
    enum class PolygonMode : ::std::uint8_t;
    enum class CullMode : ::std::uint8_t;
    enum class FrontFace : ::std::uint8_t;
    enum class BlendFactor : ::std::uint8_t;
    enum class BlendOp : ::std::uint8_t;
    enum class LoadOp : ::std::uint8_t;
    enum class StoreOp : ::std::uint8_t;
    enum class QueueClass : ::std::uint8_t;
    enum class TextureLayout : ::std::uint8_t;
    enum class AccelerationStructureType : ::std::uint8_t;
    enum class AccelerationStructureGeometryType : ::std::uint8_t;
    enum class RayTracingShaderGroupType : ::std::uint8_t;
    enum class QueryType : ::std::uint8_t;
    enum class StencilOp : ::std::uint8_t;
    enum class AccelerationStructureCopyMode : ::std::uint8_t;
    enum class ShadingRate : ::std::uint8_t;
    enum class ShadingRateCombiner : ::std::uint8_t;
    enum class OpacityMicromapFormat : ::std::uint8_t;

    // ─── Forward-declared bridge structs ─────────────────────────────────────────
    struct RhiStatus;
    struct BufferHandle;
    struct TextureHandle;
    struct TextureViewHandle;
    struct SamplerHandle;
    struct ShaderModuleHandle;
    struct BindGroupLayoutHandle;
    struct BindGroupHandle;
    struct PipelineLayoutHandle;
    struct RenderPipelineHandle;
    struct ComputePipelineHandle;
    struct RayTracingPipelineHandle;
    struct AccelerationStructureHandle;
    struct CommandBufferHandle;
    struct QuerySetHandle;
    struct QuerySetDesc;
    struct TextureSubresourceLayers;
    struct TextureBlit;
    struct ClearDepthStencilValue;
    struct BufferDesc;
    struct TextureDesc;
    struct TextureViewDesc;
    struct SamplerDesc;
    struct ShaderModuleDesc;
    struct ShaderEntry;
    struct BindGroupLayoutEntry;
    struct BindGroupLayoutDesc;
    struct BindGroupEntry;
    struct BindGroupDesc;
    struct PushConstantRange;
    struct PipelineLayoutDesc;
    struct VertexAttribute;
    struct VertexBufferLayout;
    struct RasterizationState;
    struct DepthStencilState;
    struct MultisampleState;
    struct BlendComponent;
    struct ColorTargetState;
    struct RenderPipelineDesc;
    struct ComputePipelineDesc;
    struct CommandEncoderDesc;
    struct ClearColor;
    struct ColorAttachment;
    struct DepthStencilAttachment;
    struct Rect2D;
    struct RenderPassDesc;
    struct Viewport;
    struct BufferCopy;
    struct BufferTextureCopy;
    struct DrawArgs;
    struct DrawIndexedArgs;
    struct GlobalBarrier;
    struct BufferBarrier;
    struct TextureSubresourceRange;
    struct TextureBarrier;
    struct AccelerationStructureBuildSizes;
    struct AccelerationStructureDesc;
    struct AccelerationStructureTrianglesDesc;
    struct AccelerationStructureInstancesDesc;
    struct AccelerationStructureGeometryDesc;
    struct AccelerationStructureBuildRangeInfo;
    struct AccelerationStructureBuildDesc;
    struct RayTracingShaderGroupDesc;
    struct RayTracingPipelineDesc;
    struct ShaderBindingTableRegion;
    struct TraceRaysDesc;
    struct AccelerationStructureInstance;
    struct StencilFaceState;
    struct SampleLocation;
    struct RayTracingProperties;
    struct MeshShaderProperties;
    struct VariableRateShadingProperties;
    struct SubgroupProperties;
    struct DescriptorIndexingProperties;
    struct SparseResourceProperties;
    struct FeatureProperties;
    struct RenderBundleHandle;
    struct SemaphoreHandle;
    struct FenceHandle;
    struct QueueLane;
    struct QueueOwnershipTransfer;
    struct DrawMeshTasksArgs;
    struct RenderBundleDesc;
    struct SemaphoreDesc;
    struct FenceDesc;
    struct QueueSemaphoreOp;
    struct SubmitDesc;
    struct OpacityMicromapHandle;
    struct OpacityMicromapUsageCount;
    struct OpacityMicromapDesc;
    struct OpacityMicromapBuildSizes;
    struct OpacityMicromapBuildDesc;

    RhiDevice *engine_rhi_device(EngineView &engine) noexcept;
    struct DeviceLimits;
    struct ExtensionInfo;
    struct QueueInfo;
    DeviceLimits rhi_device_limits(const RhiDevice &device) noexcept;
    rust::Vec<rust::String> rhi_enabled_features(const RhiDevice &device);
    rust::Vec<rust::String> rhi_all_feature_names();
    rust::Vec<ExtensionInfo> rhi_enabled_extensions(const RhiDevice &device);
    rust::Vec<QueueInfo> rhi_queue_infos(const RhiDevice &device);
    struct RendererTextureResources;
    bool renderer_texture_resources(EngineView &engine, std::uint64_t renderer_texture, RendererTextureResources &out) noexcept;

    FeatureProperties rhi_feature_properties(const RhiDevice &device) noexcept;

    // ─── Resources ────────────────────────────────────────────────────────────────
    BufferHandle rhi_create_buffer(RhiDevice &device, const BufferDesc &desc, RhiStatus &status);
    void rhi_destroy_buffer(RhiDevice &device, BufferHandle handle) noexcept;
    void rhi_write_buffer(RhiDevice &device, BufferHandle buffer, std::uint64_t offset,
                          rust::Slice<const std::uint8_t> data, RhiStatus &status);
    std::uint64_t rhi_buffer_device_address(const RhiDevice &device, BufferHandle buffer, RhiStatus &status);
    std::uint8_t *rhi_map_buffer(RhiDevice &device, BufferHandle buffer, std::uint64_t &len, RhiStatus &status);
    void rhi_unmap_buffer(RhiDevice &device, BufferHandle buffer) noexcept;

    TextureHandle rhi_create_texture(RhiDevice &device, const TextureDesc &desc, RhiStatus &status);
    void rhi_destroy_texture(RhiDevice &device, TextureHandle handle) noexcept;
    TextureViewHandle rhi_create_texture_view(RhiDevice &device, const TextureViewDesc &desc, RhiStatus &status);
    void rhi_destroy_texture_view(RhiDevice &device, TextureViewHandle handle) noexcept;

    SamplerHandle rhi_create_sampler(RhiDevice &device, const SamplerDesc &desc, RhiStatus &status);
    void rhi_destroy_sampler(RhiDevice &device, SamplerHandle handle) noexcept;

    ShaderModuleHandle rhi_create_shader_module(RhiDevice &device, const ShaderModuleDesc &desc, RhiStatus &status);
    void rhi_destroy_shader_module(RhiDevice &device, ShaderModuleHandle handle) noexcept;

    BindGroupLayoutHandle rhi_create_bind_group_layout(RhiDevice &device, const BindGroupLayoutDesc &desc, RhiStatus &status);
    void rhi_destroy_bind_group_layout(RhiDevice &device, BindGroupLayoutHandle handle) noexcept;

    BindGroupHandle rhi_create_bind_group(RhiDevice &device, const BindGroupDesc &desc, RhiStatus &status);
    void rhi_destroy_bind_group(RhiDevice &device, BindGroupHandle handle) noexcept;

    PipelineLayoutHandle rhi_create_pipeline_layout(RhiDevice &device, const PipelineLayoutDesc &desc, RhiStatus &status);
    void rhi_destroy_pipeline_layout(RhiDevice &device, PipelineLayoutHandle handle) noexcept;

    RenderPipelineHandle rhi_create_render_pipeline(RhiDevice &device, const RenderPipelineDesc &desc, RhiStatus &status);
    void rhi_destroy_render_pipeline(RhiDevice &device, RenderPipelineHandle handle) noexcept;

    ComputePipelineHandle rhi_create_compute_pipeline(RhiDevice &device, const ComputePipelineDesc &desc, RhiStatus &status);
    void rhi_destroy_compute_pipeline(RhiDevice &device, ComputePipelineHandle handle) noexcept;

    // ─── Query sets ───────────────────────────────────────────────────────────────
    QuerySetHandle rhi_create_query_set(RhiDevice &device, const QuerySetDesc &desc, RhiStatus &status);
    void rhi_destroy_query_set(RhiDevice &device, QuerySetHandle handle) noexcept;
    void rhi_get_query_set_results(RhiDevice &device, QuerySetHandle query_set, std::uint32_t first,
                                   std::uint32_t count, rust::Slice<std::uint8_t> dst, std::uint64_t stride,
                                   std::uint32_t flags, RhiStatus &status);
    void rhi_reset_query_set(RhiDevice &device, QuerySetHandle query_set, std::uint32_t first, std::uint32_t count) noexcept;

    // ─── Ray tracing ──────────────────────────────────────────────────────────────
    AccelerationStructureBuildSizes rhi_acceleration_structure_build_sizes(
        const RhiDevice &device, const AccelerationStructureBuildDesc &desc, RhiStatus &status);
    AccelerationStructureHandle rhi_create_acceleration_structure(
        RhiDevice &device, const AccelerationStructureDesc &desc, RhiStatus &status);
    void rhi_destroy_acceleration_structure(RhiDevice &device, AccelerationStructureHandle handle) noexcept;
    std::uint64_t rhi_acceleration_structure_device_address(
        const RhiDevice &device, AccelerationStructureHandle handle, RhiStatus &status);

    OpacityMicromapBuildSizes rhi_opacity_micromap_build_sizes(const RhiDevice &device, const OpacityMicromapDesc &desc,
                                                               RhiStatus &status);
    OpacityMicromapHandle rhi_create_opacity_micromap(RhiDevice &device, const OpacityMicromapDesc &desc, std::uint64_t size,
                                                      RhiStatus &status);
    void rhi_destroy_opacity_micromap(RhiDevice &device, OpacityMicromapHandle handle) noexcept;

    RayTracingPipelineHandle rhi_create_ray_tracing_pipeline(
        RhiDevice &device, const RayTracingPipelineDesc &desc, RhiStatus &status);
    void rhi_destroy_ray_tracing_pipeline(RhiDevice &device, RayTracingPipelineHandle handle) noexcept;
    void rhi_write_ray_tracing_shader_group_handles(
        RhiDevice &device, RayTracingPipelineHandle pipeline, std::uint32_t first_group,
        std::uint32_t group_count, rust::Slice<std::uint8_t> dst, RhiStatus &status);

    rust::Vec<std::uint8_t> rhi_pack_acceleration_structure_instances(
        rust::Slice<const AccelerationStructureInstance> instances);

    // ─── Command encoding ─────────────────────────────────────────────────────────
    std::unique_ptr<CommandEncoder> rhi_create_command_encoder(RhiDevice &device, const CommandEncoderDesc &desc, RhiStatus &status);
    void rhi_submit(RhiDevice &device, rust::Slice<const CommandBufferHandle> command_buffers, RhiStatus &status);
    void rhi_wait_idle(RhiDevice &device) noexcept;
    void rhi_submit_with(RhiDevice &device, const SubmitDesc &desc, RhiStatus &status);

    // ─── Synchronization ──────────────────────────────────────────────────────────
    SemaphoreHandle rhi_create_semaphore(RhiDevice &device, const SemaphoreDesc &desc, RhiStatus &status);
    void rhi_destroy_semaphore(RhiDevice &device, SemaphoreHandle handle) noexcept;
    std::uint64_t rhi_semaphore_value(const RhiDevice &device, SemaphoreHandle handle, RhiStatus &status);
    void rhi_wait_semaphore(RhiDevice &device, SemaphoreHandle handle, std::uint64_t value, std::uint64_t timeout_ns,
                            RhiStatus &status);
    void rhi_signal_semaphore(RhiDevice &device, SemaphoreHandle handle, std::uint64_t value, RhiStatus &status);
    FenceHandle rhi_create_fence(RhiDevice &device, const FenceDesc &desc, RhiStatus &status);
    void rhi_destroy_fence(RhiDevice &device, FenceHandle handle) noexcept;
    bool rhi_wait_fences(RhiDevice &device, rust::Slice<const FenceHandle> fences, bool wait_all, std::uint64_t timeout_ns,
                         RhiStatus &status);
    void rhi_reset_fences(RhiDevice &device, rust::Slice<const FenceHandle> fences, RhiStatus &status);

    // ─── Render bundles ───────────────────────────────────────────────────────────
    std::unique_ptr<RenderBundleEncoder> rhi_create_render_bundle_encoder(RhiDevice &device, const RenderBundleDesc &desc,
                                                                          RhiStatus &status);
    void rhi_destroy_render_bundle(RhiDevice &device, RenderBundleHandle handle) noexcept;

    void rb_set_pipeline(RenderBundleEncoder &rb, RenderPipelineHandle pipeline) noexcept;
    void rb_set_bind_group(RenderBundleEncoder &rb, std::uint32_t index, BindGroupHandle bind_group,
                           rust::Slice<const std::uint32_t> dynamic_offsets) noexcept;
    void rb_set_vertex_buffer(RenderBundleEncoder &rb, std::uint32_t slot, BufferHandle buffer, std::uint64_t offset) noexcept;
    void rb_set_index_buffer(RenderBundleEncoder &rb, BufferHandle buffer, IndexFormat format, std::uint64_t offset) noexcept;
    void rb_set_push_constants(RenderBundleEncoder &rb, std::uint32_t stages, std::uint32_t offset,
                               rust::Slice<const std::uint8_t> data) noexcept;
    void rb_set_viewport(RenderBundleEncoder &rb, const Viewport &viewport) noexcept;
    void rb_set_scissor(RenderBundleEncoder &rb, const Rect2D &scissor) noexcept;
    void rb_set_blend_constant(RenderBundleEncoder &rb, const ClearColor &color) noexcept;
    void rb_set_stencil_reference(RenderBundleEncoder &rb, std::uint32_t reference) noexcept;
    void rb_set_depth_bounds(RenderBundleEncoder &rb, float min_depth, float max_depth) noexcept;
    void rb_set_sample_locations(RenderBundleEncoder &rb, std::uint32_t samples_per_pixel, std::uint32_t grid_width,
                                 std::uint32_t grid_height, rust::Slice<const SampleLocation> locations) noexcept;
    void rb_draw(RenderBundleEncoder &rb, const DrawArgs &args) noexcept;
    void rb_draw_indexed(RenderBundleEncoder &rb, const DrawIndexedArgs &args) noexcept;
    void rb_draw_mesh_tasks(RenderBundleEncoder &rb, const DrawMeshTasksArgs &args) noexcept;
    void rb_draw_indirect(RenderBundleEncoder &rb, BufferHandle indirect_buffer, std::uint64_t offset) noexcept;
    void rb_draw_indexed_indirect(RenderBundleEncoder &rb, BufferHandle indirect_buffer, std::uint64_t offset) noexcept;
    void rb_draw_indirect_multi(RenderBundleEncoder &rb, BufferHandle indirect_buffer, std::uint64_t offset,
                                std::uint32_t draw_count, std::uint32_t stride) noexcept;
    void rb_draw_indexed_indirect_multi(RenderBundleEncoder &rb, BufferHandle indirect_buffer, std::uint64_t offset,
                                        std::uint32_t draw_count, std::uint32_t stride) noexcept;
    void rb_draw_indirect_count(RenderBundleEncoder &rb, BufferHandle indirect_buffer, std::uint64_t indirect_offset,
                                BufferHandle count_buffer, std::uint64_t count_offset, std::uint32_t max_draws,
                                std::uint32_t stride) noexcept;
    void rb_draw_indexed_indirect_count(RenderBundleEncoder &rb, BufferHandle indirect_buffer, std::uint64_t indirect_offset,
                                        BufferHandle count_buffer, std::uint64_t count_offset, std::uint32_t max_draws,
                                        std::uint32_t stride) noexcept;
    void rb_draw_mesh_tasks_indirect(RenderBundleEncoder &rb, BufferHandle indirect_buffer, std::uint64_t offset) noexcept;
    void rb_draw_mesh_tasks_indirect_count(RenderBundleEncoder &rb, BufferHandle indirect_buffer,
                                           std::uint64_t indirect_offset, BufferHandle count_buffer,
                                           std::uint64_t count_offset, std::uint32_t max_draws,
                                           std::uint32_t stride) noexcept;
    RenderBundleHandle rb_finish(RenderBundleEncoder &rb, RhiStatus &status);

    std::unique_ptr<RenderPassEncoder> ce_begin_render_pass(CommandEncoder &encoder, const RenderPassDesc &desc, RhiStatus &status);
    std::unique_ptr<ComputePassEncoder> ce_begin_compute_pass(CommandEncoder &encoder, rust::Str label, RhiStatus &status);

    void ce_copy_buffer_to_buffer(CommandEncoder &encoder, BufferHandle src, BufferHandle dst, const BufferCopy &region) noexcept;
    void ce_copy_buffer_to_texture(CommandEncoder &encoder, BufferHandle src, TextureHandle dst, const BufferTextureCopy &region) noexcept;
    void ce_copy_texture_to_buffer(CommandEncoder &encoder, TextureHandle src, BufferHandle dst, const BufferTextureCopy &region) noexcept;

    void ce_barrier(CommandEncoder &encoder,
                    rust::Slice<const GlobalBarrier> global_barriers,
                    rust::Slice<const BufferBarrier> buffer_barriers,
                    rust::Slice<const TextureBarrier> texture_barriers) noexcept;

    void ce_fill_buffer(CommandEncoder &encoder, BufferHandle buffer, std::uint64_t offset, std::uint64_t size,
                        std::uint32_t value) noexcept;
    void ce_update_buffer(CommandEncoder &encoder, BufferHandle buffer, std::uint64_t offset,
                          rust::Slice<const std::uint8_t> data) noexcept;
    void ce_blit_texture(CommandEncoder &encoder, TextureHandle src, TextureHandle dst, const TextureBlit &region,
                         Filter filter) noexcept;
    void ce_clear_color_texture(CommandEncoder &encoder, TextureHandle texture, const ClearColor &color,
                                const TextureSubresourceRange &range) noexcept;
    void ce_clear_depth_stencil_texture(CommandEncoder &encoder, TextureHandle texture,
                                        const ClearDepthStencilValue &value,
                                        const TextureSubresourceRange &range) noexcept;

    void ce_build_acceleration_structures(CommandEncoder &encoder, rust::Slice<const AccelerationStructureBuildDesc> builds);
    void ce_build_opacity_micromaps(CommandEncoder &encoder, rust::Slice<const OpacityMicromapBuildDesc> builds);
    void ce_copy_acceleration_structure(CommandEncoder &encoder, AccelerationStructureHandle src,
                                        AccelerationStructureHandle dst, AccelerationStructureCopyMode mode) noexcept;
    void ce_set_ray_tracing_pipeline(CommandEncoder &encoder, RayTracingPipelineHandle pipeline) noexcept;
    void ce_trace_rays(CommandEncoder &encoder, const TraceRaysDesc &desc) noexcept;

    void ce_reset_query_set(CommandEncoder &encoder, QuerySetHandle query_set, std::uint32_t first, std::uint32_t count) noexcept;
    void ce_write_timestamp(CommandEncoder &encoder, std::uint64_t stage, QuerySetHandle query_set, std::uint32_t index) noexcept;
    void ce_begin_pipeline_statistics_query(CommandEncoder &encoder, QuerySetHandle query_set, std::uint32_t index) noexcept;
    void ce_end_pipeline_statistics_query(CommandEncoder &encoder) noexcept;
    void ce_resolve_query_set(CommandEncoder &encoder, QuerySetHandle query_set, std::uint32_t first, std::uint32_t count,
                              BufferHandle dst, std::uint64_t dst_offset, std::uint64_t stride, std::uint32_t flags) noexcept;

    void ce_push_debug_group(CommandEncoder &encoder, rust::Str label) noexcept;
    void ce_pop_debug_group(CommandEncoder &encoder) noexcept;

    CommandBufferHandle ce_finish(CommandEncoder &encoder, RhiStatus &status);

    void rp_set_pipeline(RenderPassEncoder &rp, RenderPipelineHandle pipeline) noexcept;
    void rp_set_bind_group(RenderPassEncoder &rp, std::uint32_t index, BindGroupHandle bind_group,
                           rust::Slice<const std::uint32_t> dynamic_offsets) noexcept;
    void rp_set_vertex_buffer(RenderPassEncoder &rp, std::uint32_t slot, BufferHandle buffer, std::uint64_t offset) noexcept;
    void rp_set_index_buffer(RenderPassEncoder &rp, BufferHandle buffer, IndexFormat format, std::uint64_t offset) noexcept;
    void rp_set_push_constants(RenderPassEncoder &rp, std::uint32_t stages, std::uint32_t offset,
                               rust::Slice<const std::uint8_t> data) noexcept;
    void rp_set_viewport(RenderPassEncoder &rp, const Viewport &viewport) noexcept;
    void rp_set_scissor(RenderPassEncoder &rp, const Rect2D &scissor) noexcept;
    void rp_set_blend_constant(RenderPassEncoder &rp, const ClearColor &color) noexcept;
    void rp_set_stencil_reference(RenderPassEncoder &rp, std::uint32_t reference) noexcept;
    void rp_set_depth_bounds(RenderPassEncoder &rp, float min_depth, float max_depth) noexcept;
    void rp_set_sample_locations(RenderPassEncoder &rp, std::uint32_t samples_per_pixel, std::uint32_t grid_width,
                                 std::uint32_t grid_height, rust::Slice<const SampleLocation> locations) noexcept;
    void rp_set_shading_rate(RenderPassEncoder &rp, ShadingRate rate, ShadingRateCombiner primitive_combiner,
                             ShadingRateCombiner attachment_combiner) noexcept;
    void rp_draw(RenderPassEncoder &rp, const DrawArgs &args) noexcept;
    void rp_draw_indexed(RenderPassEncoder &rp, const DrawIndexedArgs &args) noexcept;
    void rp_draw_mesh_tasks(RenderPassEncoder &rp, const DrawMeshTasksArgs &args) noexcept;
    void rp_draw_mesh_tasks_indirect(RenderPassEncoder &rp, BufferHandle indirect_buffer, std::uint64_t offset) noexcept;
    void rp_draw_mesh_tasks_indirect_count(RenderPassEncoder &rp, BufferHandle indirect_buffer,
                                           std::uint64_t indirect_offset, BufferHandle count_buffer,
                                           std::uint64_t count_offset, std::uint32_t max_draws,
                                           std::uint32_t stride) noexcept;
    void rp_execute_bundles(RenderPassEncoder &rp, rust::Slice<const RenderBundleHandle> bundles) noexcept;
    void rp_draw_indirect(RenderPassEncoder &rp, BufferHandle indirect_buffer, std::uint64_t offset) noexcept;
    void rp_draw_indexed_indirect(RenderPassEncoder &rp, BufferHandle indirect_buffer, std::uint64_t offset) noexcept;
    void rp_draw_indirect_multi(RenderPassEncoder &rp, BufferHandle indirect_buffer, std::uint64_t offset,
                                std::uint32_t draw_count, std::uint32_t stride) noexcept;
    void rp_draw_indexed_indirect_multi(RenderPassEncoder &rp, BufferHandle indirect_buffer, std::uint64_t offset,
                                        std::uint32_t draw_count, std::uint32_t stride) noexcept;
    void rp_draw_indirect_count(RenderPassEncoder &rp, BufferHandle indirect_buffer, std::uint64_t indirect_offset,
                                BufferHandle count_buffer, std::uint64_t count_offset, std::uint32_t max_draws,
                                std::uint32_t stride) noexcept;
    void rp_draw_indexed_indirect_count(RenderPassEncoder &rp, BufferHandle indirect_buffer, std::uint64_t indirect_offset,
                                        BufferHandle count_buffer, std::uint64_t count_offset, std::uint32_t max_draws,
                                        std::uint32_t stride) noexcept;
    void rp_begin_occlusion_query(RenderPassEncoder &rp, QuerySetHandle query_set, std::uint32_t index) noexcept;
    void rp_end_occlusion_query(RenderPassEncoder &rp) noexcept;
    void rp_end(RenderPassEncoder &rp) noexcept;

    void cp_set_pipeline(ComputePassEncoder &cp, ComputePipelineHandle pipeline) noexcept;
    void cp_set_bind_group(ComputePassEncoder &cp, std::uint32_t index, BindGroupHandle bind_group,
                           rust::Slice<const std::uint32_t> dynamic_offsets) noexcept;
    void cp_set_push_constants(ComputePassEncoder &cp, std::uint32_t stages, std::uint32_t offset,
                               rust::Slice<const std::uint8_t> data) noexcept;
    void cp_dispatch(ComputePassEncoder &cp, std::uint32_t group_count_x, std::uint32_t group_count_y,
                     std::uint32_t group_count_z) noexcept;
    void cp_dispatch_indirect(ComputePassEncoder &cp, BufferHandle indirect_buffer, std::uint64_t offset) noexcept;
    void cp_end(ComputePassEncoder &cp) noexcept;

} // namespace sturdy_rs::rhi
