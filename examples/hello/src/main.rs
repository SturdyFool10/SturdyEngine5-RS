use std::process::ExitCode;

use sturdy::prelude::*;
use sturdy::reflection;
use sturdy::sync::{mpsc, Mutex};
use sturdy::task;
use sturdy::render::{CursorIcon, SurfaceHandle, TextInputArea, WindowEffectKind};
use sturdy::{ScheduleTarget, R, W};

use std::sync::Arc;
use sturdy::sync::RwLock;
use sturdy::task::JoinSet;
use sturdy::time::{self, Duration, Instant};

/// Exercises `sturdy::task`/`sturdy::sync` end to end: a handful of spawned tasks (trivial work,
/// cooperative yielding, a panic, an abort), an mpsc channel between two tasks, and a shared
/// counter behind an async `Mutex` incremented concurrently.
///
/// Called from `GameLogic::init` (see below), where the engine's scheduler is guaranteed to
/// already be running (`sturdy::task::spawn` submits work to it) — it is not backed by its own
/// thread pool. Driven with `sturdy::task::block_on`, which just parks the calling thread (the
/// engine's init-callback thread here) between wakeups while the scheduler's worker threads do the
/// actual polling.
fn demo_async_tasks() {
    task::block_on(async {
        // -- Trivial CPU work -------------------------------------------------------------
        let trivial = task::spawn(async { 2 + 2 });

        // -- Cooperative yielding: yield_now() a couple of times, proving Pending-then-Ready --
        let yielding = task::spawn(async {
            let mut steps = 0;
            task::yield_now().await;
            steps += 1;
            task::yield_now().await;
            steps += 1;
            steps
        });

        // -- A task that deliberately panics -----------------------------------------------
        //
        // Safe to catch here (see `task.rs`'s module doc): a spawned task's poll loop never
        // crosses into C++, unlike the game-logic/UI-draw/log-sink callbacks elsewhere in
        // `sturdy`/`sturdy-sys`, which each guard their own C++ boundary individually with
        // `catch_unwind` + an explicit `abort()` regardless of this workspace's panic strategy
        // (see the `[profile.release]` comment in the workspace `Cargo.toml`).
        let panicking = task::spawn(async {
            #[allow(unreachable_code)]
            if true {
                panic!("boom: deliberate panic to prove JoinError::is_panic()");
            }
            0
        });

        // -- A task aborted before it can complete -------------------------------------------
        let abort_target = task::spawn(async {
            loop {
                task::yield_now().await;
            }
        });
        abort_target.abort();

        eprintln!("[task-demo] trivial = {:?}", trivial.await);

        match yielding.await {
            Ok(steps) => eprintln!("[task-demo] yielding task completed after {steps} yields"),
            Err(e) => eprintln!("[task-demo] yielding task failed unexpectedly: {e}"),
        }

        match panicking.await {
            Ok(_) => eprintln!("[task-demo] panicking task unexpectedly succeeded"),
            Err(e) => {
                eprintln!(
                    "[task-demo] panicking task -> JoinError: is_panic={} is_cancelled={} ({e})",
                    e.is_panic(),
                    e.is_cancelled()
                );
            }
        }

        match abort_target.await {
            Ok(_) => eprintln!("[task-demo] aborted task unexpectedly succeeded"),
            Err(e) => {
                eprintln!(
                    "[task-demo] aborted task -> JoinError: is_panic={} is_cancelled={} ({e})",
                    e.is_panic(),
                    e.is_cancelled()
                );
            }
        }

        // -- mpsc: producer/consumer, confirm ordering ---------------------------------------
        let (tx, mut rx) = mpsc::channel::<i32>(2);
        let producer = task::spawn(async move {
            for value in 1..=5 {
                tx.send(value).await.expect("receiver still alive");
                task::yield_now().await;
            }
        });
        let consumer = task::spawn(async move {
            let mut received = Vec::new();
            while let Some(value) = rx.recv().await {
                received.push(value);
            }
            received
        });
        producer.await.expect("producer task panicked");
        let received = consumer.await.expect("consumer task panicked");
        eprintln!("[task-demo] mpsc received in order: {received:?} (expected [1, 2, 3, 4, 5])");
        assert_eq!(received, vec![1, 2, 3, 4, 5]);

        // -- Async Mutex: two tasks concurrently incrementing a shared counter ----------------
        //
        // No `yield_now()` in the hot loop here: the two tasks already run genuinely
        // concurrently (each spawned task is polled from the engine's own worker pool, so both
        // can be mid-`lock()` on different OS threads at once), which is exactly the real
        // contention this is meant to exercise. Adding a yield per iteration would only force an
        // extra full scheduler round-trip (spawn -> worker wakeup) per increment for no benefit
        // — `yield_now` is already proven separately above.
        const INCREMENTS_PER_TASK: u32 = 1000;
        let counter = std::sync::Arc::new(Mutex::new(0_u32));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let counter = counter.clone();
            handles.push(task::spawn(async move {
                for _ in 0..INCREMENTS_PER_TASK {
                    let mut guard = counter.lock().await;
                    *guard += 1;
                }
            }));
        }
        for handle in handles {
            handle.await.expect("counter task panicked");
        }
        let final_count = *counter.lock().await;
        let expected = INCREMENTS_PER_TASK * 2;
        eprintln!("[task-demo] shared counter after concurrent increments: {final_count} (expected {expected})");
        assert_eq!(final_count, expected);

        eprintln!("[task-demo] all checks passed");
    });
}

/// Exercises the async-API extensions added on top of `sturdy::task`/`sturdy::sync`
/// (`select!` including `biased`/`if guard`/`else`, `JoinSet` including `detach_all`,
/// `sturdy::time::{sleep, timeout, interval}`, the unbounded mpsc channel, and `RwLock`), logged
/// with an `[async-ext]` prefix to keep it separate from `demo_async_tasks`'s own
/// `[task-demo]`-prefixed lines. Same driving pattern as `demo_async_tasks`: called from
/// `GameLogic::init` and driven with `task::block_on`.
fn demo_async_ext() {
    task::block_on(async {
        // -- select!: race two tasks, confirm exactly one side effect happened ----------------
        //
        // `fast` finishes almost immediately (a couple of `yield_now`s); `slow` sleeps for a full
        // second. `select!` must run `fast`'s arm and drop (cancel) `slow`'s branch before it
        // ever gets to increment `slow_ran`, proving the loser's side effect never happened.
        let fast_ran = Arc::new(Mutex::new(false));
        let slow_ran = Arc::new(Mutex::new(false));
        {
            let fast_ran = fast_ran.clone();
            let slow_ran = slow_ran.clone();
            let fast = async {
                task::yield_now().await;
                task::yield_now().await;
                *fast_ran.lock().await = true;
                "fast"
            };
            let slow = async {
                time::sleep(Duration::from_secs(1)).await;
                *slow_ran.lock().await = true;
                "slow"
            };
            let winner = sturdy::select! {
                result = fast => { result }
                result = slow => { result }
            };
            eprintln!("[async-ext] select! winner = {winner:?}");
            assert_eq!(winner, "fast");
        }
        let fast_ran = *fast_ran.lock().await;
        let slow_ran = *slow_ran.lock().await;
        eprintln!("[async-ext] select! side effects: fast_ran={fast_ran} slow_ran={slow_ran} (expected true, false)");
        assert!(fast_ran, "the winning branch's side effect must have happened");
        assert!(!slow_ran, "the losing branch must have been cancelled before its side effect ran");

        // -- select!: `biased;`, a disabled `if guard` branch, and an `else` arm ---------------
        let biased_winner = sturdy::select! {
            biased;
            v = async { 1 } => { v }
            v = async { 2 } => { v }
        };
        eprintln!("[async-ext] select! biased winner = {biased_winner} (expected 1, first-listed wins a tie)");
        assert_eq!(biased_winner, 1);

        let guard_winner = sturdy::select! {
            v = async { "enabled" }, if true => { v }
            v = async { "disabled" }, if false => { v }
        };
        eprintln!("[async-ext] select! guard winner = {guard_winner} (expected \"enabled\")");
        assert_eq!(guard_winner, "enabled");

        let else_winner = sturdy::select! {
            _v = std::future::pending::<()>(), if false => { "never" }
            else => { "else" }
        };
        eprintln!("[async-ext] select! all-disabled -> else = {else_winner} (expected \"else\")");
        assert_eq!(else_winner, "else");

        // -- JoinSet: several tasks (including one that panics), drained in completion order ---
        let mut set: JoinSet<u32> = JoinSet::new();
        set.spawn(async {
            task::yield_now().await;
            task::yield_now().await;
            task::yield_now().await;
            1
        });
        set.spawn(async { 2 });
        set.spawn(async {
            task::yield_now().await;
            panic!("boom: deliberate JoinSet panic to prove join_next surfaces it as Err");
        });
        set.spawn(async {
            task::yield_now().await;
            4
        });
        assert_eq!(set.len(), 4);

        let mut ok_sum = 0_u32;
        let mut ok_count = 0;
        let mut panic_count = 0;
        while let Some(result) = set.join_next().await {
            match result {
                Ok(value) => {
                    eprintln!("[async-ext] JoinSet task completed: {value}");
                    ok_sum += value;
                    ok_count += 1;
                }
                Err(e) => {
                    eprintln!("[async-ext] JoinSet task failed: is_panic={} ({e})", e.is_panic());
                    assert!(e.is_panic());
                    panic_count += 1;
                }
            }
        }
        assert!(set.is_empty());
        eprintln!(
            "[async-ext] JoinSet drained: {ok_count} ok (sum={ok_sum}), {panic_count} panicked (expected 3 ok summing to 7, 1 panicked)"
        );
        assert_eq!(ok_count, 3);
        assert_eq!(ok_sum, 7);
        assert_eq!(panic_count, 1);

        // -- JoinSet::detach_all: forgotten tasks keep running but stop being tracked ----------
        let mut detached: JoinSet<()> = JoinSet::new();
        for _ in 0..3 {
            detached.spawn(async {
                task::yield_now().await;
            });
        }
        assert_eq!(detached.len(), 3);
        detached.detach_all();
        eprintln!("[async-ext] JoinSet::detach_all: len after detach = {} (expected 0)", detached.len());
        assert_eq!(detached.len(), 0);
        assert!(detached.is_empty());
        assert!(detached.join_next().await.is_none(), "a detached set has nothing left to join");

        // -- time::interval: first tick fires immediately, later ticks land on schedule -------
        let period = Duration::from_millis(50);
        let mut ticker = time::interval(period);
        let before = Instant::now();
        ticker.tick().await;
        let first_tick_elapsed = before.elapsed();
        eprintln!("[async-ext] interval first tick elapsed = {first_tick_elapsed:?} (expected ~immediate)");
        assert!(first_tick_elapsed < period, "the first interval tick must not wait a full period");
        ticker.tick().await;
        let two_ticks_elapsed = before.elapsed();
        eprintln!("[async-ext] interval second tick elapsed since start = {two_ticks_elapsed:?} (expected >= {period:?})");
        assert!(two_ticks_elapsed >= period, "the second interval tick must land roughly one period after the first");

        // -- time::sleep: prove real time actually elapsed -------------------------------------
        let sleep_for = Duration::from_millis(200);
        let before = Instant::now();
        time::sleep(sleep_for).await;
        let measured = before.elapsed();
        eprintln!("[async-ext] sleep({sleep_for:?}) measured elapsed = {measured:?}");
        assert!(measured >= sleep_for, "sleep must not return before its deadline");

        // -- time::timeout: a future that never finishes, racing a timer that does ------------
        let before = Instant::now();
        let result = time::timeout(Duration::from_millis(150), std::future::pending::<()>()).await;
        let measured = before.elapsed();
        eprintln!("[async-ext] timeout(150ms, pending()) -> {:?}, measured elapsed = {measured:?}", result.is_err());
        assert!(result.is_err(), "timeout must elapse against a future that never completes");
        assert!(measured >= Duration::from_millis(150));

        // A timeout racing a future that finishes comfortably in time must come back `Ok`.
        let quick = time::timeout(Duration::from_secs(1), async {
            task::yield_now().await;
            42
        })
        .await;
        eprintln!("[async-ext] timeout(1s, quick task) -> {quick:?}");
        assert_eq!(quick.ok(), Some(42));

        // -- unbounded mpsc: producer/consumer, confirm ordering, send never pends ------------
        let (tx, mut rx) = mpsc::unbounded_channel::<i32>();
        // Unbounded `send` is synchronous: fire off every value before the consumer even starts,
        // something the bounded channel (capacity 2) in `demo_async_tasks` cannot do.
        for value in 1..=5 {
            tx.send(value).expect("receiver still alive");
        }
        drop(tx);
        let consumer = task::spawn(async move {
            let mut received = Vec::new();
            while let Some(value) = rx.recv().await {
                received.push(value);
            }
            received
        });
        let received = consumer.await.expect("unbounded consumer task panicked");
        eprintln!("[async-ext] unbounded mpsc received in order: {received:?} (expected [1, 2, 3, 4, 5])");
        assert_eq!(received, vec![1, 2, 3, 4, 5]);

        // -- RwLock: concurrent readers + a writer, readers see consistent data ---------------
        let rw = Arc::new(RwLock::new((0_i64, 0_i64))); // invariant: .0 == .1 always
        let mut readers = JoinSet::new();
        for _ in 0..4 {
            let rw = rw.clone();
            readers.spawn(async move {
                let mut consistent = true;
                for _ in 0..50 {
                    let guard = rw.read().await;
                    if guard.0 != guard.1 {
                        consistent = false;
                    }
                    drop(guard);
                    task::yield_now().await;
                }
                consistent
            });
        }
        let writer = {
            let rw = rw.clone();
            task::spawn(async move {
                for i in 1..=20_i64 {
                    let mut guard = rw.write().await;
                    // Briefly break the invariant mid-write, on purpose: if a reader ever
                    // observed this, `consistent` above would go false, since `.0` and `.1` are
                    // set in two separate statements while holding the exclusive write guard.
                    guard.0 = i;
                    task::yield_now().await;
                    guard.1 = i;
                    drop(guard);
                    task::yield_now().await;
                }
            })
        };
        writer.await.expect("RwLock writer task panicked");
        let mut all_consistent = true;
        while let Some(result) = readers.join_next().await {
            all_consistent &= result.expect("RwLock reader task panicked");
        }
        let final_value = *rw.read().await;
        eprintln!(
            "[async-ext] RwLock: readers always saw a consistent pair = {all_consistent}, final value visible to a reader after the writer = {final_value:?} (expected true, (20, 20))"
        );
        assert!(all_consistent, "no reader should ever observe a torn write");
        assert_eq!(final_value, (20, 20));

        eprintln!("[async-ext] all checks passed");
    });
}

