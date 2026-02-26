# Renderer Module

This directory contains the modular renderer implementation for compiling DSL scenes to WGSL and building ShaderSpaces.

## Structure

```
src/renderer/
├── mod.rs                          # Module entry point and re-exports
├── types.rs                        # Core type definitions
├── utils.rs                        # Utility functions
├── validation.rs                   # WGSL validation using naga (4 tests)
├── scene_prep/                     # Scene preparation and validation
├── render_plan/                    # Pass graph + geometry metric helpers
├── geometry_resolver/              # Canonical coord/geometry context resolver
├── wgsl.rs                         # WGSL shader generation
├── shader_space/                   # ShaderSpace construction
└── node_compiler/                  # Node compilation infrastructure
    ├── mod.rs                      # Compiler framework and dispatch
    ├── input_nodes.rs              # ColorInput, FloatInput, etc. (5 tests)
    ├── math_nodes.rs               # MathAdd, MathMultiply, etc. (2 tests)
    ├── attribute.rs                # Attribute node (4 tests)
    ├── texture_nodes.rs            # ImageTexture (3 tests)
    ├── trigonometry_nodes.rs       # Sin, Cos, Time (3 tests)
    ├── vector_nodes.rs             # VectorMath, DotProduct, etc. (4 tests)
    ├── color_nodes.rs              # ColorMix, ColorRamp, HSVAdjust (3 tests)
    └── legacy_nodes.rs             # Backward compatibility nodes (3 tests)
```

## Module Responsibilities

### types.rs
Core type definitions used throughout the renderer:
- `ValueType` - WGSL type enum (F32, Vec2, Vec3, Vec4)
- `TypedExpr` - Typed WGSL expression with time dependency tracking
- `MaterialCompileContext` - Tracks image textures and bindings during compilation
- `Params` - Uniform parameters passed to render passes
- `PassBindings` - Binding information for render passes
- `WgslShaderBundle` - Complete WGSL shader bundle (vertex, fragment, module)

### utils.rs
Utility functions for WGSL generation:
- `fmt_f32()` - Format floats for WGSL (removes trailing zeros)
- `array8_f32_wgsl()` - Format float arrays as WGSL array literals
- `sanitize_wgsl_ident()` - Convert strings to valid WGSL identifiers
- `splat_f32()` - Splat scalar to vector types
- `coerce_for_binary()` - Type coercion for binary operations
- `to_vec4_color()` - Convert expressions to vec4 color format
- `decode_data_url()` - Parse data URLs for image loading
- Memory utilities (`as_bytes`, `as_bytes_slice`)

### validation.rs
WGSL validation using the naga library:
- `validate_wgsl()` - Validate WGSL source code
- `validate_wgsl_with_context()` - Validate with contextual error messages
- Includes 4 unit tests covering valid/invalid WGSL cases

### scene_prep/
Scene preparation and validation:
- `prepare_scene()` - Prepare scene for rendering (topological sort, validation)
- Internal scene-prep stages auto-wrap primitive values into render passes
- Port type utilities for connection validation
- Composition layer ordering (`Composite.pass` + `dynamic_*`)
- Output contract: `RenderTarget.pass <- Composite` and `Composite.target <- RenderTexture`

### render_plan/
Render-plan helpers used by WGSL generation and ShaderSpace assembly:
- `pass_graph.rs` - Pass dependency traversal and sampled-pass detection
- `geometry.rs` - Canonical geometry metrics/transform resolution for draw paths
- Shared GLTF/OBJ pixel-space loader (`load_gltf_geometry_pixel_space`)

### geometry_resolver/
Canonical coordination and geometry context inference:
- `resolve_scene_draw_contexts(..., asset_store)` - Single entrypoint for draw-pass context resolution
- Node role classification: `DrawPass`, `CompositionRoute`, `Other`
- Resolved geometry placement is preserved across both processing and composition edges
- Composition contexts and consumer mappings used by assembler

### wgsl.rs
WGSL shader generation:
- `build_pass_wgsl_bundle()` - Generate WGSL bundle for a render pass
- `build_all_pass_wgsl_bundles_from_scene()` - Generate WGSL for all passes
- Gaussian blur utilities for post-processing effects
- Helper functions for formatting WGSL code

### shader_space/
ShaderSpace construction:
- `ShaderSpaceBuilder` - Build complete ShaderSpace from scene
- `ShaderSpaceBuilder::build_error()` - Build error visualization ShaderSpace
- `update_pass_params()` - Update uniform parameters for passes
- Texture creation, geometry buffers, uniform buffers, pipelines
- Resolver-driven pass placement and composition routing
- Implicit `Composite -> Composite` fullscreen blit synthesis

### node_compiler/
Node compilation infrastructure and implementations:

#### mod.rs
- `compile_material_expr()` - Main dispatch function for compiling material expressions
- Dispatches to specific node compiler modules based on node type
- Test utilities for creating test scenes

