use std::collections::{HashMap, HashSet};

use anyhow::{anyhow, bail, Result};

use crate::{
    dsl::{incoming_connection, SceneDSL},
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
        "Upsample" => {
            // Upsample depends on the upstream pass provided on its `source` input.
            let source_conn = incoming_connection(scene, pass_node_id, "source")
                .ok_or_else(|| anyhow!("Upsample.source missing for {pass_node_id}"))?;
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

pub(crate) fn sampled_pass_node_ids_from_roots(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, crate::dsl::Node>,
    roots_in_draw_order: &[String],
) -> Result<HashSet<String>> {
    let reachable = compute_pass_render_order(scene, nodes_by_id, roots_in_draw_order)?;
    let reachable_set: HashSet<String> = reachable.iter().cloned().collect();

    let mut out: HashSet<String> = HashSet::new();
    for node_id in reachable {
        let Some(node) = nodes_by_id.get(&node_id) else {
            continue;
        };
        if !is_pass_like_node_type(&node.node_type) {
            continue;
        }

        // Composite being reachable should not by itself force its input layers
        // to be treated as sampled outputs. We only mark passes sampled when a
        // non-Composite pass node actually samples them as textures.
        if node.node_type == "Composite" {
            continue;
        }

        let deps = deps_for_pass_node(scene, nodes_by_id, node_id.as_str())?;
        for dep in deps {
            if reachable_set.contains(&dep) {
                out.insert(dep);
            }
        }
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use anyhow::Result;
    use serde_json::json;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, NodePort, SceneDSL};

    use super::{compute_pass_render_order, sampled_pass_node_ids_from_roots};

    fn node(id: &str, node_type: &str) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
        }
    }

    #[test]
    fn upsample_depends_on_source_pass_in_render_order() -> Result<()> {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: Metadata {
                name: "upsample-pass-order".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("source_comp", "Composite"),
                node("upsample", "Upsample"),
                node("out_comp", "Composite"),
            ],
            connections: vec![
                Connection {
                    id: "c_source".to_string(),
                    from: Endpoint {
                        node_id: "source_comp".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "upsample".to_string(),
                        port_id: "source".to_string(),
                    },
                },
                Connection {
                    id: "c_out".to_string(),
                    from: Endpoint {
                        node_id: "upsample".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "out_comp".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
            ],
            outputs: None,
            groups: Vec::new(),
            assets: HashMap::new(),
        };

        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let order = compute_pass_render_order(&scene, &nodes_by_id, &[String::from("out_comp")])?;
        assert_eq!(order, vec!["source_comp", "upsample", "out_comp"]);
        Ok(())
    }

    #[test]
    fn sampled_pass_ids_from_roots_marks_reachable_processing_dependencies() -> Result<()> {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: Metadata {
                name: "sampled-from-roots-live".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("out", "Composite"),
                Node {
                    id: "rt".to_string(),
                    node_type: "RenderTexture".to_string(),
                    params: HashMap::from([
                        ("width".to_string(), json!(100)),
                        ("height".to_string(), json!(100)),
                    ]),
                    inputs: vec![],
                    input_bindings: vec![],
                    outputs: vec![],
                },
                node("p_live", "RenderPass"),
                node("ds_live", "Downsample"),
            ],
            connections: vec![
                Connection {
                    id: "c_target".to_string(),
                    from: Endpoint {
                        node_id: "rt".to_string(),
                        port_id: "texture".to_string(),
                    },
                    to: Endpoint {
                        node_id: "out".to_string(),
                        port_id: "target".to_string(),
                    },
                },
                Connection {
                    id: "c_source".to_string(),
                    from: Endpoint {
                        node_id: "p_live".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "ds_live".to_string(),
                        port_id: "source".to_string(),
                    },
                },
                Connection {
                    id: "c_out".to_string(),
                    from: Endpoint {
                        node_id: "ds_live".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "out".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
            ],
            outputs: Some(HashMap::from([(
                String::from("composite"),
                String::from("out"),
            )])),
            groups: Vec::new(),
            assets: HashMap::new(),
        };

        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let roots = vec![String::from("ds_live")];
        let sampled = sampled_pass_node_ids_from_roots(&scene, &nodes_by_id, &roots)?;
        assert!(
            sampled.contains("p_live"),
            "expected p_live sampled by ds_live, got: {sampled:?}"
        );
        assert!(
            !sampled.contains("ds_live"),
            "ds_live should not be considered sampled here, got: {sampled:?}"
        );
        Ok(())
    }

    #[test]
    fn sampled_pass_ids_from_roots_tracks_blur_non_pass_source_transitive_deps() -> Result<()> {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: Metadata {
                name: "sampled-from-roots-blur-mathclosure".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("blur", "GuassianBlurPass"),
                node("p0", "RenderPass"),
                node("p1", "RenderPass"),
                Node {
                    id: "mc".to_string(),
                    node_type: "MathClosure".to_string(),
                    params: HashMap::from([(
                        "source".to_string(),
                        json!(
                            "vec4 c0 = samplePass(l0, vUv);\nvec4 c1 = samplePass(l1, vUv);\noutput = c0 + c1;"
                        ),
                    )]),
                    inputs: vec![
                        NodePort {
                            id: "dynamic_l0".to_string(),
                            name: Some("l0".to_string()),
                            port_type: Some("pass".to_string()),
                        },
                        NodePort {
                            id: "dynamic_l1".to_string(),
                            name: Some("l1".to_string()),
                            port_type: Some("pass".to_string()),
                        },
                    ],
                    input_bindings: vec![],
                    outputs: vec![NodePort {
                        id: "output".to_string(),
                        name: Some("output".to_string()),
                        port_type: Some("color".to_string()),
                    }],
                },
            ],
            connections: vec![
                Connection {
                    id: "c_p0".to_string(),
                    from: Endpoint {
                        node_id: "p0".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "mc".to_string(),
                        port_id: "dynamic_l0".to_string(),
                    },
                },
                Connection {
                    id: "c_p1".to_string(),
                    from: Endpoint {
                        node_id: "p1".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "mc".to_string(),
                        port_id: "dynamic_l1".to_string(),
                    },
                },
                Connection {
                    id: "c_blur".to_string(),
                    from: Endpoint {
                        node_id: "mc".to_string(),
                        port_id: "output".to_string(),
                    },
                    to: Endpoint {
                        node_id: "blur".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
            ],
            outputs: None,
            groups: Vec::new(),
            assets: HashMap::new(),
        };

        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let roots = vec![String::from("blur")];
        let sampled = sampled_pass_node_ids_from_roots(&scene, &nodes_by_id, &roots)?;
        assert!(
            sampled.contains("p0"),
            "expected p0 sampled by blur->MathClosure(samplePass), got: {sampled:?}"
        );
        assert!(
            sampled.contains("p1"),
            "expected p1 sampled by blur->MathClosure(samplePass), got: {sampled:?}"
        );
        assert!(
            !sampled.contains("blur"),
            "blur itself should not be marked sampled here, got: {sampled:?}"
        );

        Ok(())
    }

    #[test]
    fn sampled_pass_ids_from_roots_ignores_dead_branch_sampling() -> Result<()> {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: Metadata {
                name: "sampled-from-roots-dead".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("out", "Composite"),
                Node {
                    id: "rt".to_string(),
                    node_type: "RenderTexture".to_string(),
                    params: HashMap::from([
                        ("width".to_string(), json!(100)),
                        ("height".to_string(), json!(100)),
                    ]),
                    inputs: vec![],
                    input_bindings: vec![],
                    outputs: vec![],
                },
                node("p_live", "RenderPass"),
                node("ds_dead", "Downsample"),
            ],
            connections: vec![
                Connection {
                    id: "c_target".to_string(),
                    from: Endpoint {
                        node_id: "rt".to_string(),
                        port_id: "texture".to_string(),
                    },
                    to: Endpoint {
                        node_id: "out".to_string(),
                        port_id: "target".to_string(),
                    },
                },
                Connection {
                    id: "c_out".to_string(),
                    from: Endpoint {
                        node_id: "p_live".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "out".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
                Connection {
                    id: "c_dead_source".to_string(),
                    from: Endpoint {
                        node_id: "p_live".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "ds_dead".to_string(),
                        port_id: "source".to_string(),
                    },
                },
            ],
            outputs: Some(HashMap::from([(
                String::from("composite"),
                String::from("out"),
            )])),
            groups: Vec::new(),
            assets: HashMap::new(),
        };

        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();

        let roots = vec![String::from("p_live")];
        let sampled = sampled_pass_node_ids_from_roots(&scene, &nodes_by_id, &roots)?;
        assert!(
            !sampled.contains("p_live"),
            "dead branch should not force p_live sampled, got: {sampled:?}"
        );
        Ok(())
    }
}