/// Exercises the RHI extensions added on top of the base bridge (buffer mapping, fill/update
/// buffer, blit, texture clears, query sets, debug groups, indirect dispatch) against the real
/// `RhiDevice`. Logged with an `[rhi-ext]` prefix. Must run once the renderer is actually up (a
/// real frame callback, not `init`), since `Engine::rhi` needs a live `RhiDevice` — see
/// `Engine::gpu`'s doc comment ("once the renderer is initialized").
fn demonstrate_rhi_ext(engine: &mut Engine<'_>) {
    use sturdy::rhi::{
        BufferDesc, BufferUsage, ClearColor, ClearDepthStencilValue, CommandEncoderDesc, Format,
        HandleExt, MemoryLocation, PipelineStage, QueryResultFlags, QuerySetDesc, QueryType,
        QueueClass, SampleCount, TextureDesc, TextureDimension, TextureSubresourceRange,
        TextureUsage,
    };

    let mut rhi = engine.rhi();

    // -- Device introspection: limits, features, extensions, queues ------------------------------
    {
        let limits = rhi.limits();
        let features = rhi.enabled_features();
        let all_features = sturdy::rhi::all_feature_names();
        let extensions = rhi.enabled_extensions();
        let queues = rhi.queue_infos();
        eprintln!(
            "[rhi-ext] limits: max_tex2d={} max_bind_groups={} max_push={} workgroup=({},{},{}) ubo_align={} ts_period={}ns",
            limits.max_texture_dimension_2d, limits.max_bind_groups, limits.max_push_constants_size,
            limits.max_compute_workgroup_size_x, limits.max_compute_workgroup_size_y, limits.max_compute_workgroup_size_z,
            limits.min_uniform_buffer_offset_alignment, limits.timestamp_period_ns
        );
        eprintln!(
            "[rhi-ext] features: {} of {} enabled (first: {:?}); extensions: {} (first: {:?}); queues: {:?}",
            features.len(),
            all_features.len(),
            features.first(),
            extensions.len(),
            extensions.first(),
            queues.iter().map(|q| (q.queue, q.capabilities, q.lane_count)).collect::<Vec<_>>()
        );
        assert!(limits.max_texture_dimension_2d >= 4096 && limits.max_bind_groups >= 4);
        assert!(limits.min_uniform_buffer_offset_alignment.is_power_of_two());
        assert!(!all_features.is_empty() && features.len() <= all_features.len());
        assert!(features.iter().all(|f| all_features.contains(f)));
        assert!(queues.iter().any(|q| q.queue == QueueClass::Graphics && q.capabilities & 1 != 0));
    }

    // -- Feature/device properties: cheap, read-only, no pipeline/resource needed ----------------
    let properties = rhi.feature_properties();
    eprintln!(
        "[rhi-ext] feature_properties: ray_tracing.max_ray_recursion_depth={} \
         ray_tracing.shader_group_handle_size={} mesh_shader.max_mesh_output_vertices={} \
         subgroup.min_subgroup_size={} (0 in any field means that feature is unsupported on this device)",
        properties.ray_tracing.max_ray_recursion_depth,
        properties.ray_tracing.shader_group_handle_size,
        properties.mesh_shader.max_mesh_output_vertices,
        properties.subgroup.min_subgroup_size,
    );

    // -- Buffer mapping: map a HostUpload buffer, write through the mapped slice, read it back --
    let buffer = rhi
        .create_buffer(&BufferDesc {
            size: 256,
            usage: (BufferUsage::TRANSFER_SRC | BufferUsage::TRANSFER_DST).bits(),
            memory: MemoryLocation::HostUpload,
            label: "rhi-ext-mapped-buffer".to_string(),
        })
        .expect("create mapped buffer");
    assert!(buffer.is_valid());

    {
        let mut mapped = rhi.map_buffer(buffer).expect("map buffer");
        for (i, byte) in mapped.iter_mut().enumerate() {
            *byte = (i % 256) as u8;
        }
        // `mapped` drops here, unmapping the buffer.
    }
    {
        let mapped = rhi.map_buffer(buffer).expect("re-map buffer to read back");
        let ok = mapped.iter().enumerate().all(|(i, &b)| b == (i % 256) as u8);
        eprintln!("[rhi-ext] map/write/unmap/remap round trip: {} (expected true)", ok);
        assert!(ok, "buffer contents must survive an unmap/remap cycle");
    }

    // -- fill_buffer / update_buffer, verified via a HostReadback buffer + copy_buffer_to_buffer --
    let readback = rhi
        .create_buffer(&BufferDesc {
            size: 256,
            usage: BufferUsage::TRANSFER_DST.bits(),
            memory: MemoryLocation::HostReadback,
            label: "rhi-ext-readback-buffer".to_string(),
        })
        .expect("create readback buffer");

    let mut encoder = rhi
        .create_command_encoder(&CommandEncoderDesc { queue: QueueClass::Graphics, label: "rhi-ext-encoder".to_string() })
        .expect("create command encoder");

    // Debug-group-wrapped work: fill_buffer with a known 32-bit pattern, then update_buffer to
    // overwrite the first 8 bytes, all inside a push/pop debug group pair (visible in RenderDoc/
    // Nsight captures; here it just proves the calls don't crash and the work still executes).
    encoder.push_debug_group("rhi-ext: fill+update+clear");
    encoder.fill_buffer(buffer, 0, 256, 0xAABBCCDDu32.to_le());
    let update_bytes: [u8; 8] = [1, 2, 3, 4, 5, 6, 7, 8];
    encoder.update_buffer(buffer, 0, &update_bytes);

    // -- Texture clear: create a small color texture, clear it, and copy a texel out via a buffer
    let texture = rhi
        .create_texture(&TextureDesc {
            dimension: TextureDimension::Dim2D,
            format: Format::RGBA8Unorm,
            width: 4,
            height: 4,
            depth_or_layers: 1,
            mip_levels: 1,
            samples: SampleCount::X1,
            usage: (TextureUsage::TRANSFER_SRC | TextureUsage::TRANSFER_DST | TextureUsage::COLOR_ATTACHMENT).bits(),
            label: "rhi-ext-clear-texture".to_string(),
        })
        .expect("create clear texture");

    encoder.barrier(
        &[],
        &[],
        &[sturdy::rhi::TextureBarrier {
            texture,
            src_stage: PipelineStage::NONE.bits(),
            src_access: 0,
            dst_stage: PipelineStage::TRANSFER.bits(),
            dst_access: sturdy::rhi::AccessFlags::TRANSFER_WRITE.bits(),
            ownership: Default::default(),
            old_layout: sturdy::rhi::TextureLayout::Undefined,
            new_layout: sturdy::rhi::TextureLayout::TransferDst,
            range: TextureSubresourceRange { base_mip_level: 0, mip_level_count: 1, base_array_layer: 0, array_layer_count: 1 },
        }],
    );
    encoder.clear_color_texture(
        texture,
        &ClearColor { r: 0.25, g: 0.5, b: 0.75, a: 1.0 },
        &TextureSubresourceRange { base_mip_level: 0, mip_level_count: 1, base_array_layer: 0, array_layer_count: 1 },
    );
    // Also exercise clear_depth_stencil_texture's argument plumbing on a throwaway depth texture.
    let depth_texture = rhi
        .create_texture(&TextureDesc {
            dimension: TextureDimension::Dim2D,
            format: Format::D32Float,
            width: 4,
            height: 4,
            depth_or_layers: 1,
            mip_levels: 1,
            samples: SampleCount::X1,
            usage: TextureUsage::DEPTH_STENCIL_ATTACHMENT.bits(),
            label: "rhi-ext-depth-texture".to_string(),
        })
        .expect("create depth texture");
    encoder.barrier(
        &[],
        &[],
        &[sturdy::rhi::TextureBarrier {
            texture: depth_texture,
            src_stage: PipelineStage::NONE.bits(),
            src_access: 0,
            dst_stage: PipelineStage::EARLY_FRAGMENT_TESTS.bits(),
            dst_access: sturdy::rhi::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE.bits(),
            ownership: Default::default(),
            old_layout: sturdy::rhi::TextureLayout::Undefined,
            new_layout: sturdy::rhi::TextureLayout::DepthStencilAttachment,
            range: TextureSubresourceRange { base_mip_level: 0, mip_level_count: 1, base_array_layer: 0, array_layer_count: 1 },
        }],
    );
    encoder.clear_depth_stencil_texture(
        depth_texture,
        &ClearDepthStencilValue { depth: 0.0, stencil: 0 },
        &TextureSubresourceRange { base_mip_level: 0, mip_level_count: 1, base_array_layer: 0, array_layer_count: 1 },
    );
    encoder.pop_debug_group();

    // Barrier the clear-target texture to TransferSrc, then copy its top-left texel into the
    // readback buffer so the clear color can be verified on the CPU.
    encoder.barrier(
        &[],
        &[],
        &[sturdy::rhi::TextureBarrier {
            texture,
            src_stage: PipelineStage::TRANSFER.bits(),
            src_access: sturdy::rhi::AccessFlags::TRANSFER_WRITE.bits(),
            dst_stage: PipelineStage::TRANSFER.bits(),
            dst_access: sturdy::rhi::AccessFlags::TRANSFER_READ.bits(),
            ownership: Default::default(),
            old_layout: sturdy::rhi::TextureLayout::TransferDst,
            new_layout: sturdy::rhi::TextureLayout::TransferSrc,
            range: TextureSubresourceRange { base_mip_level: 0, mip_level_count: 1, base_array_layer: 0, array_layer_count: 1 },
        }],
    );
    encoder.copy_texture_to_buffer(
        texture,
        readback,
        &sturdy::rhi::BufferTextureCopy {
            buffer_offset: 0,
            buffer_row_length: 4,
            buffer_image_height: 4,
            mip_level: 0,
            base_array_layer: 0,
            array_layer_count: 1,
            offset_x: 0,
            offset_y: 0,
            offset_z: 0,
            extent_width: 4,
            extent_height: 4,
            extent_depth_or_layers: 1,
        },
    );

    // Copy the fill/update-buffer target's first 32 bytes into the second half of the readback
    // buffer so both checks share one submission.
    encoder.copy_buffer_to_buffer(buffer, readback, &sturdy::rhi::BufferCopy { src_offset: 0, dst_offset: 64, size: 32 });

    // -- Query set round trip: a timestamp query written mid-encoder, resolved into `readback` --
    let query_set = rhi
        .create_query_set(&QuerySetDesc { query_type: QueryType::Timestamp, count: 1, statistics: 0, label: "rhi-ext-timestamps".to_string() })
        .expect("create timestamp query set");
    encoder.reset_query_set(query_set, 0, 1);
    encoder.write_timestamp(PipelineStage::TRANSFER, query_set, 0);

    let command_buffer = encoder.finish().expect("finish rhi-ext command buffer");
    rhi.submit(&[command_buffer]).expect("submit rhi-ext command buffer");
    // `submit` does not itself block; wait for the GPU work to actually finish before touching
    // its results (the readback-buffer contents, the query result) from the CPU.
    rhi.wait_idle();

    {
        let mapped = rhi.map_buffer(readback).expect("map readback buffer");
        let texel = &mapped[0..4];
        eprintln!(
            "[rhi-ext] clear_color_texture readback texel (RGBA8) = {:?} (expected close to [64, 128, 191, 255])",
            texel
        );
        let update_readback = &mapped[64..72];
        eprintln!(
            "[rhi-ext] fill_buffer+update_buffer readback bytes[0..8] = {:?} (expected {:?})",
            update_readback, update_bytes
        );
        assert_eq!(update_readback, &update_bytes[..], "update_buffer must have overwritten fill_buffer's pattern");
    }

    let mut timestamp_bytes = [0u8; 8];
    match rhi.get_query_set_results(query_set, 0, 1, &mut timestamp_bytes, 8, QueryResultFlags::RESULT_64_BIT | QueryResultFlags::WAIT) {
        Ok(()) => {
            let ts = u64::from_le_bytes(timestamp_bytes);
            eprintln!("[rhi-ext] query set round trip: timestamp = {ts} (expected nonzero)");
        }
        Err(e) => eprintln!("[rhi-ext] query set round trip: get_query_set_results failed: {e} (backend may not support timestamps)"),
    }

    rhi.destroy_query_set(query_set);
    rhi.destroy_texture(depth_texture);
    rhi.destroy_texture(texture);
    rhi.destroy_buffer(readback);
    rhi.destroy_buffer(buffer);

    eprintln!("[rhi-ext] all RHI-extension demonstration checks passed");
}

/// Slang source for `demonstrate_render_pipeline_ext`: three flat-shaded triangles sharing one
/// vertex layout (`position: float2`, `color: float3`), selected by `first_vertex` at draw time --
/// left-red (vertices 0..3), right-green (3..6), and full-screen-white (6..9, used only by the
/// blend-constant pass). No `import`s (this binding's `compile_spirv` passes no search path), and
/// no resources (no uniforms/textures), so an empty `PipelineLayoutDesc` is valid for it.
const PIPELINE_EXT_SHADER_SOURCE: &str = r#"
struct VertexInput {
    float2 position : POSITION;
    float3 color : COLOR0;
};

struct VertexOutput {
    float4 position : SV_Position;
    float3 color : COLOR0;
};

[shader("vertex")]
VertexOutput vertexMain(VertexInput input) {
    VertexOutput output;
    output.position = float4(input.position, 0.0, 1.0);
    output.color = input.color;
    return output;
}

[shader("fragment")]
float4 fragmentMain(VertexOutput input) : SV_Target {
    return float4(input.color, 1.0);
}
"#;

