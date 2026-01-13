//! Renderer module for compiling DSL scenes to WGSL and building ShaderSpaces.
//!
//! This module is organized into several submodules:
//! - `types`: Core type definitions (ValueType, TypedExpr, Params, etc.)
//! - `utils`: Utility functions for formatting and data conversion
//! - `node_compiler`: Node compilation infrastructure
//! - `validation`: WGSL validation using naga
//! - `legacy`: The original renderer.rs code (TEMPORARY - will be split into scene_prep, wgsl, shader_space)
//!
//! The main entry points are:
//! - `build_all_pass_wgsl_bundles_from_scene`: Generate WGSL for all passes
//! - `build_shader_space_from_scene`: Build a complete ShaderSpace from a scene
//! - `build_error_shader_space`: Build an error visualization ShaderSpace

pub mod types;
pub mod utils;
pub mod node_compiler;
pub mod validation;
pub mod scene_prep;
pub mod wgsl;

// TEMPORARY: The legacy module contains ShaderSpace construction code.
// This will be extracted into shader_space.rs in Phase 5.
// Once fully migrated, this module can be removed.
mod legacy;

// Re-export key types and functions for backward compatibility
pub use types::{Params, PassBindings, WgslShaderBundle};
pub use validation::{validate_wgsl, validate_wgsl_with_context};
pub use node_compiler::compile_material_expr;
pub use scene_prep::{PreparedScene, prepare_scene, auto_wrap_primitive_pass_inputs};
pub use wgsl::{build_pass_wgsl_bundle, build_all_pass_wgsl_bundles_from_scene};

// Re-export legacy functions that are still used externally
// TEMPORARY: These re-exports allow existing code to continue working.
pub use legacy::{
    build_error_shader_space,
    build_shader_space_from_scene,
    update_pass_params,
};
