use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};
use rust_wgpu_fiber::ResourceName;

use crate::{
    dsl::{Node, SceneDSL, find_node, incoming_connection},
    renderer::utils::cpu_num_u32_min_1,
    schema,
};

use super::{
    composite::composition_layers_by_id,
    data_parse::bake_data_parse_nodes,
    graph::{topo_sort, upstream_reachable},
    group_expand::expand_group_instances,
    image_inline::inline_image_file_connections_into_image_textures,
    image_pass::expand_image_passes,
    pass_dedup::dedup_identical_passes,
    resource_pool::project_selected_resource_pools,
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
    crate::dsl::normalize_scene_color_literals(&mut expanded, &schema::load_default_scheme()?);
    let expanded_group_instances = expand_group_instances(&mut expanded)?;
    super::procedural_test_texture::expand(&mut expanded)?;
    let consumer_scoped_coercions =
        crate::renderer::render_plan::coercion::plan_consumer_scoped_coercions(&mut expanded)?;
    expand_image_passes(&mut expanded)?;
    let matrix_source_scene = expanded.clone();

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
    project_selected_resource_pools(&mut expanded, &render_target_id)?;

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
        working_color_space: expanded.working_color_space,
        metadata: expanded.metadata.clone(),
        nodes,
        connections,
        outputs: expanded.outputs.clone(),
        // Groups are not needed after expansion; keep for completeness.
        groups: expanded.groups.clone(),
        assets: expanded.assets.clone(),
        state_machine: expanded.state_machine.clone(),
        debug_artifacts: expanded.debug_artifacts.clone(),
    };

    let mut scene = scene;
    let auto_wrapped_pass_inputs = 0;

    // Deduplicate after lowering so equivalent explicit ImagePass draws can merge.
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

    // Presentation consumes a texture. Its producer owns its extent and format.
    let output = incoming_connection(&scene, &render_target_id, "texture")
        .map(|c| c.from.clone())
        .ok_or_else(|| anyhow!("RenderTarget.texture has no incoming connection"))?;

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

    let composition_layers_by_id = composition_layers_by_id(&scene, &nodes_by_id)?;
    let source = crate::renderer::pass_source::resolve_texture_source_ref(&scene, &nodes_by_id, &output)?;
    let mut composite_layers_in_draw_order = Vec::new();
    let source_node_id = match source {
        crate::renderer::pass_source::TextureSourceRef::Surface(source) => {
            composite_layers_in_draw_order.extend(
                composition_layers_by_id
                    .get(&source.source.node_id)
                    .into_iter()
                    .flatten()
                    .cloned(),
            );
            composite_layers_in_draw_order.push(source.source.node_id.clone());
            source.source.node_id
        }
        crate::renderer::pass_source::TextureSourceRef::Image { image_node_id, .. } => image_node_id,
    };
    let source_node = find_node(&nodes_by_id, &source_node_id)?;
    let output_target_node_id = if source_node.node_type == "Composite" {
        Some(
            incoming_connection(&scene, &source_node_id, "target")
                .ok_or_else(|| anyhow!("Composite.target has no incoming connection"))?
                .from.node_id.clone(),
        )
    } else {
        None
    };
    // This is only a planning default for unconnected geometry properties. The final
    // presentation size is read from the produced texture, never from Screen dimensions.
    let resolution = if let Some(target_id) = &output_target_node_id {
        let target = find_node(&nodes_by_id, target_id)?;
        if target.node_type != "RenderTexture" {
            bail!("Composite.target must come from RenderTexture, got {}", target.node_type);
        }
        [
            cpu_num_u32_min_1(&scene, &nodes_by_id, target, "width", 1024)?,
            cpu_num_u32_min_1(&scene, &nodes_by_id, target, "height", 1024)?,
        ]
    } else {
        [1024, 1024]
    };

    let baked_data_parse = bake_data_parse_nodes(&scene, &nodes_by_id, "__global", 1)?;

    let prepared = PreparedScene {
        matrix_source_scene,
        scene,
        nodes_by_id,
        ids,
        topo_order,
        composite_layers_in_draw_order,
        composition_layers_by_id,
        output,
        output_target_node_id,
        resolution,
        baked_data_parse,
    };

    let report = ScenePrepReport {
        expanded_group_instances,
        auto_wrapped_pass_inputs,
        consumer_scoped_coercions,
        inlined_image_file_bindings,
    };

    Ok((prepared, report))
}
