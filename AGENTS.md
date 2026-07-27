# AGENTS.md — node-forge-render-server

Purpose: compact onboarding for coding agents working in this repo.

## Repo snapshot
- Rust 2024.
- Local renderer (`wgpu`/`eframe`) + WebSocket.
- Node sender tool under `tools/`.

## Build and run
```bash
cargo build
cargo build --release
cargo run --release
```

Headless one-shot render:
```bash
cargo run -q -- \
  --headless \
  --nforge ./tests/fixtures/render/<group>/<case>/scene.nforge \
  --outputdir ./tmp/out
```

Headless flags (`src/main.rs`): `--headless`, `--dsl-json`, `--outputdir` or `--output`, `--render-to-file` (requires `--output`).

## Test commands
```bash
cargo test
cargo test --test render_cases
cargo test --test scene_delta
cargo test --test file_render_target
cargo test scene_delta_applies_in_correct_order_and_preserves_outputs_when_missing
cargo test --lib renderer::node_compiler::input_nodes
```

WGSL goldens:
```bash
UPDATE_GOLDENS=1 cargo test --test render_cases
```

Render-case layout:
- `tests/fixtures/render/editor-examples/<case>/scene.nforge`
- `tests/fixtures/render/renderer-only/<case>/scene.nforge`
- `<case>/expected/wgsl/` expected WGSL and `<case>/expected/baseline.*` images
- `<case>/out/` ignored render outputs

## Lint/format
```bash
cargo fmt
cargo clippy
```

For targeted edits, do not run broad whole-repo formatting unless the user explicitly asks for a
formatting pass. `cargo fmt` can reflow unrelated Rust files in a dirty worktree and create noisy
diffs. Prefer small `apply_patch` edits; if formatting is truly needed, restrict it as much as
practical and revert unrelated formatting churn before finishing.

## Key paths
- `src/main.rs`: CLI + UI/headless entry.
- `src/renderer/`: WGSL generation and shader-space construction.
- `src/renderer/node_compiler/`: node compiler implementations.
- `tests/`: integration harnesses (`render_cases.rs`, `scene_delta.rs`, etc.).
- `docs/testing-wgsl-generation.md`: golden-testing details.

## Coding conventions
- Naming: `PascalCase` types, `snake_case` functions/modules/vars, `SCREAMING_SNAKE_CASE` constants.
- Imports: std, third-party, then crate-local.
- Errors: prefer `anyhow::{Result, Context}` + `anyhow!`/`bail!`; avoid panics in non-test code.
- Keep type correctness explicit in WGSL graph code (`ValueType`, `TypedExpr`).

## Renderer invariants (do not break)

### MotionEngine Runtime Architecture

State, Mutation Function, and Transition are descriptions. `MotionEngine` is the single source of
truth for all live values, velocities, and driver state:

```text
State S + Mutation Function M + Transition T + frame inputs u(t)
                              │
                              ▼
MotionEngine
  Mutation setTo/to -> target system Q,Qdot
  Transition driver -> error system E,Edot
  physical state    -> P=Q+E, V=Qdot+Edot
                              │
                              ▼
Uniform packing / Render Graph
```

For every writable declaration, the engine owns:

- target value and velocity `Q,Qdot`;
- Mutation driver state, including a continuously retargeted spring;
- transition error and error velocity `E,Edot`;
- Transition driver state;
- final physical value and velocity `P,V`.

The renderer, condition/debug readers that need presentation state, the next Transition, and
interruption logic read only engine-owned `P,V`. There is no post-motion snapshot plus Mutation
overlay.

#### Mutation Function ABI

A Function input bound to a declaration present in both `MutationDefinition.inputs` and
`MutationDefinition.outputs` is a call-scoped handle:

```ts
interface MotionParam<T> {
  readonly value: T
  readonly velocity: T
  setTo(target: T, velocity?: T): T
  to(target: T, spring: { duration: number; bounce: number }): T
}
```

`value` and `velocity` expose `Q,Qdot`, not `P,V`. `setTo` and `to` forward to the same
`MotionEngine` implementations used elsewhere; the Function runtime must never implement a second
spring. Both methods return the resulting `Q` for same-frame pure derivation. Ordinary Function
return values may feed pure graph computation but never write a uniform implicitly.

Handles derive identity from the formal Mutation input binding (`nodeId:paramName`). They are
unforgeable, valid only during one call, and cannot target strings, consumers, `GroupInstance`
nodes, or group-internal renderer nodes. `MutationDefinition.outputs` is the writable capability
list used for validation, editor UI, tracing, and handle creation.

#### Mutation transaction

