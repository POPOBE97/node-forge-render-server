use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};

use crate::{
    dsl::{SceneDSL, incoming_connection},
    renderer::{
        geometry_resolver::is_pass_like_node_type,
        scene_prep::composite_layers_in_draw_order,
        types::PassOutputRegistry,
        wgsl::{build_blur_image_wgsl_bundle, build_pass_wgsl_bundle},
    },
};

use super::types::PassTextureBinding;

pub(crate) fn resolve_pass_texture_bindings(
    pass_output_registry: &PassOutputRegistry,
    pass_node_ids: &[String],
) -> Result<Vec<PassTextureBinding>> {
    let mut out: Vec<PassTextureBinding> = Vec::with_capacity(pass_node_ids.len());
    for upstream_pass_id in pass_node_ids {
        let Some(tex) = pass_output_registry.get_texture(upstream_pass_id) else {
            bail!(
                "PassTexture references upstream pass {upstream_pass_id}, but its output texture is not registered yet. \
Ensure the upstream pass is rendered earlier in Composite draw order."
            );
        };
        out.push(PassTextureBinding {
            texture: tex.clone(),
            image_node_id: None,
        });
    }
    Ok(out)
}

fn deps_for_pass_node(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    pass_node_id: &str,
) -> Result<Vec<String>> {
    let Some(node) = nodes_by_id.get(pass_node_id) else {
        bail!("missing node for pass id: {pass_node_id}");
    };

    match node.node_type.as_str() {
        "RenderPass" => {
            let bundle = build_pass_wgsl_bundle(
                scene,
                nodes_by_id,
                None,
                None,
                pass_node_id,
                false,
                None,
                Vec::new(),
                String::new(),
                false,
            )?;
            Ok(bundle.pass_textures)
        }
        "GuassianBlurPass" => {
            let bundle = build_blur_image_wgsl_bundle(scene, nodes_by_id, pass_node_id)?;
            Ok(bundle.pass_textures)
        }
        "Downsample" => {
            // Downsample depends on the upstream pass provided on its `source` input.
            let source_conn = incoming_connection(scene, pass_node_id, "source")
                .ok_or_else(|| anyhow!("Downsample.source missing for {pass_node_id}"))?;
            Ok(vec![source_conn.from.node_id.clone()])
        }
        "Composite" => composite_layers_in_draw_order(scene, nodes_by_id, pass_node_id),
        "GradientBlur" => {
            // GradientBlur reads "source" input (not "pass").
            let Some(conn) = incoming_connection(scene, pass_node_id, "source") else {
                return Ok(Vec::new());
            };
            let source_is_pass = nodes_by_id
                .get(&conn.from.node_id)
                .is_some_and(|n| is_pass_like_node_type(&n.node_type));
            if source_is_pass {
                Ok(vec![conn.from.node_id.clone()])
            } else {
                // Non-pass source: compile the material expression to find transitive pass deps.
                let mut ctx = crate::renderer::types::MaterialCompileContext::default();
                let mut cache = std::collections::HashMap::new();
                crate::renderer::node_compiler::compile_material_expr(
                    scene,
                    nodes_by_id,
                    &conn.from.node_id,
                    Some(&conn.from.port_id),
                    &mut ctx,
                    &mut cache,
                )?;
                Ok(ctx.pass_textures)
            }
        }
        other => bail!("expected a pass node id, got node type {other} for {pass_node_id}"),
    }
}

fn visit_pass_node(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    pass_node_id: &str,
    deps_cache: &mut HashMap<String, Vec<String>>,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    out: &mut Vec<String>,
) -> Result<()> {
    if visited.contains(pass_node_id) {
        return Ok(());
    }
    if !visiting.insert(pass_node_id.to_string()) {
        bail!("cycle detected in pass dependencies at: {pass_node_id}");
    }

    let deps = if let Some(existing) = deps_cache.get(pass_node_id) {
        existing.clone()
    } else {
        let deps = deps_for_pass_node(scene, nodes_by_id, pass_node_id)?;
        deps_cache.insert(pass_node_id.to_string(), deps.clone());
        deps
    };

    for dep in deps {
        visit_pass_node(
            scene,
            nodes_by_id,
            dep.as_str(),
            deps_cache,
            visiting,
            visited,
            out,
        )?;
    }

    visiting.remove(pass_node_id);
    visited.insert(pass_node_id.to_string());
    out.push(pass_node_id.to_string());
    Ok(())
}

pub(crate) fn compute_pass_render_order(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    roots_in_draw_order: &[String],
) -> Result<Vec<String>> {
    let mut deps_cache: HashMap<String, Vec<String>> = HashMap::new();
    let mut visiting: HashSet<String> = HashSet::new();
    let mut visited: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    for root in roots_in_draw_order {
        visit_pass_node(
            scene,
            nodes_by_id,
            root.as_str(),
            &mut deps_cache,
            &mut visiting,
            &mut visited,
            &mut out,
        )?;
    }

    Ok(out)
}
