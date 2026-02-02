# Skill: implement-new-node

Purpose: implement a newly-added editor node end-to-end (scheme → compiler → tests) with stable WGSL output and correct type coercions.

## Inputs
- The editor “adaptation note” (node type string, port IDs/types, params + defaults, semantics).
- A test case directory under `tests/cases/<case>/` that exercises the node.

## Checklist (SOP)

### 1) Confirm scheme entry (ports + defaults)
- Open `assets/node-scheme.json` and find the node entry by `"type": "<NodeType>"`.
- Verify:
  - input port IDs + types (must match the exported SceneDSL connections)
  - output port IDs + types
  - `defaultParams` includes required defaults (e.g. `ior: 1.5`)
- If missing/incorrect, update the scheme (this is what `dsl::normalize_scene_defaults` merges into `Node.params`).

### 2) Find compile dispatch location
- Material/vertex expression compilation dispatch is string-based:
  - `src/renderer/node_compiler/mod.rs` → `match node.node_type.as_str()`
- Add a new match arm for the node type string.

### 3) Implement the node compiler
- Prefer placing the compiler next to similar nodes (e.g. vector ops in `vector_nodes.rs`).
- Follow existing compiler signature pattern:
  - `pub fn compile_<node>(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn) -> Result<TypedExpr>`
- Inputs:
  - Use `incoming_connection(scene, &node.id, "<portId>")` for required inputs.
  - For optional inputs with defaults:
    - if connected: compile upstream and `coerce_to_type(..., expected)`
    - else: read `node.params["<portId>"]` (defaults already merged by scheme) and emit a stable literal.
- Type coercion:
  - Use `crate::renderer::utils::coerce_to_type` to implement PORT_TYPE_COMPATIBILITY behavior.
  - Do coercions at the semantic boundary described by the adaptation note (e.g. **coerce to vec3 before normalize**).
- Output:
  - Validate `out_port` and `bail!` on unsupported ports.
  - Return `TypedExpr::with_time(...)` with correct `ValueType` and `uses_time` propagation.

### 4) Run targeted render case test
- List render-case tests:
  - `cargo test --test render_cases -- --list`
- Run the specific case:
  - `cargo test --test render_cases case_<case_name>`

### 5) Golden updates (only if intentional)
- If WGSL output changes are intended, update goldens:
  - `UPDATE_GOLDENS=1 cargo test --test render_cases case_<case_name>`
- Otherwise, treat golden diffs as regressions to fix.

### 6) Full verification
- Run:
  - `cargo test`
- Ensure no new type errors; do not suppress with `as any` / `@ts-ignore` (not applicable in Rust, but same spirit: no hacks).

## Notes / Known conventions
- Scene node types are **strings** (`Node.node_type`), not Rust enums.
- Defaults come from the scheme:
  - `src/dsl.rs`: `normalize_scene_defaults()` merges `defaultParams` into `Node.params`.
- Canonical coercion implementation lives in:
  - `src/renderer/utils.rs` (`coerce_to_type`).
