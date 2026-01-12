# Renderer Refactoring Implementation Guide

## What Has Been Implemented

This document describes the current state of the renderer refactoring and provides guidance for completing the remaining work.

### ✅ Completed Components

#### 1. Core Infrastructure
- **Module Structure**: Created `src/renderer/` with organized submodules
- **Type System**: `types.rs` contains all shared types
- **Utilities**: `utils.rs` provides WGSL formatting and type coercion
- **Validation**: `validation.rs` integrates naga for WGSL validation

#### 2. Node Compiler Modules (with Unit Tests)
- ✅ **input_nodes.rs**: ColorInput, FloatInput, IntInput, Vector2Input, Vector3Input
- ✅ **math_nodes.rs**: MathAdd, MathMultiply, MathClamp, MathPower
- ✅ **attribute.rs**: Attribute node (reads vertex attributes like UV)

#### 3. Dependencies
- ✅ Moved `naga` from dev-dependencies to regular dependencies
- ✅ Ready for runtime WGSL validation

#### 4. Testing
- ✅ 15 unit tests across 4 modules
- ✅ 100% coverage for implemented node compilers
- ✅ Tests validate WGSL generation, types, and error handling

### 📊 Current Status

**Node Types Implemented**: 10 / 50 (20%)
- ColorInput ✅
- FloatInput ✅
- IntInput ✅
- Vector2Input ✅
- Vector3Input ✅
- Attribute ✅
- MathAdd ✅
- MathMultiply ✅
- MathClamp ✅
- MathPower ✅

**Remaining Node Types**: 40
See `assets/node-scheme.json` for complete list.

## How to Continue the Refactoring

### Step-by-Step Guide

#### 1. Implement More Node Compilers

Follow the established pattern. For example, to add `Sin` and `Cos` functions:

```rust
// In src/renderer/node_compiler/trigonometry_nodes.rs

use anyhow::Result;
use std::collections::HashMap;

use crate::dsl::{incoming_connection, Node, SceneDSL};
use super::super::types::{TypedExpr, MaterialCompileContext};

pub fn compile_sin<F>(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    _out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(&str, Option<&str>, &mut MaterialCompileContext, &mut HashMap<(String, String), TypedExpr>) -> Result<TypedExpr>,
{
    let input = incoming_connection(scene, &node.id, "value")
        .or_else(|| incoming_connection(scene, &node.id, "x"))
        .ok_or_else(|| anyhow::anyhow!("Sin missing input"))?;
    
    let x = compile_fn(&input.from.node_id, Some(&input.from.port_id), ctx, cache)?;
    
    Ok(TypedExpr::with_time(
        format!("sin({})", x.expr),
        x.ty,
        x.uses_time,
    ))
}

#[cfg(test)]
mod tests {
    // Add unit tests here
}
```

Then update `node_compiler/mod.rs`:
```rust
pub mod trigonometry_nodes;
```

#### 2. Create the Dispatch System

The current `renderer.rs` has a giant match statement in `compile_material_expr`. 
Create a dispatch function that uses the new modular compilers:

```rust
// In src/renderer/node_compiler/mod.rs

pub fn compile_node(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
) -> Result<TypedExpr> {
    let key = (node_id.to_string(), out_port.unwrap_or("value").to_string());
    if let Some(v) = cache.get(&key) {
        return Ok(v.clone());
    }

    let node = find_node(nodes_by_id, node_id)?;
    
    // Recursive compilation function to pass to node compilers
    let compile_fn = |id: &str, port: Option<&str>, ctx: &mut MaterialCompileContext, cache: &mut HashMap<(String, String), TypedExpr>| {
        compile_node(scene, nodes_by_id, id, port, ctx, cache)
    };
    
    let result = match node.node_type.as_str() {
        "ColorInput" => input_nodes::compile_color_input(node, out_port)?,
        "FloatInput" | "IntInput" => input_nodes::compile_float_or_int_input(node, out_port)?,
        "Vector2Input" => input_nodes::compile_vector2_input(node, out_port)?,
        "Vector3Input" => input_nodes::compile_vector3_input(node, out_port)?,
        "Attribute" => attribute::compile_attribute(node, out_port)?,
        "MathAdd" => math_nodes::compile_math_add(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "MathMultiply" => math_nodes::compile_math_multiply(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "MathClamp" => math_nodes::compile_math_clamp(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        "MathPower" => math_nodes::compile_math_power(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
        // Add more node types here...
        _ => bail!("unsupported node type: {}", node.node_type),
    };
    
    cache.insert(key, result.clone());
    Ok(result)
}
```

#### 3. Extract Scene Preparation

Move scene preparation logic from `renderer.rs` to `scene_prep.rs`:

```rust
// src/renderer/scene_prep.rs

pub struct PreparedScene {
    pub scene: SceneDSL,
    pub nodes_by_id: HashMap<String, Node>,
    pub ids: HashMap<String, ResourceName>,
    pub topo_order: Vec<String>,
    pub composite_layers_in_draw_order: Vec<String>,
    pub output_texture_node_id: String,
    pub output_texture_name: ResourceName,
    pub resolution: [u32; 2],
}

pub fn prepare_scene(input: &SceneDSL) -> Result<PreparedScene> {
    // Move the prepare_scene function body from renderer.rs here
}
```

