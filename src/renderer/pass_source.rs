use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};

use crate::{
    dsl::{Endpoint, Node, SceneDSL, incoming_connection},
    renderer::{geometry_resolver::is_pass_like_node_type, types::PassTextureRef},
};

/// Resolve a pass-typed endpoint to the concrete pass output that owns the texture.
///
/// `PassTexture.pass` is a zero-copy alias. The outermost alias owns the consumer-side binding
/// identity and sampler, while the returned source endpoint always names a real pass producer.
pub(crate) fn resolve_pass_source_ref(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    endpoint: &Endpoint,
) -> Result<PassTextureRef> {
    let mut visiting = HashSet::new();
    resolve_pass_source_ref_inner(scene, nodes_by_id, endpoint, &mut visiting)
}

fn resolve_pass_source_ref_inner(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    endpoint: &Endpoint,
    visiting: &mut HashSet<String>,
) -> Result<PassTextureRef> {
    let node = nodes_by_id.get(&endpoint.node_id).ok_or_else(|| {
        anyhow!(
            "pass source node '{}' does not exist for output '{}'",
            endpoint.node_id,
            endpoint.port_id
        )
    })?;

    if node.node_type == "PassTexture" && endpoint.port_id == "pass" {
        if !visiting.insert(node.id.clone()) {
            bail!(
                "cycle detected while resolving PassTexture.pass alias '{}'",
                node.id
            );
        }
        let input = incoming_connection(scene, &node.id, "pass")
            .ok_or_else(|| anyhow!("PassTexture.pass input is not connected for '{}'", node.id))?;
        let mut resolved =
            resolve_pass_source_ref_inner(scene, nodes_by_id, &input.from, visiting)?;
        visiting.remove(&node.id);
        resolved.binding_id = node.id.clone();
        resolved.sampler_node_id = Some(node.id.clone());
        return Ok(resolved);
    }

    if !is_pass_like_node_type(&node.node_type) {
        bail!(
            "pass source '{}.{}' must resolve to a pass producer, got {}",
            endpoint.node_id,
            endpoint.port_id,
            node.node_type
        );
    }

    Ok(PassTextureRef::direct(&endpoint.node_id, &endpoint.port_id))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use crate::dsl::{Connection, Endpoint, Metadata, Node, SceneDSL};

    use super::resolve_pass_source_ref;

    fn node(id: &str, node_type: &str) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: HashMap::new(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        }
    }

    fn edge(id: &str, from: &str, to: &str) -> Connection {
        Connection {
            id: id.to_string(),
            from: Endpoint {
                node_id: from.to_string(),
                port_id: "pass".to_string(),
            },
            to: Endpoint {
                node_id: to.to_string(),
                port_id: "pass".to_string(),
            },
        }
    }

    fn scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
        SceneDSL {
            version: "5.0".to_string(),
            metadata: Metadata {
                name: "pass aliases".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections,
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    #[test]
    fn outermost_pass_texture_alias_owns_the_sampler() {
        let scene = scene(
            vec![
                node("source", "RenderPass"),
                node("inner", "PassTexture"),
                node("outer", "PassTexture"),
            ],
            vec![
                edge("source-inner", "source", "inner"),
                edge("inner-outer", "inner", "outer"),
            ],
        );
        let nodes_by_id = scene
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.id.clone(), node))
            .collect();

        let resolved = resolve_pass_source_ref(
            &scene,
            &nodes_by_id,
            &Endpoint {
                node_id: "outer".to_string(),
                port_id: "pass".to_string(),
            },
        )
        .expect("alias should resolve");

        assert_eq!(resolved.source.node_id, "source");
        assert_eq!(resolved.source.port_id, "pass");
        assert_eq!(resolved.binding_id, "outer");
        assert_eq!(resolved.sampler_node_id.as_deref(), Some("outer"));
    }

    #[test]
    fn rejects_pass_texture_alias_cycles() {
        let scene = scene(
            vec![node("a", "PassTexture"), node("b", "PassTexture")],
            vec![edge("a-b", "a", "b"), edge("b-a", "b", "a")],
        );
        let nodes_by_id = scene
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.id.clone(), node))
            .collect();

        let error = resolve_pass_source_ref(
            &scene,
            &nodes_by_id,
            &Endpoint {
                node_id: "a".to_string(),
                port_id: "pass".to_string(),
            },
        )
        .expect_err("cycle must fail");

        assert!(error.to_string().contains("cycle detected"));
    }
}
