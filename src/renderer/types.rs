//! Core type definitions for the renderer module.

use rust_wgpu_fiber::ResourceName;
use rust_wgpu_fiber::eframe::wgpu::TextureFormat;
use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

/// WGSL value type for shader expressions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ValueType {
    F32,
    I32,
    U32,
    Bool,
    /// Opaque texture handle (a paired texture_2d + sampler binding).
    ///
    /// This is not a "storable" value in WGSL and must only be used by nodes that explicitly
    /// know how to sample it (e.g. GlassMaterial).
    Texture2D,
    Vec2,
    Vec3,
    Vec4,
}

#[derive(Clone, Debug)]
pub enum BakedValue {
    F32(f32),
    I32(i32),
    U32(u32),
    Bool(bool),
    Vec2([f32; 2]),
    Vec3([f32; 3]),
    Vec4([f32; 4]),
}

#[derive(Clone, Debug, Default)]
pub struct BakedDataParseMeta {
    pub outputs_per_instance: u32,
    pub slot_by_output: HashMap<(String, String, String), u32>,
    pub pass_id: String,
}

impl BakedDataParseMeta {
    pub fn slot_for(&self, pass_id: &str, node_id: &str, port_id: &str) -> Option<u32> {
        self.slot_by_output
            .get(&(
                pass_id.to_string(),
                node_id.to_string(),
                port_id.to_string(),
            ))
            .copied()
    }
}

/// Output specification for any pass node that produces a texture.
///
/// This trait enables chain composition - any node that outputs a texture
/// can be used as input to another pass node, allowing chains like:
/// `RenderPass -> GuassianBlurPass -> GuassianBlurPass -> ...`
#[derive(Clone, Debug)]
pub struct PassOutputSpec {
    /// The node ID that produces this output.
    pub node_id: String,
    /// The output texture resource name.
    pub texture_name: ResourceName,
    /// Resolution of the output texture [width, height].
    pub resolution: [u32; 2],
    /// Texture format.
    pub format: TextureFormat,
}

/// Information about a pass node's input requirements.
#[derive(Clone, Debug)]
pub struct PassInputSpec {
    /// The node ID that requires this input.
    pub node_id: String,
    /// The port ID for the input (e.g., "pass").
    pub port_id: String,
    /// Expected resolution (if explicitly specified, otherwise inherited).
    pub explicit_resolution: Option<[u32; 2]>,
}

/// Registry of pass outputs for resolving chain dependencies.
#[derive(Default, Clone, Debug)]
pub struct PassOutputRegistry {
    /// Map from node_id to its output specification.
    outputs: HashMap<String, PassOutputSpec>,
}

impl PassOutputRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a pass output.
    pub fn register(&mut self, spec: PassOutputSpec) {
        self.outputs.insert(spec.node_id.clone(), spec);
    }

    /// Get the output spec for a node.
    pub fn get(&self, node_id: &str) -> Option<&PassOutputSpec> {
        self.outputs.get(node_id)
    }

    /// Get the output texture name for a node.
    pub fn get_texture(&self, node_id: &str) -> Option<&ResourceName> {
        self.outputs.get(node_id).map(|s| &s.texture_name)
    }

    /// Get the resolution for a node's output.
    pub fn get_resolution(&self, node_id: &str) -> Option<[u32; 2]> {
        self.outputs.get(node_id).map(|s| s.resolution)
    }

    /// Resolve the effective resolution for a pass input.
    /// If explicit_resolution is Some, use it. Otherwise inherit from upstream.
    pub fn resolve_resolution(
        &self,
        upstream_node_id: &str,
        explicit_resolution: Option<[u32; 2]>,
        default_resolution: [u32; 2],
    ) -> [u32; 2] {
        explicit_resolution
            .or_else(|| self.get_resolution(upstream_node_id))
            .unwrap_or(default_resolution)
    }
}

