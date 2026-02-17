//! Renderer module for compiling DSL scenes to WGSL and building ShaderSpaces.
//!
//! This module is organized into several submodules:
//! - `types`: Core type definitions (ValueType, TypedExpr, Params, etc.)
//! - `utils`: Utility functions for formatting and data conversion
//! - `node_compiler`: Node compilation infrastructure
//! - `validation`: WGSL validation using naga
//! - `scene_prep`: Scene preparation and validation
//! - `render_plan`: Render planning utilities and pass graph helpers
//! - `wgsl`: WGSL shader generation
//! - `shader_space`: ShaderSpace construction
//! - `wgsl_templates`: Reusable WGSL templates for fullscreen effects
//!
//! The main entry points are:
//! - `build_all_pass_wgsl_bundles_from_scene`: Generate WGSL for all passes
//! - `ShaderSpaceBuilder`: Build ShaderSpace resources from a scene

pub mod glsl_snippet;
pub mod graph_uniforms;
pub mod node_compiler;
pub mod render_plan;
pub mod scene_prep;
pub mod shader_space;
pub mod types;
pub mod utils;
pub mod validation;
pub mod wgsl;
pub mod wgsl_gradient_blur;
pub mod wgsl_templates;

// Re-export key types and functions for backward compatibility
pub use node_compiler::compile_material_expr;
pub use scene_prep::{PreparedScene, prepare_scene};
pub use shader_space::{
    ShaderSpaceBuildOptions, ShaderSpaceBuildResult, ShaderSpaceBuilder,
    ShaderSpacePresentationMode, render_scene_to_png_headless, update_pass_params,
};
pub use types::{Params, PassBindings, WgslShaderBundle};
pub use validation::{validate_wgsl, validate_wgsl_with_context};
pub use wgsl::{
    build_all_pass_wgsl_bundles_from_scene, build_all_pass_wgsl_bundles_from_scene_with_assets,
    build_pass_wgsl_bundle,
};