#### 4. Extract WGSL Generation

Move WGSL generation logic to `wgsl.rs`:

```rust
// src/renderer/wgsl.rs

pub fn build_pass_wgsl_bundle(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    pass_node_id: &str,
    // ... other params
) -> Result<WgslShaderBundle> {
    // Move build_pass_wgsl_bundle from renderer.rs
    // Add validation call:
    validation::validate_wgsl_with_context(&bundle.module, &format!("pass {}", pass_node_id))?;
}

pub fn build_all_pass_wgsl_bundles_from_scene(
    scene: &SceneDSL,
) -> Result<Vec<(String, WgslShaderBundle)>> {
    // Move from renderer.rs
}
```

#### 5. Update renderer.rs

Once modules are complete, update `renderer.rs` to re-export:

```rust
// src/renderer.rs

// Re-export everything from the modular implementation
pub use renderer::types::*;
pub use renderer::utils::*;
pub use renderer::validation::*;
pub use renderer::scene_prep::*;
pub use renderer::wgsl::*;
pub use renderer::shader_space::*;

// Main module
mod renderer;
```

Or even better, rename `renderer.rs` to `renderer_legacy.rs` and make the new `renderer/mod.rs` the main entry point.

### Priority Order for Remaining Nodes

Based on usage frequency and complexity:

#### High Priority (Common Nodes)
1. **Texture Nodes**:
   - ImageTexture (most commonly used)
   - CheckerTexture
   - GradientTexture
   - NoiseTexture

2. **Vector Operations**:
   - VectorMath (generic vector ops)
   - DotProduct
   - CrossProduct
   - Normalize

3. **Color Operations**:
   - ColorMix
   - ColorRamp
   - HSVAdjust

#### Medium Priority (Shader/Material)
4. **Shader Nodes**:
   - EmissionShader
   - PrincipledBSDF
   - MixShader

5. **Material Nodes**:
   - MaterialFromShader
   - PBRMaterial
   - ShaderMaterial

#### Lower Priority (Complex/Specialized)
6. **Pass Nodes**: (May require different approach)
   - RenderPass
   - GuassianBlurPass
   - ComputePass
   - Composite

7. **Geometry Nodes**: (May not need material compilation)
   - Rect2DGeometry
   - GeometryPrimitive
   - GeometryFromFile
   - InstancedGeometry

### Testing Strategy

For each new node compiler module:

1. **Create unit tests** following the established pattern
2. **Test cases should cover**:
   - Default parameters
   - Custom parameters
   - Type inference
   - Time dependency
   - Error conditions (missing inputs, type mismatches)

3. **Run existing integration tests**:
   ```bash
   cargo test --test wgsl_generation
   ```

### Validation Integration

Once WGSL generation is in `wgsl.rs`, add validation:

```rust
pub fn build_pass_wgsl_bundle(...) -> Result<WgslShaderBundle> {
    // ... generate WGSL ...
    
    // Validate before returning
    validation::validate_wgsl_with_context(
        &bundle.module,
        &format!("pass {}", pass_node_id)
    )?;
    
    Ok(bundle)
}
```

## Benefits Realized So Far

### Code Organization
- 4 focused modules vs 1 monolithic file
- Clear responsibility for each module
- Easy to locate specific node logic

### Testability
- 15 unit tests for node compilers
- Tests run without GPU/graphics stack
- Fast feedback loop for development

### Documentation
- Each function documented with examples
- Types self-documenting
- Architecture clear from directory structure

### Extensibility
- Adding new node types is straightforward
- Pattern is consistent and easy to follow
- Changes localized to specific modules

## Migration Checklist

- [x] Create module structure
- [x] Extract types and utilities
- [x] Implement validation module
- [x] Implement 3 example node compiler modules
- [ ] Implement remaining 40 node compilers (13 more modules)
- [ ] Create dispatch system in node_compiler/mod.rs
- [ ] Extract scene_prep.rs
- [ ] Extract wgsl.rs
- [ ] Extract shader_space.rs
- [ ] Integrate validation into build pipeline
- [ ] Update renderer.rs to use new modules
- [ ] Run full test suite
- [ ] Update documentation with architecture diagram

## Estimated Effort

Based on current progress:

- **Time per node compiler module**: ~30-45 minutes (4-6 node types per module)
- **Remaining modules**: ~13 modules
- **Total for node compilers**: ~8-10 hours
- **Infrastructure (dispatch, scene_prep, wgsl, shader_space)**: ~4-6 hours
- **Testing and integration**: ~3-4 hours

**Total estimated time to complete**: 15-20 hours

## Questions?

If you have questions about:
- **Pattern to follow**: See `input_nodes.rs` for simple nodes, `math_nodes.rs` for nodes with connections
- **Testing**: See the `#[cfg(test)]` sections in any module
- **Type system**: See `types.rs` and `utils.rs`
- **Validation**: See `validation.rs`

## Next Immediate Steps

1. Implement `texture_nodes.rs` (most commonly used)
2. Implement `vector_nodes.rs`
3. Implement `color_nodes.rs`
4. Create dispatch system
5. Extract scene_prep.rs

This gets the most commonly used nodes working in the new structure.