impl ValueType {
    /// Returns the WGSL type name for this value type.
    pub fn wgsl(self) -> &'static str {
        match self {
            ValueType::F32 => "f32",
            ValueType::I32 => "i32",
            ValueType::U32 => "u32",
            ValueType::Bool => "bool",
            ValueType::Texture2D => "texture_2d<f32>",
            ValueType::Vec2 => "vec2f",
            ValueType::Vec3 => "vec3f",
            ValueType::Vec4 => "vec4f",
        }
    }

    pub fn glsl(self) -> &'static str {
        match self {
            ValueType::F32 => "float",
            ValueType::I32 => "int",
            ValueType::U32 => "uint",
            ValueType::Bool => "bool",
            ValueType::Texture2D => "sampler2D",
            ValueType::Vec2 => "vec2",
            ValueType::Vec3 => "vec3",
            ValueType::Vec4 => "vec4",
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
    pub baked_data_parse: Option<Arc<HashMap<(String, String, String), Vec<BakedValue>>>>,
    pub baked_data_parse_meta: Option<Arc<BakedDataParseMeta>>,

    /// Set when the compiled shader needs `@builtin(instance_index)` in the vertex stage.
    ///
    /// Today we only use this for vertex-stage logic, but we keep the name generic because
    /// in the future we may also forward it into the fragment stage.
    pub uses_instance_index: bool,

    /// List of ImageTexture node IDs referenced in order.
    pub image_textures: Vec<String>,
    /// Map from node ID to texture binding index.
    pub image_index_by_node: HashMap<String, usize>,
    /// List of PassTexture node IDs (upstream pass nodes) referenced in order.
    pub pass_textures: Vec<String>,
    /// Map from pass node ID to texture binding index.
    pub pass_index_by_node: HashMap<String, usize>,

    /// Extra WGSL helper declarations emitted by node compilers (e.g. MathClosure).
    ///
    /// Keyed by a stable symbol name to avoid duplicate definitions.
    pub extra_wgsl_decls: BTreeMap<String, String>,

    /// Inline WGSL statements emitted by node compilers for the function body.
    ///
    /// These statements are emitted in order before the final return expression.
    /// Used for MathClosure nodes to generate inline `{ }` blocks that isolate
    /// variable scope and avoid naming conflicts.
    pub inline_stmts: Vec<String>,
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

    /// Register a pass texture node and return its binding index.
    /// The binding index is offset by the number of image textures to avoid conflicts.
    pub fn register_pass_texture(&mut self, pass_node_id: &str) -> usize {
        if let Some(&idx) = self.pass_index_by_node.get(pass_node_id) {
            return idx;
        }
        let idx = self.pass_textures.len();
        self.pass_textures.push(pass_node_id.to_string());
        self.pass_index_by_node
            .insert(pass_node_id.to_string(), idx);
        idx
    }

    /// Generate the WGSL variable name for a texture binding.
    pub fn tex_var_name(node_id: &str) -> String {
        format!(
            "img_tex_{}",
            crate::renderer::utils::sanitize_wgsl_ident(node_id)
        )
    }

    /// Generate the WGSL variable name for a sampler binding.
    pub fn sampler_var_name(node_id: &str) -> String {
        format!(
            "img_samp_{}",
            crate::renderer::utils::sanitize_wgsl_ident(node_id)
        )
    }

    /// Generate the WGSL variable name for a pass texture binding.
    pub fn pass_tex_var_name(pass_node_id: &str) -> String {
        format!(
            "pass_tex_{}",
            crate::renderer::utils::sanitize_wgsl_ident(pass_node_id)
        )
    }

    /// Generate the WGSL variable name for a pass sampler binding.
    pub fn pass_sampler_var_name(pass_node_id: &str) -> String {
        format!(
            "pass_samp_{}",
            crate::renderer::utils::sanitize_wgsl_ident(pass_node_id)
        )
    }

    /// Build the fragment body with inline statements prepended to the return expression.
    ///
    /// Inline statements (from MathClosure nodes) are emitted before the final return.
    pub fn build_fragment_body(&self, return_expr: &str) -> String {
        if self.inline_stmts.is_empty() {
            format!("return {};", return_expr)
        } else {
            format!(
                "{}\n    return {};",
                self.inline_stmts.join("\n"),
                return_expr
            )
        }
    }
}

/// Uniform parameters passed to each render pass.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Params {
    pub target_size: [f32; 2],
    pub geo_size: [f32; 2],
    pub center: [f32; 2],

    // TransformGeometry (applied in vertex shader)
    pub geo_translate: [f32; 2],
    pub geo_scale: [f32; 2],

    // Pack to 16-byte boundary.
    pub time: f32,
    pub _pad0: f32,

    // 16-byte aligned.
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
    /// PassTexture node ids (upstream pass nodes) referenced by this pass's material graph, in binding order.
    pub pass_textures: Vec<String>,
}
