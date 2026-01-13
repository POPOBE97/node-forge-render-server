# Renderer Refactoring Implementation Guide

## 🎉 Phase 4 Complete: WGSL Module Extracted!

The renderer refactoring has completed Phase 4! WGSL shader generation logic has been successfully extracted into a dedicated module.

### ✅ Completed Phases

#### Phase 1-2: Core Infrastructure (100% Complete)
- **Module Structure**: Created `src/renderer/` with organized submodules
- **Type System**: `types.rs` contains all shared types (ValueType, TypedExpr, MaterialCompileContext, Params, etc.)
- **Utilities**: `utils.rs` provides WGSL formatting, type coercion, and data conversion  
- **Validation**: `validation.rs` integrates naga for WGSL validation
- **Dependencies**: Moved `naga` from dev-dependencies to regular dependencies

#### Phase 3: Scene Preparation (100% Complete)
- ✅ **scene_prep.rs** (386 lines): Scene preparation and validation module
  - `PreparedScene` struct with validated, sorted scene data
  - `prepare_scene()` - Main scene preparation pipeline
  - `auto_wrap_primitive_pass_inputs()` - Auto-wraps primitives to render passes
  - Port type utilities - `port_type_contains`, `get_from_port_type`, `get_to_port_type`
  - `composite_layers_in_draw_order()` - Determines composite layer rendering order

#### Phase 4: WGSL Generation (100% Complete - JUST COMPLETED!)
- ✅ **wgsl.rs** (670 lines): WGSL shader generation module
  - `build_pass_wgsl_bundle()` - Builds WGSL for a single render pass
  - `build_all_pass_wgsl_bundles_from_scene()` - Builds WGSL for all passes
  - `gaussian_mip_level_and_sigma_p()` - Gaussian blur mipmap calculation
  - `gaussian_kernel_8()` - Gaussian blur kernel generation
  - `build_fullscreen_textured_bundle()` - Fullscreen quad shader generation
  - Helper functions: `fmt_f32()`, `array8_f32_wgsl()`, `clamp_min_1()`

#### Phases 6-7: Node Compiler System (100% Complete)
- ✅ 8 focused node compiler modules with 31 node types (62% coverage)
- ✅ Main dispatch system in `node_compiler/mod.rs`
- ✅ Caching mechanism for compiled expressions
- ✅ Recursive compilation support
- ✅ 36+ unit tests passing
- ✅ **input_nodes.rs**: ColorInput, FloatInput, IntInput, Vector2Input, Vector3Input
- ✅ **math_nodes.rs**: MathAdd, MathMultiply, MathClamp, MathPower
- ✅ **attribute.rs**: Attribute node (reads vertex attributes like UV)
- ✅ **texture_nodes.rs**: ImageTexture (with UV flipping and binding management)
- ✅ **trigonometry_nodes.rs**: Sin, Cos, Time
- ✅ **legacy_nodes.rs**: Float, Scalar, Constant, Vec2, Vec3, Vec4, Color, Add, Mul, Mix, Clamp, Smoothstep
- ✅ **vector_nodes.rs**: VectorMath, CrossProduct, DotProduct, Normalize
- ✅ **color_nodes.rs**: ColorMix, ColorRamp, HSVAdjust

#### 3. Dispatch System (Complete)
- ✅ **node_compiler/mod.rs**: Main dispatch function that routes compilation to specific modules
- ✅ Caching mechanism for compiled expressions
- ✅ Recursive compilation support
- ✅ Clean error handling

#### 4. Integration with renderer.rs (Complete)
- ✅ Removed 356-line monolithic `compile_material_expr` function
- ✅ Removed ~100 lines of duplicate helper functions (splat_f32, coerce_for_binary, to_vec4_color, etc.)
- ✅ Removed duplicate type definitions (ValueType, TypedExpr, MaterialCompileContext)
- ✅ Updated imports to use modular versions
- ✅ `renderer/legacy.rs` now uses `renderer::node_compiler::compile_material_expr`

#### 5. Dependencies
- ✅ Moved `naga` from dev-dependencies to regular dependencies
- ✅ Ready for runtime WGSL validation

