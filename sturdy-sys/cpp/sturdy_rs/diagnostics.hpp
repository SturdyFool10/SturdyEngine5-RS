// Native-GPU-handle interop, extended GPU inventory/capabilities, window diagnostics, log-sink,
// and task-scheduler shim. See `sturdy-sys/src/diagnostics.rs` for the bridge this backs and the
// doc comments describing lifetimes/threading rules of everything declared here.
#pragma once

#include <cstdint>
#include <memory>
#include <utility>

#include <rust/cxx.h>

#include <Async/Scheduler.hpp>

#include "sturdy_rs/shim.hpp"

namespace sturdy_rs::diagnostics {

    // cxx's generated bridge code fully-qualifies `EngineView` as `sturdy_rs::diagnostics::EngineView`
    // (the bridge module's own namespace) even though the Rust side spells it
    // `type EngineView = crate::ffi::EngineView;`; without a local alias that qualified name would
    // not resolve to the root bridge's `sturdy_rs::EngineView` (declared in shim.hpp).
    using EngineView = ::sturdy_rs::EngineView;

    struct NativeVulkanHandles;
    struct NativeD3D12Handles;
    struct NativeWebGpuHandles;
    struct GpuCapabilities;
    struct PhysicalGpuInfo;
    struct WindowInfo;
    struct LogSink;
    struct SchedulerTask;
    enum class LogLevel : std::uint8_t;

    // -- Native GPU handle interop ---------------------------------------------------------------

    bool native_vulkan_available(EngineView &engine) noexcept;
    bool native_vulkan_handles(EngineView &engine, NativeVulkanHandles &out) noexcept;
    bool native_d3d12_available(EngineView &engine) noexcept;
    bool native_d3d12_handles(EngineView &engine, NativeD3D12Handles &out) noexcept;
    bool native_webgpu_available(EngineView &engine) noexcept;
    bool native_webgpu_handles(EngineView &engine, NativeWebGpuHandles &out) noexcept;

    // -- Extended GPU inventory & capabilities ----------------------------------------------------

    bool gpu_capabilities(const EngineView &engine, GpuCapabilities &out) noexcept;
    std::uint32_t gpu_inventory_count(const EngineView &engine) noexcept;
    bool gpu_inventory_get(const EngineView &engine, std::uint32_t index, PhysicalGpuInfo &out);

    // -- Window diagnostics -----------------------------------------------------------------------

    std::uint32_t window_count(const EngineView &engine) noexcept;
    bool window_snapshot(const EngineView &engine, std::uint32_t index, WindowInfo &out) noexcept;
    bool window_primary(const EngineView &engine, WindowInfo &out) noexcept;
    bool window_find(const EngineView &engine, std::uint64_t window_id, WindowInfo &out) noexcept;

    // -- Process-wide log sink --------------------------------------------------------------------

    std::uint64_t log_add_sink(rust::Box<LogSink> sink);
    void log_remove_sink(std::uint64_t id) noexcept;

    // -- Process-wide task scheduler --------------------------------------------------------------

    /// Wraps `SFT::Async::TaskHandle<void>` (a template cxx cannot bind directly) so the scheduler
    /// can hand a spawned task back to Rust as an opaque, owned pointer.
    class TaskHandle {
      public:
        explicit TaskHandle(SFT::Async::TaskHandle<void> handle) noexcept : handle_(std::move(handle)) {}

        [[nodiscard]] bool is_done() const noexcept { return handle_.is_done(); }
        void wait() const noexcept { handle_.wait(); }

      private:
        SFT::Async::TaskHandle<void> handle_;
    };

    std::uint32_t scheduler_worker_count() noexcept;
    bool scheduler_is_running() noexcept;
    std::unique_ptr<TaskHandle> scheduler_spawn(rust::Box<SchedulerTask> task, bool heavy);
    bool task_handle_is_done(const TaskHandle &handle) noexcept;
    void task_handle_wait(const TaskHandle &handle) noexcept;

} // namespace sturdy_rs::diagnostics
