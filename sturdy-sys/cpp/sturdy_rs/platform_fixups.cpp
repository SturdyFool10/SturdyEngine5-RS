// Definitions the engine forgot on non-Windows targets.
//
// Core/Vulkan/Rhi/VulkanRhiBridgeComposition.cpp provides Windows-only implementations plus
// `#else` stubs, but the stub set lacks `resize_composition_swapchain_resources`, which
// VulkanRhiBridgeSwapchain.cpp still references unconditionally (its runtime guard,
// `composition_present_compiled()`, is false here). The engine's own executables only link
// because linker GC drops that path; anything that keeps the swapchain code reachable does not.
//
// Weak on purpose: once the engine grows its own stub, that one silently wins.
#if !defined(_WIN32)

#include <utility>

#include <Core/Vulkan/Rhi/VulkanRhiBridgeComposition.hpp>

namespace SFT::Core::Vulkan {

    __attribute__((weak)) RendererExpected<CompositionSwapchainResources>
    resize_composition_swapchain_resources(VkDevice,
                                           VkPhysicalDevice,
                                           CompositionSwapchainResources &&,
                                           VkFormat,
                                           VkImageUsageFlags,
                                           u32,
                                           u32) {
        return graphics_backend_error(GraphicsBackendErrorCode::Unsupported,
                                      "Composition present is implemented only on Windows.");
    }

} // namespace SFT::Core::Vulkan

#endif
