# Node Forge Renderer - Contributor Guide

## Type Coercion Contract

The DSL renderer follows an explicit implicit-conversion contract for port types to ensure graph flexibility while maintaining WGSL type safety.

**Canonical Implementation:** `src/renderer/utils.rs`
- `coerce_to_type(TypedExpr, ValueType)`: Coerces an expression to a specific target type.
- `coerce_for_binary(a, b)`: Finds a common type for binary operations (preferring `f32` or vector splatting).

### Supported Conversions
- **Scalar Numeric**: `f32` ↔ `i32`, `bool` → `f32`/`i32`.
- **Scalar Splat**: `f32`|`i32`|`bool` → `vec2`/`vec3`/`vec4` (e.g., `1.0` becomes `vec3f(1.0)`).
- **Vector Promotion**: `vec2` → `vec3` (z=0.0), `vec3` → `vec4` (w=0.0), `vec2` → `vec4` (z=0.0, w=1.0).
- **Vector Demotion**: `vec4` → `vec3` (xyz), `vec4` → `vec2` (xy), `vec3` → `vec2` (xy).

### Critical Reminder: Vertex Shader Inputs
Nodes targeting vertex shader inputs often have strict type requirements.
- **Example**: `TransformGeometry.translate` must be a `vec3` in the generated WGSL.
- **Contract**: If an upstream graph produces a `vec2` (common for 2D transforms), the compiler **must** call `coerce_to_type(expr, ValueType::Vec3)` to ensure `vec3f(val, 0.0)` is emitted before the vertex shader receives it.

## Resource Naming Protocol

This repo uses string `ResourceName`s to identify GPU resources (passes, textures, buffers) and to
generate stable WGSL bundles/goldens. Names must be:
- ASCII only
- deterministic (no timestamps/random)
- readable (so debug dumps like `/tmp/node-forge-pass__*.wgsl` are useful)
- collision-resistant vs user node ids

### General Rules

- **Separator**: prefer dot-separated segments (`.`). Avoid `__` for new names.
- **Prefixing**:
  - User-authored node ids stay as-is (from DSL).
  - System/generated resources SHOULD be prefixed with `sys.` to avoid collisions.
- **Determinism**: include only stable ids (node id, connection id, or pass id). Never include
  wall-clock times.
- **Scope in name**: encode intent in the name (e.g. `present`, `blur`, `premultiply`).
- **WGSL identifiers**: resource names may contain `.` etc for readability, but any time a name is
  embedded into WGSL identifiers it MUST be sanitized to `[A-Za-z_][A-Za-z0-9_]*`.
  (See `src/renderer/types.rs` `MaterialCompileContext::sanitize_wgsl_ident`.)

### Canonical Patterns

Base = a stable node id (e.g. `RenderTexture` node id) or a stable system id.

- **Textures**:
  - Base texture: `<nodeId>` (e.g. `output`)
  - Derived/presentation texture: `<base>.present.<range>.<transfer>`
    - Current SDR: `<base>.present.sdr.srgb`
    - Future HDR examples:
      - `<base>.present.hdr.pq`
      - `<base>.present.hdr.hlg`

- **Render passes** (suffix `.pass`): `<base>.<role>.pass`
  - Example: `output.present.sdr.srgb.pass`

- **Geometry buffers** (suffix `.geo`): `<base>.<role>.geo`
  - Example: `output.present.sdr.srgb.geo`

- **Uniform/params buffers**: `params.<base>.<role>`
  - Example: `params.output.present.sdr.srgb`

### Legacy Names

Some existing internal resources still use `__` (e.g. `__auto_fullscreen_pass__*`, blur chain
passes like `{layer_id}__downsample_*`). When touching those systems, migrate to `sys.*` +
dot-separated names (e.g. `sys.auto.fullscreen.pass.<edgeId>`, `<layerId>.blur.downsample.<n>`)
but keep changes scoped because pass renames affect WGSL golden stability.
