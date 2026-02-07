use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};

use crate::{
    dsl::{SceneDSL, incoming_connection},
    renderer::{
        types::PassOutputRegistry,
        wgsl::{build_blur_image_wgsl_bundle, build_pass_wgsl_bundle},
    },
};

use super::types::PassTextureBinding;

pub(crate) fn sampled_pass_node_ids(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
) -> Result<HashSet<String>> {
    // A pass must render into a dedicated intermediate texture if it will be sampled later.
    //
    // Originally we only treated passes referenced by explicit PassTexture nodes as "sampled".
    // But some material nodes (e.g. GlassMaterial) can directly depend on upstream pass textures
    // without a PassTexture node in the graph. Those dependencies show up in WGSL bundle
    // `pass_textures`, so we detect sampling by scanning all pass nodes and collecting their
    // referenced pass textures.
    let mut out: HashSet<String> = HashSet::new();

    for (node_id, node) in nodes_by_id {
        if !matches!(
            node.node_type.as_str(),
            "RenderPass" | "GuassianBlurPass" | "Downsample"
        ) {
            continue;
        }
        let deps = deps_for_pass_node(scene, nodes_by_id, node_id.as_str())?;
        out.extend(deps);
    }

    Ok(out)
}

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