#### input_nodes.rs
Compilers for constant input nodes:
- `compile_color_input()` - ColorInput nodes (vec4f RGBA constants)
- `compile_float_or_int_input()` - FloatInput/IntInput nodes (f32 constants)
- `compile_vector2_input()` - Vector2Input nodes (vec2f constants)
- `compile_vector3_input()` - Vector3Input nodes (vec3f constants)

**Tests**: 5 unit tests covering default/custom parameters

#### math_nodes.rs
Compilers for math operation nodes:
- `compile_math_add()` - Add two values with type promotion
- `compile_math_multiply()` - Multiply two values with type promotion
- `compile_math_clamp()` - Clamp value between min/max
- `compile_math_power()` - Raise base to exponent

**Tests**: 2 unit tests covering basic operations

#### attribute.rs
Compiler for attribute nodes:
- `compile_attribute()` - Read vertex attributes (currently supports UV)

**Tests**: 4 unit tests covering default/case-insensitive/error cases

#### texture_nodes.rs
Compilers for texture nodes:
- `compile_image_texture()` - ImageTexture nodes with texture sampling

**Tests**: 3 unit tests covering texture binding registration

#### trigonometry_nodes.rs
Compilers for trigonometric and time nodes:
- `compile_sin()` - Sine function
- `compile_cos()` - Cosine function
- `compile_time()` - Animation time value

**Tests**: 3 unit tests covering sin/cos/time operations

#### vector_nodes.rs
Compilers for vector math nodes:
- `compile_vector_math()` - Generic vector math operations
- `compile_dot_product()` - Dot product of two vectors
- `compile_cross_product()` - Cross product of two vec3 vectors
- `compile_normalize()` - Normalize vector to unit length

**Tests**: 4 unit tests covering vector operations

#### color_nodes.rs
Compilers for color manipulation nodes:
- `compile_color_mix()` - Mix two colors based on a factor
- `compile_color_ramp()` - Map scalar through color gradient
- `compile_hsv_adjust()` - Adjust hue, saturation, value

**Tests**: 3 unit tests covering color operations

#### legacy_nodes.rs
Compilers for backward compatibility with legacy node types:
- `compile_float_scalar_constant()` - Float/Scalar/Constant nodes
- `compile_vec2/3/4_color()` - Legacy vector/color nodes
- `compile_add/mul/mix/clamp/smoothstep()` - Legacy math operations

**Tests**: 3 unit tests covering legacy node compatibility

## Usage Examples

### Compiling a Simple Node

```rust
use crate::renderer::node_compiler::input_nodes;

// Create a ColorInput node
let node = Node {
    id: "color1".to_string(),
    node_type: "ColorInput".to_string(),
    params: HashMap::from([
        ("value".to_string(), json!([1.0, 0.0, 0.0, 1.0]))
    ]),
    inputs: Vec::new(),
    outputs: Vec::new(),
};

// Compile to WGSL
let result = input_nodes::compile_color_input(&node, None)?;
// result.expr = "vec4f(1, 0, 0, 1)"
// result.ty = ValueType::Vec4
// result.uses_time = false
```

### Validating WGSL

```rust
use crate::renderer::validation;

let wgsl = r#"
    @vertex
    fn vs_main() -> @builtin(position) vec4f {
        return vec4f(0.0, 0.0, 0.0, 1.0);
    }
"#;

// Validate WGSL syntax
match validation::validate_wgsl(wgsl) {
    Ok(module) => println!("Valid WGSL"),
    Err(e) => eprintln!("WGSL error: {}", e),
}
```

### Type Coercion

```rust
use crate::renderer::utils;
use crate::renderer::types::{TypedExpr, ValueType};

let scalar = TypedExpr::new("2.0", ValueType::F32);
let vector = TypedExpr::new("vec3f(1.0, 2.0, 3.0)", ValueType::Vec3);

// Coerce for binary operation (scalar will be splatted to vec3)
let (a, b, result_ty) = utils::coerce_for_binary(scalar, vector)?;
// a.expr = "vec3f(2.0)"
// b.expr = "vec3f(1.0, 2.0, 3.0)"
// result_ty = ValueType::Vec3
```

## Testing

All node compilers include comprehensive unit tests. Run them with:

```bash
# Run all tests
cargo test

# Run specific module tests
cargo test --lib renderer::node_compiler::input_nodes
cargo test --lib renderer::validation

# Run with output
cargo test -- --nocapture
```

### Test Coverage

Current coverage:
- `validation.rs` - 4 tests (valid/invalid WGSL)
- `input_nodes.rs` - 5 tests (all input node types)
- `math_nodes.rs` - 2 tests (add/multiply operations)
- `attribute.rs` - 4 tests (uv attribute variations)
- `texture_nodes.rs` - 3 tests (texture binding)
- `trigonometry_nodes.rs` - 3 tests (sin/cos/time)
- `vector_nodes.rs` - 4 tests (dot/cross/normalize/vector math)
- `color_nodes.rs` - 3 tests (mix/ramp/hsv)
- `legacy_nodes.rs` - 3 tests (backward compatibility)

