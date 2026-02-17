use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use rust_wgpu_fiber::ResourceName;

use crate::{
    dsl::{Node, SceneDSL, find_node, incoming_connection},
    graph::{topo_sort, upstream_reachable},
    renderer::utils::cpu_num_u32_min_1,
    schema,
};

use super::{
    auto_wrap::auto_wrap_primitive_pass_inputs,
    composite::composite_layers_in_draw_order,
    data_parse::bake_data_parse_nodes,
    group_expand::expand_group_instances,
    image_inline::inline_image_file_connections_into_image_textures,
    pass_dedup::dedup_identical_passes,
    types::{PreparedScene, ScenePrepReport},
};

pub fn prepare_scene(input: &SceneDSL) -> Result<PreparedScene> {
    prepare_scene_with_report(input).map(|(prepared, _report)| prepared)
}

pub(crate) fn prepare_scene_with_report(
    input: &SceneDSL,
) -> Result<(PreparedScene, ScenePrepReport)> {
    // Expand group instances before any filtering/validation.
    let mut expanded = input.clone();
    let expanded_group_instances = expand_group_instances(&mut expanded)?;

    // 1) Locate the RenderTarget-category node. Without it, the graph has no "main" entry.
    let scheme = schema::load_default_scheme()?;
    let render_targets: Vec<&Node> = expanded
        .nodes
        .iter()
        .filter(|n| {
            scheme
                .nodes
                .get(&n.node_type)
                .and_then(|s| s.category.as_deref())
                == Some("RenderTarget")
        })
        .collect();

    if render_targets.is_empty() {
        bail!("missing RenderTarget category node (e.g. Screen/File)");
    }
    if render_targets.len() != 1 {
        let ids: Vec<String> = render_targets
            .iter()
            .map(|n| format!("{} ({})", n.id, n.node_type))
            .collect();
        bail!(
            "expected exactly 1 RenderTarget node, got {}: {}",
            render_targets.len(),
            ids.join(", ")
        );
    }

    let render_target_id = render_targets[0].id.clone();

    // 2) Keep only the upstream subgraph that contributes to the RenderTarget.
    // This avoids validation/compile failures caused by unrelated leftover subgraphs.
    let keep = upstream_reachable(&expanded, &render_target_id);

    let nodes: Vec<Node> = expanded
        .nodes
        .iter()
        .filter(|n| keep.contains(&n.id))
        .cloned()
        .collect();
    let connections = expanded
        .connections
        .iter()
        .filter(|c| keep.contains(&c.from.node_id) && keep.contains(&c.to.node_id))
        .cloned()
        .collect();
    let scene = SceneDSL {
        version: expanded.version.clone(),
        metadata: expanded.metadata.clone(),
        nodes,
        connections,
        outputs: expanded.outputs.clone(),
        // Groups are not needed after expansion; keep for completeness.
        groups: expanded.groups.clone(),
        assets: expanded.assets.clone(),
    };

    // Coerce primitive shader values into passes by synthesizing a fullscreen RenderPass.
    let mut scene = scene;
    let auto_wrapped_pass_inputs = auto_wrap_primitive_pass_inputs(&mut scene, &scheme);

    // Deduplicate identical pass subgraphs after auto-wrap so that synthesized
    // fullscreen bridge passes can also be merged.
    let dedup_report = dedup_identical_passes(&mut scene);
    if dedup_report.deduped_passes > 0 {
        eprintln!(
            "pass dedup: removed {} duplicate passes, {} orphaned nodes",
            dedup_report.deduped_passes, dedup_report.removed_nodes,
        );
    }

    // Inline ImageFile -> ImageTexture.image connections into ImageTexture params.
    let inlined_image_file_bindings =
        inline_image_file_connections_into_image_textures(&mut scene)?;

    // 3) The RenderTarget must be driven by Composite.pass.
    let output_node_id: String = incoming_connection(&scene, &render_target_id, "pass")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("RenderTarget.pass has no incoming connection"))?;

    // 4) Validate only the kept subgraph.
    schema::validate_scene_against(&scene, &scheme)?;

    let nodes_by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    let mut ids: HashMap<String, ResourceName> = HashMap::new();
    for n in &scene.nodes {
        ids.insert(n.id.clone(), n.id.clone().into());
    }

    let topo_order = topo_sort(&scene)?;

    let composite_layers_in_draw_order =
        composite_layers_in_draw_order(&scene, &nodes_by_id, &output_node_id)?;

    let output_node = find_node(&nodes_by_id, &output_node_id)?;
    if output_node.node_type != "Composite" {
        bail!(
            "RenderTarget.pass must come from Composite, got {}",
            output_node.node_type
        );
    }

    // New DSL contract: output target must be provided by Composite.target.
    let output_texture_node_id: String = incoming_connection(&scene, &output_node_id, "target")
        .map(|c| c.from.node_id.clone())
        .ok_or_else(|| anyhow!("Composite.target has no incoming connection"))?;

    let output_texture_name: ResourceName = ids
        .get(&output_texture_node_id)
        .cloned()
        .ok_or_else(|| anyhow!("missing name for node: {}", output_texture_node_id))?;

    let output_texture_node = find_node(&nodes_by_id, &output_texture_node_id)?;
    if output_texture_node.node_type != "RenderTexture" {
        bail!(
            "Composite.target must come from RenderTexture, got {}",
            output_texture_node.node_type
        );
    }

    let width = cpu_num_u32_min_1(&scene, &nodes_by_id, output_texture_node, "width", 1024)?;
    let height = cpu_num_u32_min_1(&scene, &nodes_by_id, output_texture_node, "height", 1024)?;
    let resolution = [width, height];

    let baked_data_parse = bake_data_parse_nodes(&nodes_by_id, "__global", 1)?;

    let prepared = PreparedScene {
        scene,
        nodes_by_id,
        ids,
        topo_order,
        composite_layers_in_draw_order,
        output_texture_node_id,
        output_texture_name,
        resolution,
        baked_data_parse,
    };

    let report = ScenePrepReport {
        expanded_group_instances,
        auto_wrapped_pass_inputs,
        inlined_image_file_bindings,
    };

    Ok((prepared, report))
}