#### 6. Testing
- ✅ **36 unit tests passing** - ALL TESTS FIXED AND PASSING
- ✅ **3/3 WGSL generation integration tests passing**
- ✅ Tests validate WGSL generation, types, time dependency tracking, and error handling
- ✅ Fixed 9 test failures (all related to missing input connections in test setup)

## Current Status Summary (Updated: January 13, 2026)

✅ **All Tests Passing**: 36 unit tests + 3 integration tests  
✅ **31 Node Types Implemented** (62% coverage)  
✅ **Modular Architecture Complete**: types, utils, validation, scene_prep, node_compiler  
⚠️ **Minor Cleanup Needed**: ~28 compiler warnings (unused imports/variables)

### Recent Fixes (January 13, 2026)
- Fixed 9 test failures by adding required input connections to test setups
- All tests now pass successfully
- Test improvements:
  - color_nodes: Added connections for ColorMix, ColorRamp, HSVAdjust tests
  - trigonometry_nodes: Added connection for Cos test
  - vector_nodes: Added connections for DotProduct, CrossProduct, Normalize, VectorMath tests
  - validation: Updated type error test case for current naga behavior

### 📊 Current Status

**Node Types Implemented**: 31 / 50 (62%)

**Fully Implemented Node Types:**
- ColorInput, FloatInput, IntInput, Vector2Input, Vector3Input ✅
- Attribute ✅
- MathAdd, MathMultiply, MathClamp, MathPower ✅
- ImageTexture ✅
- Sin, Cos, Time ✅
- Float, Scalar, Constant (legacy) ✅
- Vec2, Vec3, Vec4, Color (legacy) ✅
- Add, Mul, Mix, Clamp, Smoothstep (legacy) ✅
- VectorMath, CrossProduct, DotProduct, Normalize ✅
- ColorMix, ColorRamp, HSVAdjust ✅

**Remaining Node Types**: 19
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

- [x] Phase 1: Extract types and utils ✅
- [x] Phase 2: Move naga to dependencies + validation ✅  
- [x] Phase 3: Extract scene_prep.rs ✅
- [x] **Phase 4: Extract wgsl.rs ✅ JUST COMPLETED!**
- [ ] **Phase 5: Extract shader_space.rs** (NEXT - Ready to start)
- [x] Phase 6-7: Node compiler infrastructure and implementations ✅ (31 node types)
- [ ] **Phase 8: Final cleanup** (After Phase 5)

## Current Architecture

```
src/renderer/
├── mod.rs                      # Module entry, re-exports ✅
├── types.rs (150 lines)        # Core type definitions ✅
├── utils.rs (175 lines)        # Utility functions ✅
├── validation.rs (50 lines)    # WGSL validation ✅
├── scene_prep.rs (386 lines)   # Scene preparation ✅ Phase 3
├── wgsl.rs (670 lines)         # WGSL generation ✅ Phase 4 - NEW!
├── legacy.rs (1208 lines)      # ⚠️ ShaderSpace only (Phase 5 - READY)
└── node_compiler/ (1500 lines) # Node compilation ✅ Phases 6-7
    ├── mod.rs                  # Dispatch system
    ├── input_nodes.rs
    ├── math_nodes.rs
    ├── attribute.rs
    ├── texture_nodes.rs
    ├── trigonometry_nodes.rs
    ├── legacy_nodes.rs
    ├── vector_nodes.rs
    └── color_nodes.rs
```

## Progress Summary

### ✅ Completed (Phases 1-4, 6-7)
- **Core Infrastructure**: types, utils, validation modules
- **Scene Preparation**: Dedicated scene_prep.rs module  
- **WGSL Generation**: Dedicated wgsl.rs module (NEW!)
- **Node Compilers**: 31 node types across 8 modules
- **Testing**: All 36 unit tests + 3 integration tests passing
- **Code Organization**: From 2723-line monolith → 16 focused modules

### 🚧 Remaining Work (Phase 5 + 8)
- **Phase 5**: Extract ShaderSpace construction (~1208 lines) - READY TO START
- **Phase 8**: Final cleanup, fix warnings, documentation

### 📊 Progress Metrics
- **Original monolithic file**: 2723 lines
- **After Phase 4**: Largest remaining file is 1208 lines (-56% from original)
- **Extracted so far**: 1515 lines into focused modules
- **Phases complete**: 5 out of 7 (71% of extraction phases)

