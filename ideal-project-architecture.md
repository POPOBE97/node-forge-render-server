# Ideal Project Architecture

This document describes the target-state architecture for `node-forge-render-server`.

The key update is that "ideal" does not mean "maximally split into folders". It means:

- ownership boundaries are true
- the renderer pipeline is explicit
- runtime and transport concerns do not leak into planning
- future extractions happen only when they pay for themselves

The project already has the right core shape. The remaining work is mostly about removing the last
legacy seams and only then doing smaller structural cleanup.

## 1. What "ideal" means here

The ideal structure is optimized for:

- clear subsystem boundaries
- predictable data flow
- GPU-free planning and compilation tests
- scene updates without hidden renderer coupling
- safe growth in node types, pass types, headless paths, and tooling
- explicit ownership of animation, planning, and GPU materialization

This is intentionally not a "largest possible directory tree" document.

The architecture should optimize for these truths first:

1. the live renderer path is `prepare -> plan -> finalize`
2. planning happens before GPU materialization
3. one subsystem owns one truth
4. dependency direction stays one-way
5. module splits are justified by ownership, not symmetry

## 2. Core Architectural Principles

### A. The renderer is a compiler pipeline

The renderer should be treated as a staged compiler:

`SceneDSL -> PreparedScene -> RenderPlan -> ShaderSpaceBuildResult -> Runtime execution`

Each stage has one job:

- `SceneDSL`: editor-facing graph format
- `PreparedScene`: normalized, validated, topology-safe scene
- `RenderPlan`: GPU-free execution plan
- `ShaderSpaceBuildResult`: materialized GPU resources and bindings
- runtime execution: frame stepping, uniform updates, presentation

This pipeline is already the canonical path in the codebase. `ShaderSpaceBuilder::build()` in
`api.rs` calls `RenderPlanner::new(opts).plan(…)` then `ShaderSpaceFinalizer::finalize(…)`.

Key implementations:

- `scene_prep::prepare_scene_with_report()`
- `render_plan::planner::RenderPlanner::plan()`
- `shader_space::finalizer::ShaderSpaceFinalizer::finalize()`
- `shader_space::api::ShaderSpaceBuilder`

The remaining work is to remove the legacy parallel paths (especially `assembler.rs` helpers) and
resolve the pass-assembler location problem (see Section 2G) so that the pipeline boundary is
structurally enforced, not just conventionally followed.

### B. Purity before materialization

Anything that does not require a GPU should happen before `ShaderSpace` construction.

That includes:

- resource naming
- pass ordering
- texture and buffer declarations
- sampled-pass routing
- image prepasses
- presentation routing
- graph buffer schemas
- load/store and resolve decisions

If a decision can be tested without a GPU, it belongs in preparation or planning.

### C. One subsystem owns one truth

Examples:

- `scene_prep/` owns normalization, reachability, topology, and DataParse baking
- `geometry_resolver/` owns geometry and coordinate-domain inference
- `render_plan/` owns pass planning and planned resources
- `node_compiler/` owns typed expression compilation
- `shader_space/` owns GPU materialization
- `state_machine/` owns validated state-machine semantics
- `animation/` owns playback/session semantics
- `app/` owns UI interaction and presentation
- WebSocket code owns protocol adaptation, not renderer policy

No duplicate "almost-the-same" planning contexts should survive long term.

### D. Dependency direction must stay one-way

Compile/build flow:

`dsl/schema/asset_store -> scene_prep -> geometry_resolver/render_plan/node_compiler/wgsl -> shader_space`

Playback/update flow:

`dsl/state_machine -> animation -> app::scene_runtime -> app/headless`

The opposite direction should not happen.

Examples of dependencies that should not exist:

- `node_compiler` depending on `app`
- `scene_prep` depending on `shader_space`
- `animation` depending on `shader_space`
- `ws` mutating renderer internals directly
- UI code deciding GPU resource names

### E. Two scene time sources, one `Params.time` sink

The codebase has two distinct time sources that both feed into the single `Params.time` uniform
consumed by shaders. This is intentional, not a bug.

#### Source 1: Fixed-step animation time

When an `AnimationSession` is active (scene has a `stateMachine`), time is driven by
`FixedStepClock` in `animation/session.rs`. This clock advances in deterministic fixed-size ticks
(default 60 fps) regardless of real frame rate. It drives:

- state-machine `sceneElapsedTime` and transition progress
- `AnimationStep.scene_time_secs` which is written to `app.runtime.time_value_secs`

The ownership chain:

`FixedStepClock::advance(real_dt) -> AnimationSession::step() -> AnimationStep -> app::frame::advance -> time_value_secs = step.scene_time_secs`

Fixed-step time is deterministic and reproducible. Two runs with the same `real_dt` sequence
produce identical state-machine behavior.

#### Source 2: Continuous wall-clock time

When no `AnimationSession` exists but `time_updates_enabled` is true (the default), time
accumulates continuously from real frame deltas:

`app.runtime.time_value_secs += delta_t`

This drives shader `Time` nodes for scenes that use time-based effects (e.g. `Sin(Time)`) but
have no state machine. It is not deterministic across runs — it depends on actual frame timing.

#### The sink

Both sources write to the same field: `app.runtime.time_value_secs`. In `render_analysis.rs`,
this value is copied into `Params.time` for every pass:

`params.time = app.runtime.time_value_secs` → `update_pass_params(shader_space, pass, &params)`

#### Headless time

Headless one-shot renders (`--headless`) do not advance time. `Params.time` stays at its initial
value (0.0). This is correct for static scene snapshots.

#### Rules

- when an `AnimationSession` is active, it is the sole authority for `time_value_secs` — the
  continuous accumulation path must not run
- when no session exists, continuous accumulation is acceptable for shader preview
- the two sources must never both write in the same frame
- UI-only motion (panel animations, hover effects) should use a separate UI clock and never
  touch `time_value_secs`
- if headless animated export is added in the future, it must use `FixedStepClock` to produce
  deterministic frame sequences, not wall-clock accumulation

### F. Conservative extraction is a feature

Not every large file deserves a new top-level subsystem.

Current examples:

- `app::scene_runtime` should stay in `app/` until there is a second real consumer
- `protocol.rs` is already a clean standalone module and does not need to move
- `dsl.rs` does not need to become five files yet
- the next `ws.rs` split should be narrow, not a new `transport/` umbrella

### G. Pass assemblers must live with the planning layer

The pass assemblers (`render_pass.rs`, `composite.rs`, `gaussian_blur.rs`, `bloom.rs`,
`downsample.rs`, `upsample.rs`, `gradient_blur.rs`) currently live under
`shader_space/pass_assemblers/`. But they are called by `pass_handlers.rs` in `render_plan/`,
and they produce planning artifacts (texture declarations, geometry buffers, render pass specs,
blend states, composition routing) — not GPU resources.

This is a dependency direction violation: the planning layer (`render_plan/`) depends on code
that lives in the materialization layer (`shader_space/`). The `PassPlanner` trait in
`pass_handlers.rs` dispatches to `pass_assemblers::render_pass::assemble_render_pass(…)` etc.,
meaning `render_plan -> shader_space::pass_assemblers` is a real import path today.

The fix is to move `pass_assemblers/` into `render_plan/` (since they produce `RenderPassSpec`,
`TextureDecl`, and other planning types). This also requires co-locating the types they depend on:
`pass_spec.rs`, `resource_naming.rs`, and the `SceneContext`/`BuilderState` argument bundles.

Until this move happens, the planning/materialization boundary is conventional, not structural.
This is the single most important structural inconsistency in the current codebase.

## 3. Near-Term Target Layout

This is the architecture the project should optimize for now.