/// Exercises the RHI surface that actually needs a real render pipeline + render pass to reach:
/// [`sturdy::rhi::Rhi::feature_properties`] and the RHI-extension buffer/texture/query-set work in
/// `demonstrate_rhi_ext` never touch a `RenderPipeline` at all, so
/// `RenderPass::draw_indirect_multi`/`draw_indexed_indirect_multi`, `set_depth_bounds`,
/// `set_stencil_reference`, and `set_blend_constant` (plus the `DepthStencilState`/
/// `MultisampleState` pipeline fields backing depth-bounds/stencil/sample-locations) had never run
/// against a real draw before this. Compiles a real Slang shader to SPIR-V
/// ([`sturdy::shader_compiler`]) and renders into a small offscreen target, reading back individual
/// texels to check actual pixel output rather than just "didn't crash".
///
/// What each pass proves:
///  - Pass 1: `draw_indirect_multi` with `draw_count = 1` (always legal even without
///    `multiDrawIndirect`) draws the left/red triangle; `set_stencil_reference`/
///    `set_depth_bounds(0.0, 1.0)` are exercised alongside it (stencil is a smoke test only --
///    this binding has no stencil-aspect-specific texture readback yet to verify the write
///    landed, only that enabling it didn't break color output).
///  - Pass 2: the same `draw_indirect_multi` call with `draw_count = 2` draws *both* triangles
///    from one call -- the actual "multi" in multi-draw-indirect (assumes the GPU supports
///    `multiDrawIndirect`; every desktop GPU this is likely to run on does, but this binding has
///    no capability query for it to check first).
///  - Pass 3: `set_depth_bounds(0.0, 0.5)` against a depth attachment cleared to 1.0 -- the bounds
///    test compares against whatever is *currently stored* in the depth attachment, not the
///    incoming fragment's own depth, so this must reject the whole draw (leaving the clear color)
///    rather than silently ignoring the call.
///  - Pass 4: a second pipeline with blending enabled, `set_blend_constant` set to a known value,
///    blending a solid white triangle over a black clear with `ConstantColor`/
///    `OneMinusConstantColor` factors -- the resulting texel is an exact, predictable value, not
///    just "some color changed".
///
/// Logged with a `[pipeline-ext]` prefix. Must run once the renderer is up, same gating as
/// `demonstrate_rhi_ext`.
fn demonstrate_render_pipeline_ext(engine: &mut Engine<'_>) {
    use sturdy::rhi::{
        BlendComponent, BlendFactor, BlendOp, BufferDesc, BufferUsage, ClearColor,
        ColorAttachment, ColorTargetState, ColorWriteMask, CommandEncoderDesc,
        CompareOp, CullMode, DepthStencilAttachment, DepthStencilState, DrawArgs, Format, FrontFace,
        MemoryLocation, MultisampleState, PipelineLayoutDesc, PipelineStage, PolygonMode,
        PrimitiveTopology, QueueClass, RasterizationState, Rect2D, RenderPassDesc, RenderPipelineDesc,
        SampleCount, ShaderEntry, ShaderLanguage, ShaderModuleDesc, ShaderStage, StencilFaceState,
        StencilOp, StoreOp, TextureBarrier, TextureDesc, TextureDimension, TextureLayout,
        TextureSubresourceRange, TextureUsage, TextureViewDesc, TextureViewType, VertexAttribute,
        VertexBufferLayout, VertexFormat, VertexStepMode, Viewport,
    };
    use sturdy::rhi::AccessFlags;
    use sturdy::shader_compiler::{self, EntryPoint};

    const SIZE: u32 = 32;
    // Sampled at y = 0 (NDC), which maps to the middle row regardless of whichever way this
    // backend's viewport Y-convention actually flips -- avoids needing to know that convention to
    // predict a pixel coordinate.
    const MID_ROW: u32 = SIZE / 2;
    const LEFT_COL: u32 = SIZE / 4; // inside the red triangle at y=0 (spans x in [-0.9, -0.1])
    const RIGHT_COL: u32 = SIZE * 3 / 4; // inside the green triangle at y=0 (mirror of red)
    const CENTER_COL: u32 = SIZE / 2; // inside the full-screen white triangle

    // -- Compile the shared shader once -----------------------------------------------------------
    let mut modules = shader_compiler::compile_spirv(
        PIPELINE_EXT_SHADER_SOURCE,
        "hello_pipeline_ext",
        &[
            EntryPoint::new("vertexMain", shader_compiler::ShaderStage::Vertex),
            EntryPoint::new("fragmentMain", shader_compiler::ShaderStage::Fragment),
        ],
    )
    .expect("compile pipeline-ext shader to SPIR-V");
    let fragment_spirv = modules.pop().expect("fragmentMain bytecode");
    let vertex_spirv = modules.pop().expect("vertexMain bytecode");
    eprintln!(
        "[pipeline-ext] compiled Slang -> SPIR-V: vertexMain={} bytes, fragmentMain={} bytes",
        vertex_spirv.len(),
        fragment_spirv.len()
    );

    let mut rhi = engine.rhi();

    let vertex_module = rhi
        .create_shader_module(&ShaderModuleDesc { language: ShaderLanguage::SpirV, code: vertex_spirv, label: "pipeline-ext-vs".to_string() })
        .expect("create vertex shader module");
    let fragment_module = rhi
        .create_shader_module(&ShaderModuleDesc { language: ShaderLanguage::SpirV, code: fragment_spirv, label: "pipeline-ext-fs".to_string() })
        .expect("create fragment shader module");

    let pipeline_layout = rhi
        .create_pipeline_layout(&PipelineLayoutDesc { bind_group_layouts: vec![], push_constant_ranges: vec![], label: "pipeline-ext-layout".to_string() })
        .expect("create empty pipeline layout");

    // -- Geometry: 3 triangles, interleaved (position: f32x2, color: f32x3) = 20 bytes/vertex -----
    #[rustfmt::skip]
    let vertices: [f32; 9 * 5] = [
        // left / red -- apex at (-0.1, 0.0), so y=0 sits at the triangle's widest interior point
        -0.9, -0.9, 1.0, 0.0, 0.0,
        -0.9,  0.9, 1.0, 0.0, 0.0,
        -0.1,  0.0, 1.0, 0.0, 0.0,
        // right / green -- mirror of the left triangle
         0.1, -0.9, 0.0, 1.0, 0.0,
         0.1,  0.9, 0.0, 1.0, 0.0,
         0.9,  0.0, 0.0, 1.0, 0.0,
        // full-screen / white -- only used by the blend-constant pass
        -1.0, -1.0, 1.0, 1.0, 1.0,
         1.0, -1.0, 1.0, 1.0, 1.0,
         0.0,  1.0, 1.0, 1.0, 1.0,
    ];
    let vertex_bytes: Vec<u8> = vertices.iter().flat_map(|f| f.to_le_bytes()).collect();
    let vertex_buffer = rhi
        .create_buffer(&BufferDesc {
            size: vertex_bytes.len() as u64,
            usage: (BufferUsage::VERTEX | BufferUsage::TRANSFER_DST).bits(),
            memory: MemoryLocation::HostUpload,
            label: "pipeline-ext-vertices".to_string(),
        })
        .expect("create vertex buffer");
    rhi.write_buffer(vertex_buffer, 0, &vertex_bytes).expect("upload vertex buffer");

    // -- Indirect draw buffer: two DrawArgs records, red then green -------------------------------
    let indirect_records = [
        DrawArgs { vertex_count: 3, instance_count: 1, first_vertex: 0, first_instance: 0 },
        DrawArgs { vertex_count: 3, instance_count: 1, first_vertex: 3, first_instance: 0 },
    ];
    let indirect_bytes: Vec<u8> = indirect_records
        .iter()
        .flat_map(|d| [d.vertex_count, d.instance_count, d.first_vertex, d.first_instance])
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let indirect_stride = (indirect_bytes.len() / indirect_records.len()) as u32;
    let indirect_buffer = rhi
        .create_buffer(&BufferDesc {
            size: indirect_bytes.len() as u64,
            usage: (BufferUsage::INDIRECT | BufferUsage::TRANSFER_DST).bits(),
            memory: MemoryLocation::HostUpload,
            label: "pipeline-ext-indirect".to_string(),
        })
        .expect("create indirect buffer");
    rhi.write_buffer(indirect_buffer, 0, &indirect_bytes).expect("upload indirect buffer");

    // -- Render targets: a color texture + a depth/stencil texture, both SIZE x SIZE --------------
    let color_texture = rhi
        .create_texture(&TextureDesc {
            dimension: TextureDimension::Dim2D,
            format: Format::RGBA8Unorm,
            width: SIZE,
            height: SIZE,
            depth_or_layers: 1,
            mip_levels: 1,
            samples: SampleCount::X1,
            usage: (TextureUsage::COLOR_ATTACHMENT | TextureUsage::TRANSFER_SRC).bits(),
            label: "pipeline-ext-color".to_string(),
        })
        .expect("create color target");
    let color_view = rhi
        .create_texture_view(&TextureViewDesc {
            texture: color_texture,
            view_type: TextureViewType::View2D,
            format: Format::RGBA8Unorm,
            base_mip_level: 0,
            mip_level_count: 1,
            base_array_layer: 0,
            array_layer_count: 1,
            label: "pipeline-ext-color-view".to_string(),
        })
        .expect("create color view");

    let depth_texture = rhi
        .create_texture(&TextureDesc {
            dimension: TextureDimension::Dim2D,
            format: Format::D24UnormS8Uint,
            width: SIZE,
            height: SIZE,
            depth_or_layers: 1,
            mip_levels: 1,
            samples: SampleCount::X1,
            usage: TextureUsage::DEPTH_STENCIL_ATTACHMENT.bits(),
            label: "pipeline-ext-depth".to_string(),
        })
        .expect("create depth/stencil target");
    let depth_view = rhi
        .create_texture_view(&TextureViewDesc {
            texture: depth_texture,
            view_type: TextureViewType::View2D,
            format: Format::D24UnormS8Uint,
            base_mip_level: 0,
            mip_level_count: 1,
            base_array_layer: 0,
            array_layer_count: 1,
            label: "pipeline-ext-depth-view".to_string(),
        })
        .expect("create depth view");

    // -- Pipeline A: opaque, depth-bounds test + per-face stencil ops enabled ----------------------
    let vertex_layout = VertexBufferLayout {
        stride: 20,
        step_mode: VertexStepMode::Vertex,
        attributes: vec![
            VertexAttribute { format: VertexFormat::Float32x2, offset: 0, shader_location: 0, semantic_name: "POSITION".to_string(), semantic_index: 0 },
            VertexAttribute { format: VertexFormat::Float32x3, offset: 8, shader_location: 1, semantic_name: "COLOR".to_string(), semantic_index: 0 },
        ],
    };
    let rasterization = RasterizationState {
        polygon_mode: PolygonMode::Fill,
        cull_mode: CullMode::None,
        front_face: FrontFace::CounterClockwise,
        depth_bias_constant: 0.0,
        depth_bias_slope_scale: 0.0,
        depth_bias_clamp: 0.0,
        line_width: 1.0,
    };
    let opaque_color_target = ColorTargetState {
        format: Format::RGBA8Unorm,
        blend_enable: false,
        color: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::Zero, op: BlendOp::Add },
        alpha: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::Zero, op: BlendOp::Add },
        write_mask: ColorWriteMask::ALL.bits(),
    };
    let stencil_face = StencilFaceState { fail_op: StencilOp::Keep, depth_fail_op: StencilOp::Keep, pass_op: StencilOp::Replace, compare: CompareOp::Always };

    let opaque_pipeline = rhi
        .create_render_pipeline(&RenderPipelineDesc {
            layout: pipeline_layout,
            vertex: ShaderEntry { module: vertex_module, entry_point: "vertexMain".to_string(), stage: ShaderStage::VERTEX.bits() },
            fragment: ShaderEntry { module: fragment_module, entry_point: "fragmentMain".to_string(), stage: ShaderStage::FRAGMENT.bits() },
            vertex_buffers: vec![vertex_layout.clone()],
            topology: PrimitiveTopology::TriangleList,
            rasterization: rasterization.clone(),
            multisample: MultisampleState { samples: SampleCount::X1, sample_mask: !0, alpha_to_coverage_enable: false, sample_locations_enable: false },
            depth_stencil: DepthStencilState {
                format: Format::D24UnormS8Uint,
                depth_test_enable: true,
                depth_write_enable: true,
                depth_compare: CompareOp::Always,
                stencil_test_enable: true,
                stencil_front: stencil_face,
                stencil_back: stencil_face,
                stencil_read_mask: 0xFF,
                stencil_write_mask: 0xFF,
                depth_bounds_test_enable: true,
            },
            color_targets: vec![opaque_color_target],
            label: "pipeline-ext-opaque".to_string(),
        })
        .expect("create opaque render pipeline (depth-bounds + stencil enabled)");

    // -- Pipeline B: blending enabled, no depth/stencil, for the blend-constant pass ---------------
    let blend_pipeline = rhi
        .create_render_pipeline(&RenderPipelineDesc {
            layout: pipeline_layout,
            vertex: ShaderEntry { module: vertex_module, entry_point: "vertexMain".to_string(), stage: ShaderStage::VERTEX.bits() },
            fragment: ShaderEntry { module: fragment_module, entry_point: "fragmentMain".to_string(), stage: ShaderStage::FRAGMENT.bits() },
            vertex_buffers: vec![vertex_layout],
            topology: PrimitiveTopology::TriangleList,
            rasterization,
            multisample: MultisampleState { samples: SampleCount::X1, sample_mask: !0, alpha_to_coverage_enable: false, sample_locations_enable: false },
            depth_stencil: DepthStencilState {
                format: Format::Undefined,
                depth_test_enable: false,
                depth_write_enable: false,
                depth_compare: CompareOp::Always,
                stencil_test_enable: false,
                stencil_front: StencilFaceState { fail_op: StencilOp::Keep, depth_fail_op: StencilOp::Keep, pass_op: StencilOp::Keep, compare: CompareOp::Always },
                stencil_back: StencilFaceState { fail_op: StencilOp::Keep, depth_fail_op: StencilOp::Keep, pass_op: StencilOp::Keep, compare: CompareOp::Always },
                stencil_read_mask: 0,
                stencil_write_mask: 0,
                depth_bounds_test_enable: false,
            },
            color_targets: vec![ColorTargetState {
                format: Format::RGBA8Unorm,
                blend_enable: true,
                color: BlendComponent { src_factor: BlendFactor::ConstantColor, dst_factor: BlendFactor::OneMinusConstantColor, op: BlendOp::Add },
                alpha: BlendComponent { src_factor: BlendFactor::One, dst_factor: BlendFactor::Zero, op: BlendOp::Add },
                write_mask: ColorWriteMask::ALL.bits(),
            }],
            label: "pipeline-ext-blend".to_string(),
        })
        .expect("create blend render pipeline");

    // -- Readback buffer: one RGBA8 texel per check, copied directly (no full-texture copy needed) -
    let readback = rhi
        .create_buffer(&BufferDesc { size: 7 * 4, usage: BufferUsage::TRANSFER_DST.bits(), memory: MemoryLocation::HostReadback, label: "pipeline-ext-readback".to_string() })
        .expect("create readback buffer");

    let mut encoder = rhi
        .create_command_encoder(&CommandEncoderDesc { queue: QueueClass::Graphics, label: "pipeline-ext-encoder".to_string() })
        .expect("create pipeline-ext command encoder");

    let full_range = TextureSubresourceRange { base_mip_level: 0, mip_level_count: 1, base_array_layer: 0, array_layer_count: 1 };
    let color_attachment = |load_op| ColorAttachment {
        view: color_view,
        resolve_view: sturdy::rhi::TextureViewHandle::default(),
        load_op,
        store_op: StoreOp::Store,
        clear_color: ClearColor { r: 0.0, g: 0.0, b: 0.0, a: 1.0 },
    };
    let depth_attachment = DepthStencilAttachment {
        view: depth_view,
        depth_load_op: sturdy::rhi::LoadOp::Clear,
        depth_store_op: StoreOp::Store,
        stencil_load_op: sturdy::rhi::LoadOp::Clear,
        stencil_store_op: StoreOp::Store,
        clear_depth: 1.0,
        clear_stencil: 0,
    };
    let viewport = Viewport { x: 0.0, y: 0.0, width: SIZE as f32, height: SIZE as f32, min_depth: 0.0, max_depth: 1.0 };
    let scissor = Rect2D { x: 0, y: 0, width: SIZE, height: SIZE };

    // Undefined -> attachment layouts, once, before the first pass.
    encoder.barrier(
        &[],
        &[],
        &[
            TextureBarrier {
                texture: color_texture,
                src_stage: PipelineStage::NONE.bits(),
                src_access: 0,
                dst_stage: PipelineStage::COLOR_ATTACHMENT_OUTPUT.bits(),
                dst_access: AccessFlags::COLOR_ATTACHMENT_WRITE.bits(),
                ownership: Default::default(),
                old_layout: TextureLayout::Undefined,
                new_layout: TextureLayout::ColorAttachment,
                range: full_range,
            },
            TextureBarrier {
                texture: depth_texture,
                src_stage: PipelineStage::NONE.bits(),
                src_access: 0,
                dst_stage: PipelineStage::EARLY_FRAGMENT_TESTS.bits(),
                dst_access: AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE.bits(),
                ownership: Default::default(),
                old_layout: TextureLayout::Undefined,
                new_layout: TextureLayout::DepthStencilAttachment,
                range: full_range,
            },
        ],
    );

    /// Barriers `color_texture` from `ColorAttachment` to `TransferSrc`, copies one texel out to
    /// `readback` at `slot`, then barriers back to `ColorAttachment` for the next pass.
    fn readback_texel(
        encoder: &mut sturdy::rhi::CommandEncoder,
        color_texture: sturdy::rhi::TextureHandle,
        readback: sturdy::rhi::BufferHandle,
        full_range: TextureSubresourceRange,
        x: u32,
        y: u32,
        slot: u32,
    ) {
        let to_transfer_src = TextureBarrier {
            texture: color_texture,
            src_stage: PipelineStage::COLOR_ATTACHMENT_OUTPUT.bits(),
            src_access: AccessFlags::COLOR_ATTACHMENT_WRITE.bits(),
            dst_stage: PipelineStage::TRANSFER.bits(),
            dst_access: AccessFlags::TRANSFER_READ.bits(),
            ownership: Default::default(),
            old_layout: TextureLayout::ColorAttachment,
            new_layout: TextureLayout::TransferSrc,
            range: full_range,
        };
        encoder.barrier(&[], &[], &[to_transfer_src]);
        encoder.copy_texture_to_buffer(
            color_texture,
            readback,
            &sturdy::rhi::BufferTextureCopy {
                buffer_offset: (slot * 4) as u64,
                buffer_row_length: 1,
                buffer_image_height: 1,
                mip_level: 0,
                base_array_layer: 0,
                array_layer_count: 1,
                offset_x: x as i32,
                offset_y: y as i32,
                offset_z: 0,
                extent_width: 1,
                extent_height: 1,
                extent_depth_or_layers: 1,
            },
        );
        let back_to_color_attachment = TextureBarrier {
            texture: color_texture,
            src_stage: PipelineStage::TRANSFER.bits(),
            src_access: AccessFlags::TRANSFER_READ.bits(),
            dst_stage: PipelineStage::COLOR_ATTACHMENT_OUTPUT.bits(),
            dst_access: AccessFlags::COLOR_ATTACHMENT_WRITE.bits(),
            ownership: Default::default(),
            old_layout: TextureLayout::TransferSrc,
            new_layout: TextureLayout::ColorAttachment,
            range: full_range,
        };
        encoder.barrier(&[], &[], &[back_to_color_attachment]);
    }

    // -- Pass 1: draw_indirect_multi(draw_count=1) + set_stencil_reference + set_depth_bounds(pass) -
    {
        let mut rp = encoder
            .begin_render_pass(&RenderPassDesc {
                color_attachments: vec![color_attachment(sturdy::rhi::LoadOp::Clear)],
                has_depth_stencil: true,
                depth_stencil: depth_attachment.clone(),
                render_area: scissor.clone(),
                label: "pipeline-ext-pass1".to_string(),
            })
            .expect("begin pass 1");
        rp.set_viewport(&viewport);
        rp.set_scissor(&scissor);
        rp.set_pipeline(opaque_pipeline);
        rp.set_vertex_buffer(0, vertex_buffer, 0);
        rp.set_stencil_reference(7);
        rp.set_depth_bounds(0.0, 1.0);
        rp.draw_indirect_multi(indirect_buffer, 0, 1, indirect_stride);
    }
    readback_texel(&mut encoder, color_texture, readback, full_range, LEFT_COL, MID_ROW, 0);

    // -- Pass 2: draw_indirect_multi(draw_count=2) -- both triangles from one call ------------------
    {
        let mut rp = encoder
            .begin_render_pass(&RenderPassDesc {
                color_attachments: vec![color_attachment(sturdy::rhi::LoadOp::Clear)],
                has_depth_stencil: true,
                depth_stencil: depth_attachment.clone(),
                render_area: scissor.clone(),
                label: "pipeline-ext-pass2".to_string(),
            })
            .expect("begin pass 2");
        rp.set_viewport(&viewport);
        rp.set_scissor(&scissor);
        rp.set_pipeline(opaque_pipeline);
        rp.set_vertex_buffer(0, vertex_buffer, 0);
        rp.set_stencil_reference(7);
        rp.set_depth_bounds(0.0, 1.0);
        rp.draw_indirect_multi(indirect_buffer, 0, 2, indirect_stride);
    }
    readback_texel(&mut encoder, color_texture, readback, full_range, LEFT_COL, MID_ROW, 1);
    readback_texel(&mut encoder, color_texture, readback, full_range, RIGHT_COL, MID_ROW, 2);

    // -- Pass 3: bounds [0.0, 0.5] must reject the whole draw -----------------------------------------
    //
    // The depth-bounds test compares against whatever is *currently stored* in the depth
    // attachment at that pixel -- not the incoming fragment's own depth -- confirmed empirically
    // while writing this test: an earlier version used bounds [0.5, 1.0] expecting z=0 geometry to
    // be rejected, but the depth attachment was cleared to 1.0 (which *is* inside [0.5, 1.0]), so
    // the bounds test passed regardless of the triangle's own depth. Clearing to 1.0 and testing
    // against [0.0, 0.5] (which excludes the cleared value) is what actually exercises rejection.
    {
        let mut rp = encoder
            .begin_render_pass(&RenderPassDesc {
                color_attachments: vec![color_attachment(sturdy::rhi::LoadOp::Clear)],
                has_depth_stencil: true,
                depth_stencil: depth_attachment.clone(),
                render_area: scissor.clone(),
                label: "pipeline-ext-pass3".to_string(),
            })
            .expect("begin pass 3");
        rp.set_viewport(&viewport);
        rp.set_scissor(&scissor);
        rp.set_pipeline(opaque_pipeline);
        rp.set_vertex_buffer(0, vertex_buffer, 0);
        rp.set_stencil_reference(7);
        rp.set_depth_bounds(0.0, 0.5);
        rp.draw(&DrawArgs { vertex_count: 3, instance_count: 1, first_vertex: 0, first_instance: 0 });
    }
    readback_texel(&mut encoder, color_texture, readback, full_range, LEFT_COL, MID_ROW, 3);

    // -- Pass 4: blend pipeline, white triangle over black clear, exact blend-constant result -------
    {
        let mut rp = encoder
            .begin_render_pass(&RenderPassDesc {
                color_attachments: vec![color_attachment(sturdy::rhi::LoadOp::Clear)],
                has_depth_stencil: false,
                depth_stencil: DepthStencilAttachment {
                    view: sturdy::rhi::TextureViewHandle::default(),
                    depth_load_op: sturdy::rhi::LoadOp::DontCare,
                    depth_store_op: StoreOp::DontCare,
                    stencil_load_op: sturdy::rhi::LoadOp::DontCare,
                    stencil_store_op: StoreOp::DontCare,
                    clear_depth: 1.0,
                    clear_stencil: 0,
                },
                render_area: scissor.clone(),
                label: "pipeline-ext-pass4".to_string(),
            })
            .expect("begin pass 4");
        rp.set_viewport(&viewport);
        rp.set_scissor(&scissor);
        rp.set_pipeline(blend_pipeline);
        rp.set_vertex_buffer(0, vertex_buffer, 0);
        rp.set_blend_constant(&ClearColor { r: 0.25, g: 0.25, b: 0.25, a: 0.25 });
        rp.draw(&DrawArgs { vertex_count: 3, instance_count: 1, first_vertex: 6, first_instance: 0 });
    }
    readback_texel(&mut encoder, color_texture, readback, full_range, CENTER_COL, MID_ROW, 4);

    // -- Pass 5: a render bundle, recorded once up front, replayed via execute_bundles ------------
    //
    // The bundle carries *all* of its own state (viewport, scissor, pipeline, vertex buffer,
    // dynamic state) and draws only the green triangle; the pass itself does nothing but replay
    // it. Green on the right and the black clear on the left proves the bundle's recorded draw --
    // and only that draw -- actually executed.
    let bundle = {
        let mut rb = rhi
            .create_render_bundle_encoder(&sturdy::rhi::RenderBundleDesc {
                color_formats: vec![Format::RGBA8Unorm],
                depth_stencil_format: Format::D24UnormS8Uint,
                samples: SampleCount::X1,
                view_mask: 0,
                label: "pipeline-ext-bundle".to_string(),
            })
            .expect("create render bundle encoder");
        rb.set_viewport(&viewport);
        rb.set_scissor(&scissor);
        rb.set_pipeline(opaque_pipeline);
        rb.set_vertex_buffer(0, vertex_buffer, 0);
        rb.set_stencil_reference(7);
        rb.set_depth_bounds(0.0, 1.0);
        rb.draw(&DrawArgs { vertex_count: 3, instance_count: 1, first_vertex: 3, first_instance: 0 });
        rb.finish().expect("finish render bundle")
    };
    {
        let mut rp = encoder
            .begin_render_pass(&RenderPassDesc {
                color_attachments: vec![color_attachment(sturdy::rhi::LoadOp::Clear)],
                has_depth_stencil: true,
                depth_stencil: depth_attachment,
                render_area: scissor.clone(),
                label: "pipeline-ext-pass5-bundle".to_string(),
            })
            .expect("begin pass 5");
        rp.execute_bundles(&[bundle]);
    }
    readback_texel(&mut encoder, color_texture, readback, full_range, LEFT_COL, MID_ROW, 5);
    readback_texel(&mut encoder, color_texture, readback, full_range, RIGHT_COL, MID_ROW, 6);

    // -- Submit with a fence + a GPU-signaled timeline semaphore, instead of wait_idle ----------
    let fence = rhi
        .create_fence(&sturdy::rhi::FenceDesc { signaled: false, label: "pipeline-ext-fence".to_string() })
        .expect("create fence");
    let timeline = rhi
        .create_semaphore(&sturdy::rhi::SemaphoreDesc { initial_value: 0, label: "pipeline-ext-timeline".to_string() })
        .expect("create timeline semaphore");

    // CPU-side signal/read round trip before any GPU involvement.
    rhi.signal_semaphore(timeline, 5).expect("CPU-signal timeline semaphore");
    let cpu_signaled = rhi.semaphore_value(timeline).expect("read timeline semaphore");
    eprintln!("[pipeline-ext] timeline semaphore after CPU signal(5) = {cpu_signaled} (expected 5)");
    assert_eq!(cpu_signaled, 5);

    let command_buffer = encoder.finish().expect("finish pipeline-ext command buffer");
    rhi.submit_with(&sturdy::rhi::SubmitDesc {
        queue: sturdy::rhi::QueueLane::default(),
        command_buffers: vec![command_buffer],
        waits: vec![sturdy::rhi::QueueSemaphoreOp { semaphore: timeline, value: 5, stages: PipelineStage::ALL_COMMANDS.bits() }],
        signals: vec![sturdy::rhi::QueueSemaphoreOp { semaphore: timeline, value: 10, stages: PipelineStage::ALL_COMMANDS.bits() }],
        fence,
        one_shot: true,
        label: "pipeline-ext-submit".to_string(),
    })
    .expect("submit_with pipeline-ext command buffer");

    let completed = rhi.wait_fences(&[fence], true, sturdy::rhi::WAIT_FOREVER).expect("wait on pipeline-ext fence");
    eprintln!("[pipeline-ext] wait_fences after submit_with = {completed} (expected true)");
    assert!(completed, "the submission's fence must signal");
    rhi.wait_semaphore(timeline, 10, sturdy::rhi::WAIT_FOREVER).expect("wait for GPU-signaled timeline value");
    let gpu_signaled = rhi.semaphore_value(timeline).expect("read timeline semaphore after submit");
    eprintln!("[pipeline-ext] timeline semaphore after the submission's signal(10) = {gpu_signaled} (expected 10)");
    assert_eq!(gpu_signaled, 10, "the submission must have signaled the timeline semaphore");

    rhi.reset_fences(&[fence]).expect("reset fence");
    let reset_poll = rhi.wait_fences(&[fence], true, 0).expect("poll reset fence");
    eprintln!("[pipeline-ext] zero-timeout poll of the fence after reset_fences = {reset_poll} (expected false)");
    assert!(!reset_poll, "a reset fence must read as unsignaled");
    rhi.destroy_fence(fence);
    rhi.destroy_semaphore(timeline);
    rhi.destroy_render_bundle(bundle);

    {
        let mapped = rhi.map_buffer(readback).expect("map pipeline-ext readback buffer");
        let texel = |slot: usize| -> [u8; 4] { mapped[slot * 4..slot * 4 + 4].try_into().unwrap() };

        let pass1_red = texel(0);
        eprintln!("[pipeline-ext] pass 1 (draw_indirect_multi, count=1) left pixel = {pass1_red:?} (expected ~[255,0,0,255])");
        assert!(pass1_red[0] > 200 && pass1_red[1] < 50 && pass1_red[2] < 50, "pass 1 must show the red triangle");

        let pass2_red = texel(1);
        let pass2_green = texel(2);
        eprintln!(
            "[pipeline-ext] pass 2 (draw_indirect_multi, count=2) left={pass2_red:?} right={pass2_green:?} (expected ~[255,0,0,255] / ~[0,255,0,255])"
        );
        assert!(pass2_red[0] > 200 && pass2_red[1] < 50, "pass 2 must show the red triangle on the left");
        assert!(pass2_green[1] > 200 && pass2_green[0] < 50, "pass 2 must show the green triangle on the right -- proves draw_count=2 issued two real draws, not one");

        let pass3_bg = texel(3);
        eprintln!("[pipeline-ext] pass 3 (set_depth_bounds(0.0, 0.5) against a depth attachment cleared to 1.0) left pixel = {pass3_bg:?} (expected the black clear color, [0,0,0,255] -- the whole draw must be rejected)");
        assert!(pass3_bg[0] < 20 && pass3_bg[1] < 20 && pass3_bg[2] < 20, "set_depth_bounds must actually reject out-of-range geometry, not just accept the call");

        let pass4_blend = texel(4);
        eprintln!("[pipeline-ext] pass 4 (set_blend_constant(0.25)) center pixel = {pass4_blend:?} (expected ~[64,64,64,255] = white * 0.25 + black * 0.75)");
        for channel in 0..3 {
            let value = pass4_blend[channel] as i32;
            assert!((value - 64).abs() <= 10, "blend-constant result channel {channel} = {value}, expected close to 64");
        }

        let pass5_left = texel(5);
        let pass5_right = texel(6);
        eprintln!(
            "[pipeline-ext] pass 5 (execute_bundles, bundle draws only the green triangle) left={pass5_left:?} right={pass5_right:?} (expected [0,0,0,255] / ~[0,255,0,255])"
        );
        assert!(pass5_left[0] < 20 && pass5_left[1] < 20, "pass 5 left must be the clear color -- the bundle only draws green");
        assert!(pass5_right[1] > 200 && pass5_right[0] < 50, "pass 5 right must be green -- the bundle's recorded draw must replay");
    }

    rhi.destroy_buffer(readback);
    rhi.destroy_render_pipeline(blend_pipeline);
    rhi.destroy_render_pipeline(opaque_pipeline);
    rhi.destroy_texture_view(depth_view);
    rhi.destroy_texture(depth_texture);
    rhi.destroy_texture_view(color_view);
    rhi.destroy_texture(color_texture);
    rhi.destroy_buffer(indirect_buffer);
    rhi.destroy_buffer(vertex_buffer);
    rhi.destroy_pipeline_layout(pipeline_layout);
    rhi.destroy_shader_module(fragment_module);
    rhi.destroy_shader_module(vertex_module);

    eprintln!("[pipeline-ext] all render-pipeline-extension demonstration checks passed");
}

