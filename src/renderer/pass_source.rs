use std::collections::{HashMap, HashSet};

use anyhow::{Result, anyhow, bail};

use crate::{
    dsl::{Endpoint, Node, SceneDSL, incoming_connection},
    renderer::{geometry_resolver::is_pass_like_node_type, types::PassTextureRef},
};

/// A texture-domain resource keeps native image textures zero-copy while allowing pass
/// endpoints to be materialized by the render planner for the current consumer context.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) enum TextureSourceRef {
    Image {
        binding_id: String,
        image_node_id: String,
        sampler_node_id: Option<String>,
    },
    Surface(PassTextureRef),
}

pub(crate) fn processing_input_connection<'a>(
    scene: &'a SceneDSL,
    node_id: &str,
) -> Result<(&'a crate::dsl::Connection, &'static str)> {
    if let Some(connection) = incoming_connection(scene, node_id, "texture") {
        return Ok((connection, "texture"));
    }

    // A blurred ImagePass is a lowered draw-producer macro, not a public processing node.
    // Its pass-domain invocation accepts a private RenderPass source while its author-facing ABI
    // continues to expose both pass and texture outputs.
    let is_lowered_image_pass = scene
        .nodes
        .iter()
        .any(|node| node.id == node_id && node.node_type == "ImagePass");
    if is_lowered_image_pass {
        if let Some(connection) = incoming_connection(scene, node_id, "pass") {
            return Ok((connection, "pass"));
        }
    }

    Err(anyhow!(
        "processing node '{node_id}' requires texture input"
    ))
}

pub(crate) fn resolve_texture_source_ref(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    endpoint: &Endpoint,
) -> Result<TextureSourceRef> {
    let mut visiting = HashSet::new();
    resolve_texture_source_ref_inner(scene, nodes_by_id, endpoint, &mut visiting)
}

fn resolve_texture_source_ref_inner(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    endpoint: &Endpoint,
    visiting: &mut HashSet<String>,
) -> Result<TextureSourceRef> {
    let node = nodes_by_id.get(&endpoint.node_id).ok_or_else(|| {
        anyhow!(
            "texture source node '{}' does not exist for output '{}'",
            endpoint.node_id,
            endpoint.port_id
        )
    })?;

    if node.node_type == "TextureSampler" && endpoint.port_id == "texture" {
        if !visiting.insert(node.id.clone()) {
            bail!(
                "cycle detected while resolving TextureSampler.texture alias '{}'",
                node.id
            );
        }
        let input = incoming_connection(scene, &node.id, "texture").ok_or_else(|| {
            anyhow!(
                "TextureSampler.texture input is not connected for '{}'",
                node.id
            )
        })?;
        let mut resolved =
            resolve_texture_source_ref_inner(scene, nodes_by_id, &input.from, visiting)?;
        visiting.remove(&node.id);
        match &mut resolved {
            TextureSourceRef::Image {
                binding_id,
                sampler_node_id,
                ..
            } => {
                *binding_id = node.id.clone();
                *sampler_node_id = Some(node.id.clone());
            }
            TextureSourceRef::Surface(texture_ref) => {
                texture_ref.binding_id = node.id.clone();
                texture_ref.sampler_node_id = Some(node.id.clone());
            }
        }
        return Ok(resolved);
    }

    if node.node_type == "ImageTexture" && endpoint.port_id == "texture" {
        return Ok(TextureSourceRef::Image {
            binding_id: node.id.clone(),
            image_node_id: node.id.clone(),
            sampler_node_id: None,
        });
    }

    if is_pass_like_node_type(&node.node_type) {
        return Ok(TextureSourceRef::Surface(PassTextureRef::direct(
            &endpoint.node_id,
            &endpoint.port_id,
        )));
    }

    bail!(
        "texture source '{}.{}' must resolve to ImageTexture.texture or a pass producer, got {}",
        endpoint.node_id,
        endpoint.port_id,
        node.node_type
    )
}

/// Resolve a pass-typed endpoint to the concrete pass output that owns the texture.
///
/// `TextureSampler.texture` is a zero-copy alias. The outermost alias owns the consumer-side binding
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

    if node.node_type == "TextureSampler" && endpoint.port_id == "texture" {
        if !visiting.insert(node.id.clone()) {
            bail!(
                "cycle detected while resolving TextureSampler.texture alias '{}'",
                node.id
            );
        }
        let input = incoming_connection(scene, &node.id, "texture").ok_or_else(|| {
            anyhow!(
                "TextureSampler.texture input is not connected for '{}'",
                node.id
            )
        })?;
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

    use super::{TextureSourceRef, resolve_texture_source_ref};

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
                port_id: "texture".to_string(),
            },
            to: Endpoint {
                node_id: to.to_string(),
                port_id: "texture".to_string(),
            },
        }
    }

    fn scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
        SceneDSL {
            version: "6.0".to_string(),
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
    fn outermost_texture_sampler_alias_owns_the_sampler() {
        let scene = scene(
            vec![
                node("source", "RenderPass"),
                node("inner", "TextureSampler"),
                node("outer", "TextureSampler"),
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

        let resolved = resolve_texture_source_ref(
            &scene,
            &nodes_by_id,
            &Endpoint {
                node_id: "outer".to_string(),
                port_id: "texture".to_string(),
            },
        )
        .expect("alias should resolve");

        let TextureSourceRef::Surface(resolved) = resolved else {
            panic!("expected a materialized surface");
        };
        assert_eq!(resolved.source.node_id, "source");
        assert_eq!(resolved.source.port_id, "texture");
        assert_eq!(resolved.binding_id, "outer");
        assert_eq!(resolved.sampler_node_id.as_deref(), Some("outer"));
    }

    #[test]
    fn rejects_texture_sampler_alias_cycles() {
        let scene = scene(
            vec![node("a", "TextureSampler"), node("b", "TextureSampler")],
            vec![edge("a-b", "a", "b"), edge("b-a", "b", "a")],
        );
        let nodes_by_id = scene
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.id.clone(), node))
            .collect();

        let error = resolve_texture_source_ref(
            &scene,
            &nodes_by_id,
            &Endpoint {
                node_id: "a".to_string(),
                port_id: "texture".to_string(),
            },
        )
        .expect_err("cycle must fail");

        assert!(error.to_string().contains("cycle detected"));
    }
}