Run all active Mutation Functions for a frame against one cloned MotionEngine working state.
Commit that state atomically only after the entire Mutation graph succeeds. A throw, timeout,
invalid type, missing capability, or duplicate motion call discards the working state and preserves
the previous committed engine exactly.

One declaration may receive at most one `setTo` or `to` call per frame. Duplicate calls are
diagnostics, never last-write-wins. Each driver advances at most once per frame.

Repeated per-frame `to(target, spring)` retargets the existing spring from its saved value and
velocity and advances it once; it never recreates the driver or resets elapsed time merely because
the Function called it again. `setTo` writes `Q` directly. With no explicit velocity, finite
difference adjacent target samples; activation at `dt=0` starts at zero velocity and does not
advance sample history.

#### State activation and Transition

On entry:

1. Save the old physical `P0,V0`.
2. Activate the target State and reset its local time.
3. Run its Mutation Function once with `dt=0` to establish `Q0,Qdot0`.
4. Create `E0=P0-Q0` and `Edot0=V0-Qdot0`.

Each tick runs Mutation first to update `Q,Qdot`, advances the Transition ErrorDriver toward zero,
then stores `P=Q+E` and `V=Qdot+Edot`. Spring, timeline, and instant Transition nodes all operate on
error: spring/timeline decay it to zero; instant sets it to zero.

Transition completion tests only `E,Edot`. A Mutation spring may continue after
`active_transition_id` clears. If Mutation and Transition both use springs, they drive different
systems: Mutation drives `Q`; Transition drives `E`. Interruption snapshots that frame's final
`P,V`, establishes the new target `Q,Qdot`, and rebuilds error, preserving physical continuity.

#### Debug and trace

Per-channel diagnostics expose target `Q,Qdot`, transition error `E,Edot`, final `P,V`, Mutation
driver kind, Transition driver kind, completion, and active transition identity. Debugging must not
infer any of these from renderer uniforms.

#### Forbidden models

- Mutation frame overlays or post-motion patches;
- Function return value to uniform writes;
- Mutation output/passthrough value propagation as a write path;
- Transition follow/repeat drivers or persistent transition channels;
- runtime state outside `MotionEngine`;
- implicit target resolution through consumers, IDs, suffixes, group bindings, or port names.

The renderer executes the canonical Render Graph and never reconstructs missing authoring intent.
Treat missing declarations, graph edges, or bindings as schema errors. Preserve semantic types
through GPU packing; ABI-compatible storage such as `packed<color>` and `packed<vector4>` does not
make those types interchangeable.

Type coercion (`src/renderer/utils.rs`):
- Scalar numeric: `f32` <-> `i32`, `bool` -> `f32`/`i32`
- Scalar splat: `f32|i32|bool` -> `vec2/vec3/vec4`
- Vector promote: `vec2 -> vec3/vec4`, `vec3 -> vec4`
- Vector demote: `vec4 -> vec3/vec2`, `vec3 -> vec2`
- Vertex strictness example: `TransformGeometry.translate` must be `vec3`; coerce `vec2` inputs with `coerce_to_type(..., ValueType::Vec3)`.

UV convention:
- Internal `in.uv` is top-left origin.
- GLSL-like local pixel coord: `local_px = vec2(uv.x, 1.0 - uv.y) * geo_size`.
- User-facing `Attribute.uv`: `vec2(in.uv.x, 1.0 - in.uv.y)`.

Resource naming:
- ASCII, deterministic, readable.
- Prefer dot-separated names; avoid introducing new `__` names.
- System names use `sys.` prefix.
- No timestamps/random suffixes.
- Patterns: texture `<nodeId>` / `<base>.present.sdr.srgb`, pass `<base>.<role>.pass`, geometry `<base>.<role>.geo`, params `params.<base>.<role>`.
- Legacy `__` internals exist; rename cautiously because WGSL goldens are sensitive.

UI helper:
- Use `tailwind_button(...)` in `src/app.rs` with `ButtonGroupPosition` (`Single` for standalone controls) unless intentionally diverging.

## Working style
- Prefer small targeted edits over broad refactors.
- Keep resource naming stable.
- Do not update WGSL goldens unless output changes are intentional.
- Persisted schema changes do not keep compatibility loaders. The canonical archive set is
  `../node-forge-editor/examples/*.nforge`: upgrade affected examples once, sync the corresponding
  `tests/fixtures/render/editor-examples/*/scene.nforge` fixtures through the parent script, and delete all migration/fallback
  code and migration tests before finishing.

## Tooling
Node sender:
```bash
cd tools
npm install
node tools/ws-send-scene.js assets/node-forge-example.1.json ws://127.0.0.1:8080
```
