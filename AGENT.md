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