```text
src/
  main.rs
  lib.rs

  dsl.rs
  schema.rs
  protocol.rs
  asset_store.rs

  app/
    mod.rs
    scene_runtime.rs
    frame/
    canvas/
    texture_bridge.rs

  animation/
    mod.rs
    session.rs
    runloop.rs
    task.rs
    value_pool.rs

  renderer/
    mod.rs
    types.rs
    utils.rs
    validation.rs

    scene_prep/
      mod.rs
      pipeline.rs
      auto_wrap.rs
      group_expand.rs
      composite.rs
      image_inline.rs
      pass_dedup.rs
      data_parse.rs
      graph.rs                 # moved from crate root
      data_parse_runtime.rs    # current ts_runtime.rs re-homed
      types.rs

    geometry_resolver/
      mod.rs
      resolver.rs
      types.rs

    render_plan/
      mod.rs
      planner.rs
      planning_state.rs
      pass_graph.rs
      pass_handlers.rs
      pass_assemblers/           # moved from shader_space/
        mod.rs
        render_pass.rs
        composite.rs
        gaussian_blur.rs
        gradient_blur.rs
        bloom.rs
        downsample.rs
        upsample.rs
      geometry.rs
      blend.rs
      kernel.rs
      pass_spec.rs               # moved from shader_space/
      resource_naming.rs         # moved from shader_space/
      types.rs

    node_compiler/
      mod.rs
      input_nodes.rs
      math_nodes.rs
      math_closure.rs
      vector_nodes.rs
      texture_nodes.rs
      geometry_nodes.rs
      color_nodes.rs
      trigonometry_nodes.rs
      sdf_nodes.rs
      remap_nodes.rs
      glass_material.rs
      attribute.rs
      data_parse.rs
      legacy_nodes.rs            # backward compat; see Section 7

    shader_space/
      mod.rs
      api.rs
      finalizer.rs
      error_space.rs
      headless.rs
      sampler.rs
      image_utils.rs
      texture_caps.rs

  state_machine/
    mod.rs
    runtime.rs
    validation.rs
    timeline.rs
    mutation.rs
    trace.rs
    easing.rs
    types.rs

  ui/
    mod.rs
    components/
    design_tokens.rs
    typography.rs

  ws/
    mod.rs
    scene_delta.rs
    asset_transfer.rs
```

Notes:

- `ws.rs` can remain a single file until the split actually lands; the target is the narrow split
  above, not a broader `transport/` tree.
- `app::scene_runtime.rs` remains in `app/` for now.
- `asset_store.rs` can remain a single file until a real ownership split emerges.
- `dsl.rs` can remain a single file until a minimal split is justified.

## 4. Longer-Term Optional Extractions

These are valid future moves, but only after the near-term ownership work is complete.

- `dsl.rs -> dsl/mod.rs` plus a minimal companion such as `dsl/schema.rs`
- `renderer/wgsl.rs`, `wgsl_bloom.rs`, and `wgsl_gradient_blur.rs -> renderer/wgsl/`
- `asset_store.rs -> asset_store/` if loading/manifests meaningfully diverge
- a top-level runtime module only if there is another runtime consumer besides the app
- broader CLI extraction only if subcommands or materially larger argument parsing arrive

Not part of the ideal near-term target:

- `transport/`
- `telemetry/`
- `cli/`
- speculative subtrees added only for symmetry

## 5. Renderer Architecture

The renderer should settle into five explicit layers.

### Layer 1: Scene ingestion and preparation

Input:

- `SceneDSL`
- asset references
- outputs and render-target settings

Output:

- `PreparedScene`
- preparation report data

Responsibilities:

- schema validation
- default filling
- group expansion
- reachability pruning
- topological sort
- primitive auto-wrap
- connection and output validation
- DataParse baking

This layer is deterministic and GPU-free.

### Layer 2: Geometry and planning

Input:

- `PreparedScene`

Output:

- `RenderPlan`

Responsibilities:

- resolve draw and composition contexts
- determine pass ordering
- determine sampled dependencies
- declare textures, buffers, samplers, and prepasses
- decide presentation routing
- register graph bindings and output routing

This is the most important architecture boundary in the project.

If it cannot be unit-tested without a GPU, it is not fully planned yet.

### Layer 3: Node compilation and WGSL generation

Input:

- planned pass requirements
- pass-local node graph dependencies

Output:

- WGSL shader bundles

Responsibilities:

- compile typed node expressions
- apply coercion rules
- emit pass WGSL
- validate WGSL

Long-term rule:

`node_compiler/` should know about typed expressions and DSL nodes, but not app runtime,
headless export policy, or WebSocket transport.

### Layer 4: ShaderSpace finalization

Input:

- `RenderPlan`
- WGSL bundles
- GPU context

Output:

- `ShaderSpaceBuildResult`

Responsibilities:

- create textures, buffers, and samplers
- register passes and compositions
- upload initial params and graph buffers
- compute `PassBindings`
- compute `pipeline_signature`

This layer is a materializer, not a second planner.

### Layer 5: Runtime update path

Responsibilities:

- choose rebuild vs uniform-only update
- consume animation frame results and shared scene time
- write graph buffers and params
- manage presentation textures
- coordinate app rendering and headless export

