use std::collections::HashMap;

use rust_wgpu_fiber::ResourceName;

use crate::dsl::{Node, SceneDSL};

/// Prepared scene with topologically sorted nodes and metadata.
pub struct PreparedScene {
    pub scene: SceneDSL,
    pub nodes_by_id: HashMap<String, Node>,
    pub ids: HashMap<String, ResourceName>,
    pub topo_order: Vec<String>,
    pub composite_layers_in_draw_order: Vec<String>,
    pub composition_layers_by_id: HashMap<String, Vec<String>>,
    pub output_texture_node_id: String,
    pub output_texture_name: ResourceName,
    pub resolution: [u32; 2],

    pub baked_data_parse:
        HashMap<(String, String, String), Vec<crate::renderer::types::BakedValue>>,
}

/// Lightweight diagnostics collected during scene prep stages.
#[derive(Clone, Debug, Default)]
pub struct ScenePrepReport {
    pub expanded_group_instances: usize,
    pub auto_wrapped_pass_inputs: usize,
    pub inlined_image_file_bindings: usize,
}