**Total: 31 unit tests**

## Adding New Node Types

To add a new node compiler:

1. **Create a new file** in `node_compiler/` (e.g., `vector_nodes.rs`)

2. **Implement compiler functions** following the pattern:

```rust
pub fn compile_your_node<F>(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(&str, Option<&str>, &mut MaterialCompileContext, &mut HashMap<(String, String), TypedExpr>) -> Result<TypedExpr>,
{
    // Get inputs if needed
    let input = incoming_connection(scene, &node.id, "input_port")?;
    let value = compile_fn(&input.from.node_id, Some(&input.from.port_id), ctx, cache)?;
    
    // Generate WGSL expression
    Ok(TypedExpr::with_time(
        format!("your_wgsl_function({})", value.expr),
        value.ty,
        value.uses_time,
    ))
}
```

3. **Add unit tests** in the same file:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    // ... test implementations
}
```

4. **Register in mod.rs**:

```rust
pub mod vector_nodes;
```

5. **Update dispatch** (when implemented):

```rust
"YourNodeType" => vector_nodes::compile_your_node(...),
```

See `docs/renderer-refactoring-implementation-guide.md` for detailed instructions.

## Design Principles

### Type Safety
- All WGSL expressions are strongly typed using `ValueType`
- Automatic type coercion where appropriate
- Type mismatches caught at compile time

### Caching
- Expression compilation results are cached by (node_id, port_id)
- Prevents recompilation of shared subgraphs
- Improves performance for complex material graphs

### Time Tracking
- Each expression tracks whether it depends on animation time
- Enables optimization (time-independent expressions don't need updates)
- Propagated through expression tree

### Error Handling
- All functions return `Result<T>` with descriptive errors
- Validation errors include source context
- Missing inputs and type mismatches clearly reported

## Architecture

The renderer follows a compiler-like architecture:

```
DSL JSON
    ↓
Schema Validation (src/schema.rs)
    ↓
Scene Preparation (scene_prep/)
    ├── Tree-shake unused nodes
    ├── Auto-wrap primitives
    ├── Resolve composition layers/targets
    └── Topological sort
    ↓
Geometry And Coord Resolution (geometry_resolver/)
    ├── Resolve composition target domains
    ├── Resolve draw contexts per consumer edge
    ├── Preserve resolved geometry placement for all consumer edges
    └── Tree-shake dead pass-like branches for inference
    ↓
Node Compilation (node_compiler/*)
    ├── Dispatch to specific compiler
    ├── Recursively compile inputs
    ├── Type coercion
    └── Cache results
    ↓
WGSL Generation (wgsl.rs)
    └── Build shader bundles
    ↓
WGSL Validation (validation.rs)
    └── Validate with naga
    ↓
ShaderSpace Construction (shader_space/)
    ├── Create GPU resources
    ├── Build pipelines
    ├── Synthesize compose passes (including nested composite blits)
    └── Setup render order
    ↓
Render (rust-wgpu-fiber)
```

## Current Status

**Implemented**: ~30 node types (60%)

### Input Nodes (5)
- ✅ ColorInput, FloatInput, IntInput, Vector2Input, Vector3Input

### Math Nodes (4)
- ✅ MathAdd, MathMultiply, MathClamp, MathPower

### Attribute Nodes (1)
- ✅ Attribute

### Texture Nodes (1)
- ✅ ImageTexture

### Trigonometry Nodes (3)
- ✅ Sin, Cos, Time

### Vector Nodes (4)
- ✅ VectorMath, DotProduct, CrossProduct, Normalize

### Color Nodes (3)
- ✅ ColorMix, ColorRamp, HSVAdjust

### Legacy Nodes (backward compatibility)
- ✅ Float, Scalar, Constant
- ✅ Vec2, Vec3, Vec4, Color
- ✅ Add, Mul, Multiply, Mix, Clamp, Smoothstep

**Remaining**: See `assets/node-scheme.json` for complete list and `docs/renderer-refactoring-implementation-guide.md` for roadmap.

## Documentation

- **Architecture**: `docs/renderer-refactoring-architecture.md`
- **Geometry/Coord Refactor (Source of Truth)**: `docs/geometry-coordination-resolver-refactor.md`
- **Implementation Guide**: `docs/renderer-refactoring-implementation-guide.md`
- **Original Implementation**: `docs/dsl-to-rust-wgpu-fiber-implementation.md`

## Migration from renderer.rs

The modular renderer structure is now **fully implemented**:
- `scene_prep/` - Scene preparation and validation
- `wgsl.rs` - WGSL shader generation
- `shader_space/` - ShaderSpace construction (1185 lines)
- `node_compiler/*` - All node compilers organized by category

The original monolithic `renderer.rs` has been successfully decomposed into this modular structure.