This layer consumes `ShaderSpaceBuildResult`. It should not reconstruct planning knowledge.

## 6. Runtime, Transport, and Animation Stance

### App runtime

Today the runtime orchestration lives in `app::scene_runtime`, which is acceptable.

It currently owns:

- rebuild vs uniform-only selection
- cached-scene lifecycle
- graph buffer updates
- wiring incoming scene updates to renderer rebuilds

That can remain in `app/` until there is another real consumer.

### WebSocket transport

The WebSocket layer should only own:

- server lifecycle
- message decoding and encoding
- scene delta application
- asset transfer state
- resync and retry protocol behavior

It should not own:

- renderer rebuild policy
- renderer planning rules
- shader-space mutation internals

The practical split is:

- `ws/mod.rs`: server and client handling
- `ws/scene_delta.rs`: cache and delta application
- `ws/asset_transfer.rs`: chunked asset transfer state machine

### Animation

Animation remains a first-class subsystem:

- `state_machine/` stays pure and reusable
- `animation/` owns playback/session semantics and per-frame outputs
- `app/` forwards events and presents state
- `renderer/` consumes results but does not step animation

Long term, `animation/` should produce explicit frame deltas, and the runtime layer should apply
them to the live scene and renderer state.

## 7. Explicit Decisions for Current Orphans

The ideal architecture needs explicit disposition for the current orphan modules.

### `graph.rs`

- keep the functionality — it has one active caller: `scene_prep/pipeline.rs` imports
  `topo_sort` and `upstream_reachable`
- move it to `scene_prep/graph.rs`
- treat it as scene-graph preparation infrastructure, not a crate-root utility

### `ts_runtime.rs`

- keep the functionality — it has two active callers: `dsl.rs` (inline DataParse evaluation) and
  `scene_prep/data_parse.rs` (bake-time evaluation)
- re-home it under the DataParse path as `scene_prep/data_parse_runtime.rs`
- the `dsl.rs` caller should be updated to import from the new location
- treat it as DataParse runtime support, not a general utility

### `stream.rs`

- 360 bytes, a single `SceneSource` trait with zero importers
- delete it — if a real scene-source abstraction is needed later, it should be designed against
  actual requirements, not this placeholder

### `vm.rs`

- today it has no Rust call sites (confirmed) and `src/shaders/bytecode_vm.wgsl` has no
  `include_str!` references
- the `as_bytes` / `as_bytes_slice` helpers it defines are duplicated in `renderer/utils.rs`
- recommendation: delete both `vm.rs` and `src/shaders/bytecode_vm.wgsl`
- if the bytecode VM work is revived, start fresh with a dedicated `vm/` module rather than
  resurrecting a dead crate-root file

### `shader_space/assembler.rs`

Status: mostly already a shell. `build_shader_space_from_scene_internal()` is ~20 lines that
delegate to `RenderPlanner::plan()` then `ShaderSpaceFinalizer::finalize()`. The planning logic
has already migrated out.

What remains:

- `parse_kernel_source_js_like()` — belongs in `render_plan/kernel.rs` (which already exists)
- `sampled_pass_node_ids()`, `sampled_pass_node_ids_from_roots()`, `deps_for_pass_node()`,
  `visit_pass_node()`, `compute_pass_render_order()` — pass-graph traversal helpers that overlap
  with `render_plan/pass_graph.rs`
- `build_error_shader_space_internal()` — error-space construction (already has a proper home in
  `error_space.rs`)
- ~25 unit tests that test planning behaviors

Action:

- move the orphaned helper functions to their canonical homes in `render_plan/`
- move tests alongside the functions they test
- delete the file once empty
- this is a half-day task, not a phase

### `shader_space/pass_assemblers/args.rs` and `assemble_ctx.rs`

These contain three overlapping types:

- `AssembleContext` (in `assemble_ctx.rs`): owns everything — GPU caps, device, asset store,
  target format, and all the mutable builder vectors (geometry_buffers, textures,
  render_pass_specs, etc.)
- `SceneContext` (in `args.rs`): immutable scene reference — prepared scene, composition
  contexts, draw coord sizes, device, adapter
- `BuilderState` (in `args.rs`): mutable builder state as references into the planner's locals

`SceneContext` + `BuilderState` is the better design — it separates immutable context from
mutable accumulator. `AssembleContext` mixes both and is the older pattern.

Action:

- retire `AssembleContext` — do not merge everything into one blob
- keep the `SceneContext`/`BuilderState` decomposition as the canonical argument pattern
- move both into `render_plan/` alongside the pass assemblers (see Section 2G)
- do not preserve two competing planning vocabularies

### `src/shaders/`

Contains three WGSL files (`bytecode_vm.wgsl`, `fixed_ubo_rect.wgsl`, `simple_rect.wgsl`) with
zero references from Rust code. These are likely artifacts from the bytecode VM era.

Action:

- delete the directory unless the bytecode VM work is revived
- if `vm.rs` is kept, co-locate its WGSL files with it rather than in a separate `shaders/` tree

### `node_compiler/legacy_nodes.rs`

Provides backward-compatible compilers for old node type names (`Float`, `Scalar`, `Constant`,
`Vec2`, `Vec3`, `Vec4`, `Color`, `Add`, `Mul`, `Multiply`, `Mix`, `Clamp`, `Smoothstep`).

Action:

- keep indefinitely — these are part of the DSL compatibility contract with editors
- do not fold into canonical node compiler files; the separation makes the legacy boundary visible
- if a legacy node type is formally retired from the editor protocol, remove its compiler here
  and update the dispatch in `node_compiler/mod.rs`

## 8. Dependency Rules

These rules matter more than the exact file tree.

### Desired dependencies

- `ws -> dsl, protocol, asset_store, app runtime entrypoints`
- `app::scene_runtime -> dsl, asset_store, renderer, animation, ws message types`
- `animation -> dsl, state_machine`
- `renderer::scene_prep -> dsl, schema, asset_store, DataParse runtime support`
- `renderer::geometry_resolver -> scene_prep, renderer::types`
- `renderer::render_plan -> scene_prep, geometry_resolver, renderer::types, pass_assemblers (once co-located)`
- `renderer::wgsl -> node_compiler, renderer::types, validation`
- `renderer::shader_space -> render_plan, wgsl, renderer::types, renderer::utils`
- `ui -> app-facing state only`

### Known violations (to be resolved)

- `render_plan::pass_handlers -> shader_space::pass_assemblers` — the planning layer imports
  code from the materialization layer. Fix: move `pass_assemblers/` into `render_plan/` (see 2G).

### Disallowed dependencies

- `scene_prep -> shader_space`
- `node_compiler -> app`
- `animation -> shader_space`
- `animation -> app`
- `ws -> shader_space`
- `ui -> ws`
- `ui -> animation` internals
- `state_machine -> app`
- `state_machine -> renderer`
- `render_plan -> shader_space` (target state; currently violated by pass_assemblers location)

### DataParse cross-cut

DataParse is a legitimate cross-cut and should be documented as such:

- CPU-side baking and JS evaluation belong on the prep/planning side
- WGSL accessors for baked slots belong in `node_compiler/data_parse.rs`
- the JS runtime support belongs with the DataParse subsystem, not as a root utility

## 9. Migration Order

The recommended order of work is:

### Phase 1: Clean up `assembler.rs` and finish the pipeline split

`assembler.rs` is already mostly a shell — `build_shader_space_from_scene_internal()` just
delegates to `RenderPlanner` then `ShaderSpaceFinalizer`. The remaining work:

- move `parse_kernel_source_js_like()` to `render_plan/kernel.rs`
- move `sampled_pass_node_ids()`, `compute_pass_render_order()`, and related helpers to
  `render_plan/pass_graph.rs` (which already has the canonical versions)
- move `build_error_shader_space_internal()` to `error_space.rs`
- move the ~25 unit tests to their canonical implementation homes
- delete `assembler.rs`

This is a small, low-risk task. Do it first to remove the most visible legacy seam.

### Phase 2: Move pass assemblers into `render_plan/`

This is the most important structural change (see Section 2G):

- move `shader_space/pass_assemblers/` to `render_plan/pass_assemblers/`
- move `shader_space/pass_spec.rs` and `shader_space/resource_naming.rs` to `render_plan/`
- retire `AssembleContext` — keep the `SceneContext`/`BuilderState` decomposition from `args.rs`
  as the canonical argument pattern
- update `shader_space/` imports to reference the new locations
- verify `shader_space/` no longer contains any planning logic

After this phase, the planning/materialization boundary is structurally enforced.

### Phase 3: Split `ws.rs` along natural seams

