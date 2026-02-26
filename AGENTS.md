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
  --dsl-json ./tests/cases/<case>/scene.json \
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
- `tests/cases/<case>/scene.json`
- `tests/cases/<case>/wgsl/` expected WGSL
- `tests/cases/<case>/out/` render outputs

## Lint/format
```bash
cargo fmt
cargo clippy
```

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

## Tooling
Node sender:
```bash
cd tools
npm install
node tools/ws-send-scene.js assets/node-forge-example.1.json ws://127.0.0.1:8080
```