/// Exercises the asset-management extensions added on top of the original load/query/unload
/// surface: `create_texture` (raw pixel buffer -> texture, no file decode), `create_orm_texture`
/// (packed occlusion/roughness/metallic), `texture_handle` (Renderer-level handle readback), and
/// `file_bytes` (raw buffer readback for an already-loaded asset). Logged with an `[assets-ext]`
/// prefix. Gated the same way as `demonstrate_rhi_ext`: `AssetManager` is constructed from a
/// `Renderer&`, so this needs to run once a GPU is actually up, not from `init`.
fn demonstrate_assets_ext(engine: &mut Engine<'_>) {
    use sturdy::assets::{TextureColorSpace, TextureDynamicRange, TextureKind, TexturePixelFormat};

    let mut assets = engine.assets();

    // -- create_texture: a procedural 4x4 checkerboard, straight from a raw RGBA8 pixel buffer,
    // no image decoder involved. Verified by reading the dimensions/color-space back through
    // `texture_info` (the only readback this asset kind exposes -- there is no raw-pixel readback
    // API for GPU textures, unlike `file_bytes`/`sound_samples` for File/Sound assets).
    const SIZE: u32 = 4;
    let mut pixels = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let idx = ((y * SIZE + x) * 4) as usize;
            let color: [u8; 4] = if (x + y) % 2 == 0 { [255, 32, 32, 255] } else { [32, 32, 255, 255] };
            pixels[idx..idx + 4].copy_from_slice(&color);
        }
    }
    let texture = assets
        .create_texture(
            &pixels,
            SIZE,
            SIZE,
            TexturePixelFormat::Rgba8,
            TextureColorSpace::Linear,
            TextureKind::ColorAlpha,
            "assets-ext-checkerboard",
            false, // allow_compression
            false, // generate_mipmaps
        )
        .expect("create_texture from a raw 4x4 RGBA8 checkerboard buffer");
    eprintln!("[assets-ext] create_texture: asset={texture:?}");

    let info = assets.texture_info(texture).expect("texture_info for the procedural checkerboard");
    eprintln!(
        "[assets-ext] texture_info: {}x{} color_space={:?} (expected {SIZE}x{SIZE} Linear)",
        info.size.x, info.size.y, info.color_space
    );
    assert_eq!(info.size, glam::uvec2(SIZE, SIZE));
    assert_eq!(info.color_space, TextureColorSpace::Linear);

    // -- texture_handle: the Renderer-registry handle backing that same texture asset.
    let renderer_handle = assets.texture_handle(texture).expect("texture_handle for the procedural checkerboard");
    eprintln!("[assets-ext] texture_handle: {renderer_handle:?} (expected a valid/nonzero handle)");
    assert!(renderer_handle.is_valid(), "a freshly created texture must have a valid Renderer-level handle");

    // -- rhi_texture: resolve that Renderer handle to its RHI texture, then read the texels back
    // through plain RHI calls -- proving it's the real GPU resource holding the checkerboard.
    let resources = assets.rhi_texture(renderer_handle).expect("rhi_texture for a live texture");
    eprintln!(
        "[assets-ext] rhi_texture: texture={:?} view={:?} sampler={:?} {}x{} mips={} format={:?}",
        resources.texture, resources.view, resources.sampler, resources.size.x, resources.size.y, resources.mip_levels, resources.format
    );
    assert_eq!(resources.size, glam::uvec2(SIZE, SIZE));
    assert!(resources.texture.value != 0 && resources.view.value != 0);
    assert!(assets.rhi_texture(sturdy::assets::RendererTextureHandle::default()).is_none());
    drop(assets);
    {
        use sturdy::rhi::{AccessFlags, BufferDesc, BufferUsage, CommandEncoderDesc, MemoryLocation, PipelineStage, QueueClass, TextureBarrier, TextureLayout, TextureSubresourceRange};
        let mut rhi = engine.rhi();
        let readback = rhi
            .create_buffer(&BufferDesc { size: (SIZE * SIZE * 4) as u64, usage: BufferUsage::TRANSFER_DST.bits(), memory: MemoryLocation::HostReadback, label: "assets-ext-readback".to_string() })
            .expect("create readback buffer");
        let mut encoder = rhi
            .create_command_encoder(&CommandEncoderDesc { queue: QueueClass::Graphics, label: "assets-ext-readback".to_string() })
            .expect("create command encoder");
        let range = TextureSubresourceRange { base_mip_level: 0, mip_level_count: 1, base_array_layer: 0, array_layer_count: 1 };
        let barrier = |old_layout, new_layout, src_access: AccessFlags, dst_access: AccessFlags| TextureBarrier {
            texture: resources.texture,
            src_stage: PipelineStage::ALL_COMMANDS.bits(),
            src_access: src_access.bits(),
            dst_stage: PipelineStage::ALL_COMMANDS.bits(),
            dst_access: dst_access.bits(),
            ownership: Default::default(),
            old_layout,
            new_layout,
            range,
        };
        encoder.barrier(&[], &[], &[barrier(TextureLayout::ShaderReadOnly, TextureLayout::TransferSrc, AccessFlags::SHADER_READ, AccessFlags::TRANSFER_READ)]);
        encoder.copy_texture_to_buffer(
            resources.texture,
            readback,
            &sturdy::rhi::BufferTextureCopy {
                buffer_offset: 0,
                buffer_row_length: SIZE,
                buffer_image_height: SIZE,
                mip_level: 0,
                base_array_layer: 0,
                array_layer_count: 1,
                offset_x: 0,
                offset_y: 0,
                offset_z: 0,
                extent_width: SIZE,
                extent_height: SIZE,
                extent_depth_or_layers: 1,
            },
        );
        encoder.barrier(&[], &[], &[barrier(TextureLayout::TransferSrc, TextureLayout::ShaderReadOnly, AccessFlags::TRANSFER_READ, AccessFlags::SHADER_READ)]);
        let commands = encoder.finish().expect("finish readback encoder");
        rhi.submit(&[commands]).expect("submit readback");
        rhi.wait_idle();
        let matches = {
            let mapped = rhi.map_buffer(readback).expect("map readback");
            let matches = mapped[..pixels.len()] == pixels[..];
            eprintln!("[assets-ext] RHI readback of the asset texture: first texels {:?} -> matches uploaded checkerboard = {matches}", &mapped[0..8]);
            matches
        };
        rhi.destroy_buffer(readback);
        assert!(matches, "the RHI texture behind the asset must hold the uploaded pixels");
    }
    let mut assets = engine.assets();

    // -- create_orm_texture: pack a solid-white occlusion buffer and a metallic-roughness buffer
    // (roughness=128, metallic=255 in every texel) into one ORM texture asset.
    let orm_size = 2u32;
    let occlusion_rgba8 = vec![255u8; (orm_size * orm_size * 4) as usize];
    let mut metallic_roughness_rgba8 = vec![0u8; (orm_size * orm_size * 4) as usize];
    for texel in metallic_roughness_rgba8.chunks_exact_mut(4) {
        texel[0] = 128; // roughness
        texel[1] = 0;
        texel[2] = 255; // metallic
        texel[3] = 255;
    }
    let orm_texture = assets
        .create_orm_texture(&occlusion_rgba8, &metallic_roughness_rgba8, orm_size, orm_size, "assets-ext-orm")
        .expect("create_orm_texture from occlusion + metallic-roughness buffers");
    let orm_info = assets.texture_info(orm_texture).expect("texture_info for the ORM texture");
    eprintln!(
        "[assets-ext] create_orm_texture: asset={orm_texture:?}, texture_info={}x{} color_space={:?} (expected {orm_size}x{orm_size})",
        orm_info.size.x, orm_info.size.y, orm_info.color_space
    );
    assert_eq!(orm_info.size, glam::uvec2(orm_size, orm_size));

    // -- file_bytes: load this very source file as a raw `File` asset, then confirm the bytes
    // read back through the bridge match what's actually on disk.
    let this_file = concat!(env!("CARGO_MANIFEST_DIR"), "/src/main.rs");
    let on_disk = std::fs::read(this_file).expect("read main.rs from disk for comparison");
    let file_asset = assets.load_file(this_file, "assets-ext-main-rs").expect("load_file(main.rs)");
    let read_back = assets.file_bytes(file_asset).expect("file_bytes for the loaded File asset");
    eprintln!(
        "[assets-ext] file_bytes: loaded {} bytes, on-disk {} bytes (expected equal)",
        read_back.len(),
        on_disk.len()
    );
    assert_eq!(read_back, on_disk, "file_bytes must return exactly what's on disk");

    // Also prove create_texture rejects a garbage size the way the doc comment says it should
    // (mismatched pixel buffer length is caught by the engine, not just silently truncated).
    let bad = assets.create_texture(
        &pixels[..pixels.len() - 1],
        SIZE,
        SIZE,
        TexturePixelFormat::Rgba8,
        TextureColorSpace::Linear,
        TextureKind::ColorAlpha,
        "assets-ext-bad-size",
        false,
        false,
    );
    eprintln!("[assets-ext] create_texture with a truncated pixel buffer -> {bad:?} (expected Err)");
    assert!(bad.is_err(), "a pixel buffer shorter than width*height*4 must be rejected");

    assets.unload(texture).expect("unload procedural checkerboard texture");
    assets.unload(orm_texture).expect("unload ORM texture");
    assets.unload(file_asset).expect("unload main.rs File asset");

    // `TextureDynamicRange`/`load_texture`'s HDR path is exercised by `create_texture`'s sibling
    // API already covered above (`load_texture`/`create_texture_from_bytes`) in earlier passes of
    // this binding; nothing further to demonstrate here.
    let _ = TextureDynamicRange::Sdr;

    eprintln!("[assets-ext] all asset-management-extension demonstration checks passed");
}