- extract scene cache and delta application into `ws/scene_delta.rs`
- extract asset transfer into `ws/asset_transfer.rs`
- keep `protocol.rs` where it is
- do not create a `transport/` umbrella

### Phase 4: Audit orphans and dead modules

- move `graph.rs` to `scene_prep/graph.rs` (one caller: `scene_prep/pipeline.rs`)
- move `ts_runtime.rs` to `scene_prep/data_parse_runtime.rs` (two callers: `dsl.rs` and
  `scene_prep/data_parse.rs`); update the `dsl.rs` import path
- delete `stream.rs` (zero importers, placeholder trait)
- delete `vm.rs` (zero call sites, `as_bytes` helpers duplicated in `renderer/utils.rs`)
- delete `src/shaders/` (three WGSL files with zero references from Rust code)

### Phase 5: Do small structural cleanup that now has a real owner

- minimal `dsl.rs` split if justified
- WGSL helper regrouping if justified
- asset-store regrouping if justified
- no extraction just for visual symmetry

### Phase 6: Revisit runtime extraction only if it becomes real

- extract a top-level runtime module only if another non-app consumer appears
- otherwise keep runtime orchestration in `app::scene_runtime`

### Phase 7: Harden the public surface and tests

- keep public entrypoints narrow
- make render-plan and preparation tests the main regression net
- keep GPU-backed render cases as end-to-end validation

## 10. Architectural Anti-Goals

The codebase should explicitly avoid these patterns.

### Anti-goal: a giant smart assembler

One file should not:

- normalize scenes
- infer geometry
- decide pass order
- generate WGSL
- create textures
- upload buffers
- choose runtime update policy

That is exactly the coupling this architecture is trying to eliminate.

### Anti-goal: transport-driven renderer logic

Wire protocol quirks should never dictate renderer internals.

### Anti-goal: folder-first refactors

If a change improves the tree but leaves duplicate planning logic alive, it is cosmetic.

### Anti-goal: hidden update modes

The distinction between rebuild and uniform-only update should remain explicit and testable.

### Anti-goal: multiple competing scene clocks

The two scene time sources (fixed-step and continuous) must never both write `time_value_secs` in
the same frame. When an `AnimationSession` is active, it is the sole time authority. The
continuous fallback only runs when no session exists. A third time source should not be introduced
without updating this contract.

### Anti-goal: duplicate canonical models

There should not be two active versions of:

- pass specs
- resource declarations
- planning contexts
- output routing state

### Anti-goal: mixing UI motion with scene animation

UI polish and scene playback can share concepts, but they should not share ownership.

## 11. What "Done" Looks Like

The project is close to its ideal structure when all of these are true:

- the live renderer path is exactly `prepare -> plan -> finalize`
- `RenderPlan` owns every non-GPU renderer decision
- `ShaderSpaceFinalizer` only materializes planned state
- `assembler.rs` is deleted
- `pass_assemblers/` lives under `render_plan/`, not `shader_space/`
- `AssembleContext` is retired; `SceneContext`/`BuilderState` is the canonical argument pattern
- duplicate planning contexts are gone
- `ws.rs` is split along scene-delta and asset-transfer seams without a new transport supertree
- `graph.rs` and DataParse runtime support live with their real owners in `scene_prep/`
- `stream.rs`, `vm.rs`, and `src/shaders/` are deleted
- `legacy_nodes.rs` is kept as an explicit backward-compat boundary
- runtime update policy is explicit and testable
- animation semantics live in `animation/`, not app glue
- `state_machine/` remains pure and reusable
- the two scene time sources (`FixedStepClock` and continuous fallback) are mutually exclusive
  per frame, with `AnimationSession` taking priority when active
- app, headless, and WebSocket ingestion all reuse the same renderer contracts
- most renderer regressions can be caught without a GPU
- error propagation contracts between pipeline stages are documented

## 12. Practical Rule While Working Toward This

Do not chase the final directory tree first.

The real sequence is:

1. make ownership boundaries true
2. make data contracts explicit
3. remove duplicate logic
4. then rename and reorganize modules

If a change makes the planning/materialization boundary cleaner, it is architectural progress. If
it mostly changes folders while preserving duplicate ownership, it is not.

## 13. Test Architecture

The project's regression safety depends on a layered testing strategy. Each layer has a different
cost/coverage tradeoff.