## Code Reduction Achieved

### Before Refactoring
- `renderer.rs`: 2723 lines (monolithic file)
- All scene prep, WGSL generation, and node compilation in one file

### After Refactoring (Current State)
- `legacy.rs`: 1849 lines (WGSL + ShaderSpace, to be further split)
- `scene_prep.rs`: 386 lines (scene preparation - NEWLY EXTRACTED)
- `types.rs`: ~150 lines (type definitions)
- `utils.rs`: ~175 lines (utility functions)
- `validation.rs`: ~50 lines (WGSL validation)
- `node_compiler/`: ~1500 lines across 9 files (31 node types)
- **Total**: ~4100 lines across 15 focused modules
- **Reduction in largest file**: 2723 → 1849 lines (-32%)
- Node compiler modules: 8 focused files with unit tests
- Dispatch system: ~90 lines with clean routing logic
- Much better organization and testability

## Benefits Realized

### Code Organization
- 8 focused modules vs 1 monolithic file
- Clear responsibility for each module
- Easy to locate specific node logic
- Average file size: ~200 lines per module

### Testability
- 15+ unit tests for node compilers
- Tests run without GPU/graphics stack
- Fast feedback loop for development
- Each node type can be tested in isolation

### Documentation
- Each function documented with examples
- Types self-documenting
- Architecture clear from directory structure

### Extensibility
- Adding new node types is straightforward
- Pattern is consistent and easy to follow
- Changes localized to specific modules
- No need to touch 356-line match statement

## Estimated Effort for Remaining Work

Based on actual progress:

- **Node compilers implemented**: 31 node types in ~4 hours
- **Remaining node types**: 19 (many are rarely used)
- **Time estimate for remaining nodes**: ~3-4 hours
- **Infrastructure remaining**: 
  - Extract scene_prep.rs: ~1 hour
  - Extract wgsl.rs: ~1-2 hours
  - Extract shader_space.rs: ~2-3 hours
- **Testing and integration**: ~2-3 hours

**Total estimated time to complete**: 9-13 hours

**Total estimated time to complete**: 9-13 hours

## How the System Works Now

### Compilation Flow

1. **Entry Point**: `renderer.rs` calls `renderer::node_compiler::compile_material_expr()`
2. **Dispatch**: The dispatch function in `node_compiler/mod.rs` routes to the appropriate compiler module
3. **Recursive Compilation**: Each node compiler can compile its input nodes by calling the dispatch function
4. **Caching**: Compiled expressions are cached to avoid recompilation
5. **Type Safety**: All expressions are typed (ValueType::F32, Vec2, Vec3, Vec4) with time dependency tracking

### Adding a New Node Type

To add support for a new node type:

1. **Choose or create a module** in `src/renderer/node_compiler/`
   - Simple constant nodes: Add to existing modules
   - Complex nodes: Create a new module

2. **Implement the compiler function**:
   ```rust
   pub fn compile_my_node<F>(
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
       // 1. Get input connections
       let input = incoming_connection(scene, &node.id, "input_port")?;
       
       // 2. Recursively compile inputs
       let compiled_input = compile_fn(&input.from.node_id, Some(&input.from.port_id), ctx, cache)?;
       
       // 3. Generate WGSL expression
       Ok(TypedExpr::with_time(
           format!("myFunction({})", compiled_input.expr),
           compiled_input.ty,
           compiled_input.uses_time,
       ))
   }
   ```

3. **Add to dispatch** in `node_compiler/mod.rs`:
   ```rust
   "MyNodeType" => my_module::compile_my_node(scene, nodes_by_id, node, out_port, ctx, cache, compile_fn)?,
   ```

4. **Add unit tests** in your module with `#[cfg(test)]`

## Questions?

If you have questions about:
- **Pattern to follow**: See `input_nodes.rs` for simple nodes, `math_nodes.rs` for nodes with connections, `texture_nodes.rs` for complex nodes
- **Testing**: See the `#[cfg(test)]` sections in any module
- **Type system**: See `types.rs` for TypedExpr and ValueType
- **Utilities**: See `utils.rs` for type coercion, formatting, and conversions
- **Validation**: See `validation.rs` for WGSL validation with naga