/// Imports a real glTF file (`assets/box.gltf` -- see `assets/README.md` for provenance/license)
/// through `Assets::import_gltf`, then spawns it into the ECS world with `GltfScene::spawn_all`
/// and confirms the resulting entities carry the expected components. Logged with a `[gltf-ext]`
/// prefix. Gated the same way as `demonstrate_assets_ext`: needs a live GPU.
fn demonstrate_gltf_ext(engine: &mut Engine<'_>) {
    use sturdy::scene::{ModelRenderer, WorldTransform};

    const BOX_GLTF: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/box.gltf");

    // The importer needs a real shader asset for every primitive (an invalid handle is rejected with
    // "Model primitive references an invalid shader asset"); the engine's own G-buffer shader is
    // embedded in the binary, so this needs no file on disk.
    let shader = engine
        .assets()
        .load_shader("Shaders/gbuffer_geometry.slang", "gltf-ext-shader")
        .expect("load embedded gbuffer shader");
    let scene = engine.assets().import_gltf(BOX_GLTF, shader).expect("import assets/box.gltf");
    eprintln!(
        "[gltf-ext] import_gltf(box.gltf) -> {} models, {} instances, {} lights (expected 1, 1, 0)",
        scene.models.len(),
        scene.instances.len(),
        scene.lights.len()
    );
    assert_eq!(scene.models.len(), 1);
    assert_eq!(scene.instances.len(), 1);
    assert!(scene.lights.is_empty(), "the Box sample has no lights");
    assert!(scene.models[0].is_valid());
    eprintln!(
        "[gltf-ext] instance \"{}\" -> model={:?} world_transform.translation={:?}",
        scene.instances[0].name,
        scene.instances[0].model,
        scene.instances[0].world_transform.transform_point3(glam::Vec3::ZERO)
    );

    let mut world = engine.ecs();
    let entities = scene.spawn_all(&mut world).expect("spawn_all the imported glTF scene");
    eprintln!("[gltf-ext] spawn_all -> {} entities (expected 1)", entities.len());
    assert_eq!(entities.len(), 1);

    let transform: WorldTransform = world.get(entities[0]).expect("read spawned WorldTransform");
    let renderer: ModelRenderer = world.get(entities[0]).expect("read spawned ModelRenderer");
    eprintln!(
        "[gltf-ext] spawned entity {:?}: WorldTransform.translation={:?}, ModelRenderer.model={:?} visible={}",
        entities[0],
        transform.translation(),
        renderer.model,
        renderer.visible()
    );
    assert_eq!(renderer.model.asset(), scene.models[0]);
    assert!(renderer.visible());

    eprintln!("[gltf-ext] all glTF-import-extension demonstration checks passed");
}

/// Queues every window runtime-mutation request (`sturdy::render`'s `set_cursor_icon`/
/// `set_cursor_grabbed`/`set_window_mode`/`set_decorated`/`set_transparent`/
/// `set_relative_mouse_mode`/`set_text_input_active`/`set_text_input_area`/`set_window_effect`)
/// against the primary window, then `set_mouse_locked(true)` last. All of these are
/// enqueue-and-forget (queued via the engine's own `WindowRequests`, applied on its next update --
/// see `sturdy-sys`'s doc comment on them) with no per-call result to check, so most are just
/// exercised and logged; `set_mouse_locked` is the one with an independent readback
/// (`Diagnostics::window`'s `mouse_locked` snapshot field), so `check_window_mutation_demo` polls
/// for that one actually landing.
fn demonstrate_window_mutation(engine: &mut Engine<'_>, surface: SurfaceHandle) {
    engine.set_cursor_icon(surface, CursorIcon::Pointer);
    engine.set_cursor_grabbed(surface, false);
    engine.set_window_mode(surface, sturdy::WindowMode::Windowed);
    engine.set_decorated(surface, true);
    engine.set_transparent(surface, false);
    engine.set_relative_mouse_mode(surface, false);
    engine.set_window_effect(surface, WindowEffectKind::Blur, false);
    engine.set_text_input_active(surface, false);
    engine.set_text_input_area(surface, TextInputArea { position: glam::vec2(10.0, 10.0), size: glam::vec2(100.0, 20.0), cursor_offset_x: 0.0 });
    eprintln!(
        "[window-ext] queued cursor-icon/cursor-grab/window-mode/decorated/transparent/relative-mouse/effect/text-input requests"
    );

    engine.set_mouse_locked(surface, true);
    eprintln!("[window-ext] queued set_mouse_locked(true); verifying via diagnostics().window(0) from frame()");
}

/// Polled from `GameLogic::frame` until `demonstrate_window_mutation`'s `set_mouse_locked(true)`
/// request has visibly applied (via `Diagnostics::window`'s `mouse_locked` field), or a generous
/// frame budget runs out. Immediately queues `set_mouse_locked(false)` once confirmed, so the demo
/// doesn't leave the real OS mouse cursor locked for the rest of the process. Returns `true` once
/// done, so the caller can stop polling.
fn check_window_mutation_demo(engine: &mut Engine<'_>, surface: SurfaceHandle, frames_waited: u32) -> bool {
    let locked = engine.diagnostics().window(0).map(|info| info.mouse_locked).unwrap_or(false);
    if !locked {
        assert!(
            frames_waited < 300,
            "[window-ext] set_mouse_locked(true) never applied after {frames_waited} frames"
        );
        return false;
    }
    eprintln!("[window-ext] confirmed mouse_locked=true via diagnostics().window(0) after {frames_waited} frame(s)");
    engine.set_mouse_locked(surface, false);
    eprintln!("[window-ext] queued set_mouse_locked(false) to restore normal mouse behavior");
    eprintln!("[window-ext] all window-mutation demonstration checks passed");
    true
}

/// Per-frame results of `install_ui_draw_demo`'s draw hook, read back from `frame()`. Atomics
/// because the hook is a `Send + 'static` closure the engine calls on its own schedule.
struct UiDemoResults {
    frames: std::sync::atomic::AtomicU32,
    stroke_paths_ok: std::sync::atomic::AtomicBool,
    fill_quads_ok: std::sync::atomic::AtomicBool,
    fill_sectors_ok: std::sync::atomic::AtomicBool,
    stroke_custom_ok: std::sync::atomic::AtomicBool,
    /// Set once `scroll_metrics` reports the floating element at its declared 80x30 size.
    floating_laid_out: std::sync::atomic::AtomicBool,
    graph_line_drawn: std::sync::atomic::AtomicBool,
    graph_pie_drawn: std::sync::atomic::AtomicBool,
    /// `add_panel` results: a, b, c, then a duplicate "a" (expected true, true, true, false).
    dock_add_results: std::sync::Mutex<Vec<bool>>,
    /// Panels whose content region opened in the latest frame.
    dock_visible: std::sync::Mutex<Vec<String>>,
    /// Set once an element built inside a docked panel's content shows up in the layout.
    dock_content_laid_out: std::sync::atomic::AtomicBool,
}

/// Installs a UI draw hook exercising the raw UI draw primitives -- `Ui::stroke_paths`/
/// `stroke_polyline` (solid + dashed), `fill_quads` (rounded), `fill_sectors` (a donut chart), and
/// `stroke_custom` through the engine's own `Shaders/ui_stroke_custom_demo.slang` (a 16-byte
/// push-constant tail: `time` + 3 pads, after the engine's 48-byte stroke prefix). Each call's
/// success is recorded in the returned `UiDemoResults` for `check_ui_draw_demo` to assert on.
///
/// Logged with a `[ui-ext]` prefix. Pixel output isn't read back (the UI overlay renders into the
/// swapchain, not a texture this binding can copy from); what's checked is that every call reached
/// an open UI session and the engine accepted it, across several real frames.
fn install_ui_draw_demo() -> Arc<UiDemoResults> {
    use sturdy::ui::{
        AttachPoint, Border, ChildAlignment, Clip, CornerRadius, Cursor, CustomShader, ElementDesc,
        FillQuad, FloatTarget, Floating, GraphAxis, GraphDesc, GraphSeries, GraphType, PieSlice,
        Sector, Sizing, StrokeCap, StrokePath, StrokeStyle, DockPlacement, DockWorkspace, DockZone,
    };
    use std::sync::atomic::Ordering;

    let results = Arc::new(UiDemoResults {
        frames: std::sync::atomic::AtomicU32::new(0),
        stroke_paths_ok: std::sync::atomic::AtomicBool::new(false),
        fill_quads_ok: std::sync::atomic::AtomicBool::new(false),
        fill_sectors_ok: std::sync::atomic::AtomicBool::new(false),
        stroke_custom_ok: std::sync::atomic::AtomicBool::new(false),
        floating_laid_out: std::sync::atomic::AtomicBool::new(false),
        graph_line_drawn: std::sync::atomic::AtomicBool::new(false),
        graph_pie_drawn: std::sync::atomic::AtomicBool::new(false),
        dock_add_results: std::sync::Mutex::new(Vec::new()),
        dock_visible: std::sync::Mutex::new(Vec::new()),
        dock_content_laid_out: std::sync::atomic::AtomicBool::new(false),
    });
    let hook_results = results.clone();
    let start = Instant::now();

    // Docking: "a" and "b" share the root leaf as tabs ("b" becomes the visible one), "c" splits
    // off to the right of that leaf (node 0 is the root leaf of a fresh tree).
    let mut dock = DockWorkspace::new("ui-ext-dock");
    {
        let mut adds = results.dock_add_results.lock().unwrap();
        adds.push(dock.add_panel("dock-a", "A", true, None));
        adds.push(dock.add_panel("dock-b", "B", true, None));
        adds.push(dock.add_panel("dock-c", "C", false, Some(DockPlacement { target_node: 0, zone: DockZone::Right })));
        adds.push(dock.add_panel("dock-a", "A again", true, None));
    }

    sturdy::ui::install_draw_hook(move |ui| {
        let panel = ElementDesc::new()
            .id("ui-ext-panel".to_string())
            .width(Sizing::Fixed(220.0))
            .height(Sizing::Fixed(120.0))
            .align(ChildAlignment::center())
            .border(Border::all(glam::vec4(1.0, 1.0, 1.0, 0.6), 1))
            .cursor(Cursor::Pointer)
            .debug_label("ui-ext panel".to_string());

        let chart = ElementDesc::new().width(Sizing::Fixed(200.0)).height(Sizing::Fixed(100.0));
        let axis = StrokePath {
            points: vec![glam::vec2(0.0, 100.0), glam::vec2(200.0, 100.0)],
            style: StrokeStyle { color: glam::vec4(0.8, 0.8, 0.8, 1.0), width: 1.0, ..StrokeStyle::default() },
        };
        let series = StrokePath {
            points: vec![
                glam::vec2(0.0, 80.0),
                glam::vec2(50.0, 30.0),
                glam::vec2(100.0, 60.0),
                glam::vec2(150.0, 10.0),
                glam::vec2(200.0, 40.0),
            ],
            style: StrokeStyle { color: glam::vec4(0.2, 0.7, 1.0, 1.0), width: 2.0, ..StrokeStyle::default() },
        };
        let dashed = StrokePath {
            points: vec![glam::vec2(0.0, 50.0), glam::vec2(200.0, 50.0)],
            style: StrokeStyle {
                color: glam::vec4(1.0, 1.0, 1.0, 0.5),
                dash_length: 6.0,
                dash_gap: 4.0,
                cap: StrokeCap::Butt,
                ..StrokeStyle::default()
            },
        };

        let _panel_scope = ui.element(&panel);
        let stroked = ui.stroke_paths(&chart, &[axis, series, dashed]);
        let filled = ui.fill_quads(
            &chart,
            &[
                FillQuad {
                    position: glam::vec2(10.0, 10.0),
                    size: glam::vec2(40.0, 20.0),
                    color: glam::vec4(1.0, 0.3, 0.3, 0.9),
                    corner_radius: CornerRadius::all(4.0),
                },
                FillQuad {
                    position: glam::vec2(60.0, 10.0),
                    size: glam::vec2(40.0, 20.0),
                    color: glam::vec4(0.3, 1.0, 0.3, 0.9),
                    corner_radius: CornerRadius::default(),
                },
            ],
        );
        let tau = std::f32::consts::TAU;
        let sectors = ui.fill_sectors(
            &chart,
            &[
                Sector {
                    center: glam::vec2(150.0, 50.0),
                    inner_radius: 20.0,
                    outer_radius: 40.0,
                    start_angle: 0.0,
                    end_angle: tau * 0.6,
                    color: glam::vec4(1.0, 0.8, 0.2, 1.0),
                    ..Sector::default()
                },
                Sector {
                    center: glam::vec2(150.0, 50.0),
                    inner_radius: 20.0,
                    outer_radius: 40.0,
                    start_angle: tau * 0.6,
                    end_angle: tau,
                    color: glam::vec4(0.6, 0.3, 1.0, 1.0),
                    ..Sector::default()
                },
            ],
        );
        let time = start.elapsed().as_secs_f32();
        let push_constants: Vec<u8> = [time, 0.0, 0.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect();
        let custom = ui.stroke_custom(
            &chart,
            &[glam::vec2(10.0, 90.0), glam::vec2(100.0, 70.0), glam::vec2(190.0, 90.0)],
            3.0,
            1.0,
            &CustomShader {
                shader_path: "Shaders/ui_stroke_custom_demo.slang".to_string(),
                module_name: "ui_stroke_custom_demo".to_string(),
                fragment_entry_point: None,
                push_constants,
            },
        );

        // A floating, clipped badge anchored to the panel's top-right corner. Clipping makes it
        // visible to `scroll_metrics`, which only reports elements from the last *finished*
        // frame -- so a match proves the floating decl made it through layout.
        {
            let badge = ElementDesc::new()
                .id("ui-ext-float".to_string())
                .width(Sizing::Fixed(80.0))
                .height(Sizing::Fixed(30.0))
                .background(glam::vec4(0.1, 0.1, 0.1, 0.9))
                .clip(Clip { horizontal: false, vertical: true })
                .floating(Some(Floating {
                    element_attach_point: AttachPoint::RightBottom,
                    parent_attach_point: AttachPoint::RightTop,
                    offset: glam::vec2(0.0, -4.0),
                    z_index: 10,
                    ..Floating::new(FloatTarget::Element("ui-ext-panel".to_string()))
                }))
                .z(5);
            let _badge_scope = ui.element(&badge);
        }
        // The chart widget: a two-series line chart with a titled, fixed-range y axis, and a
        // donut pie chart.
        // Charts need a stable id: they size themselves from the previous frame's layout bounds.
        let chart_box = ElementDesc::new()
            .id("ui-ext-line-chart".to_string())
            .width(Sizing::Fixed(260.0))
            .height(Sizing::Fixed(140.0));
        let xs: Vec<f64> = (0..20).map(f64::from).collect();
        let line_drawn = ui.graph(
            &chart_box,
            &GraphDesc {
                graph_type: GraphType::Line,
                y_axis: GraphAxis { min: Some(-1.0), max: Some(1.0), title: "sin/cos".to_string(), ..GraphAxis::default() },
                series: vec![
                    GraphSeries { name: "sin".to_string(), x: xs.clone(), y: xs.iter().map(|x| (x * 0.3).sin()).collect(), ..GraphSeries::default() },
                    GraphSeries {
                        name: "cos".to_string(),
                        x: xs.clone(),
                        y: xs.iter().map(|x| (x * 0.3).cos()).collect(),
                        color: glam::vec4(1.0, 0.5, 0.2, 1.0),
                        marker_radius: 2.0,
                        ..GraphSeries::default()
                    },
                ],
                ..GraphDesc::default()
            },
        );
        let pie_drawn = ui.graph(
            &ElementDesc::new().id("ui-ext-pie-chart".to_string()).width(Sizing::Fixed(140.0)).height(Sizing::Fixed(140.0)),
            &GraphDesc {
                graph_type: GraphType::Pie,
                pie_hole_ratio: 0.5,
                pie_slices: vec![
                    PieSlice { name: "a".to_string(), value: 3.0, color: glam::vec4(0.3, 0.7, 1.0, 1.0) },
                    PieSlice { name: "b".to_string(), value: 1.0, color: glam::vec4(1.0, 0.6, 0.2, 1.0) },
                ],
                ..GraphDesc::default()
            },
        );
        hook_results.graph_line_drawn.store(line_drawn, Ordering::Relaxed);
        hook_results.graph_pie_drawn.store(pie_drawn, Ordering::Relaxed);

        if let Some(metrics) = ui.scroll_metrics("ui-ext-float") {
            if metrics.container_size == glam::vec2(80.0, 30.0) {
                hook_results.floating_laid_out.store(true, Ordering::Relaxed);
            }
        }

        dock.begin_frame(ui, glam::vec4(320.0, 40.0, 400.0, 220.0), 1.0 / 60.0);
        let mut visible = Vec::new();
        for id in ["dock-a", "dock-b", "dock-c"] {
            if let Some(_content) = dock.panel_content(ui, id) {
                visible.push(id.to_string());
                if id == "dock-c" {
                    let probe = ElementDesc::new()
                        .id("ui-ext-dock-probe".to_string())
                        .width(Sizing::Fixed(50.0))
                        .height(Sizing::Fixed(20.0))
                        .clip(Clip { horizontal: false, vertical: true });
                    let _probe = ui.element(&probe);
                }
            }
        }
        let _events = dock.end_frame(ui);
        *hook_results.dock_visible.lock().unwrap() = visible;
        if ui.scroll_metrics("ui-ext-dock-probe").is_some_and(|m| m.container_size == glam::vec2(50.0, 20.0)) {
            hook_results.dock_content_laid_out.store(true, Ordering::Relaxed);
        }

        hook_results.stroke_paths_ok.store(stroked, Ordering::Relaxed);
        hook_results.fill_quads_ok.store(filled, Ordering::Relaxed);
        hook_results.fill_sectors_ok.store(sectors, Ordering::Relaxed);
        hook_results.stroke_custom_ok.store(custom, Ordering::Relaxed);
        hook_results.frames.fetch_add(1, Ordering::Relaxed);
    });
    eprintln!("[ui-ext] installed UI draw hook (stroke_paths/fill_quads/fill_sectors/stroke_custom)");
    results
}

/// Returns `true` once the UI draw hook has run for a few frames with every primitive accepted.
fn check_ui_draw_demo(results: &UiDemoResults, frames_waited: u32) -> bool {
    use std::sync::atomic::Ordering;
    let frames = results.frames.load(Ordering::Relaxed);
    if frames < 5 {
        assert!(frames_waited < 600, "[ui-ext] UI draw hook ran only {frames} time(s) in {frames_waited} frames");
        return false;
    }
    let stroked = results.stroke_paths_ok.load(Ordering::Relaxed);
    let filled = results.fill_quads_ok.load(Ordering::Relaxed);
    let sectors = results.fill_sectors_ok.load(Ordering::Relaxed);
    let custom = results.stroke_custom_ok.load(Ordering::Relaxed);
    eprintln!(
        "[ui-ext] after {frames} UI frames: stroke_paths={stroked} fill_quads={filled} fill_sectors={sectors} stroke_custom={custom} (expected all true)"
    );
    assert!(stroked && filled && sectors && custom, "every raw UI draw call must be accepted by an open UI session");
    let floating = results.floating_laid_out.load(Ordering::Relaxed);
    eprintln!("[ui-ext] floating badge (attached to the panel by id) laid out at its declared 80x30 = {floating} (expected true)");
    assert!(floating, "the floating element must appear in the finished layout at its declared size");
    let line = results.graph_line_drawn.load(Ordering::Relaxed);
    let pie = results.graph_pie_drawn.load(Ordering::Relaxed);
    eprintln!("[ui-ext] graph widget: line chart drawn={line} donut pie drawn={pie} (expected true, true)");
    assert!(line && pie, "the chart widget must report both charts drawn");
    let adds = results.dock_add_results.lock().unwrap().clone();
    let visible = results.dock_visible.lock().unwrap().clone();
    let content = results.dock_content_laid_out.load(Ordering::Relaxed);
    eprintln!(
        "[ui-ext] docking: add_panel(a, b, c, duplicate a) = {adds:?} (expected [true, true, true, false]); visible panels = {visible:?} (expected [dock-b, dock-c]); content inside dock-c laid out = {content}"
    );
    assert_eq!(adds, vec![true, true, true, false]);
    assert_eq!(visible, vec!["dock-b".to_string(), "dock-c".to_string()], "one visible tab per leaf: b (added last) in the left leaf, c in the split");
    assert!(content, "an element built inside a docked panel's content region must reach the layout");
    eprintln!("[ui-ext] all UI-draw demonstration checks passed");
    true
}

struct Spinner {
    yaw: f32,
    rhi_demo_done: bool,
    pipeline_demo_done: bool,
    assets_demo_done: bool,
    schedule_demo_checked: bool,
    schedule_demo_frames_waited: u32,
    schedule_targets: Option<(Entity, Entity)>,
    window_demo_queued: bool,
    window_demo_checked: bool,
    window_demo_frames_waited: u32,
    ui_demo: Option<Arc<UiDemoResults>>,
    ui_demo_checked: bool,
    ui_demo_frames_waited: u32,
}

/// Demo component: a world-space position. `#[repr(C)]` + `bytemuck::Pod`/`Zeroable` is what makes
/// it eligible for `Component` — no padding-sensitive layout, no pointers.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Position {
    x: f32,
    y: f32,
    z: f32,
}

