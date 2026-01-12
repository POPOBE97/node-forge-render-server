//! Renderer module for compiling DSL scenes to WGSL and building ShaderSpaces.
//!
//! This module is organized into several submodules:
//! - `types`: Core type definitions (ValueType, TypedExpr, Params, etc.)
//! - `utils`: Utility functions for formatting and data conversion
//! - `node_compiler`: Node compilation infrastructure (currently inline in renderer.rs)
//!
//! The main entry points are:
//! - `build_all_pass_wgsl_bundles_from_scene`: Generate WGSL for all passes
//! - `build_shader_space_from_scene`: Build a complete ShaderSpace from a scene
//! - `build_error_shader_space`: Build an error visualization ShaderSpace

pub mod types;
pub mod utils;
pub mod node_compiler;
pub mod validation;

// Re-export key types and functions for backward compatibility
pub use types::{Params, PassBindings, WgslShaderBundle};
pub use validation::{validate_wgsl, validate_wgsl_with_context};

// Note: The bulk of the renderer logic is still in src/renderer.rs
// This module structure is being incrementally built to organize the code better.
// Eventually, scene_prep.rs, wgsl.rs, shader_space.rs, and individual node compilers
// will be extracted into their own files.
