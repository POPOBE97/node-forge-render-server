# Renderer Refactoring Architecture

## Status (2026-03)

The core architectural thesis is unchanged: the renderer should behave like a compiler pipeline.
That pipeline already exists in production form:

- `scene_prep::prepare_scene_with_report()` prepares and validates the input scene.
- `render_plan::planner::RenderPlanner::plan()` builds a `RenderPlan`.
- `shader_space::finalizer::ShaderSpaceFinalizer::finalize()` materializes GPU resources.
- `shader_space::api::ShaderSpaceBuilder` already routes through that path.

This means the remaining work is not "invent the architecture". The remaining work is to remove
the legacy ownership seams that still sit around the planner/finalizer pipeline.

## What Is Actually Left

The most important remaining problems are structural, not cosmetic:

- `src/renderer/shader_space/assembler.rs` is still a large legacy container.
  - `build_shader_space_from_scene_internal()` already delegates to planner + finalizer.
  - pass-graph and kernel helpers still exist there even though canonical versions now live under
    `render_plan/`.
  - the test coverage in that file is useful, but much of it is validating logic that now belongs
    elsewhere.
- Planning state is still split across old names and old homes.
  - `shader_space/assemble_ctx.rs` owns `AssembleContext`.
  - `shader_space/pass_assemblers/args.rs` owns `SceneContext` and `BuilderState`.
  - those structs carry overlapping planning data and preserve a pre-pipeline mental model.
- `src/ws.rs` is too large and mixes unrelated concerns.
  - server lifecycle and client handling
  - scene delta cache/update logic
  - asset transfer state machine
- Several crate-root modules have unclear ownership and need an explicit decision:
  - `src/graph.rs`
  - `src/vm.rs`
  - `src/stream.rs`
  - `src/ts_runtime.rs`

## Anti-Goals

This refactor should not optimize for symmetry over ownership.

- Do not extract `app::scene_runtime` into a top-level `runtime/` module yet.
  - There is only one real consumer today: the app.
  - extraction is only justified once there is another runtime surface that needs the same logic.
- Do not create a broad `transport/` umbrella just to split `ws.rs`.
- Do not split `dsl.rs` into many files until there is a real ownership boundary.
- Do not introduce speculative modules such as `telemetry/` without a concrete consumer.
- Do not rename resources casually or churn WGSL goldens unless behavior is intentionally changing.

## Current Compiler Pipeline

```text
SceneDSL
  -> schema validation
  -> scene_prep::prepare_scene_with_report
  -> geometry_resolver::resolve_scene_draw_contexts
  -> render_plan::planner::RenderPlanner::plan
     -> pass_handlers
     -> pass_assemblers/*
  -> shader_space::finalizer::ShaderSpaceFinalizer::finalize
  -> ShaderSpace / PassBindings / pipeline signature
```

Important nuance: the planner/finalizer split is real, but the planner still drives legacy-shaped
pass assembly adapters (`SceneContext`, `BuilderState`, `AssembleContext`). That is the main debt to
pay down next.

## Revised Priority Order

### Phase 1: Remove Legacy Assembler Ownership

Goal: make `render_plan/` the only home for planning logic.

Facts already true in the codebase:

- `shader_space/api.rs` is the canonical public builder entrypoint.
- `assembler.rs::build_shader_space_from_scene_internal()` already delegates to planner +
  finalizer.
- `assembler.rs` still duplicates logic that now has natural homes:
  - pass dependency traversal and render ordering belong to `render_plan/pass_graph.rs`
  - kernel parsing belongs to `render_plan/kernel.rs`

Work:

- remove the dead wrapper entrypoint once builder/finalizer coverage is sufficient
- migrate or delete duplicate helper functions from `assembler.rs`
- move or rewrite tests so they live with the canonical implementation instead of the legacy file
- reduce `assembler.rs` to code that still truly belongs to error-space/finalization, or delete the
  file entirely if that remainder disappears

Exit criteria:

- planning logic is no longer duplicated between `assembler.rs` and `render_plan/`
- `assembler.rs` is either gone or clearly limited to non-planning responsibilities

### Phase 2: Collapse Duplicate Planning State

Goal: replace the old split state model with one render-plan-owned planning vocabulary.

Facts already true in the codebase:

- pass assemblers are currently called from `render_plan::pass_handlers`
- there is no second active "old assembler" path using those pass assemblers

Work:

- converge `AssembleContext`, `SceneContext`, and `BuilderState` into a single ownership-aligned
  planning state model
- move that state into `render_plan/` under a name that reflects its role
  - `planning_state.rs` is a reasonable candidate
  - `args.rs` is not
- decide whether the current `shader_space/pass_assemblers/` helpers stay temporarily in place or
  move under `render_plan/`
- remove the remaining planner-facing ownership leak from `shader_space/`

Exit criteria:

- planner code uses one coherent state model
- `shader_space/` no longer owns planner state types
- `args.rs` is gone

### Phase 3: Split `ws.rs` Along Natural Seams

