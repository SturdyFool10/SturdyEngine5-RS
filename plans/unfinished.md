# Unfinished binding work

Audited against the vendored engine's own reference C API (`FFI/src/FFI/*.cpp` — a foreign-caller
surface the engine ships that `sturdy-sys`'s hand-written shims were clearly modeled on) by diffing
every `sturdy_*` exported function name against `sturdy-sys`/`sturdy`. Naming differs a lot between
the two (e.g. `sturdy_render_pass_draw` → `rp_draw`, `sturdy_ecs_get_component` → `ecs_read_component`),
so a raw name diff produced ~280 "misses" out of ~330 reference functions; the great majority of
those were spot-checked and are false positives (renamed or consolidated, not missing). What's below
is what survived that check — confirmed by reading both sides, not just grepping names. See "Verified
NOT a gap" at the bottom for the false-positive clusters, so this doesn't get re-litigated.

## ECS / scheduling

- [x] `Engine::RenderFrameParameters::lighting` (`SceneLighting`) bound as `sturdy::SceneLighting` /
      `FrameDescription::lighting`.
- [x] ECS rendering components (`WorldTransform`, `ModelRenderer`, `LightGizmoRenderer`,
      `DirectionalLightRenderer`, `SpotLightRenderer`, `PointLightRenderer`) bound in
      `sturdy::scene`. (Confirms `sturdy_render_spawn`/`set_transform`/`set_model`/`set_*_light` in
      the reference API are just this same ECS pattern through a flat C wrapper — fully covered.)
- [x] `Commands` (deferred spawn/despawn from inside a `schedule` system) — threaded through end to
      end: `sturdy-sys/src/schedule.rs` now declares an opaque `Commands` bridge type plus
      `commands_spawn`/`commands_add_component`/`commands_remove_component`/`commands_destroy`;
      `schedule.cpp` mints one per dispatch via `Schedule::add_erased_{system,global_system}`'s
      `prepare` callback and forwards it through `entity_system_trampoline`/
      `global_system_trampoline`; `sturdy::schedule::Commands` wraps it and is now the mandatory
      last argument to every system body (`add_system`/`add_global_system`/`add_system_bytes`/
      `add_global_system_bytes`). Verified live in `examples/hello`
      (`demonstrate_schedule_and_commands`/`check_schedule_and_commands_demo`, `[schedule-ext]`
      lines) — `spawn_one`, `insert` (add_component), `remove_component`, and `destroy` all
      confirmed applied through a real scheduled system. `Commands` intentionally stays
      byte-level/`ComponentId`-keyed rather than `T: Component`-generic (resolving a `ComponentId`
      needs `World::register`, which needs exclusive world access not available mid-dispatch); see
      `sturdy/src/schedule.rs`'s module doc.
- [x] Multi-arity `World::query`/system tuples: `QueryTuple`, `SystemTuple`, and `ResourceTuple`
      all now go up to 12-tuples (std's own tuple-trait ceiling; truly unbounded arity needs
      variadic generics Rust doesn't have — use the `*_bytes` variants past 12). Verified live: a
      12-component `World::query` in `examples/hello` (`[ecs-ext] ... 12-tuple`); 12-wide
      `SystemTuple`/`ResourceTuple` impls compile-checked there (not scheduled live).
- [ ] Event access from inside a `schedule` system (blocked upstream — the erased `ErasedSystemFn`
      C++ API has no way to resolve/drain an event channel for a system body). Workaround:
      `World::read_events` from `GameLogic::frame`.
- [ ] Ordering/dependency hooks beyond the scheduler's own conflict detection (blocked upstream —
      `SFT::Ecs::Schedule` has no "run after system X" API).

## Windows — runtime mutation

`sturdy::render` covers window *creation* (`open_window`/`recreate_window`/`close_window`) plus
`notify_resize`, and now also the full `WindowRequests`-queue-backed runtime-mutation surface:

- [x] `window_set_cursor_grabbed`, `window_set_cursor_icon`
- [x] `window_set_decorated`, `window_set_transparent` (post-creation; `WindowDesc`'s own fields
      still cover the creation-time initial values)
- [x] `window_set_mode` (fullscreen/windowed toggle at runtime, bound as `Engine::set_window_mode`
      to avoid colliding with `WindowDesc::window_mode`)
- [x] `window_set_mouse_locked`, `window_set_relative_mouse_mode`
- [x] `window_set_text_input_active`, `window_set_text_input_area` (IME positioning)
- [x] `window_set_effect` (blur/vibrancy-style window effects, `WindowEffectKind`)

All nine bound as `Engine::set_cursor_icon`/`set_cursor_grabbed`/`set_window_mode`/`set_decorated`/
`set_transparent`/`set_relative_mouse_mode`/`set_mouse_locked`/`set_text_input_active`/
`set_text_input_area`/`set_window_effect` (`sturdy-sys`: `render.rs`'s new `CursorIcon`/
`WindowEffectKind`/`TextInputArea` types plus the `window_set_*` bridge functions; C++ shim:
`render.cpp`, each just forwarding to `Engine::window_requests().set_*(window_id, ...)`).

**Correction to this file's own earlier reasoning**: the previous entry claimed binding a `set_*`
call would also require binding `window_take_completions`/the completion-draining mechanism. That
was wrong — checked this pass by reading `Engine/WindowRequests.hpp` and `ApplicationImpl.cpp`'s
`process_window_requests`: `WindowRequestCompletion`/`WindowRequestKind` only exist for
`Spawn`/`Close`/`RecreatePrimary` (i.e. exactly what `window_open`/`window_recreate`/`window_close`
already cover through their own direct, synchronous `EngineView` calls — those don't even go
through `WindowRequests` in this binding). Every `set_*` request is genuinely fire-and-forget with
no completion counterpart at all, applied by the engine's own per-frame
`Application::process_window_requests` loop with nothing to poll. `window_take_completions` itself
is still unbound, but only because nothing in this binding produces a `WindowRequestId` to poll a
completion for (`window_open`/`_recreate`/`_close` don't need it either) — not because these
mutation setters needed it. Verified live in `examples/hello` (`demonstrate_window_mutation`/
`check_window_mutation_demo`, `[window-ext]` lines): every setter queued once, and
`set_mouse_locked(true)` independently confirmed applied via `Diagnostics::window`'s
`mouse_locked` snapshot field (the one setter with an existing readback), then unlocked again as
cleanup.

## Time — two accessors (false positive, both already bound)

- [x] `time_unscaled_delta_seconds` — this pass's audit was wrong: `Engine::unscaled_delta_seconds`
      (`sturdy/src/engine.rs:49`) already calls `ffi::engine_unscaled_delta_seconds`, bound since
      before this pass.
- [x] `time_tick_index` — same: `Engine::tick_index` (`sturdy/src/engine.rs:53`) already calls
      `ffi::engine_tick_index`. Both were already correctly bound; nothing changed here.

## Diagnostics — native WebGPU handles

- [x] `native_webgpu` — added end to end: `sturdy-sys/src/diagnostics.rs`'s `NativeWebGpuHandles` +
      `native_webgpu_available`/`native_webgpu_handles`, `diagnostics.cpp`'s
      `webgpu_native_access`/`native_webgpu_handles` (guarded on `STURDY_ENABLE_WEBGPU`, which
      `Core` publishes as a `PUBLIC` define so it propagates into `sturdy_rs_shim` automatically —
      mirrors `FFI/src/FFI/Native.cpp`'s reference-API `sturdy_native_webgpu` exactly), and
      `sturdy::diagnostics::Diagnostics::native_webgpu`. Compiles clean in the default (non-`webgpu`
      feature) build, where the guard's `false` branch is taken; not separately verified with the
      `webgpu` feature enabled (that build roughly doubles compile time by fetching/building Dawn)
      — code is a direct mirror of the already-verified `native_vulkan`/`native_d3d12` pattern, but
      flag this if something's off when actually run against a Dawn backend.

## RHI (`sturdy-sys/src/rhi.rs`, `sturdy/src/rhi.rs`)

Not full parity with the engine's internal RHI surface by design (`RhiResources.cpp` alone is
2600+ lines); the priority per-frame ops are covered (confirmed: `rp_*`/`cp_*`/`ce_*` cover the
whole draw/dispatch/copy/barrier/bind-group/pipeline/viewport/scissor path, just abbreviated).
Status:

- [x] Render bundles — `Rhi::create_render_bundle_encoder` → `RenderBundleEncoder` (full draw/
      state surface minus `set_shading_rate`, which the engine itself doesn't allow in bundles) →
      `finish()` → `RenderPass::execute_bundles`. **Verified live** (pipeline-ext pass 5: a bundle
      carrying all of its own state draws only the green triangle; readback shows green on the
      right and the clear color on the left).
- [x] Opacity micromaps — `Rhi::opacity_micromap_build_sizes`/`create_opacity_micromap`/
      `destroy_opacity_micromap`, `CommandEncoder::build_opacity_micromaps`, and the
      `opacity_micromap*` fields on `AccelerationStructureTrianglesDesc`. Procedural geometry
      (`AccelerationStructureGeometryType::Aabbs` + `AccelerationStructureAabbsDesc`), noted as
      deferred in the bridge, was added alongside. Compiles clean; **not live-tested** — the dev
      GPU reports zero ray-tracing limits (`[rhi-ext] feature_properties`), and there's no RT demo.
- [x] Swapchain/surface/present — **verified not a gap**: the engine owns its swapchains and
      presentation loop; a Rust caller never holds a `SwapchainHandle`, and `sturdy::render`'s
      window/surface/presentation API is the intended layer. Binding raw `create_swapchain`/
      `present` would let Rust fight the engine's own presentation. Documented in both RHI module
      docs.
- [x] Semaphores/fences — timeline semaphores (`create_semaphore`/`semaphore_value`/
      `wait_semaphore`/`signal_semaphore`), fences (`create_fence`/`wait_fences`/`reset_fences`),
      and `Rhi::submit_with(&SubmitDesc)` (queue lane, semaphore waits/signals, fence, one-shot
      flag). **Verified live**: CPU signal→read, a submission that waits on value 5 and signals 10
      plus a fence, `wait_fences` true, semaphore reads 10 afterward, and a zero-timeout poll
      after `reset_fences` reads false. The render-pipeline demo now uses this instead of
      `wait_idle`.
- [x] HDR (RHI-level) — **verified not a gap**: `Engine::query_hdr_capabilities`/
      `update_hdr_content_light_level` (bound in `sturdy::render`) forward to the RHI-level calls
      for the engine's own swapchain; the RHI-level versions take a `SwapchainHandle` Rust never has.
- [x] Mesh/task shader draws — `RenderPass`/`RenderBundleEncoder::draw_mesh_tasks`/
      `draw_mesh_tasks_indirect`/`draw_mesh_tasks_indirect_count` (`DrawMeshTasksArgs`). Compiles
      clean; **not live-tested** — the dev GPU reports zero mesh-shader limits, and pipeline
      creation doesn't expose task/mesh stages yet (the draws are callable, but there's no way to
      build a mesh pipeline through `RenderPipelineDesc`'s vertex+fragment shape).
- [x] Variable-rate shading — `RenderPass::set_shading_rate(ShadingRate, ShadingRateCombiner,
      ShadingRateCombiner)`. Compiles clean; not live-tested (no VRS support on the dev GPU).
      `RenderPassDesc::shading_rate_attachment` (attachment-based VRS) is still unbound.
- [x] Custom MSAA sample locations — `MultisampleState::sample_locations_enable` (pipeline
      creation) + `RenderPass::set_sample_locations` (dynamic per-draw positions). Compiles clean;
      **not live-tested** — exercising it needs an actual multisampled pipeline (a resolve target,
      `samples > X1`), which the new render-pipeline demo (see below) deliberately doesn't set up,
      to keep that demo's own risk surface down (`VK_EXT_sample_locations` is a real, less-common
      GPU extension — see the demo's own doc comment for why `multiDrawIndirect`/`depthBounds`
      were judged acceptable risk to test live but this wasn't).
- [x] Depth-bounds test — `DepthStencilState::depth_bounds_test_enable` (pipeline creation) +
      `RenderPass::set_depth_bounds` (dynamic per-draw range). **Verified live** — see below.
- [x] Per-face stencil ops — `DepthStencilState::stencil_test_enable`/`stencil_front`/
      `stencil_back`/`stencil_read_mask`/`stencil_write_mask` (pipeline creation, new
      `StencilFaceState`/`StencilOp` types) + `RenderPass::set_stencil_reference` (dynamic). Also
      added `RenderPass::set_blend_constant` alongside these — found missing while reading the
      same `RenderPassEncoder` dynamic-state methods, not separately listed in this file before.
      **Partially verified live**: `examples/hello`'s new render-pipeline demo enables stencil
      testing and calls `set_stencil_reference` as part of every pass and confirms color output is
      still correct with it active, but doesn't independently confirm the stencil *write* landed —
      this binding has no stencil-aspect-specific texture readback (`BufferTextureCopy` has no
      aspect-mask field) to check that separately yet.
- [x] Queue-ownership transfers — `ownership: QueueOwnershipTransfer { src: QueueLane, dst:
      QueueLane, enabled }` on both `BufferBarrier` and `TextureBarrier` (both now derive
      `Default`, as do `QueueClass`/`TextureLayout`, so `..Default::default()` works). The
      disabled default path runs through every barrier in the live demos; an actual cross-queue
      transfer isn't exercised (would need a second queue lane doing real work).
- [x] Multi-draw-indirect (`RenderPass::draw_indirect_multi`/`draw_indexed_indirect_multi` — the
      engine's `draw_indirect`/`draw_indexed_indirect` turned out to already be an *overload set*:
      a 2-arg single-draw form, already bound, and a 4-arg `(buffer, offset, draw_count, stride)`
      form that wasn't. **Verified live** — see below, including `draw_count = 2` actually issuing
      two distinct draws from one indirect-buffer call (confirmed via two different triangles
      rendered to two different pixels), not just accepting the call.
- [x] Acceleration structure copy/compaction — `CommandEncoder::copy_acceleration_structure`
      (new `AccelerationStructureCopyMode` enum: Clone/Compact/Serialize/Deserialize). Compiles
      clean; not live-tested — `examples/hello` has no ray tracing/acceleration-structure demo at
      all yet, so building even a minimal BLAS to copy would be new scaffolding, not just wiring
      this one call into an existing demo (unlike the render-pipeline items above, which all
      shared one pipeline demo once it existed).
- [x] Ray tracing device properties/limits query — bound as part of a broader
      `Rhi::feature_properties()` (`RhiDevice::feature_properties()` returns one struct covering
      ray tracing, mesh shader, variable-rate-shading, subgroup, descriptor-indexing, and
      sparse-resource limits together, so this pass bound all of it in one call rather than just
      the ray-tracing slice). **Verified live** in `examples/hello` (`[rhi-ext] feature_properties`
      line).

### The render-pipeline demo (`examples/hello`'s `demonstrate_render_pipeline_ext`)

Standing this up was itself the blocker noted in an earlier version of this file: `examples/hello`
had no render pipeline/render pass demo at all (only buffer/texture/query-set-level RHI ops), so
none of `RenderPass`'s draw/dynamic-state methods had ever run against a real draw. Fixed by adding
a small standalone Slang-compiler binding (see `sturdy::shader_compiler`, `sturdy-sys::
shader_compiler`) that compiles source straight to SPIR-V bytes (distinct from `Assets::
load_shader`'s file-based, engine-`Asset`-producing path, which has no bridge back to a raw
`ShaderModuleHandle`) — this is a new, generally-useful capability for any caller wanting to build
a real RHI pipeline from Rust, not just this demo.

The demo compiles a trivial flat-shading Slang shader, builds two render pipelines (opaque with
depth-bounds+stencil enabled; blend-enabled) over three triangles in one vertex buffer, and reads
back individual texels via `RenderPass`/`CommandEncoder` to check exact pixel output, not just
"didn't crash":
- Pass 1: `draw_indirect_multi(draw_count=1)` (always legal even without `multiDrawIndirect`) —
  red triangle rendered correctly.
- Pass 2: same call with `draw_count=2` — **both** triangles rendered from one call, proving real
  multi-draw-indirect (assumes `multiDrawIndirect` GPU support; no capability query exists through
  this binding to check first, but it's near-universal on desktop GPUs).
- Pass 3: `set_depth_bounds(0.0, 0.5)` against a depth attachment cleared to 1.0 — draw correctly
  rejected. (Caught a wrong assumption while building this: the depth-bounds test compares against
  whatever is *already stored* in the depth attachment, not the incoming fragment's own depth — an
  earlier version of this test used bounds that included the clear value and wrongly expected
  rejection based on the triangle's z instead.)
- Pass 4: blend-enabled pipeline, `set_blend_constant(0.25)`, white triangle over a black clear
  with `ConstantColor`/`OneMinusConstantColor` factors — exact predicted result (`~64` per channel)
  confirmed by pixel readback.

All four passes pass on the dev GPU this was tested against. Re-run this if picking the RHI work
back up further — building the acceleration-structure demo next would let `copy_acceleration_structure`
share this same infrastructure pattern (compile once, build once, reuse across checks).

## UI (`sturdy-sys/src/ui.rs`, `sturdy/src/ui.rs`)

Element/text primitives, hover/click/pointer queries, images, svg, scroll containers, and focus
were already bound. This pass closed the rest, all **verified live** by a new UI draw-hook demo in
`examples/hello` (`install_ui_draw_demo`/`check_ui_draw_demo`, `[ui-ext]` lines). The UI overlay
renders into the swapchain (no readback path), so checks use the engine's own acceptance signals
and layout readback (`Ui::scroll_metrics` on clipped probe elements) rather than pixels.

- [x] Custom shaders — `Ui::custom_element` (element background drawn by a caller's Slang shader)
      and `Ui::stroke_custom` (polyline through a caller's shader), via `CustomShader`. Verified
      with the engine's embedded `Shaders/ui_stroke_custom_demo.slang`: it compiled and drew with
      no push-constant-size error (the UI renderer rejects a mismatch loudly).
- [x] Raw stroke/fill/sector draws — `Ui::stroke_paths`/`stroke_polyline` (solid, dashed, caps,
      joins, glow), `fill_quads` (rounded), `fill_sectors` (donut). All accepted every frame.
- [x] Floating/docking — floating via `ElementDesc::floating: Option<Floating>` (attach to parent/
      element-by-id/root, attach points, offset, z-index, pointer capture, clip); verified by a
      badge anchored to another element by id appearing in the layout at its declared size. The
      same pass bound the rest of `ElementDecl` that was missing: `align` (`ChildAlignment`),
      `border`, `z`, `cursor`, `debug_label`. Docking via an owned `DockWorkspace` (`add_panel` with
      optional `DockPlacement`, `begin_frame`, `panel_content`, `end_frame` → `DockEvents` for
      tear-offs/close requests); verified: duplicate ids rejected, exactly one visible tab per leaf
      after a tab + split layout, and content built inside a panel reaches the layout. Not bound:
      `DockWorkspace`'s foreign-drag-preview API (cross-workspace drags) and `DockWorkspaceStyle`
      customization (defaults only, plus `set_content_background`).
- [x] The graph widget — `Ui::graph(&ElementDesc, &GraphDesc)`: line/area/bar/scatter/pie, axes
      (linear/log/symlog, fixed or autoscaled range, categorical, titles, gridlines), series
      styling, legend. Verified: a two-series line chart and a donut pie both report drawn. Found
      while testing: the widget sizes itself from the *previous* frame's bounds for its element id,
      so it needs a stable non-empty id and returns `false` on its first frame — documented on
      `Ui::graph`. Not bound: `AxisConfig`'s `std::function` hooks (custom scale transform, tick
      formatter). The reference FFI's `sturdy_ui_graph_series_*` ring buffer isn't mirrored — it's
      a `VecDeque` in Rust.

## Render graph (`sturdy/src/render.rs`)

- [x] `SceneRenderSettings` (scene integrator / path tracing knobs) — `RenderGraphSettings::scene`
      (`SceneIntegrator` enum; `background_color: Option<[f32; 4]>` bridged as a
      `has_background_color` flag + array since cxx has no optional arrays). Verified by unit
      tests in `sturdy/src/render.rs` (`cargo test --release -p sturdy --lib render::`): an inverted
      wavelength interval and a NaN background color are both rejected by the engine's own
      `RenderGraph::validate()`, proving the fields actually reach it.
- [x] `DebugOverlayRenderSettings` — `RenderGraphSettings::debug_overlay`; same unit tests.
- [ ] Anything requiring a live `RenderGraph`/frame handle rather than the settings description
      alone

## Reflection (`sturdy-sys/src/reflection.rs`, `sturdy/src/reflection.rs`)

Correction from an earlier pass: **method enumeration and zero-arg invocation ARE bound**
(`methods()`/`invoke_method0`) — an earlier version of this file wrongly said methods weren't
enumerable; that was a misreading of a narrow doc comment about one summary-count field, not the
actual API. Confirmed by reading `sturdy/src/reflection.rs:412,478`.

What's genuinely unbound, none of it mentioned in the module's own (otherwise very thorough)
"Scope" doc comment as a considered-and-rejected item, unlike methods/constructors/events-
subscription which *are* explicitly discussed there:

- [x] Static field get/set (`get_static_field`/`set_static_field`/`get_static_field_as`/
      `set_static_field_value`) — needed a real `copy_static_field_out`/`_in` call (unlike instance
      fields, a static field's storage has no per-instance byte view to slice into). Verified live
      in `examples/hello`: the demo `DemoActor` type gained a reflected `spawn_count` static field,
      read/written, and confirmed shared (not per-instance) across two separately constructed
      instances.
- [x] Runtime type registration (`type_builder_create`/`add_field`/`add_method`/`add_event`/
      `set_constructors`/`finish`/`discard`, `unregister_type`) — `reflection::TypeBuilder`
      (`new`/`for_pod`/`with_default`, `field`/`field_with_attributes`/`method`/`event`/
      `attribute`, `register`; dropping unregistered = discard) + `unregister_type`. Runtime types
      are plain bytes (memcpy move/copy, no-op destroy, `set_constructors` is implicit), so
      field/param/return types are limited to fundamentals and other runtime types; method
      bodies are Rust closures (fixed 1024-slot trampoline table, since `MethodInfo::invoke` has
      no user data). Also added `reflection::invoke_method` (arbitrary arity, byte-marshalled,
      same plain-bytes restriction). Verified live: register with attributes + default bytes,
      descriptors/attributes read back, default construct, zero-arg and one-arg Rust methods
      (with a hook), event fire → subscriber, duplicate/non-plain-field/unknown-param rejected,
      nested runtime type, unregister + re-register.
- [x] Method overrides (`register_override`/`clear_override` — these are *method* overrides,
      not field overrides as an earlier version of this file said) — `override_method` →
      `MethodOverride` (clears on drop). Verified live: 3x override, restored on drop.
- [x] Before/after method-call hooks (`add_before_hook`/`add_after_hook`/`remove_*_hook`) —
      `add_method_hook` → `MethodHook`. Verified live (hook counts, dropped hooks stop running).
- [x] Event subscription (`subscribe_event` → `EventSubscription`) — previously listed as out of
      scope for panic safety; closures run under `diagnostics::guard` (abort on panic). Verified
      live.
- [x] Runtime overlay (not in the FFI reference, engine-only): type/field/method display-name
      and attribute overrides (`set_name_override`/`clear_name_override`/
      `add_attribute_override`/`clear_overlay`/`effective_name`/`effective_attributes`).
      Verified live against the demo type.
- [x] Event descriptor lookup by name (`find_event`) and firing (`fire_event`) — separate from
      subscription (now bound too, see above).
      `fire_event` strictly validates argument count/sizes against the event's own declared
      parameter types before touching any argument bytes (fails closed on a mismatch rather than
      risking an out-of-bounds read), and is a defined no-op when nobody has subscribed. Verified
      live: `find_event`/`fire_event` against the demo type's `on_died` event, plus a rejected
      wrong-arity fire.
- [x] Container element access: `container_get_element`/`set_element`/`resize`/`size` — bound as
      `reflection_container_size`/`_get_element`/`_set_element`/`_resize`, restricted to
      `element_trivial` elements (a non-trivial element's placement-copy would leave a live C++
      object in a plain Rust byte buffer with no way to safely destroy it — same restriction
      instance field read/write already has). Verified live: resize/set/get/len round-tripped
      against the demo type's `tags: std::vector<int>` field, plus an out-of-range read correctly
      reporting `None`.

## Async (`sturdy/src/select.rs`, `sturdy/src/task.rs`)

- [x] `select!`: `biased` mode (accepted as a no-op — the macro was already effectively biased),
      an `else` branch, per-branch `if guard =>` conditions. Verified both by unit tests
      (`sturdy/src/select.rs`'s `#[cfg(test)] mod tests`, no engine needed) and live in
      `examples/hello` (`[async-ext] select! biased/guard/all-disabled->else` lines).
- [x] `time::interval` (`tokio::time::interval`-shaped repeating timer, `MissedTickBehavior::
      Burst`-equivalent). Verified live in `examples/hello`.
- [x] `JoinSet::detach_all`. Verified live in `examples/hello`.

## Assets (`sturdy/src/assets.rs`)

- [x] `RendererTextureHandle` → RHI-level conversion — `Assets::rhi_texture(handle)` →
      `rhi::RendererTextureResources` (texture/view/sampler + size/mips/format) via
      `Renderer::texture()`. Handles stay Renderer-owned. Verified live: the procedural
      checkerboard asset's RHI texture copied to a readback buffer with plain RHI calls matches the
      uploaded pixels byte for byte; a zero handle returns `None`.
- [x] `Gltf::spawn_all`-equivalent convenience — added as `GltfScene::spawn_all(&self, world:
      &mut World<'_>) -> Result<Vec<Entity>, EcsError>`, spawning a `WorldTransform` +
      `ModelRenderer` per instance and a `WorldTransform` + the matching light-renderer component
      per light (converting `GltfLight`'s degree-based cone angles to `SpotLightRenderer`'s cosine
      fields). Pure Rust on top of already-bound `World::spawn_builder`/`sturdy::scene` — no new
      C++ needed. Verified live (`[gltf-ext]` in `examples/hello`, importing the checked-in Khronos
      Box sample): 1 model/1 instance imported, `spawn_all` spawned an entity whose
      `WorldTransform`/`ModelRenderer` read back correctly. Live testing also caught that
      `import_gltf` rejects `Asset::invalid()` as its shader (docs previously said it meant
      "engine default") — fixed.

## Verified NOT a gap (checked this session, don't re-flag)

- **Render.cpp** (`render_spawn`/`set_transform`/`get_transform`/`set_model`/`set_*_light`/
  `create_*`/`load_*`): fully covered — spawning/transform/lights via `sturdy::scene` + `World`,
  asset creation/loading via `sturdy::assets` (`assets_create_model`, `assets_create_texture*`,
  `assets_load_shader`, `assets_load_texture*`, `assets_set_model_float/vec4/texture`). The
  reference API's `*_desc_init`/`*_params_init` zero-init helpers are just `Default` in Rust.
- **FrameSettings.cpp**: all seven settings categories present in `sturdy-sys/src/render.rs`
  (`ShadowSettings`, `AmbientOcclusionSettings`, `AntiAliasingSettings`, `BloomSettings`,
  `ToneMappingSettings`, `RestirGiSettings`, `MotionBlurSettings`).
- **Presentation.cpp**: `presentation_settings_get/set`, `presentation_resolution_query`,
  `hdr_capabilities_query`, `hdr_update_content_light_level` all present, just reordered names.
- **Input.cpp**: bound 1:1 under an `input_*` prefix instead of `engine_key_*`/`engine_mouse_*`.
- **Async.cpp**: deliberately *not* mirrored — `sturdy::task`/`sync`/`time` bind the engine's raw
  `SFT::Async::Scheduler` (`scheduler_spawn`) directly and drive real Rust `Future`s, instead of the
  reference API's separate handle/poll model. A better design for Rust, not a gap.
- **Runtime.cpp**: `runtime_run` → `run_runtime`; `gpu_enumerate` → `gpu_inventory_count`/`_get`/
  `gpu_capabilities`; `runtime_config_init` is just `RuntimeOptions`'s `Default`.
- **Gltf.cpp** (`gltf_instance_at/count/name`, `gltf_light_at/count/name`, `gltf_model_at/count/
  name`, `gltf_release`): `gltf_import`'s `GltfImportOutcome` already returns full `models`/
  `instances`/`lights` vectors by value in one call — no import-session handle exists to need
  paginated accessors or a `release` call.
- **Ecs.cpp/EcsResources.cpp**: `ecs_get_component`/`set_component`/`for_each` etc. map onto
  `ecs_read_component`/`write_component`/`query`; `ecs_clear_events` → `ecs_clear_event_channel`;
  `ecs_resource_id` → `ecs_resource_key`; `ecs_event_count` has no counterpart but is unnecessary
  (`read_events` returns a `Vec`, just check `.len()`).
- **Native.cpp** Vulkan/D3D12: bound as `native_vulkan_available/handles`,
  `native_d3d12_available/handles`.
- **GraphWidget.cpp**: now bound (`Ui::graph`, see UI above). An earlier version of this file
  listed it as out of scope.

## Doc hygiene

Stale "not yet implemented"-type comments fixed in earlier passes: `sturdy/src/render.rs` and
`sturdy-sys/src/render.rs` module docs (render-graph *is* wired into `FrameDescription`),
`sturdy/src/diagnostics.rs` (module *is* wired into the public tree), and several `README.md`
claims. This pass's own correction: this file's previous claim that reflection "methods are not
bound" was wrong (see Reflection section above) — a reminder that even this plan file needs
re-verification against the actual code, not just trusted and propagated.

- [x] Re-run the reference-API diff (done; remaining name-misses are the renamed/consolidated clusters listed above, plus `ecs_component_info`/`_name`/`add_tag`/`register_tag_component`, `async_is_worker_thread`, `native_*_queue`, `render_create_shape_model` (covered by `Shape`), `window_take_completions`, and the `reflection_find_field`/`find_method`/`get_field`/`set_field`/`invoke_static_method` handle-style accessors — small, unbound convenience gaps rather than missing subsystems) (`FFI/src/FFI/*.cpp` exported function names vs. `sturdy-sys`/
      `sturdy`) after implementing any item above, and update this file's "Verified NOT a gap"
      section if the false-positive clusters change shape.
