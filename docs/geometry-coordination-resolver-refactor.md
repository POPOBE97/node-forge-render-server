# Geometry And Coordination Resolver Refactor

## Status

Implemented in the renderer codebase as of February 2026.

This document is the source of truth for geometry and coordination behavior in the render pipeline.

## Goals

- Move geometry and coordinate-domain decisions behind one canonical resolver entrypoint.
- Ensure all draw-call passes use identical inference rules.
- Treat `Composite` as a routing/target-binding node, not a draw node.
- Make nested composition and pass-to-pass chains deterministic.
- Remove duplicated inference paths between render planning and shader-space assembly.

## Scope

In scope:

- Draw passes: `RenderPass`, `Downsample`, `GuassianBlurPass`, `GradientBlur`.
- Composition routing and target domain inference.
- Geometry placement preservation across both processing and composition edges.
- Nested `Composite -> Composite` behavior.

Out of scope:

- `ComputePass` geometry behavior (unchanged).
- Material expression semantics unrelated to geometry placement.

## Canonical Modules

- Resolver API and types:
  - `src/renderer/geometry_resolver/mod.rs`
  - `src/renderer/geometry_resolver/types.rs`
  - `src/renderer/geometry_resolver/resolver.rs`
- Geometry metric/transform primitives:
  - `src/renderer/render_plan/geometry.rs`
- Composition layer ordering:
  - `src/renderer/scene_prep/composite.rs`
- Runtime consumption and synthetic compose pass generation:
  - `src/renderer/shader_space/assembler.rs`

## Core Data Model

### Node roles

`geometry_resolver::types::NodeRole` classifies nodes as:

- `DrawPass`
- `CompositionRoute`
- `Other`

Draw-pass set is currently fixed by `DRAW_PASS_NODE_TYPES`:

- `RenderPass`
- `GuassianBlurPass`
- `Downsample`
- `GradientBlur`

### Resolver outputs

`resolve_scene_draw_contexts(...) -> Result<ResolvedSceneContexts>` returns:

- `draw_contexts: Vec<ResolvedDrawContext>`
- `composition_contexts: HashMap<String, ResolvedCompositionContext>`
- `composition_consumers_by_source: HashMap<String, Vec<String>>`
- `node_roles: HashMap<String, NodeRole>`

`ResolvedDrawContext` contains:

- source pass (`pass_node_id`)
- downstream consumer edge (`downstream_node_id`, `downstream_port_id`)
- inferred coordinate domain (`CoordDomain`)
- resolved geometry footprint (`ResolvedGeometry`)

`ResolvedCompositionContext` contains:

- composition node id
- target `RenderTexture` node id and resource name
- target size in pixels
- inbound layers in draw order

## Semantics

### 1. Coordinate-domain inference

A coordinate domain is defined only by a `RenderTexture` connected to `Composite.target`.

For each draw context:

- If consumer is `Composite`, use that composition's target domain.
- If consumer is another draw pass, use the nearest downstream `Composite` on the same branch.
- No cross-branch inference is allowed.
- Missing `Composite.target` or non-`RenderTexture` target is a hard error.

### 2. Geometry inference (draw passes only)

- `RenderPass`:
  - Direct `geometry` connection wins.
  - Missing `geometry` falls back to fullscreen of inferred coord domain.
- Non-`RenderPass` draw passes (`Downsample`, `GuassianBlurPass`, `GradientBlur`):
  - Use fullscreen of inferred coord domain.

Resolved geometry reports:

- `size_px`
- `center_px`
- source kind: `DirectGeometry(node_id)` or `FullscreenFallback`

### 3. `Rect2DGeometry` defaults and precedence

Inside `resolve_geometry_for_render_pass(...)`:

- Connected `Vector2Input` on `size`/`position` has highest precedence.
- If unconnected, inline params are used.
- If still missing:
  - `size` defaults to full coord-domain size.
  - `position` defaults to coord-domain center.

Dynamic graph inputs for rect size/position are preserved through graph bindings for runtime vertex use.

### 4. Composition semantics (non-draw)

`Composite` has no geometry inference of its own.

`Composite` is responsible for:

- binding a target texture (`Composite.target`)
- defining layer draw order (`pass` + `dynamic_*` inputs)
- routing inbound pass outputs into that target

Composition output footprint is always full target extent.

### 5. Placement semantics

Placement is preserved for all draw edges:

- `RenderPass` placement center uses resolved geometry position (`geo_x`, `geo_y`)
- Processing chains no longer re-center geometry into local pass space
- Composition still preserves authored placement

### 6. Direct `Composite -> Composite`

Allowed and implemented.

Behavior:

- child composition output is treated as a texture with full target extent
- assembler synthesizes an implicit fullscreen blit pass into parent composition target
- blend preset for this synthetic compose pass is alpha

## Runtime Integration

### Scene prep and traversal

`prepare_scene(...)` keeps the existing contract:

- exactly one `RenderTarget` category node
- `RenderTarget.pass` must come from a `Composite`
- output texture is resolved from `Composite.target`

Resolver additionally performs live pass-like tree-shaking for inference:

- only pass-like nodes reachable from actual composition layers participate
- unconsumed pass chains do not require downstream composition and do not raise false errors

### Shader-space assembler

`build_shader_space_from_scene_internal(...)` uses resolver outputs to:

- choose per-pass coordinate size (`draw_coord_size_by_pass`)
- determine composition consumers per source
- synthesize per-composition compose passes when a sampled pass output must be placed into composition targets
- synthesize direct composition-to-composition blits

This removes prior split-brain behavior where geometry/coord logic diverged between planner and assembler.

## Downsample behavior

Downsample remains two-stage when domains differ:

1. Produce `targetSize` output texture (CPU-resolvable allocation).
2. Composite/upscale into downstream composition domain as needed.

This matches the expected behavior for:

- `ImageTexture(1080x2400) -> Downsample(540x1200) -> Composition(400x400)`
  - downsample to 540x1200, then fullscreen to 400x400

and for geometry-authored sources:

- `Image + Rect -> RenderPass -> Downsample(54x120) -> Composition(400x400)`
  - normalize source footprint for processing, then fullscreen compose to 400x400

and nested composition:

- `Image + Rect -> RenderPass -> Composition(100x100) -> Downsample(54x120) -> Composition(400x400)`
  - preserve placement in 100x100 child composition
  - downsample child output
  - fullscreen compose into 400x400 parent composition

## Transform and Geometry Notes

### Transform path unification

`TransformGeometry` and `SetTransform` now use a shared matrix-upload path:

- TRS and matrix modes are composed CPU-side into 4x4 matrices
- transforms are applied through base/instance matrices in the vertex pipeline
- no special translate-only vertex expression path is used

### GLTF scaling contract

GLTF/OBJ loader path is centralized at:

- `render_plan::load_gltf_geometry_pixel_space(...)`

Current pixel-space mapping intentionally uses isotropic XY scaling based on target half-width:

- `x *= half_w`
- `y *= half_w`
- `z *= half_w`

This preserves authored geometry aspect in the project's pixel-coordinate convention.

## Error Contracts

Common hard validation errors include:

- missing `Composite.target` connection
- `Composite.target` not from `RenderTexture`
- missing direct geometry node where required by pass semantics
- unsupported geometry node type for `RenderPass.geometry`

Resolver-specific behavior:

- dead pass branches are skipped (not errors)
- only live branches rooted at composition layers are validated for downstream domain inference

## Test Coverage Expectations

Unit-level:

- coordinate inference uses closest downstream composition per branch
- no cross-branch contamination
- rect default size/position fallback
- tree-shaking of unconnected processing chains

Integration-level:

- downsample then fullscreen compose
- rect-authored source normalized for processing chain
- nested composition preserve-then-process behavior
- direct composition-to-composition implicit blit
- GLTF/OBJ scaling behavior remains unchanged by resolver refactor
- `ComputePass` unchanged

## Migration Guide For New Pass Types

When adding a new pass node that performs draw calls:

1. Add node type to `DRAW_PASS_NODE_TYPES` in `geometry_resolver/types.rs`.
2. Handle pass dependency traversal in `render_plan/pass_graph.rs`.
3. Add assembler branch in `shader_space/assembler.rs` for output registration and compose routing.
4. Add WGSL generation support in `wgsl.rs` (or dedicated module).
5. Add resolver unit tests plus render-case integration tests.

If a new node is routing-only (composition-like), do not treat it as draw geometry producer.

## Backward Compatibility

- External CLI and WebSocket contracts are unchanged.
- Scene authoring contract remains: composition binds target textures.
- Existing compute-only behavior is unaffected.
