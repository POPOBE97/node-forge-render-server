mod auto_wrap;
mod composite;
mod data_parse;
pub(crate) mod data_parse_runtime;
pub(crate) mod graph;
mod group_expand;
mod image_inline;
mod image_pass;
mod pass_dedup;
mod pipeline;
mod resource_pool;
mod types;

#[cfg(test)]
pub(crate) use auto_wrap::materialize_pass_inputs;
pub use composite::{composite_layers_in_draw_order, composition_layers_by_id};
pub(crate) use data_parse::bake_data_parse_nodes;
pub use pipeline::prepare_scene;
pub(crate) use pipeline::prepare_scene_with_report;
pub use types::{PreparedScene, ScenePrepReport};
