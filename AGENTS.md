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

<!-- gitnexus:start -->
# GitNexus — Code Intelligence

This project is indexed by GitNexus as **node-forge-render-server** (3556 symbols, 9244 relationships, 266 execution flows). Use the GitNexus MCP tools to understand code, assess impact, and navigate safely.

> If any GitNexus tool warns the index is stale, run `npx gitnexus analyze` in terminal first.

## Always Do

- **MUST run impact analysis before editing any symbol.** Before modifying a function, class, or method, run `gitnexus_impact({target: "symbolName", direction: "upstream"})` and report the blast radius (direct callers, affected processes, risk level) to the user.
- **MUST run `gitnexus_detect_changes()` before committing** to verify your changes only affect expected symbols and execution flows.
- **MUST warn the user** if impact analysis returns HIGH or CRITICAL risk before proceeding with edits.
- When exploring unfamiliar code, use `gitnexus_query({query: "concept"})` to find execution flows instead of grepping. It returns process-grouped results ranked by relevance.
- When you need full context on a specific symbol — callers, callees, which execution flows it participates in — use `gitnexus_context({name: "symbolName"})`.

## When Debugging

1. `gitnexus_query({query: "<error or symptom>"})` — find execution flows related to the issue
2. `gitnexus_context({name: "<suspect function>"})` — see all callers, callees, and process participation
3. `READ gitnexus://repo/node-forge-render-server/process/{processName}` — trace the full execution flow step by step
4. For regressions: `gitnexus_detect_changes({scope: "compare", base_ref: "main"})` — see what your branch changed

## When Refactoring

- **Renaming**: MUST use `gitnexus_rename({symbol_name: "old", new_name: "new", dry_run: true})` first. Review the preview — graph edits are safe, text_search edits need manual review. Then run with `dry_run: false`.
- **Extracting/Splitting**: MUST run `gitnexus_context({name: "target"})` to see all incoming/outgoing refs, then `gitnexus_impact({target: "target", direction: "upstream"})` to find all external callers before moving code.
- After any refactor: run `gitnexus_detect_changes({scope: "all"})` to verify only expected files changed.

## Never Do

- NEVER edit a function, class, or method without first running `gitnexus_impact` on it.
- NEVER ignore HIGH or CRITICAL risk warnings from impact analysis.
- NEVER rename symbols with find-and-replace — use `gitnexus_rename` which understands the call graph.
- NEVER commit changes without running `gitnexus_detect_changes()` to check affected scope.

## Tools Quick Reference

| Tool | When to use | Command |
|------|-------------|---------|
| `query` | Find code by concept | `gitnexus_query({query: "auth validation"})` |
| `context` | 360-degree view of one symbol | `gitnexus_context({name: "validateUser"})` |
| `impact` | Blast radius before editing | `gitnexus_impact({target: "X", direction: "upstream"})` |
| `detect_changes` | Pre-commit scope check | `gitnexus_detect_changes({scope: "staged"})` |
| `rename` | Safe multi-file rename | `gitnexus_rename({symbol_name: "old", new_name: "new", dry_run: true})` |
| `cypher` | Custom graph queries | `gitnexus_cypher({query: "MATCH ..."})` |

## Impact Risk Levels

| Depth | Meaning | Action |
|-------|---------|--------|
| d=1 | WILL BREAK — direct callers/importers | MUST update these |
| d=2 | LIKELY AFFECTED — indirect deps | Should test |
| d=3 | MAY NEED TESTING — transitive | Test if critical path |

## Resources

| Resource | Use for |
|----------|---------|
| `gitnexus://repo/node-forge-render-server/context` | Codebase overview, check index freshness |
| `gitnexus://repo/node-forge-render-server/clusters` | All functional areas |
| `gitnexus://repo/node-forge-render-server/processes` | All execution flows |
| `gitnexus://repo/node-forge-render-server/process/{name}` | Step-by-step execution trace |

## Self-Check Before Finishing

Before completing any code modification task, verify:
1. `gitnexus_impact` was run for all modified symbols
2. No HIGH/CRITICAL risk warnings were ignored
3. `gitnexus_detect_changes()` confirms changes match expected scope
4. All d=1 (WILL BREAK) dependents were updated

## CLI

- Re-index: `npx gitnexus analyze`
- Check freshness: `npx gitnexus status`
- Generate docs: `npx gitnexus wiki`

<!-- gitnexus:end -->