Goal: reduce file size and coupling without inventing a broader transport architecture.

Keep:

- `protocol.rs` as the shared message schema module
- one top-level WebSocket integration surface

Split:

- `ws/scene_delta.rs`
  - `SceneDelta`
  - `SceneCache`
  - delta application
  - uniform-only delta detection
- `ws/asset_transfer.rs`
  - upload/download state
  - chunk validation
  - retry/backoff and missing-chunk recovery
- `ws/mod.rs` (or keep `ws.rs` if the directory split is deferred)
  - server loop
  - client handling
  - wiring to `app` and `renderer`

Not planned:

- `transport/`
- `decode.rs` / `encode.rs` / `server.rs` micro-splits
- moving `protocol.rs` just for tree symmetry

### Phase 4: Audit Orphan and Dead Modules

Goal: every remaining module should have an obvious owner or be removed.

- `graph.rs`
  - currently used by `renderer::scene_prep::pipeline`
  - move it near `scene_prep` or `dsl`, whichever ends up owning scene graph utilities
- `ts_runtime.rs`
  - this is DataParse infrastructure, not a general crate-root utility
  - move it next to `scene_prep/data_parse.rs` or another explicitly named DataParse runtime home
- `stream.rs`
  - currently a placeholder only
  - delete it unless a real second scene-source abstraction appears
- `vm.rs`
  - currently has no Rust call sites
  - make an explicit keep-or-kill decision
  - if kept, mark it experimental and co-locate it with the bytecode VM assets
- also audit dead wrappers and helpers still left in `assembler.rs`

Exit criteria:

- no orphan modules remain at crate root without a clear owner
- dead experimental surfaces are either removed or explicitly marked as such

### Phase 5: Small Splits Only Where They Earn Their Keep

After the ownership cleanup above, smaller file splits become straightforward and low risk.

- `dsl.rs`
  - keep as-is for now, or do the minimal split to `dsl/mod.rs` plus a small companion such as
    `schema.rs`
  - avoid creating five small files without a real ownership reason
- WGSL helper modules
  - split only after the planner/assembler debt is gone
- CLI parsing
  - keep it in `main.rs` unless subcommands or materially larger parsing logic arrive

### Phase 6: Deferred Work

These are valid follow-up tasks, but not current refactor drivers:

- runtime extraction from `app::scene_runtime.rs`
- public API hardening beyond the current builder surface
- broader test pyramid work
- telemetry or other speculative infrastructure

## Dependency and Ownership Rules

These rules should guide the remaining moves.

- `scene_prep/` owns:
  - scene normalization
  - reachability pruning
  - topological sort
  - auto-wrap and validation
  - DataParse baking
- `render_plan/` owns:
  - pass dependency traversal
  - pass ordering
  - planning accumulators/state
  - planner-facing pass handlers
- `shader_space/` owns:
  - finalization
  - GPU resource materialization
  - error-space construction
  - runtime resource setup
- `node_compiler/` owns WGSL expression generation, but DataParse is intentionally cross-cut:
  - CPU baking and JS evaluation live on the prep/planning side
  - WGSL accessors for baked slots live in `node_compiler/data_parse.rs`
  - `ts_runtime` belongs to that DataParse path, not as an orphan utility
- `app::scene_runtime` remains app-scoped until another consumer justifies extraction

## Minimal Near-Term Target Tree

This is the target shape worth optimizing for now. It is intentionally conservative.

```text
src/
├── app/
│   └── scene_runtime.rs
├── protocol.rs
├── ws/
│   ├── mod.rs
│   ├── scene_delta.rs
│   └── asset_transfer.rs
├── renderer/
│   ├── geometry_resolver/
│   ├── scene_prep/
│   │   ├── data_parse.rs
│   │   ├── graph.rs
│   │   └── ...
│   ├── render_plan/
│   │   ├── planner.rs
│   │   ├── pass_graph.rs
│   │   ├── pass_handlers.rs
│   │   ├── planning_state.rs
│   │   └── ...
│   └── shader_space/
│       ├── api.rs
│       ├── finalizer.rs
│       ├── error_space.rs
│       └── ...
└── dsl.rs
```

Notes:

- `graph.rs` should move before any broader `dsl/` split.
- `pass_assemblers/` may temporarily remain where it is while planning state is collapsed, but it
  should not be treated as a permanent `shader_space/` concern.
- `protocol.rs` stays where it is.
- `scene_runtime.rs` stays where it is.

## Concrete Next Moves

If work starts immediately, the recommended order is:

1. delete the remaining planner wrapper/debt in `assembler.rs`
2. collapse `AssembleContext` and `SceneContext` / `BuilderState`
3. split `ws.rs` into `scene_delta` and `asset_transfer`
4. move `graph.rs`, re-home `ts_runtime.rs`, and make explicit decisions on `stream.rs` and `vm.rs`
5. only then do cosmetic file-tree cleanup such as `dsl.rs` or WGSL helper splits

That sequence keeps the refactor focused on real ownership wins instead of directory churn.