impl sturdy::Component for Position {
    const NAME: &'static str = "hello.demo.position";
}

/// Demo component: a linear velocity, kept separate from `Position` to exercise multi-component
/// spawns and `World::query::<(Position, Velocity)>()`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Velocity {
    x: f32,
    y: f32,
    z: f32,
}

impl sturdy::Component for Velocity {
    const NAME: &'static str = "hello.demo.velocity";
}

/// Exercises the typed ECS API end to end against the live engine and logs a pass/fail trail via
/// `eprintln!`. This has never run against the real engine before, so every step is checked rather
/// than assumed to work.
fn demonstrate_ecs(engine: &mut Engine<'_>) {
    let mut ecs = engine.ecs();

    // Registration is idempotent: calling it up front is optional (typed spawn/insert helpers
    // register on demand), but doing it explicitly here lets `find` and `query_*` succeed even
    // before any entity carries the component.
    let position_id = ecs.register::<Position>().expect("register Position");
    let velocity_id = ecs.register::<Velocity>().expect("register Velocity");
    eprintln!("ecs: registered Position={position_id:?} Velocity={velocity_id:?}");

    // spawn_one: a single-component entity.
    let a = ecs.spawn_one(Position { x: 1.0, y: 2.0, z: 3.0 }).expect("spawn_one Position");
    eprintln!("ecs: spawned entity a={a:?} with Position only");

    // spawn_builder: a multi-component entity in one call.
    let b = ecs
        .spawn_builder()
        .with(Position { x: 10.0, y: 20.0, z: 30.0 })
        .with(Velocity { x: 0.0, y: -9.8, z: 0.0 })
        .build()
        .expect("spawn_builder Position+Velocity");
    eprintln!("ecs: spawned entity b={b:?} with Position+Velocity");

    // get: read back exactly what was written.
    let a_pos = ecs.get::<Position>(a).expect("get Position on a");
    assert_eq!(a_pos, Position { x: 1.0, y: 2.0, z: 3.0 });
    let b_pos = ecs.get::<Position>(b).expect("get Position on b");
    assert_eq!(b_pos, Position { x: 10.0, y: 20.0, z: 30.0 });
    let b_vel = ecs.get::<Velocity>(b).expect("get Velocity on b");
    assert_eq!(b_vel, Velocity { x: 0.0, y: -9.8, z: 0.0 });
    eprintln!("ecs: read-back matches what was written for a and b");

    // has / insert / remove on entity a, which started with only Position.
    assert!(!ecs.has::<Velocity>(a), "a should not have Velocity yet");
    ecs.insert(a, Velocity { x: 1.0, y: 0.0, z: 0.0 }).expect("insert Velocity on a");
    assert!(ecs.has::<Velocity>(a), "a should have Velocity after insert");
    let a_vel = ecs.get::<Velocity>(a).expect("get Velocity on a after insert");
    assert_eq!(a_vel, Velocity { x: 1.0, y: 0.0, z: 0.0 });
    eprintln!("ecs: insert/has round-trip on a succeeded");

    // set: overwrite in place.
    ecs.set(a, Position { x: 100.0, y: 200.0, z: 300.0 }).expect("set Position on a");
    assert_eq!(ecs.get::<Position>(a).expect("get Position after set"), Position { x: 100.0, y: 200.0, z: 300.0 });
    eprintln!("ecs: set/get round-trip on a succeeded");

    // remove: a goes back to Position-only.
    ecs.remove::<Velocity>(a).expect("remove Velocity from a");
    assert!(!ecs.has::<Velocity>(a), "a should not have Velocity after remove");
    eprintln!("ecs: remove Velocity from a succeeded");

    // query::<(Position,)>: both a and b carry Position.
    let positions = ecs.query::<(Position,)>().expect("query::<(Position,)>");
    assert_eq!(positions.len(), 2, "expected both entities in query::<(Position,)>");
    assert!(positions.iter().any(|(e, (p,))| *e == a && *p == Position { x: 100.0, y: 200.0, z: 300.0 }));
    assert!(positions.iter().any(|(e, (p,))| *e == b && *p == Position { x: 10.0, y: 20.0, z: 30.0 }));
    eprintln!("ecs: query::<(Position,)> returned {} entities as expected", positions.len());

    // query::<(Position, Velocity)>: only b carries both (a's Velocity was removed above).
    let both = ecs.query::<(Position, Velocity)>().expect("query::<(Position, Velocity)>");
    assert_eq!(both.len(), 1, "expected only b in query::<(Position, Velocity)>");
    assert_eq!(both[0].0, b);
    eprintln!("ecs: query::<(Position, Velocity)> returned exactly entity b as expected");

    // Error path: `get` on a component the entity doesn't carry must come back as `EcsError`, not
    // panic.
    match ecs.get::<Velocity>(a) {
        Err(EcsError::MissingComponent) => {
            eprintln!("ecs: get::<Velocity>(a) correctly reported MissingComponent after remove");
        }
        other => panic!("expected MissingComponent, got {other:?}"),
    }

    // Error path: any lookup on a despawned entity must come back as `EcsError::DeadEntity`.
    ecs.despawn(a);
    match ecs.get::<Position>(a) {
        Err(EcsError::DeadEntity) => {
            eprintln!("ecs: get::<Position>(a) correctly reported DeadEntity after despawn");
        }
        other => panic!("expected DeadEntity, got {other:?}"),
    }

    eprintln!("ecs: all typed ECS demonstration checks passed");
}

/// Demo component: hit points, used only to give the generic `World::query` demo below a third
/// component type (beyond `Position`/`Velocity`) and to give `PlayerBundle` something besides a
/// transform.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Health {
    current: f32,
    max: f32,
}

impl sturdy::Component for Health {
    const NAME: &'static str = "hello.demo.health";
}

/// Demo bundle: exercises `#[derive(Bundle)]` / `World::spawn::<PlayerBundle>`. Every field must
/// implement `Component` — the derive walks the struct's fields and registers/spawns each one as
/// its own component, exactly as chaining `SpawnBuilder::with` calls would.
#[derive(Debug, Clone, Copy, Bundle)]
struct PlayerBundle {
    position: Position,
    velocity: Velocity,
    health: Health,
}

/// Demo event: exercises the typed event-channel API (`World::create_event_channel::<T>`,
/// `send_event`, `read_events`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct DamageEvent {
    amount: f32,
}

impl sturdy::Event for DamageEvent {
    const NAME: &'static str = "hello.demo.damage_event";
}

/// Demo resource: exercises the typed resource API (`World::insert_resource`, `resource`,
/// `set_resource_typed`).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Score {
    value: u32,
}

impl sturdy::Resource for Score {
    const NAME: &'static str = "hello.demo.score";
}

/// Exercises the four ECS extensions added on top of `demonstrate_ecs` above: the fully generic
/// `World::query`, `#[derive(Bundle)]` + `World::spawn`, typed event channels (including the
/// drain-on-read clear semantics), and typed resources. Logged with an `[ecs-ext]` prefix to keep
/// it easy to pick out from `demonstrate_ecs`'s own `ecs:`-prefixed lines.
/// Twelve distinct one-field components (and resources), for exercising the widest
/// `QueryTuple`/`SystemTuple`/`ResourceTuple` impls.
macro_rules! wide_components {
    ($($name:ident = $label:literal),+ $(,)?) => {
        $(
            #[repr(C)]
            #[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
            struct $name(u32);
            impl sturdy::Component for $name {
                const NAME: &'static str = $label;
            }
            impl sturdy::Resource for $name {
                const NAME: &'static str = $label;
            }
        )+
    };
}

wide_components!(
    Wide0 = "hello.wide0", Wide1 = "hello.wide1", Wide2 = "hello.wide2", Wide3 = "hello.wide3",
    Wide4 = "hello.wide4", Wide5 = "hello.wide5", Wide6 = "hello.wide6", Wide7 = "hello.wide7",
    Wide8 = "hello.wide8", Wide9 = "hello.wide9", Wide10 = "hello.wide10", Wide11 = "hello.wide11",
);

fn demonstrate_ecs_ext(engine: &mut Engine<'_>) {
    let mut ecs = engine.ecs();

    // -- Task 1: generic World::query over 3+ component types -----------------------------------
    //
    // Three entities: one with all of Position+Velocity+Health, one missing Health, one missing
    // Velocity, so the 3-way query below must return exactly the first.
    let full = ecs
        .spawn_builder()
        .with(Position { x: 1.0, y: 2.0, z: 3.0 })
        .with(Velocity { x: 0.1, y: 0.2, z: 0.3 })
        .with(Health { current: 50.0, max: 100.0 })
        .build()
        .expect("spawn full entity");
    ecs.spawn_builder()
        .with(Position { x: 4.0, y: 5.0, z: 6.0 })
        .with(Velocity { x: 0.0, y: 0.0, z: 0.0 })
        .build()
        .expect("spawn Position+Velocity-only entity");
    ecs.spawn_builder()
        .with(Position { x: 7.0, y: 8.0, z: 9.0 })
        .with(Health { current: 10.0, max: 10.0 })
        .build()
        .expect("spawn Position+Health-only entity");

    let triples = ecs
        .query::<(Position, Velocity, Health)>()
        .expect("generic query over Position+Velocity+Health");
    assert_eq!(triples.len(), 1, "expected exactly the full entity to match the 3-way query");
    assert_eq!(triples[0].0, full);
    assert_eq!(triples[0].1 .2, Health { current: 50.0, max: 100.0 });
    eprintln!(
        "[ecs-ext] World::query::<(Position, Velocity, Health)>() returned exactly the expected entity ({} match)",
        triples.len()
    );

    // -- 12-wide tuples ---------------------------------------------------------------------------
    type Wide = (Wide0, Wide1, Wide2, Wide3, Wide4, Wide5, Wide6, Wide7, Wide8, Wide9, Wide10, Wide11);
    fn assert_system_tuple<Q: sturdy::schedule::SystemTuple>() {}
    fn assert_resource_tuple<Q: sturdy::schedule::ResourceTuple>() {}
    assert_system_tuple::<(R<Wide0>, W<Wide1>, R<Wide2>, R<Wide3>, R<Wide4>, R<Wide5>, R<Wide6>, R<Wide7>, R<Wide8>, R<Wide9>, R<Wide10>, W<Wide11>)>();
    assert_resource_tuple::<(R<Wide0>, W<Wide1>, R<Wide2>, R<Wide3>, R<Wide4>, R<Wide5>, R<Wide6>, R<Wide7>, R<Wide8>, R<Wide9>, R<Wide10>, W<Wide11>)>();
    let wide_entity = ecs
        .spawn_builder()
        .with(Wide0(0)).with(Wide1(1)).with(Wide2(2)).with(Wide3(3)).with(Wide4(4)).with(Wide5(5))
        .with(Wide6(6)).with(Wide7(7)).with(Wide8(8)).with(Wide9(9)).with(Wide10(10)).with(Wide11(11))
        .build()
        .expect("spawn 12-component entity");
    let wide = ecs.query::<Wide>().expect("12-wide query");
    assert_eq!(wide.len(), 1);
    assert_eq!(wide[0].0, wide_entity);
    let row = wide[0].1;
    let values = [row.0 .0, row.1 .0, row.2 .0, row.3 .0, row.4 .0, row.5 .0, row.6 .0, row.7 .0, row.8 .0, row.9 .0, row.10 .0, row.11 .0];
    eprintln!("[ecs-ext] World::query over a 12-tuple -> {values:?}");
    assert_eq!(values, core::array::from_fn(|i| i as u32));

    let player = ecs
        .spawn(PlayerBundle {
            position: Position { x: 0.0, y: 0.0, z: 0.0 },
            velocity: Velocity { x: 1.0, y: 0.0, z: 0.0 },
            health: Health { current: 100.0, max: 100.0 },
        })
        .expect("spawn PlayerBundle via World::spawn");
    let player_health = ecs.get::<Health>(player).expect("get Health on player");
    assert_eq!(player_health, Health { current: 100.0, max: 100.0 });
    eprintln!("[ecs-ext] World::spawn::<PlayerBundle> spawned {player:?}; Health read back matches");

    // -- Task 3: typed event channels, including drain-on-read clear semantics ------------------
    let channel = ecs.create_event_channel::<DamageEvent>().expect("create DamageEvent channel");
    ecs.send_event(channel, DamageEvent { amount: 5.0 }).expect("send DamageEvent 1");
    ecs.send_event(channel, DamageEvent { amount: 12.5 }).expect("send DamageEvent 2");
    ecs.send_event(channel, DamageEvent { amount: 3.25 }).expect("send DamageEvent 3");

    let first_read = ecs.read_events::<DamageEvent>(channel).expect("read DamageEvent batch 1");
    assert_eq!(
        first_read,
        vec![DamageEvent { amount: 5.0 }, DamageEvent { amount: 12.5 }, DamageEvent { amount: 3.25 }],
        "expected all 3 events back in send order"
    );
    eprintln!("[ecs-ext] read_events (1st call): {first_read:?} (count={})", first_read.len());

    // This binding layer never runs the engine's own Ecs::Schedule (see World::read_events_bytes's
    // doc comment), so nothing else clears the channel between calls — read_events itself drains
    // it. A second read right after the first, with no send in between, must therefore come back
    // empty rather than repeating the same events.
    let second_read = ecs.read_events::<DamageEvent>(channel).expect("read DamageEvent batch 2");
    assert!(second_read.is_empty(), "expected an empty batch: read_events drains the channel");
    eprintln!(
        "[ecs-ext] read_events (2nd call, no sends in between): {second_read:?} (count={}) -- confirms drain-on-read",
        second_read.len()
    );

    // -- Task 4: typed resources ------------------------------------------------------------------
    let score_id = ecs.insert_resource(Score { value: 0 }).expect("insert Score resource");
    let score = ecs.resource::<Score>(score_id).expect("read Score resource");
    assert_eq!(score, Score { value: 0 });
    ecs.set_resource_typed(score_id, Score { value: 42 }).expect("update Score resource");
    let updated_score = ecs.resource::<Score>(score_id).expect("read updated Score resource");
    assert_eq!(updated_score, Score { value: 42 });
    eprintln!(
        "[ecs-ext] typed resource Score: inserted={score:?}, after set_resource_typed={updated_score:?}"
    );

    eprintln!("[ecs-ext] all ECS-extension demonstration checks passed");
}