### Layer 1: Unit tests (GPU-free, fast)

- `node_compiler/` tests: verify typed expression compilation for each node type
- `render_plan/planner.rs` tests: verify plan summaries are stable for known scenes
- `scene_prep/` tests: verify preparation stages (auto-wrap, dedup, group expansion)
- `state_machine/` tests: verify tick behavior, easing, mutation evaluation
- `validation.rs` tests: verify WGSL syntax checking via naga

These are the primary regression net. They run in milliseconds, require no GPU, and should cover
every planning decision. When adding a new node type or pass type, add unit tests here first.

### Layer 2: WGSL golden tests (`cargo test --test render_cases`)

- compare generated WGSL against checked-in golden files in `tests/cases/<case>/wgsl/`
- catch unintended changes to shader output
- update with `UPDATE_GOLDENS=1 cargo test --test render_cases`

Golden tests are sensitive to resource naming and WGSL formatting. Do not update goldens unless
the output change is intentional. When a golden changes, review the diff carefully — unexpected
changes often indicate a planning regression.

### Layer 3: Integration tests (may require GPU context)

- `scene_delta.rs`: verify incremental scene updates produce correct results
- `file_render_target.rs`: verify headless file output
- `animation_values.rs`: verify animation trace generation
- `state_machine.rs`: verify state-machine integration

### Layer 4: End-to-end render tests

- headless render output in `tests/cases/<case>/out/`
- visual verification of rendered output

### When to use which layer

- new node type → unit test in `node_compiler/` + golden WGSL test
- new pass type → unit test in `render_plan/` + golden WGSL test + integration render
- planning logic change → unit test in `render_plan/` + verify goldens unchanged
- shader-space change → integration test + verify goldens unchanged
- WebSocket protocol change → unit test in `ws/` (or future `ws/scene_delta.rs`)
- animation change → unit test in `state_machine/` or `animation/`

### Test-only dependencies

Tests may depend on internal types (e.g. `RenderPlan`, `PreparedScene`) for assertion. This is
acceptable. Tests should not depend on `shader_space` internals when testing planning behavior —
if a test needs GPU context to verify a planning decision, the planning decision has not been
fully extracted yet.

## 14. Error Handling Architecture

### Pipeline stage errors

Each pipeline stage returns `Result<T>` via `anyhow`. Errors propagate upward:

- `prepare_scene()` → validation failures, missing nodes, cycles
- `RenderPlanner::plan()` → geometry resolution failures, missing connections, invalid params
- `ShaderSpaceFinalizer::finalize()` → GPU resource creation failures, WGSL compilation errors
- `ShaderSpaceBuilder::build()` → any of the above, surfaced to the caller

### Error visualization

When `ShaderSpaceBuilder::build()` fails, the app falls back to `build_error()` which creates a
minimal error-visualization shader space. This ensures the window always renders something.

### WebSocket error propagation

WebSocket errors should not crash the renderer. The contract:

- parse errors → send error response to client, keep renderer in last-good state
- scene delta errors → request resync from client
- asset transfer errors → retry with backoff, do not block scene rendering
- internal renderer errors during rebuild → fall back to error shader space, report to client

### Rules

- prefer `anyhow::{Result, Context}` + `anyhow!`/`bail!` for all non-test code
- never panic in production paths — return errors
- `context()` every fallible call with enough information to diagnose the failure
- WebSocket handler must catch all renderer errors and convert to error responses
- headless mode should exit with a non-zero status on renderer errors, not silently produce
  empty output

## 15. Warning: Read This Before Starting Work

This document is sequenced intentionally. The phases in Section 9 are ordered by dependency —
later phases depend on earlier ones being complete.

Common mistakes to avoid:

- do not start with Phase 4 (orphan cleanup) or Phase 5 (structural cleanup) — these are
  cosmetic until the planning/materialization boundary is clean
- do not move `pass_assemblers/` (Phase 2) before cleaning up `assembler.rs` (Phase 1) — the
  helper functions in `assembler.rs` will create merge conflicts
- do not split `ws.rs` (Phase 3) while planning contexts are still duplicated (Phase 2) — the
  split will be harder to review if planning types are still in flux
- do not update WGSL goldens as part of a structural refactor — if goldens change during a
  module move, something went wrong

The real sequence is always: fix ownership → fix contracts → remove duplication → then move files.
