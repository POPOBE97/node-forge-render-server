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

Use `S` for authored/inherited semantic State values, `Q,Qdot` for Mutation targets, `E,Edot` for
the remaining State-switch residual, and `P,V` for final physical values:

```text
State override / inherit -> S
State mutationGraph M(S,t) -> Q,Qdot
Transition residual -> E,Edot
MotionEngine -> P=Q-E, V=Qdot-Edot
Derivation D(P, frame inputs) -> GPU uniforms
Render Graph
```

- A regular State owns one private `mutationGraph`; Entry, Any, and Exit own none.
- Mutation Functions exist only in `state:` scopes. Their `Motion<T>` returns create MotionEngine
  channels and establish `Q,Qdot`.
- Derivation Functions exist only in `derivation:` scopes. They are pure render derivations, cannot
  return Motion, cannot mutate MotionEngine, and read final `P` rather than `S` or `Q`.
- State overrides alone define Transition eligibility and wildcard routing. Mutation-only channels
  keep `E=Edot=0`; Derivation outputs have no MotionEngine state.
- Rendering consumes absolute `P_render` values from final `P` and Derivation outputs. `S` and `Q`
  never bypass Transition into rendering.

#### Mutation Function ABI

Graph Functions use ABI v8. Mutation Functions expose the Motion helpers:

```ts
interface Motion<T> {
  readonly __nodeForgeMotionValue: T
}

declare function setTo<T>(target: T, velocity?: T): Motion<T>
declare function to<T>(
  target: T,
  spring: { duration: number; bounce: number },
): Motion<T>
```

Every Function input is an ordinary value. `Motion<T>` is return-only and each Motion output binds
exactly once to a formal State Output declaration. Derivation Functions have no Motion helpers and
must return ordinary values. Function resources carry `kind`; scope, node type, ABI, source hash,
and reflected ports must agree.

#### Mutation transaction

Mutation evaluation runs against a cloned MotionEngine transaction. A throw, timeout, invalid type,
missing binding, fixed-length mismatch, or duplicate Motion writer discards the transaction.
Activation failure rejects the State switch; ordinary-frame failure preserves the last committed
MotionEngine state. After Transition publishes `P,V`, the active shared Derivation runs. A failed
Derivation preserves only that Derivation's previous successful snapshot and never rolls back
MotionEngine.

One declaration may receive at most one Motion return per frame. Duplicate applications are
diagnostics, never last-write-wins. Each driver advances at most once per frame.

Repeated per-frame `to(target, spring)` retargets the existing spring from its saved value and
velocity and advances it once; it never recreates the driver or resets elapsed time merely because
the Function called it again. `setTo` writes `Q` directly. With no explicit velocity, finite
difference adjacent target samples; activation at `dt=0` starts at zero velocity and does not
advance sample history.

#### State activation and Transition

On entry:

1. Save the old physical `P0,V0`.
2. Resolve target State overrides plus inherited values into `S` and reset local time.
3. Evaluate Motion returns with `dt=0`. A returned `setTo` descriptor replaces the S-seeded Q
   field; `to` integrates from S.
4. For endpoint State override fields only, create `E0=Q0-P0` and
   `Edot0=Qdot0-V0`.
5. Advance target `Q,Qdot` and Transition residual `E,Edot`, producing physical `P,V`.
6. Evaluate the bound Derivation as `D(P, frame_inputs)` and send absolute render values to uniform
   packing and rendering.

Each tick first updates Motion `Q,Qdot`, advances the Transition ErrorDriver toward zero, stores
`P=Q-E` and `V=Qdot-Edot`, and finally recomputes derived values from P. Spring, timeline, and
instant Transition nodes operate on the residual selected by the authored State route:
spring/timeline decay it to zero; instant sets it to zero.

Transition completion tests only `E,Edot`. A Mutation spring may continue after
`active_transition_id` clears. If Mutation and Transition both use springs, they drive different
systems: Mutation drives `Q`; Transition drives `E`. Interruption snapshots that frame's final
`P,V`, establishes the new target `Q,Qdot`, and rebuilds error, preserving physical continuity.

#### Debug and trace

Per-channel diagnostics expose target `Q,Qdot`, transition error `E,Edot`, final `P,V`, Mutation
driver kind, Transition driver kind, completion, and active transition identity. Debugging must not
infer any of these from renderer uniforms.

#### Forbidden models

- using “Mutation” for stateless render derivation;
- implicit Derivation output writes without an explicit output declaration and binding;
- writable Function input handles or Function inputs typed as `Motion<T>`;
- implicit dependencies or forwarding not represented by a graph edge;
- putting Derivation outputs such as positions or colors into MotionEngine;
- giving derived outputs Q/E/P, velocities, springs, Transition ports, or wildcard matches;
- deriving render values from S or Q after physical P has been solved;
- sending Q or a Q overlay directly to rendering;
- Transition follow/repeat drivers or persistent transition channels;
- independent animated values, velocities, or driver state outside `MotionEngine`;
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