/// Demo resource: counts how many times the schedule demo's global system has ticked. Also gates
/// the one-shot `Commands` calls below to that system's very first dispatch.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct ScheduleCounters {
    ticks: u32,
}

impl sturdy::Resource for ScheduleCounters {
    const NAME: &'static str = "hello.demo.schedule_counters";
}

/// Registers a real, engine-scheduler-driven system (`sturdy::schedule`) and exercises every
/// `Commands` operation (`spawn_one`, `insert`/`add_component`, `remove_component`, `destroy`)
/// from inside its body, on the system's first tick. Called from `GameLogic::init`; the actual
/// assertions run later from `GameLogic::frame` (see `check_schedule_and_commands_demo`) once the
/// Update schedule has actually had a chance to run and apply the queued commands (`Commands`
/// changes only take effect once `Schedule::run` finishes the whole dispatch, so they cannot be
/// observed synchronously here in `init`).
///
/// Returns the two entities `frame()` needs to check: one `Commands::destroy` should remove, one
/// `Commands::insert`/`remove_component` should mutate.
fn demonstrate_schedule_and_commands(engine: &mut Engine<'_>) -> (Entity, Entity) {
    let mut ecs = engine.ecs();

    let target_for_destroy =
        ecs.spawn_one(Health { current: 1.0, max: 1.0 }).expect("spawn target_for_destroy");
    let target_for_components =
        ecs.spawn_one(Health { current: 2.0, max: 2.0 }).expect("spawn target_for_components");

    // Resolved once up front and captured by value into the system closure: a `Commands` cannot
    // resolve a `ComponentId` itself (see `sturdy::schedule`'s module doc for why), so this is the
    // same pattern any real caller needing `Commands::insert`/`spawn_one` inside a system would use.
    let health_id = ecs.register::<Health>().expect("register Health");
    let velocity_id = ecs.register::<Velocity>().expect("register Velocity");

    ecs.insert_resource(ScheduleCounters { ticks: 0 }).expect("insert ScheduleCounters resource");

    ecs.add_global_system::<(W<ScheduleCounters>,), _>(ScheduleTarget::Update, move |(counters,), commands| {
        if counters.ticks == 0 {
            commands.destroy(target_for_destroy);
            commands.spawn_one(health_id, Health { current: 7.0, max: 7.0 });
            commands.insert(target_for_components, velocity_id, Velocity { x: 9.0, y: 8.0, z: 7.0 });
            commands.remove_component(target_for_components, health_id);
        }
        counters.ticks += 1;
    })
    .expect("register schedule-demo global system");

    eprintln!(
        "[schedule-ext] registered a global system + pre-spawned demo entities; \
         Commands results are checked from frame() once the schedule has actually run"
    );
    (target_for_destroy, target_for_components)
}

/// Polled from `GameLogic::frame` until every `Commands` operation queued by
/// `demonstrate_schedule_and_commands` has visibly applied (or a generous frame budget runs out,
/// in which case it panics rather than silently never checking). Returns `true` once fully
/// verified, so the caller can stop polling.
fn check_schedule_and_commands_demo(
    engine: &mut Engine<'_>,
    target_for_destroy: Entity,
    target_for_components: Entity,
    frames_waited: u32,
) -> bool {
    let mut ecs = engine.ecs();

    let destroyed = !ecs.is_alive(target_for_destroy);
    let spawned = ecs.query::<(Health,)>().expect("query::<(Health,)> in schedule-ext check");
    let spawned_via_commands = spawned.iter().any(|(_, (h,))| *h == Health { current: 7.0, max: 7.0 });
    let has_velocity = ecs.has::<Velocity>(target_for_components);
    let has_health = ecs.has::<Health>(target_for_components);

    if !(destroyed && spawned_via_commands && has_velocity && !has_health) {
        assert!(
            frames_waited < 300,
            "[schedule-ext] Commands never applied after {frames_waited} frames: \
             destroyed={destroyed} spawned_via_commands={spawned_via_commands} \
             has_velocity={has_velocity} has_health={has_health}"
        );
        return false;
    }

    eprintln!(
        "[schedule-ext] Commands verified after {frames_waited} frame(s): target_for_destroy alive=false, \
         Commands::spawn_one created a Health{{current:7,max:7}} entity, target_for_components gained \
         Velocity (Commands::insert) and lost Health (Commands::remove_component)"
    );
    eprintln!("[schedule-ext] all Commands demonstration checks passed");
    true
}

/// Exercises the reflection-extension surface added on top of the original read-only type/field/
/// enum descriptors (`sturdy::reflection`): live instance field read/write, default construction,
/// zero-argument method invocation, container/optional element introspection, attributes, and
/// event descriptors. Logged with a `[reflection-ext]` prefix.
///
/// No engine-defined gameplay type is reflected by default (the vendored engine reflects only its
/// own internals), and the `Position`/`Velocity`/... structs above are plain Rust ECS components,
/// never registered with `SFT::Reflection::TypeRegistry` -- so this exercises
/// `reflection::register_demo_type()`'s tiny C++ demo type (`"sturdy_rs.demo.actor"`, declared in
/// `sturdy-sys/cpp/sturdy_rs/reflection.cpp`) instead. It has trivial float/int fields, a
/// `std::vector<int>` field, a `std::optional<int>` field, a zero-arg method, a non-zero-arg method
/// (descriptor only), an attributed field, and a declared event -- enough to cover every piece of
/// the new surface end to end.
fn demonstrate_reflection() {
    const TYPE_NAME: &str = "sturdy_rs.demo.actor";

    reflection::register_demo_type();

    let info = reflection::find_type(TYPE_NAME).expect("sturdy_rs.demo.actor must be registered");
    eprintln!(
        "[reflection-ext] find_type({TYPE_NAME:?}) -> size={} align={} field_count={} method_count={} has_default_constructor={}",
        info.size, info.align, info.field_count, info.method_count, info.has_default_constructor
    );
    assert!(info.has_default_constructor, "DemoActor must be default-constructible");

    let fields = reflection::fields(TYPE_NAME);
    let field_names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    eprintln!("[reflection-ext] fields: {field_names:?}");
    assert!(field_names.contains(&"x"));
    assert!(field_names.contains(&"health"));

    // -- Constructors: default-construct a real instance -----------------------------------------
    let mut instance = reflection::construct_default(TYPE_NAME).expect("default construction must succeed");
    eprintln!("[reflection-ext] default-constructed a {} byte instance", instance.bytes().len());

    let constructor_list = reflection::constructors(TYPE_NAME);
    eprintln!(
        "[reflection-ext] parameterized constructors: {} (expected 0 -- DemoActor declares none via SFT_REFLECT_CONSTRUCTOR)",
        constructor_list.len()
    );
    assert!(constructor_list.is_empty());

    // -- Live field read/write, cross-checked through a *different* code path (method invocation) -
    //
    // Reading the field back through reflection after writing it only proves the write and the
    // read agree with each other -- not that it actually landed in the instance's real memory.
    // Invoking `double_health` (a compiled C++ method reading `this->health` directly) after
    // writing `health` through reflection is an independent confirmation: if the write hadn't
    // really landed, the compiled method would still see the old value.
    let initial_x: f32 = reflection::read_field_as(&fields, "x", instance.bytes()).expect("read x");
    let initial_health: i32 = reflection::read_field_as(&fields, "health", instance.bytes()).expect("read health");
    eprintln!("[reflection-ext] initial x={initial_x}, health={initial_health} (expected 0, 100 -- the C++ default member initializers)");
    assert_eq!(initial_x, 0.0);
    assert_eq!(initial_health, 100);

    reflection::write_field_value::<f32>(&fields, "x", instance.bytes_mut(), 42.5).expect("write x");
    let read_back_x: f32 = reflection::read_field_as(&fields, "x", instance.bytes()).expect("read x after write");
    eprintln!("[reflection-ext] wrote x=42.5, read back x={read_back_x}");
    assert_eq!(read_back_x, 42.5);

    reflection::write_field_value::<i32>(&fields, "health", instance.bytes_mut(), 55).expect("write health");

    // -- Methods: enumerate, then invoke the zero-arg one and confirm it saw the write above ------
    let method_list = reflection::methods(TYPE_NAME);
    eprintln!(
        "[reflection-ext] methods: {:?}",
        method_list.iter().map(|m| (m.name.as_str(), m.param_type_names.len(), m.return_type_name.as_str())).collect::<Vec<_>>()
    );
    assert!(method_list.iter().any(|m| m.name == "double_health" && m.param_type_names.is_empty()));
    assert!(method_list.iter().any(|m| m.name == "heal" && m.param_type_names.len() == 1));

    let doubled = reflection::invoke_method0(TYPE_NAME, "double_health", instance.bytes_mut()).expect("invoke double_health");
    eprintln!("[reflection-ext] invoke_method0(double_health) -> {doubled:?} (expected SignedInt(110) = 2 * the health=55 written above)");
    assert_eq!(doubled, reflection::MethodValue::SignedInt(110));

    // Read health back through reflection too, confirming both paths (the compiled method call
    // above, and a direct offset re-read here) agree with what was written.
    let health_after: i32 = reflection::read_field_as(&fields, "health", instance.bytes()).expect("read health after write");
    assert_eq!(health_after, 55);
    eprintln!("[reflection-ext] health re-read via reflection after the write = {health_after} -- matches what the method call saw");

    // -- Container/optional element introspection --------------------------------------------------
    let tags_shape = reflection::field_container(TYPE_NAME, "tags").expect("tags must be a container");
    eprintln!("[reflection-ext] field_container(tags) -> {tags_shape:?}");
    assert_eq!(tags_shape.kind, reflection::ContainerKind::Vector);
    assert_eq!(tags_shape.element_type_name.as_deref(), Some("i32"));

    let target_shape = reflection::field_container(TYPE_NAME, "target").expect("target must be an optional");
    eprintln!("[reflection-ext] field_container(target) -> {target_shape:?}");
    assert_eq!(target_shape.kind, reflection::ContainerKind::Optional);
    assert_eq!(target_shape.element_type_name.as_deref(), Some("i32"));

    assert!(reflection::field_container(TYPE_NAME, "x").is_none(), "a trivial field has no container shape");

    // -- Attributes -----------------------------------------------------------------------------
    let health_attributes = reflection::field_attributes(TYPE_NAME, "health");
    eprintln!("[reflection-ext] health field attributes: {health_attributes:?}");
    assert!(health_attributes.iter().any(|a| a.name == "units" && a.value == reflection::AttributeValue::String("hp".to_owned())));
    assert!(health_attributes.iter().any(|a| a.name == "min" && a.value == reflection::AttributeValue::SignedInt(0)));

    // -- Events: enumerate, look up by name, and fire (no subscriber, so a defined no-op) ---------
    let event_list = reflection::events(TYPE_NAME);
    eprintln!("[reflection-ext] events: {:?}", event_list.iter().map(|e| e.name.as_str()).collect::<Vec<_>>());
    assert!(event_list.iter().any(|e| e.name == "on_died"));

    let on_died = reflection::find_event(TYPE_NAME, "on_died").expect("find_event(on_died)");
    eprintln!("[reflection-ext] find_event(on_died) -> {on_died:?}");
    assert!(on_died.param_type_names.is_empty(), "on_died is declared with no parameters");
    assert!(reflection::find_event(TYPE_NAME, "no_such_event").is_none());

    let fired = reflection::fire_event(TYPE_NAME, "on_died", instance.bytes_mut(), &[]);
    eprintln!("[reflection-ext] fire_event(on_died) -> {fired} (expected true: arity matches, even with no subscriber)");
    assert!(fired);
    assert!(
        !reflection::fire_event(TYPE_NAME, "on_died", instance.bytes_mut(), &[&[0u8; 4]]),
        "firing with the wrong argument count must fail closed rather than fire partially"
    );

    // -- Static fields: independent of any instance's bytes -----------------------------------------
    let initial_spawn_count: i32 =
        reflection::get_static_field_as(&fields, TYPE_NAME, "spawn_count").expect("read spawn_count");
    eprintln!("[reflection-ext] static field spawn_count = {initial_spawn_count} (expected 0, the C++ static initializer)");
    assert_eq!(initial_spawn_count, 0);

    reflection::set_static_field_value::<i32>(&fields, TYPE_NAME, "spawn_count", 7).expect("write spawn_count");
    let updated_spawn_count: i32 =
        reflection::get_static_field_as(&fields, TYPE_NAME, "spawn_count").expect("read spawn_count after write");
    eprintln!("[reflection-ext] static field spawn_count after write = {updated_spawn_count}");
    assert_eq!(updated_spawn_count, 7);

    // A second, independently default-constructed instance must see the *same* static storage --
    // proof this isn't accidentally reading/writing per-instance bytes.
    let other_instance = reflection::construct_default(TYPE_NAME).expect("construct second DemoActor");
    let shared_spawn_count: i32 = reflection::get_static_field_as(&fields, TYPE_NAME, "spawn_count")
        .expect("read spawn_count via a fresh lookup");
    eprintln!(
        "[reflection-ext] spawn_count read again after constructing a second, unrelated instance = {shared_spawn_count} (expected still 7 -- static storage is shared, not per-instance)"
    );
    assert_eq!(shared_spawn_count, 7);
    drop(other_instance);

    // -- Container element access: resize/set/get/len on the `tags` field (Vec<i32>) --------------
    assert_eq!(reflection::container_len(TYPE_NAME, "tags", instance.bytes()), 0, "tags starts empty");
    assert!(
        reflection::container_resize(TYPE_NAME, "tags", instance.bytes_mut(), 3),
        "resize tags to 3 elements"
    );
    assert_eq!(reflection::container_len(TYPE_NAME, "tags", instance.bytes()), 3);
    for (index, value) in [10_i32, 20, 30].into_iter().enumerate() {
        assert!(
            reflection::container_set_element_value(TYPE_NAME, "tags", instance.bytes_mut(), index, value),
            "set tags[{index}]"
        );
    }
    let tags_read_back: Vec<i32> = (0..3)
        .map(|index| {
            reflection::container_get_element_as::<i32>(TYPE_NAME, "tags", instance.bytes(), index)
                .unwrap_or_else(|| panic!("get tags[{index}]"))
        })
        .collect();
    eprintln!("[reflection-ext] tags after resize(3) + set_element(0..3, [10,20,30]) -> {tags_read_back:?}");
    assert_eq!(tags_read_back, vec![10, 20, 30]);
    assert!(
        reflection::container_get_element_as::<i32>(TYPE_NAME, "tags", instance.bytes(), 3).is_none(),
        "index 3 is out of range for a 3-element container"
    );

    // -- Overrides, hooks, and event subscription ----------------------------------------------
    {
        use std::sync::atomic::{AtomicU32, Ordering};
        let hook_calls = Arc::new(AtomicU32::new(0));
        let health = fields.iter().find(|f| f.name == "health").expect("health field").clone();

        let before = {
            let hook_calls = hook_calls.clone();
            reflection::add_method_hook(TYPE_NAME, "double_health", reflection::HookTiming::Before, move |_| {
                hook_calls.fetch_add(1, Ordering::SeqCst);
            })
            .expect("add before hook")
        };
        let after = {
            let hook_calls = hook_calls.clone();
            reflection::add_method_hook(TYPE_NAME, "double_health", reflection::HookTiming::After, move |_| {
                hook_calls.fetch_add(1, Ordering::SeqCst);
            })
            .expect("add after hook")
        };
        // The override reads `health` straight out of the receiver bytes and returns 3x instead of
        // 2x -- proving both the receiver slice and the return slot are wired to the real call.
        let overridden = reflection::override_method(TYPE_NAME, "double_health", move |call| {
            let object = call.object.as_deref().expect("instance method has a receiver");
            let value = i32::from_ne_bytes(object[health.offset..health.offset + 4].try_into().unwrap());
            call.ret.as_deref_mut().expect("i32 return slot").copy_from_slice(&(value * 3).to_ne_bytes());
        })
        .expect("override double_health");

        let with_override = reflection::invoke_method0(TYPE_NAME, "double_health", instance.bytes_mut()).expect("invoke overridden");
        let calls_after_one = hook_calls.load(Ordering::SeqCst);
        eprintln!(
            "[reflection-ext] override_method(double_health -> 3x) invoke = {with_override:?} (expected SignedInt(165) = 3 * 55); before+after hook calls = {calls_after_one} (expected 2)"
        );
        assert_eq!(with_override, reflection::MethodValue::SignedInt(165));
        assert_eq!(calls_after_one, 2);

        drop(overridden);
        let restored = reflection::invoke_method0(TYPE_NAME, "double_health", instance.bytes_mut()).expect("invoke restored");
        eprintln!("[reflection-ext] after dropping the override: {restored:?} (expected SignedInt(110), the original body)");
        assert_eq!(restored, reflection::MethodValue::SignedInt(110));

        drop(before);
        drop(after);
        let calls_before_last = hook_calls.load(Ordering::SeqCst);
        let _ = reflection::invoke_method0(TYPE_NAME, "double_health", instance.bytes_mut()).expect("invoke unhooked");
        let calls_final = hook_calls.load(Ordering::SeqCst);
        eprintln!("[reflection-ext] hook calls after dropping both hooks: {calls_before_last} -> {calls_final} (expected unchanged)");
        assert_eq!(calls_before_last, 4, "the restored call must still have run both hooks");
        assert_eq!(calls_final, calls_before_last, "dropped hooks must not run");

        let fired = Arc::new(AtomicU32::new(0));
        let subscription = {
            let fired = fired.clone();
            reflection::subscribe_event(TYPE_NAME, "on_died", move |call| {
                assert!(call.object.is_some() && call.args.is_empty());
                fired.fetch_add(1, Ordering::SeqCst);
            })
            .expect("subscribe on_died")
        };
        assert!(reflection::fire_event(TYPE_NAME, "on_died", instance.bytes_mut(), &[]));
        let fired_once = fired.load(Ordering::SeqCst);
        drop(subscription);
        assert!(reflection::fire_event(TYPE_NAME, "on_died", instance.bytes_mut(), &[]));
        let fired_final = fired.load(Ordering::SeqCst);
        eprintln!("[reflection-ext] subscribe_event(on_died): fires seen = {fired_once}, after unsubscribing = {fired_final} (expected 1, 1)");
        assert_eq!((fired_once, fired_final), (1, 1));

        assert!(matches!(
            reflection::override_method(TYPE_NAME, "no_such_method", |_| {}),
            Err(reflection::HookError::NotFound)
        ));
    }

    // -- Runtime overlay: presentation-only name/attribute overrides ----------------------------
    {
        use reflection::OverlayTarget;
        let health = OverlayTarget::Field("health");
        let static_attrs = reflection::effective_attributes(TYPE_NAME, health).len();
        assert!(reflection::set_name_override(TYPE_NAME, OverlayTarget::Type, "Actor"));
        assert!(reflection::set_name_override(TYPE_NAME, health, "Hit Points"));
        assert!(reflection::set_name_override(TYPE_NAME, OverlayTarget::Method("double_health"), "Double HP"));
        let tooltip = reflection::Attribute {
            name: "tooltip".into(),
            value: reflection::AttributeValue::String("Remaining health".into()),
        };
        assert!(reflection::add_attribute_override(TYPE_NAME, health, &tooltip));
        let names = (
            reflection::effective_name(TYPE_NAME, OverlayTarget::Type),
            reflection::effective_name(TYPE_NAME, health),
            reflection::effective_name(TYPE_NAME, OverlayTarget::Method("double_health")),
        );
        let attrs = reflection::effective_attributes(TYPE_NAME, health);
        let declared_still = reflection::fields(TYPE_NAME).iter().any(|f| f.name == "health");
        eprintln!(
            "[reflection-ext] overlay names = {names:?}; health attrs {static_attrs} static -> {} effective (last = {:?}); declared name untouched = {declared_still}",
            attrs.len(),
            attrs.last()
        );
        assert_eq!(names, (Some("Actor".into()), Some("Hit Points".into()), Some("Double HP".into())));
        assert_eq!(attrs.len(), static_attrs + 1);
        assert_eq!(attrs.last(), Some(&tooltip));
        assert!(declared_still && reflection::find_type(TYPE_NAME).is_some());

        assert!(reflection::clear_name_override(TYPE_NAME, OverlayTarget::Type));
        assert!(reflection::clear_overlay(TYPE_NAME, health));
        assert!(reflection::clear_overlay(TYPE_NAME, OverlayTarget::Method("double_health")));
        assert_eq!(reflection::effective_name(TYPE_NAME, OverlayTarget::Type).as_deref(), Some(TYPE_NAME));
        assert_eq!(reflection::effective_name(TYPE_NAME, health).as_deref(), Some("health"));
        assert_eq!(reflection::effective_attributes(TYPE_NAME, health).len(), static_attrs);
        assert!(!reflection::set_name_override(TYPE_NAME, OverlayTarget::Field("nope"), "x"));
        assert!(reflection::effective_name(TYPE_NAME, OverlayTarget::Method("nope")).is_none());
        eprintln!("[reflection-ext] overlay cleared: names and attributes back to their declared values");
    }

    // `instance` destroys itself (running DemoActor's real C++ destructor, which frees `tags`'
    // heap storage) when dropped here.
    drop(instance);

    eprintln!("[reflection-ext] all reflection-extension demonstration checks passed");
}