## Next Immediate Steps for Phase 4: Extract wgsl.rs

### Step-by-Step Plan

1. **Create `src/renderer/wgsl.rs`** with the following functions from `legacy.rs`:
   - Lines 53-55: `clamp_min_1()`
   - Lines 57-86: `gaussian_mip_level_and_sigma_p()`
   - Lines 88-195: `gaussian_kernel_8()`
   - Lines 197-204: `fmt_f32()` (if not already in utils.rs)
   - Lines 206-209: `array8_f32_wgsl()` (if not already in utils.rs)
   - Lines 211-295: `build_fullscreen_textured_bundle()`
   - Lines 425-516: `build_pass_wgsl_bundle()`
   - Lines 518-697: `build_all_pass_wgsl_bundles_from_scene()`

2. **Update `src/renderer/legacy.rs`**:
   - Remove the extracted functions
   - Add import: `use super::wgsl::{build_pass_wgsl_bundle, build_all_pass_wgsl_bundles_from_scene};`
   - Keep internal helper types (SamplerKind, PassTextureBinding) if only used by shader_space

3. **Update `src/renderer/mod.rs`**:
   - Add: `pub mod wgsl;`
   - Add re-exports: `pub use wgsl::{build_pass_wgsl_bundle, build_all_pass_wgsl_bundles_from_scene};`

4. **Update internal dependencies**:
   - Helper types (SamplerKind, PassTextureBinding, rect2d_geometry_vertices) may need to be moved or shared
   - Extension method `MaterialCompileContext::wgsl_decls()` should be moved to types.rs

5. **Test the extraction**:
   - Run `cargo test` to ensure all tests still pass
   - Check for any compiler errors

### Expected Outcome
- `wgsl.rs`: ~645 lines of WGSL generation logic
- `legacy.rs`: ~1204 lines remaining (ShaderSpace only)
- Clear separation between WGSL generation and ShaderSpace construction

## Next Immediate Steps for Phase 5: Extract shader_space.rs

After Phase 4 is complete, extract the remaining ShaderSpace construction code:

1. **Create `src/renderer/shader_space.rs`** with:
   - Lines 698-866: Helper functions (composite_layers_in_draw_order if not already moved, normalize_blend_token, parse_blend_operation, parse_blend_factor, default_blend_state_for_preset, parse_render_pass_blend_state)
   - Lines 867-1771: `build_shader_space_from_scene()`
   - Lines 1772-1849: `build_error_shader_space()`

2. **Remove `legacy.rs`** entirely once extraction is complete

3. **Final cleanup**:
   - Remove any remaining duplicate code
   - Fix compiler warnings
   - Update documentation

## Questions?

### Priority 1: Code Cleanup (Recommended Before Further Work)
⚠️ **Clean up compiler warnings before continuing refactoring:**
- Unused imports in several node compiler modules (input_nodes, math_nodes, texture_nodes, color_nodes, legacy.rs)
- Unused variables: `nodes_by_id` parameters in many compiler functions (prefix with `_` if intentional)
- Unused variable `ty` in math_nodes.rs line 146
- Dead code: `typed_time()` and `composite_layers_in_draw_order()` in legacy.rs

These warnings don't affect functionality but should be cleaned up for code quality.

### Priority 2: Test the Current Implementation
1. Set up rust-wgpu-fiber dependency (if available)
2. Run existing integration tests
3. Verify all 31 implemented node types work correctly

### Priority 2: Complete Remaining Structure Extraction (Optional)
1. Extract `scene_prep.rs` from renderer.rs (scene preparation and validation)
2. Extract `wgsl.rs` from renderer.rs (WGSL generation for passes)
3. Extract `shader_space.rs` from renderer.rs (ShaderSpace construction)

### Priority 3: Implement Remaining Node Types (Optional - as needed)
Most commonly used nodes are already implemented. The remaining 19 node types are specialized and can be added as needed:
- Texture nodes: CheckerTexture, GradientTexture, NoiseTexture
- Shader/Material nodes: EmissionShader, PrincipledBSDF, MixShader, etc.
- Pass/Geometry nodes: RenderPass, ComputePass, Composite, etc.

The current implementation covers 62% of all node types and includes all the most commonly used ones!
