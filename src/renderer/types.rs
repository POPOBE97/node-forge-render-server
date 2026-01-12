//! Core type definitions for the renderer module.

use std::collections::HashMap;
use rust_wgpu_fiber::ResourceName;

/// WGSL value type for shader expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    F32,
    Vec2,
    Vec3,
    Vec4,
}

impl ValueType {
    /// Returns the WGSL type name for this value type.
    pub fn wgsl(self) -> &'static str {
        match self {
            ValueType::F32 => "f32",
            ValueType::Vec2 => "vec2f",
            ValueType::Vec3 => "vec3f",
            ValueType::Vec4 => "vec4f",
        }
    }
}

/// A typed WGSL expression with metadata.
#[derive(Clone, Debug)]
pub struct TypedExpr {
    pub ty: ValueType,
    pub expr: String,
    pub uses_time: bool,
}

impl TypedExpr {
    /// Create a new typed expression without time dependency.
    pub fn new(expr: impl Into<String>, ty: ValueType) -> Self {
        Self {
            ty,
            expr: expr.into(),
            uses_time: false,
        }
    }

    /// Create a new typed expression with optional time dependency.
    pub fn with_time(expr: impl Into<String>, ty: ValueType, uses_time: bool) -> Self {
        Self {
            ty,
            expr: expr.into(),
            uses_time,
        }
    }
}

/// Context for compiling material expressions, tracking referenced resources.
#[derive(Default)]
pub struct MaterialCompileContext {
    /// List of ImageTexture node IDs referenced in order.
    pub image_textures: Vec<String>,
    /// Map from node ID to texture binding index.
    pub image_index_by_node: HashMap<String, usize>,
}

impl MaterialCompileContext {
    /// Register an image texture node and return its binding index.
    pub fn register_image_texture(&mut self, node_id: &str) -> usize {
        if let Some(&idx) = self.image_index_by_node.get(node_id) {
            return idx;
        }
        let idx = self.image_textures.len();
        self.image_textures.push(node_id.to_string());
        self.image_index_by_node.insert(node_id.to_string(), idx);
        idx
    }

    /// Generate the WGSL variable name for a texture binding.
    pub fn tex_var_name(node_id: &str) -> String {
        format!("tex_{}", node_id.replace('-', "_"))
    }

    /// Generate the WGSL variable name for a sampler binding.
    pub fn sampler_var_name(node_id: &str) -> String {
        format!("samp_{}", node_id.replace('-', "_"))
    }
}

/// Uniform parameters passed to each render pass.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub target_size: [f32; 2],
    pub geo_size: [f32; 2],
    pub center: [f32; 2],
    pub time: f32,
    pub _pad0: f32,
    pub color: [f32; 4],
}

/// Bindings for a render pass (uniform buffer and parameters).
#[derive(Clone, Debug)]
pub struct PassBindings {
    pub params_buffer: ResourceName,
    pub base_params: Params,
}

/// Complete WGSL shader bundle for a render pass.
#[derive(Clone, Debug)]
pub struct WgslShaderBundle {
    /// WGSL declarations shared between stages (types, bindings, structs).
    pub common: String,
    /// A standalone vertex WGSL module (common + @vertex entry).
    pub vertex: String,
    /// A standalone fragment WGSL module (common + @fragment entry).
    pub fragment: String,
    /// Optional compute WGSL module (common + @compute entry). Currently unused.
    pub compute: Option<String>,
    /// A combined WGSL module containing all emitted entry points.
    pub module: String,
    /// ImageTexture node ids referenced by this pass's material graph, in binding order.
    pub image_textures: Vec<String>,
}