/// Registers a brand-new reflected type from Rust (`reflection::TypeBuilder`) and drives it
/// through the same generic reflection surface a C++-declared type gets: descriptors, attributes,
/// default construction, Rust-bodied methods (zero-arg and byte-marshalled), hooks, events,
/// nesting, and unregistration. Logged with a `[reflection-rt]` prefix.
fn demonstrate_reflection_runtime_types() {
    use std::sync::atomic::{AtomicI32, AtomicU32, Ordering};

    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
    struct RuntimeCrate {
        hp: i32,
        weight: f32,
    }

    const CRATE: &str = "hello.runtime.crate";
    const PAIR: &str = "hello.runtime.crate_pair";

    fn hp_of(call: &reflection::ReflectedCall<'_>) -> i32 {
        i32::from_ne_bytes(call.object.as_deref().expect("receiver")[0..4].try_into().unwrap())
    }

    reflection::TypeBuilder::with_default(CRATE, &RuntimeCrate { hp: 40, weight: 2.5 })
        .field_with_attributes(
            "hp",
            0,
            "i32",
            &[reflection::Attribute { name: "units".into(), value: reflection::AttributeValue::String("hp".into()) }],
        )
        .field("weight", 4, "f32")
        .method("hp_times_two", "i32", &[], |call| {
            let doubled = hp_of(call) * 2;
            call.ret.as_deref_mut().expect("i32 return").copy_from_slice(&doubled.to_ne_bytes());
        })
        .method("scaled_hp", "i32", &["i32"], |call| {
            let factor = i32::from_ne_bytes(call.args[0].try_into().unwrap());
            let scaled = hp_of(call) * factor;
            call.ret.as_deref_mut().expect("i32 return").copy_from_slice(&scaled.to_ne_bytes());
        })
        .event("on_broken", &["i32"])
        .attribute(reflection::Attribute { name: "category".into(), value: reflection::AttributeValue::String("props".into()) })
        .register()
        .expect("register runtime crate type");

    let info = reflection::find_type(CRATE).expect("runtime type is findable");
    let fields = reflection::fields(CRATE);
    let field_names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    let method_names: Vec<String> = reflection::methods(CRATE).into_iter().map(|m| m.name).collect();
    eprintln!(
        "[reflection-rt] registered {CRATE}: size={} align={} fields={field_names:?} methods={method_names:?} type attrs={:?} hp attrs={:?}",
        info.size,
        info.align,
        reflection::type_attributes(CRATE),
        reflection::field_attributes(CRATE, "hp"),
    );
    assert_eq!((info.size, info.align), (8, 4));
    assert_eq!(field_names, ["hp", "weight"]);
    assert_eq!(method_names, ["hp_times_two", "scaled_hp"]);
    assert_eq!(reflection::type_attributes(CRATE).len(), 1);
    assert_eq!(reflection::field_attributes(CRATE, "hp")[0].value, reflection::AttributeValue::String("hp".into()));

    let mut instance = reflection::construct_default(CRATE).expect("default construct runtime type");
    let default: RuntimeCrate = bytemuck::pod_read_unaligned(instance.bytes());
    eprintln!("[reflection-rt] construct_default -> {default:?} (expected hp=40, weight=2.5)");
    assert_eq!(default, RuntimeCrate { hp: 40, weight: 2.5 });

    let doubled = reflection::invoke_method0(CRATE, "hp_times_two", instance.bytes_mut()).expect("invoke hp_times_two");
    let hook_calls = Arc::new(AtomicU32::new(0));
    let hook = {
        let hook_calls = hook_calls.clone();
        reflection::add_method_hook(CRATE, "scaled_hp", reflection::HookTiming::Before, move |call| {
            assert_eq!(call.args.len(), 1);
            hook_calls.fetch_add(1, Ordering::SeqCst);
        })
        .expect("hook runtime method")
    };
    let mut scaled = [0u8; 4];
    reflection::invoke_method(CRATE, "scaled_hp", instance.bytes_mut(), &[&3i32.to_ne_bytes()], &mut scaled)
        .expect("invoke scaled_hp(3)");
    let scaled = i32::from_ne_bytes(scaled);
    drop(hook);
    eprintln!(
        "[reflection-rt] hp_times_two() = {doubled:?}, scaled_hp(3) = {scaled}, before-hook calls = {} (expected SignedInt(80), 120, 1)",
        hook_calls.load(Ordering::SeqCst)
    );
    assert_eq!(doubled, reflection::MethodValue::SignedInt(80));
    assert_eq!(scaled, 120);
    assert_eq!(hook_calls.load(Ordering::SeqCst), 1);
    let bad_arg = reflection::invoke_method(CRATE, "scaled_hp", instance.bytes_mut(), &[&[1u8, 2]], &mut [0u8; 4]);
    let bad_ret = reflection::invoke_method(CRATE, "scaled_hp", instance.bytes_mut(), &[&3i32.to_ne_bytes()], &mut [0u8; 8]);
    assert!(bad_arg.is_err() && bad_ret.is_err(), "size mismatches must be rejected");

    let seen = Arc::new(AtomicI32::new(0));
    let subscription = {
        let seen = seen.clone();
        reflection::subscribe_event(CRATE, "on_broken", move |call| {
            seen.store(i32::from_ne_bytes(call.args[0].try_into().unwrap()), Ordering::SeqCst);
        })
        .expect("subscribe runtime event")
    };
    assert!(reflection::fire_event(CRATE, "on_broken", instance.bytes_mut(), &[&7i32.to_ne_bytes()]));
    drop(subscription);
    eprintln!("[reflection-rt] fire_event(on_broken, 7) -> listener saw {}", seen.load(Ordering::SeqCst));
    assert_eq!(seen.load(Ordering::SeqCst), 7);

    // Rejections: duplicate name, non-plain field type, unknown parameter type.
    let duplicate = reflection::TypeBuilder::for_pod::<RuntimeCrate>(CRATE).register();
    reflection::register_demo_type();
    let non_plain = reflection::TypeBuilder::new("hello.runtime.bad_field", 64, 8).field("actor", 0, "sturdy_rs.demo.actor").register();
    let bad_param = reflection::TypeBuilder::new("hello.runtime.bad_param", 4, 4).method("m", "void", &["no.such.type"], |_| {}).register();
    eprintln!("[reflection-rt] rejected: duplicate={duplicate:?} non_plain_field={non_plain:?} bad_param={bad_param:?}");
    assert!(duplicate.is_err() && non_plain.is_err() && bad_param.is_err());
    assert!(reflection::find_type("hello.runtime.bad_field").is_none());

    // A runtime type nesting another runtime type.
    reflection::TypeBuilder::new(PAIR, 16, 4)
        .field("left", 0, CRATE)
        .field("right", 8, CRATE)
        .register()
        .expect("register nested runtime type");
    let pair_fields = reflection::fields(PAIR);
    let right = pair_fields.iter().find(|f| f.name == "right").expect("right field");
    eprintln!("[reflection-rt] {PAIR}: right field type={:?} offset={} size={}", right.field_type_name, right.offset, right.size);
    assert_eq!((right.offset, right.size), (8, 8));
    assert_eq!(right.field_type_name.as_deref(), Some(CRATE));

    assert!(reflection::unregister_type(PAIR));
    assert!(reflection::unregister_type(CRATE));
    assert!(reflection::find_type(CRATE).is_none() && !reflection::unregister_type(CRATE));
    reflection::TypeBuilder::for_pod::<RuntimeCrate>(CRATE).field("hp", 0, "i32").register().expect("re-register after unregister");
    assert_eq!(reflection::fields(CRATE).len(), 1);
    assert!(reflection::unregister_type(CRATE));

    eprintln!("[reflection-rt] all runtime type registration checks passed");
}

impl GameLogic for Spinner {
    fn init(&mut self, engine: &mut Engine<'_>) -> Result<(), Error> {
        if let Some(gpu) = engine.gpu() {
            eprintln!("GPU: {} ({})", gpu.name, gpu.api_version);
        }
        demonstrate_ecs(engine);
        demonstrate_ecs_ext(engine);
        self.schedule_targets = Some(demonstrate_schedule_and_commands(engine));
        self.ui_demo = Some(install_ui_draw_demo());
        demonstrate_reflection();
        demonstrate_reflection_runtime_types();
        demo_async_tasks();
        demo_async_ext();
        Ok(())
    }

    fn frame(&mut self, engine: &mut Engine<'_>, frame: &Frame) -> Option<FrameDescription> {
        // `Engine::rhi` needs a live `RhiDevice`, which only exists once the renderer is up (see
        // `Engine::gpu`'s doc comment); `init` runs before that, so the RHI-extension demo runs
        // here instead, gated to the first frame where a GPU is actually reported.
        if !self.rhi_demo_done && engine.gpu().is_some() {
            demonstrate_rhi_ext(engine);
            self.rhi_demo_done = true;
        }

        if !self.pipeline_demo_done && engine.gpu().is_some() {
            demonstrate_render_pipeline_ext(engine);
            self.pipeline_demo_done = true;
        }

        if !self.assets_demo_done && engine.gpu().is_some() {
            demonstrate_assets_ext(engine);
            demonstrate_gltf_ext(engine);
            self.assets_demo_done = true;
        }

        if !self.schedule_demo_checked {
            let (target_for_destroy, target_for_components) =
                self.schedule_targets.expect("schedule_targets set by init");
            if check_schedule_and_commands_demo(
                engine,
                target_for_destroy,
                target_for_components,
                self.schedule_demo_frames_waited,
            ) {
                self.schedule_demo_checked = true;
            }
            self.schedule_demo_frames_waited += 1;
        }

        if !self.ui_demo_checked {
            let results = self.ui_demo.as_ref().expect("ui_demo set by init");
            if check_ui_draw_demo(results, self.ui_demo_frames_waited) {
                self.ui_demo_checked = true;
            }
            self.ui_demo_frames_waited += 1;
        }

        let surface = SurfaceHandle::from_window_id(frame.window_id);
        if !self.window_demo_queued {
            demonstrate_window_mutation(engine, surface);
            self.window_demo_queued = true;
        } else if !self.window_demo_checked {
            if check_window_mutation_demo(engine, surface, self.window_demo_frames_waited) {
                self.window_demo_checked = true;
            }
            self.window_demo_frames_waited += 1;
        }

        self.yaw += 45.0 * frame.delta_seconds as f32;
        if engine.input().key_just_pressed(Key::Space) {
            let scale = if engine.time_scale() == 1.0 { 0.25 } else { 1.0 };
            engine.set_time_scale(scale);
        }
        Some(FrameDescription::new(
            Camera::perspective().at([0.0, 1.0, 4.0]).euler_degrees([0.0, self.yaw, 0.0]),
        ))
    }
}

fn main() -> ExitCode {
    sturdy::run(
        RuntimeConfig::new("Hello from Rust").size(1280, 720).vsync(VSync::Adaptive),
        Spinner {
            yaw: 0.0,
            rhi_demo_done: false,
            pipeline_demo_done: false,
            assets_demo_done: false,
            schedule_demo_checked: false,
            schedule_demo_frames_waited: 0,
            schedule_targets: None,
            window_demo_queued: false,
            window_demo_checked: false,
            window_demo_frames_waited: 0,
            ui_demo: None,
            ui_demo_checked: false,
            ui_demo_frames_waited: 0,
        },
    )
}
