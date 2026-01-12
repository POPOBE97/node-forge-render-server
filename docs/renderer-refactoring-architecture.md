# Renderer Refactoring Architecture

## Overview

This document describes the refactored architecture for `renderer.rs`, transforming it from a monolithic 2723-line file into a modular, compiler-like system.

## Current Structure (Before Refactoring)

```
src/renderer.rs (2723 lines)
├── Port type utilities
├── Scene preparation
├── WGSL generation utilities
├── Material expression compilation (all nodes in one giant match)
├── Pass WGSL bundle building
├── ShaderSpace construction
└── Error shader generation
```

## New Architecture (After Refactoring)

```
src/renderer/
├── mod.rs                          # Module entry point, re-exports
├── types.rs                        # Core type definitions
├── utils.rs                        # Utility functions
├── scene_prep.rs                   # Scene preparation and validation
├── wgsl.rs                         # WGSL shader bundle generation
├── shader_space.rs                 # ShaderSpace construction
├── validation.rs                   # WGSL validation using naga
└── node_compiler/
    ├── mod.rs                      # Compiler infrastructure
    ├── context.rs                  # MaterialCompileContext
    ├── input_nodes.rs              # ColorInput, FloatInput, IntInput, Vector*Input
    ├── attribute.rs                # Attribute node
    ├── texture_nodes.rs            # ImageTexture, CheckerTexture, GradientTexture, NoiseTexture
    ├── math_nodes.rs               # MathAdd, MathMultiply, MathClamp, MathPower
    ├── vector_nodes.rs             # VectorMath, CrossProduct, DotProduct, Normalize
    ├── color_nodes.rs              # ColorMix, ColorRamp, HSVAdjust
    ├── shader_nodes.rs             # EmissionShader, PrincipledBSDF, MixShader
    ├── material_nodes.rs           # MaterialFromShader, PBRMaterial, ShaderMaterial
    ├── pass_nodes.rs               # RenderPass, GuassianBlurPass, ComputePass, Composite
    └── geometry_nodes.rs           # Rect2DGeometry, GeometryPrimitive, etc.
```

## Module Responsibilities

### types.rs
- `ValueType`: WGSL type enum (F32, Vec2, Vec3, Vec4)
- `TypedExpr`: Typed WGSL expression with time dependency tracking
- `MaterialCompileContext`: Tracks image textures and bindings
- `Params`: Uniform parameters for render passes
- `PassBindings`: Binding information for passes
- `WgslShaderBundle`: Complete WGSL shader bundle

### utils.rs
- `fmt_f32()`: Format floats for WGSL
- `array8_f32_wgsl()`: Format float arrays
- `sanitize_wgsl_ident()`: Sanitize identifiers
- `splat_f32()`: Splat scalars to vectors
- `coerce_for_binary()`: Type coercion for binary ops
- `to_vec4_color()`: Convert to vec4 color
- `as_bytes()`, `as_bytes_slice()`: Memory utilities
- `decode_data_url()`: Data URL parsing
- `load_image_from_data_url()`: Image loading

### scene_prep.rs
- `PreparedScene`: Prepared scene with topo order
- `prepare_scene()`: Main scene preparation function
- `auto_wrap_primitive_pass_inputs()`: Auto-wrap primitives to passes
- Port type utilities (`port_type_contains`, etc.)

### wgsl.rs
- `build_fullscreen_textured_bundle()`: Fullscreen quad shader
- `build_pass_wgsl_bundle()`: Build WGSL for a single pass
- `build_all_pass_wgsl_bundles_from_scene()`: Build all pass bundles
- Gaussian blur utilities
- WGSL formatting utilities

### shader_space.rs
- `build_shader_space_from_scene()`: Main ShaderSpace builder
- `build_error_shader_space()`: Error visualization builder
- Texture/sampler/buffer creation
- Composite layer handling
- Blend state parsing

### validation.rs
- `validate_wgsl()`: Validate WGSL using naga
- `format_naga_error()`: Format naga errors nicely
- Integration with build_pass_wgsl_bundle

### node_compiler/mod.rs
- `compile_material_expr()`: Main dispatcher function
- Expression caching
- Recursive compilation
- Legacy node support (Sin, Cos, Add, Mul, Mix, etc.)

### node_compiler/context.rs
- `MaterialCompileContext`: Full implementation
- `register_image_texture()`: Register texture bindings
- `tex_var_name()`, `sampler_var_name()`: Name generation

### Individual Node Compiler Modules

Each module contains compilation logic for specific node types:

```rust
// Example: input_nodes.rs
pub fn compile_color_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_vec4_value_array(node, "value").unwrap_or([1.0, 0.0, 1.0, 1.0]);
    Ok(TypedExpr::new(
        format!("vec4f({}, {}, {}, {})", v[0], v[1], v[2], v[3]),
        ValueType::Vec4
    ))
}

pub fn compile_float_input(node: &Node, _out_port: Option<&str>) -> Result<TypedExpr> {
    let v = parse_const_f32(node).unwrap_or(0.0);
    Ok(TypedExpr::new(format!("{v}"), ValueType::F32))
}
```

## Compilation Pipeline

