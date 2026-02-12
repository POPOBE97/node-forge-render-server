# AGENTS.md — node-forge-render-server

Purpose: fast onboarding for agentic coding tools working in this repo.
Keep this file aligned with AGENT.md (contributor rules) and the testing docs.

## Repo overview
- Language: Rust (edition 2024).
- App: local renderer using wgpu/eframe; communicates via WebSocket.
- Node tools: `tools/` contains a small Node.js sender.

## Build / Run
### Build
- Debug build: `cargo build`
- Release build: `cargo build --release`

### Run
- UI mode (default): `cargo run --release`

### Headless render (CLI)
The binary supports one-shot headless rendering from a SceneDSL JSON file.

```bash
cargo run -q -- \
  --headless \
  --dsl-json ./tests/cases/<case>/scene.json \
  --outputdir ./tmp/out
```

Flags (see `src/main.rs`):
- `--headless`
- `--dsl-json <scene.json>`
- `--outputdir <dir>` or `--output <abs/path.png>`
- `--render-to-file` (requires `--output`)

## Tests
### Run all tests
```bash
cargo test
```

### Run LSP diagnostics
```bash
cargo test
```

### Run a single test binary
```bash
cargo test --test render_cases
cargo test --test scene_delta
cargo test --test file_render_target
```

### Run a single test by name
```bash
cargo test scene_delta_applies_in_correct_order_and_preserves_outputs_when_missing
```

### Run a single module’s unit tests
```bash
cargo test --lib renderer::node_compiler::input_nodes
```

### WGSL golden generation tests
WGSL golden comparisons are part of the render cases harness
(`tests/render_cases.rs`, cases under `tests/cases/`).

Update WGSL goldens:
```bash
UPDATE_GOLDENS=1 cargo test --test render_cases
```

Docs: `docs/testing-wgsl-generation.md` (includes background + legacy notes).

## Lint / Format
No repo-specific lint or formatter config found (`rustfmt.toml`, `clippy.toml`).
Recommended:
```bash
cargo fmt
cargo clippy
```

## Key codebase locations
- `src/main.rs`: CLI parsing + UI/headless entrypoints.
- `src/renderer/`: WGSL generation and shader-space construction.
- `src/renderer/node_compiler/`: node compilers by category.
- `tests/`: integration tests (`render_cases.rs`, `scene_delta.rs`, etc.).
- `docs/`: renderer architecture + testing notes.

## Code style guidelines (observed)
### Rust naming
- Types/traits/enums: `PascalCase`.
- Functions/vars/modules: `snake_case`.
- Constants: `SCREAMING_SNAKE_CASE`.

### Imports
- Standard lib first, then third-party, then crate-local (`crate::...`).
- Prefer grouped `use` blocks with nested paths.

### Formatting
- Use rustfmt defaults (4-space indent, trailing commas in multi-line).
- Keep line lengths readable; wrap long match arms and builder calls.

### Error handling
- Prefer `anyhow::Result` + `anyhow!`, `bail!`, `Context` for errors.
- Use early returns for invalid args; avoid panics in non-test code.
- In tests, `panic!`/`unwrap()` is acceptable with context messages.

### Types and conversions
- Use `Option<T>` only for truly optional values; avoid `Option` for error flow.
- Preserve strong typing across WGSL expressions (`ValueType`, `TypedExpr`).
- Avoid ad-hoc numeric conversions; use helpers in `src/renderer/utils.rs`.

## Type Coercion Contract (from AGENT.md)
Canonical implementation: `src/renderer/utils.rs`.

Supported implicit conversions:
- Scalar numeric: `f32` ↔ `i32`, `bool` → `f32`/`i32`.
- Scalar splat: `f32|i32|bool` → `vec2/vec3/vec4`.
- Vector promotion: `vec2 → vec3/vec4`, `vec3 → vec4`.
- Vector demotion: `vec4 → vec3/vec2`, `vec3 → vec2`.

Critical reminder (vertex inputs):
- Some vertex-stage inputs require strict types.
- Example: `TransformGeometry.translate` must be `vec3`.
- If upstream graph yields `vec2`, call `coerce_to_type(..., ValueType::Vec3)`.

## UV conversion (short)
- Internal `in.uv` uses WGSL texture convention: top-left origin.
- GLSL-like local pixel coord uses bottom-left origin via:
  `local_px = vec2(uv.x, 1.0 - uv.y) * geo_size`.
- User-facing `Attribute.uv` is GLSL-like (bottom-left):
  `vec2(in.uv.x, 1.0 - in.uv.y)`.

## Resource Naming Protocol (from AGENT.md)
Rules:
- ASCII only, deterministic, human-readable.
- Prefer dot-separated segments; avoid `__` for new names.
- System-generated resources should use `sys.` prefix.
- Never include timestamps/random values.
- Sanitize names when embedding into WGSL identifiers.

Canonical patterns:
- Texture: `<nodeId>` (base), `<base>.present.sdr.srgb` (presented).
- Render pass: `<base>.<role>.pass`.
- Geometry: `<base>.<role>.geo`.
- Params buffer: `params.<base>.<role>`.

Legacy names:
- Some internals still use `__` (e.g., blur chain). Migrate carefully
  because renaming affects WGSL golden stability.

## UI button styling helper (from AGENT.md)
Use `tailwind_button(...)` in `src/app.rs` to match UI button styling.
Pass a `TailwindButtonGroupPosition` to control rounded corners in button groups
(use `Single` when it’s a standalone button).
Avoid raw `ui.button(...)` for those controls unless you intend to diverge.

## Testing conventions (render cases)
- Each case lives under `tests/cases/<case>/`.
- Expected WGSL lives under `tests/cases/<case>/wgsl/`.
- Output images are written under `tests/cases/<case>/out/`.
- Baselines (if any) are usually `baseline.png` in the case directory.
- When updating outputs, regenerate WGSL goldens via `UPDATE_GOLDENS=1`.

## Node tool (scene sender)
```bash
cd tools
npm install
node tools/ws-send-scene.js assets/node-forge-example.1.json ws://127.0.0.1:8080
```

## Agent tips
- Prefer small, targeted changes; avoid refactors during bug fixes.
- Do not update WGSL goldens unless output changes are intentional.
- Keep resource names stable to avoid breaking baselines/goldens.
- When editing render tests, read `docs/testing-wgsl-generation.md`.

## Project skills
- `implement-new-node`: SOP for adapting a newly-added editor node end-to-end.
  - See: `skills/implement-new-node.md`

## Known config files
- No `.cursorrules`, `.cursor/rules/`, or `.github/copilot-instructions.md` found.
- AGENT.md exists; this file consolidates and expands those rules.
