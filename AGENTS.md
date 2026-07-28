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

State, Mutation Function, and Transition are descriptions. MotionEngine solves only independent
motion declarations:

```text
Target phase
  State S -> Mutation Motion returns -> Q,Qdot
  Transition residual                -> E,Edot
  MotionEngine physical state        -> P=Q-E, V=Qdot-Edot

Derived phase
  physical P -> plain Mutation functions -> D(P), e.g. positions/colors

Uniform packing / Render Graph <- P + D(P)
```

For Motion fields, `Q = Mmotion(S)`. `MotionEngine` is the single source of truth for their animated
values, velocities, and driver state.

At the graph boundary:

```text
E = Q - P_current
P_next = P_current + Transition(E)
```

Internally the engine stores the remaining residual `R = E - Transition(E)`, so the same result is
represented as `P_next = Q - R`.

For every independent Motion field, the engine owns:

- resolved State value `S`;
- target value and velocity `Q,Qdot`;
- Mutation driver state, including a continuously retargeted spring;
- final physical value and velocity `P,V`.

Only a Function return typed `Motion<T>` contributes a MotionEngine field. A plain computed return
such as packed positions/colors is a derived render value. It owns no `S/Q/E/P`, velocity, Mutation
driver, Transition driver, or debug motion channel.

Endpoint State overrides define the authored Transition boundary and select property-specific
routes or the Any wildcard. Mutation Motion capability and plain derived outputs never expand that
boundary. Mutation-only Motion channels retain `E=Edot=0`; derived values have no residual at all.

Interruption logic reads only engine-owned `P,V`. After P is solved, the runtime evaluates plain
Mutation outputs from P and merges them with P for uniform packing. A derived value is recomputed,
not interpolated: it animates because its physical dependencies animate.

#### Mutation Function ABI

Mutation Functions use ABI v6:

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

Every Function input is an ordinary value. In the target phase, Mutation boundary declaration
inputs expose resolved `S`, and a Function-to-Function Motion edge exposes resolved `Q`. In the
derived phase, the same declaration inputs and Motion edges expose physical `P`, never the
descriptor object.

`Motion<T>` is return-only. Each Motion output binds exactly once to a formal Mutation output
declaration; that binding supplies MotionEngine identity. The same output may also connect to
downstream Functions. `setTo/to` descriptors are interpreted by MotionEngine's existing drivers;
the Function runtime must never implement a second spring. Writable input handles, consumer-based
identity, and hidden forwarding are forbidden.

#### Mutation transaction

Target evaluation runs the graph slice required by Motion outputs against one cloned MotionEngine.
Motion returns establish Q and downstream target Functions consume resolved Q. Commit only after
the target slice succeeds. A throw, timeout, invalid type, missing binding, shape mismatch, or
duplicate Motion application discards that target transaction and preserves the previous engine.

Only after that atomic Q commit may Transition advance `E,Edot` and publish `P,V`. Derived
evaluation then runs the graph slice required by plain outputs. In this phase Motion descriptors
are not applied: their edges resolve from engine P. A derived-evaluation failure preserves the
previous derived snapshot and never rolls back or mutates MotionEngine.

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
6. Evaluate plain outputs as `D(P)` and send `P + D(P)` to uniform packing and rendering.

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

- implicit Mutation frame overlays or output writes without an explicit output binding;
- writable Function input handles or Function inputs typed as `Motion<T>`;
- implicit dependencies or forwarding not represented by a graph edge;
- putting plain Mutation outputs such as positions into MotionEngine;
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