```
DSL JSON
    ↓
Schema Validation (schema.rs)
    ↓
Scene Preparation (scene_prep.rs)
    ├── Locate RenderTarget
    ├── Tree-shake unused nodes
    ├── Auto-wrap primitives
    ├── Validate connections
    └── Topological sort
    ↓
WGSL Generation (wgsl.rs + node_compiler/*)
    ├── For each RenderPass node:
    │   ├── Compile material expression (node_compiler/mod.rs)
    │   │   ├── Dispatch to specific compiler (input_nodes.rs, etc.)
    │   │   ├── Recursively compile inputs
    │   │   ├── Type coercion (utils.rs)
    │   │   └── Cache results
    │   ├── Build WGSL shader bundle (wgsl.rs)
    │   └── Validate WGSL (validation.rs + naga)
    └── Return all bundles
    ↓
ShaderSpace Construction (shader_space.rs)
    ├── Create textures (RenderTexture, ImageTexture)
    ├── Create geometry buffers (Rect2DGeometry)
    ├── Create uniform buffers (Params)
    ├── Create pipelines (RenderPass)
    ├── Handle composite layers
    └── Setup render order
    ↓
Render (rust-wgpu-fiber)
```

## naga Integration

The `naga` crate is moved from `dev-dependencies` to regular `dependencies` to enable runtime WGSL validation:

```toml
[dependencies]
naga = { version = "0.20", features = ["wgsl-in"] }
```

Usage in `validation.rs`:

```rust
pub fn validate_wgsl(source: &str) -> Result<naga::Module> {
    naga::front::wgsl::parse_str(source)
        .map_err(|e| anyhow!("WGSL validation failed: {}", format_naga_error(source, &e)))
}
```

Called from `build_pass_wgsl_bundle`:

```rust
// Validate generated WGSL
validation::validate_wgsl(&bundle.module)
    .with_context(|| format!("pass {pass_id} generated invalid WGSL"))?;
```

## Testing Strategy

### Unit Tests for Node Compilers

Each node compiler module has its own test module:

```rust
// tests/node_compilers/input_nodes_test.rs
#[test]
fn test_color_input_compilation() {
    let node = Node {
        id: "color1".to_string(),
        node_type: "ColorInput".to_string(),
        params: HashMap::from([
            ("value".to_string(), json!([0.5, 0.3, 0.8, 1.0]))
        ]),
        inputs: Vec::new(),
    };
    
    let result = compile_color_input(&node, None).unwrap();
    assert_eq!(result.ty, ValueType::Vec4);
    assert!(result.expr.contains("vec4f"));
    assert!(!result.uses_time);
}
```

### Integration Tests

Existing integration tests in `tests/wgsl_generation.rs` ensure:
- Golden file comparison (vertex, fragment, module WGSL)
- naga validation of all generated WGSL
- Auto-wrapping of primitive values
- Tree-shaking of unused nodes

### Test Coverage Goals

- [ ] 100% coverage of node compiler functions
- [ ] All 50 node types from node-scheme.json have tests
- [ ] Type coercion edge cases
- [ ] Error handling (invalid connections, missing inputs)
- [ ] WGSL validation catches syntax errors

## Migration Plan

1. **Phase 1: Extract types and utils** (Non-breaking)
   - Create types.rs with all type definitions
   - Create utils.rs with utility functions
   - Update renderer.rs to use `super::types::*` and `super::utils::*`

2. **Phase 2: Move naga to dependencies** (Non-breaking)
   - Update Cargo.toml
   - Create validation.rs
   - Add validation calls to build_pass_wgsl_bundle

3. **Phase 3: Extract scene_prep.rs** (Non-breaking)
   - Move PreparedScene and prepare_scene
   - Move auto-wrapping logic
   - Update renderer.rs imports

4. **Phase 4: Extract wgsl.rs** (Non-breaking)
   - Move WGSL bundle building functions
   - Move Gaussian blur utilities
   - Update renderer.rs imports

5. **Phase 5: Extract shader_space.rs** (Non-breaking)
   - Move build_shader_space_from_scene
   - Move blend state parsing
   - Move composite handling
   - Update renderer.rs imports

6. **Phase 6: Create node compiler infrastructure** (Non-breaking)
   - Create node_compiler/mod.rs with dispatch logic
   - Keep compile_material_expr in place initially
   - Add tests for infrastructure

7. **Phase 7: Extract node compilers incrementally** (Non-breaking)
   - Start with simple nodes (input_nodes.rs)
   - Add unit tests for each
   - Update dispatch in node_compiler/mod.rs
   - Repeat for all node types

8. **Phase 8: Final cleanup** (Non-breaking)
   - Remove empty renderer.rs
   - Update all imports to use renderer::*
   - Update documentation

## Benefits

### Maintainability
- Each module < 500 lines (vs 2723 line monolith)
- Clear separation of concerns
- Easy to locate specific node logic

### Extensibility
- Adding new node types: just add a function in the appropriate module
- Modifying a node: change one function, not a giant match statement
- node-scheme.json changes map directly to code structure

### Testability
- Unit tests for each node compiler
- Isolated testing without full scene setup
- Easy to test error cases

### Code Quality
- Early WGSL validation catches errors
- Type safety enforced throughout
- Consistent error messages

### Documentation
- Each module documents its purpose
- Node compilers self-documenting
- Architecture clear from directory structure

## Backward Compatibility

All changes maintain 100% backward compatibility:
- Public API unchanged (`build_all_pass_wgsl_bundles_from_scene`, etc.)
- DSL JSON format unchanged
- Test golden files unchanged
- WebSocket protocol unchanged

The refactoring is purely internal organization with added validation.
