//! Renderer module for compiling DSL scenes to WGSL and building ShaderSpaces.
//!
//! This module is organized into several submodules:
//! - `types`: Core type definitions (ValueType, TypedExpr, Params, etc.)
//! - `utils`: Utility functions for formatting and data conversion
//! - `node_compiler`: Node compilation infrastructure
//! - `validation`: WGSL validation using naga
//! - `scene_prep`: Scene preparation and validation
//! - `wgsl`: WGSL shader generation
//! - `shader_space`: ShaderSpace construction
//!
//! The main entry points are:
//! - `build_all_pass_wgsl_bundles_from_scene`: Generate WGSL for all passes
//! - `build_shader_space_from_scene`: Build a complete ShaderSpace from a scene
//! - `build_error_shader_space`: Build an error visualization ShaderSpace

pub mod glsl_snippet;
pub mod node_compiler;
pub mod scene_prep;
pub mod shader_space;
pub mod types;
pub mod utils;
pub mod validation;
pub mod wgsl;

// Re-export key types and functions for backward compatibility
pub use node_compiler::compile_material_expr;
pub use scene_prep::{PreparedScene, auto_wrap_primitive_pass_inputs, prepare_scene};
pub use shader_space::{
    build_error_shader_space, build_shader_space_from_scene, build_shader_space_from_scene_for_ui,
    render_scene_to_png_headless, update_pass_params,
};
pub use types::{Params, PassBindings, WgslShaderBundle};
pub use validation::{validate_wgsl, validate_wgsl_with_context};
pub use wgsl::{build_all_pass_wgsl_bundles_from_scene, build_pass_wgsl_bundle};
