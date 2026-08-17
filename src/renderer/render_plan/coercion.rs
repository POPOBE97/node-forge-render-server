use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{Result, anyhow, bail};

use crate::{
    dsl::{Connection, Endpoint, Node, NodePort, SceneDSL, incoming_connection},
    renderer::utils::cpu_num_u32_min_1,
    schema,
};

const PROCESSING_NODE_TYPES: &[&str] = &[
    "GuassianBlurPass",
    "BloomNode",
    "GradientBlur",
    "Downsample",
    "Upsample",
    "Convolution",
];

const COMPOSITE_BLEND_PARAM_KEYS: &[&str] = &[
    "blend_preset",
    "blendfunc",
    "src_factor",
    "dst_factor",
    "src_alpha_factor",
    "dst_alpha_factor",
];

fn fullscreen_pass_params_for_consumer(
    nodes: &HashMap<String, Node>,
    consumer_node_id: &str,
) -> HashMap<String, serde_json::Value> {
    let mut params = HashMap::from([("loadOp".to_string(), serde_json::json!("none"))]);
    if let Some(consumer) = nodes
        .get(consumer_node_id)
        .filter(|node| node.node_type == "Composite")
    {
        params.extend(
            consumer
                .params
                .iter()
                .filter(|(key, _)| COMPOSITE_BLEND_PARAM_KEYS.contains(&key.as_str()))
                .map(|(key, value)| (key.clone(), value.clone())),
        );
    }
    params
}

/// Returns the planner-local invocations whose result must exist as a sampleable texture.
///
/// This is an execution-role decision, not a size inference: an explicit `texture` route always
/// materializes, while a `pass` route materializes only when its immediate consumer is not a
/// Composite sink. The consumer-scoped coercion/splitting pass has already made every invocation
/// unambiguous before this function is called.
pub(crate) fn materialized_texture_output_ids(scene: &SceneDSL) -> HashSet<String> {
    let nodes = scene
        .nodes
        .iter()
        .map(|node| (node.id.as_str(), node.node_type.as_str()))
        .collect::<HashMap<_, _>>();
    scene
        .connections
        .iter()
        .filter(|connection| {
            connection.from.port_id == "texture"
                || (connection.from.port_id == "pass"
                    && nodes
                        .get(connection.to.node_id.as_str())
                        .is_some_and(|node_type| *node_type != "Composite"))
        })
        .map(|connection| connection.from.node_id.clone())
        .collect()
}

/// Split simultaneous pass/texture routes into independent planner-local invocations. This keeps
/// one authored node while making the execution identity and paired input path unambiguous.
fn split_dual_domain_routes(scene: &mut SceneDSL) {
    let mut added_nodes = Vec::new();
    let mut added_connections = Vec::new();

    for node in scene.nodes.clone() {
        let outgoing_pass = scene.connections.iter().any(|connection| {
            connection.from.node_id == node.id && connection.from.port_id == "pass"
        });
        let outgoing_texture = scene.connections.iter().any(|connection| {
            connection.from.node_id == node.id && connection.from.port_id == "texture"
        });
        if !outgoing_pass || !outgoing_texture {
            continue;
        }

        let clone_id = format!("sys.route.texture.{}", node.id);
        let mut clone = node.clone();
        clone.id = clone_id.clone();
        clone
            .params
            .insert("__authoredNodeId".to_string(), serde_json::json!(node.id));
        added_nodes.push(clone);

        for connection in &scene.connections {
            if connection.to.node_id != node.id {
                continue;
            }
            let mut cloned_connection = connection.clone();
            cloned_connection.id = format!("sys.route.texture.edge.{}", connection.id);
            cloned_connection.to.node_id = clone_id.clone();
            added_connections.push(cloned_connection);
        }

        for connection in &mut scene.connections {
            if connection.from.node_id == node.id && connection.from.port_id == "texture" {
                connection.from.node_id = clone_id.clone();
            }
        }
    }

    scene.nodes.extend(added_nodes);
    scene.connections.extend(added_connections);
}

fn authored_node_id(node: &Node) -> &str {
    node.params
        .get("__authoredNodeId")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&node.id)
}

fn endpoint_target_dependent(
    scheme: &schema::NodeScheme,
    nodes: &HashMap<String, Node>,
    dependent_nodes: &HashSet<String>,
    endpoint: &Endpoint,
) -> bool {
    let Some(node) = nodes.get(&endpoint.node_id) else {
        return false;
    };
    if endpoint.port_id == "pass"
        && output_type(scheme, nodes, endpoint)
            .as_ref()
            .is_some_and(|port_type| contains_type(port_type, "pass"))
    {
        return true;
    }
    // A pass producer's explicit texture output owns a local GeoSize surface and is therefore
    // intentionally independent from downstream TargetContext propagation.
    if endpoint.port_id == "texture"
        && crate::renderer::geometry_resolver::is_pass_like_node_type(&node.node_type)
        && !PROCESSING_NODE_TYPES.contains(&node.node_type.as_str())
    {
        return false;
    }
    dependent_nodes.contains(&endpoint.node_id)
}

fn target_dependent_nodes(
    scene: &SceneDSL,
    scheme: &schema::NodeScheme,
    nodes: &HashMap<String, Node>,
) -> HashSet<String> {
    let mut dependent = scene
        .connections
        .iter()
        .filter(|connection| {
            connection.from.port_id == "pass"
                && output_type(scheme, nodes, &connection.from)
                    .as_ref()
                    .is_some_and(|port_type| contains_type(port_type, "pass"))
        })
        .map(|connection| connection.from.node_id.clone())
        .collect::<HashSet<_>>();
    loop {
        let mut changed = false;
        for connection in &scene.connections {
            if endpoint_target_dependent(scheme, nodes, &dependent, &connection.from)
                && dependent.insert(connection.to.node_id.clone())
            {
                changed = true;
            }
        }
        if !changed {
            return dependent;
        }
    }
}

fn nearest_downstream_composites(
    scene: &SceneDSL,
    nodes: &HashMap<String, Node>,
    start_node_id: &str,
) -> Result<HashSet<String>> {
    let mut queue = VecDeque::from([start_node_id.to_string()]);
    let mut visited = HashSet::new();
    let mut composites = HashSet::new();
    while let Some(node_id) = queue.pop_front() {
        if !visited.insert(node_id.clone()) {
            continue;
        }
        let node = nodes
            .get(&node_id)
            .ok_or_else(|| anyhow!("target-context consumer node '{node_id}' does not exist"))?;
        if node.node_type == "Composite" {
            composites.insert(node_id);
            continue;
        }
        for connection in &scene.connections {
            if connection.from.node_id == node_id {
                queue.push_back(connection.to.node_id.clone());
            }
        }
    }
    Ok(composites)
}

/// Give every target-dependent invocation exactly one downstream consumer context. The expansion
/// is planner-local: authored state identity is retained in `__authoredNodeId`, while generated
/// node ids provide the execution identity required by pass-output registries and GPU resources.
fn split_multi_target_invocations(
    scene: &mut SceneDSL,
    scheme: &schema::NodeScheme,
) -> Result<usize> {
    let mut split_count = 0;
    loop {
        let nodes: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.id.clone(), node))
            .collect();
        let dependent_nodes = target_dependent_nodes(scene, scheme, &nodes);
        let mut connection_contexts = HashMap::new();
        for connection in &scene.connections {
            connection_contexts.insert(
                connection.id.clone(),
                nearest_downstream_composites(scene, &nodes, &connection.to.node_id)?,
            );
        }

        let mut selected: Option<(Node, Vec<String>)> = None;
        for node in &scene.nodes {
            if node.node_type == "Composite" || !dependent_nodes.contains(&node.id) {
                continue;
            }
            let mut contexts = HashSet::new();
            let mut downstream_already_split = true;
            for connection in &scene.connections {
                if connection.from.node_id != node.id
                    || !endpoint_target_dependent(
                        scheme,
                        &nodes,
                        &dependent_nodes,
                        &connection.from,
                    )
                {
                    continue;
                }
                let edge_contexts = connection_contexts
                    .get(&connection.id)
                    .cloned()
                    .unwrap_or_default();
                if edge_contexts.len() > 1 {
                    downstream_already_split = false;
                    break;
                }
                contexts.extend(edge_contexts);
            }
            if downstream_already_split && contexts.len() > 1 {
                let mut contexts = contexts.into_iter().collect::<Vec<_>>();
                contexts.sort();
                selected = Some((node.clone(), contexts));
                break;
            }
        }

        let Some((node, contexts)) = selected else {
            return Ok(split_count);
        };
        let original_context = contexts
            .first()
            .expect("multi-target invocation has at least two contexts")
            .clone();
        let authored_id = authored_node_id(&node).to_string();
        let incoming = scene
            .connections
            .iter()
            .filter(|connection| connection.to.node_id == node.id)
            .cloned()
            .collect::<Vec<_>>();

        for context in contexts
            .into_iter()
            .filter(|value| value != &original_context)
        {
            let clone_id = format!("sys.target.{context}.{}", node.id);
            let mut clone = node.clone();
            clone.id = clone_id.clone();
            clone.params.insert(
                "__authoredNodeId".to_string(),
                serde_json::Value::String(authored_id.clone()),
            );
            scene.nodes.push(clone);
            for connection in &incoming {
                let mut cloned_connection = connection.clone();
                cloned_connection.id = format!("sys.target.{context}.edge.{}", connection.id);
                cloned_connection.to.node_id = clone_id.clone();
                scene.connections.push(cloned_connection);
            }
            for connection in &mut scene.connections {
                if connection.from.node_id != node.id {
                    continue;
                }
                let edge_contexts = connection_contexts
                    .get(&connection.id)
                    .cloned()
                    .unwrap_or_default();
                if edge_contexts.len() == 1 && edge_contexts.contains(&context) {
                    connection.from.node_id = clone_id.clone();
                }
            }
            split_count += 1;
        }
    }
}

fn contains_type(spec: &schema::PortTypeSpec, expected: &str) -> bool {
    match spec {
        schema::PortTypeSpec::One(value) => value == expected,
        schema::PortTypeSpec::Many(values) => values.iter().any(|value| value == expected),
    }
}

fn output_type(
    scheme: &schema::NodeScheme,
    nodes: &HashMap<String, Node>,
    endpoint: &Endpoint,
) -> Option<schema::PortTypeSpec> {
    let node = nodes.get(&endpoint.node_id)?;
    node.outputs
        .iter()
        .find(|port| port.id == endpoint.port_id)
        .and_then(|port| port.port_type.as_ref())
        .map(|value| schema::PortTypeSpec::One(value.clone()))
        .or_else(|| {
            scheme
                .nodes
                .get(&node.node_type)?
                .outputs
                .get(&endpoint.port_id)
                .cloned()
        })
}

fn input_type(
    scheme: &schema::NodeScheme,
    nodes: &HashMap<String, Node>,
    endpoint: &Endpoint,
) -> Option<schema::PortTypeSpec> {
    let node = nodes.get(&endpoint.node_id)?;
    if node.node_type == "Composite" && endpoint.port_id.starts_with("dynamic_") {
        return Some(schema::PortTypeSpec::One("pass".to_string()));
    }
    scheme
        .nodes
        .get(&node.node_type)?
        .inputs
        .get(&endpoint.port_id)
        .cloned()
        .or_else(|| {
            node.inputs
                .iter()
                .find(|port| port.id == endpoint.port_id)
                .and_then(|port| port.port_type.as_ref())
                .map(|value| schema::PortTypeSpec::One(value.clone()))
        })
}

fn downstream_target_sizes(
    scene: &SceneDSL,
    nodes: &HashMap<String, Node>,
    start_node_id: &str,
) -> Result<Vec<[u32; 2]>> {
    let mut queue = VecDeque::from([start_node_id.to_string()]);
    let mut visited = HashSet::new();
    let mut sizes = Vec::new();
    while let Some(node_id) = queue.pop_front() {
        if !visited.insert(node_id.clone()) {
            continue;
        }
        let node = nodes
            .get(&node_id)
            .ok_or_else(|| anyhow!("coercion consumer node '{node_id}' does not exist"))?;
        if node.node_type == "Composite" {
            let target = incoming_connection(scene, &node_id, "target").ok_or_else(|| {
                anyhow!("Composite.target is not connected while planning coercion for '{node_id}'")
            })?;
            let target_node = nodes.get(&target.from.node_id).ok_or_else(|| {
                anyhow!(
                    "coercion target node '{}' does not exist",
                    target.from.node_id
                )
            })?;
            let width = cpu_num_u32_min_1(scene, nodes, target_node, "width", 1024)?;
            let height = cpu_num_u32_min_1(scene, nodes, target_node, "height", 1024)?;
            if !sizes.contains(&[width, height]) {
                sizes.push([width, height]);
            }
            continue;
        }
        for connection in &scene.connections {
            if connection.from.node_id == node_id {
                queue.push_back(connection.to.node_id.clone());
            }
        }
    }
    Ok(sizes)
}

/// Create target-scoped fullscreen invocations in a planner-owned execution scene.
/// The authored SceneDSL remains unchanged and no global target-size guess is used.
pub(crate) fn plan_consumer_scoped_coercions(scene: &mut SceneDSL) -> Result<usize> {
    split_dual_domain_routes(scene);
    let scheme = schema::load_default_scheme()?;
    let invocation_count = split_multi_target_invocations(scene, &scheme)?;
    let nodes: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|node| (node.id.clone(), node))
        .collect();

    struct Plan {
        connection_index: usize,
        connection_id: String,
        source: Endpoint,
        source_is_texture: bool,
        size: [u32; 2],
        pass_params: HashMap<String, serde_json::Value>,
        pass_id: String,
        geometry_id: String,
        sample_id: String,
    }

    let mut plans = Vec::new();
    for (connection_index, connection) in scene.connections.iter().enumerate() {
        let Some(to_type) = input_type(&scheme, &nodes, &connection.to) else {
            continue;
        };
        if !contains_type(&to_type, "pass") {
            continue;
        }
        let Some(from_type) = output_type(&scheme, &nodes, &connection.from) else {
            continue;
        };
        if contains_type(&from_type, "pass") {
            continue;
        }
        if !schema::port_types_compatible(&scheme, &from_type, &to_type) {
            continue;
        }
        let target_sizes = downstream_target_sizes(scene, &nodes, &connection.to.node_id)?;
        let size = match target_sizes.as_slice() {
            [size] => *size,
            [] => bail!(
                "cannot find downstream TargetContext for coercion connection '{}'",
                connection.id
            ),
            _ => bail!(
                "coercion connection '{}' reaches multiple TargetContexts; route splitting must occur before materialization",
                connection.id
            ),
        };
        plans.push(Plan {
            connection_index,
            connection_id: connection.id.clone(),
            source: connection.from.clone(),
            source_is_texture: contains_type(&from_type, "texture"),
            size,
            pass_params: fullscreen_pass_params_for_consumer(&nodes, &connection.to.node_id),
            pass_id: format!("sys.coercion.fullscreen.pass.{}", connection.id),
            geometry_id: format!("sys.coercion.fullscreen.geometry.{}", connection.id),
            sample_id: format!("sys.coercion.fullscreen.sample.{}", connection.id),
        });
    }

    let mut added_connections = Vec::new();
    for plan in &plans {
        let [width, height] = plan.size;
        scene.nodes.push(Node {
            id: plan.geometry_id.clone(),
            node_type: "Rect2DGeometry".to_string(),
            params: HashMap::from([
                ("size".to_string(), serde_json::json!([width, height])),
                (
                    "position".to_string(),
                    serde_json::json!([width as f32 * 0.5, height as f32 * 0.5]),
                ),
            ]),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        });
        scene.nodes.push(Node {
            id: plan.pass_id.clone(),
            node_type: "RenderPass".to_string(),
            params: plan.pass_params.clone(),
            inputs: Vec::new(),
            input_bindings: Vec::new(),
            outputs: Vec::new(),
            wgsl_override: None,
        });
        added_connections.push(Connection {
            id: format!("sys.coercion.geometry.edge.{}", plan.connection_id),
            from: Endpoint {
                node_id: plan.geometry_id.clone(),
                port_id: "geometry".to_string(),
            },
            to: Endpoint {
                node_id: plan.pass_id.clone(),
                port_id: "geometry".to_string(),
            },
        });

        let material_source = if plan.source_is_texture {
            scene.nodes.push(Node {
                id: plan.sample_id.clone(),
                node_type: "MathClosure".to_string(),
                params: HashMap::from([(
                    "source".to_string(),
                    serde_json::Value::String("output = samplePass(texture, uv);".to_string()),
                )]),
                inputs: vec![NodePort {
                    id: "texture".to_string(),
                    name: Some("texture".to_string()),
                    port_type: Some("texture".to_string()),
                    array_length: None,
                }],
                input_bindings: Vec::new(),
                outputs: vec![NodePort {
                    id: "output".to_string(),
                    name: Some("Output".to_string()),
                    port_type: Some("color".to_string()),
                    array_length: None,
                }],
                wgsl_override: None,
            });
            added_connections.push(Connection {
                id: format!("sys.coercion.texture.edge.{}", plan.connection_id),
                from: plan.source.clone(),
                to: Endpoint {
                    node_id: plan.sample_id.clone(),
                    port_id: "texture".to_string(),
                },
            });
            Endpoint {
                node_id: plan.sample_id.clone(),
                port_id: "output".to_string(),
            }
        } else {
            plan.source.clone()
        };
        added_connections.push(Connection {
            id: format!("sys.coercion.material.edge.{}", plan.connection_id),
            from: material_source,
            to: Endpoint {
                node_id: plan.pass_id.clone(),
                port_id: "material".to_string(),
            },
        });
        scene.connections[plan.connection_index].from = Endpoint {
            node_id: plan.pass_id.clone(),
            port_id: "pass".to_string(),
        };
    }
    scene.connections.extend(added_connections);
    Ok(plans.len() + invocation_count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::Metadata;

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

    fn render_texture(id: &str, width: u32, height: u32) -> Node {
        let mut value = node(id, "RenderTexture");
        value
            .params
            .insert("width".to_string(), serde_json::json!(width));
        value
            .params
            .insert("height".to_string(), serde_json::json!(height));
        value
    }

    fn edge(id: &str, from: (&str, &str), to: (&str, &str)) -> Connection {
        Connection {
            id: id.to_string(),
            from: Endpoint {
                node_id: from.0.to_string(),
                port_id: from.1.to_string(),
            },
            to: Endpoint {
                node_id: to.0.to_string(),
                port_id: to.1.to_string(),
            },
        }
    }

    fn scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
        SceneDSL {
            version: "6.0".to_string(),
            metadata: Metadata {
                name: "coercion test".to_string(),
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
    fn texture_to_pass_is_target_sized_fullscreen_draw() {
        let mut scene = scene(
            vec![
                node("image", "ImageTexture"),
                node("composite", "Composite"),
                render_texture("target", 320, 180),
            ],
            vec![
                edge("layer", ("image", "texture"), ("composite", "pass")),
                edge(
                    "target-edge",
                    ("target", "texture"),
                    ("composite", "target"),
                ),
            ],
        );

        assert_eq!(plan_consumer_scoped_coercions(&mut scene).unwrap(), 1);
        let bridge = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.coercion.fullscreen.pass.layer")
            .expect("fullscreen pass");
        assert_eq!(bridge.node_type, "RenderPass");
        assert_eq!(
            bridge.params.get("loadOp"),
            Some(&serde_json::json!("none"))
        );
        let geometry = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.coercion.fullscreen.geometry.layer")
            .expect("fullscreen geometry");
        assert_eq!(
            geometry.params.get("size"),
            Some(&serde_json::json!([320, 180]))
        );
        assert!(scene.nodes.iter().any(|node| {
            node.id == "sys.coercion.fullscreen.sample.layer" && node.node_type == "MathClosure"
        }));
    }

    #[test]
    fn texture_to_composite_pass_inherits_composite_blend_policy() {
        let mut composite = node("composite", "Composite");
        composite
            .params
            .insert("blend_preset".to_string(), serde_json::json!("add"));
        composite
            .params
            .insert("src_factor".to_string(), serde_json::json!("one"));
        composite
            .params
            .insert("dst_factor".to_string(), serde_json::json!("one"));
        let mut scene = scene(
            vec![
                node("image", "ImageTexture"),
                composite,
                render_texture("target", 320, 180),
            ],
            vec![
                edge("layer", ("image", "texture"), ("composite", "pass")),
                edge(
                    "target-edge",
                    ("target", "texture"),
                    ("composite", "target"),
                ),
            ],
        );

        plan_consumer_scoped_coercions(&mut scene).unwrap();
        let bridge = scene
            .nodes
            .iter()
            .find(|node| node.id == "sys.coercion.fullscreen.pass.layer")
            .expect("fullscreen pass");
        assert_eq!(
            bridge.params.get("loadOp"),
            Some(&serde_json::json!("none"))
        );
        assert_eq!(
            bridge.params.get("blend_preset"),
            Some(&serde_json::json!("add"))
        );
        assert_eq!(
            bridge.params.get("dst_factor"),
            Some(&serde_json::json!("one"))
        );
    }

    #[test]
    fn pass_and_texture_domains_do_not_insert_unrequested_bridges() {
        let mut texture_scene = scene(
            vec![
                node("image", "ImageTexture"),
                node("sampler", "TextureSampler"),
            ],
            vec![edge(
                "texture-edge",
                ("image", "texture"),
                ("sampler", "texture"),
            )],
        );
        assert_eq!(
            plan_consumer_scoped_coercions(&mut texture_scene).unwrap(),
            0
        );

        let mut pass_scene = scene(
            vec![
                node("pass", "RenderPass"),
                node("composite", "Composite"),
                render_texture("target", 100, 50),
            ],
            vec![
                edge("pass-edge", ("pass", "pass"), ("composite", "pass")),
                edge(
                    "target-edge",
                    ("target", "texture"),
                    ("composite", "target"),
                ),
            ],
        );
        assert_eq!(plan_consumer_scoped_coercions(&mut pass_scene).unwrap(), 0);
    }

    #[test]
    fn image_pass_keeps_independent_pass_and_texture_invocations() {
        let mut scene = scene(
            vec![
                node("image-pass", "ImagePass"),
                node("sampler", "TextureSampler"),
                node("composite", "Composite"),
                render_texture("target", 320, 180),
            ],
            vec![
                edge("pass-edge", ("image-pass", "pass"), ("composite", "pass")),
                edge(
                    "texture-edge",
                    ("image-pass", "texture"),
                    ("sampler", "texture"),
                ),
                edge(
                    "target-edge",
                    ("target", "texture"),
                    ("composite", "target"),
                ),
            ],
        );

        assert_eq!(plan_consumer_scoped_coercions(&mut scene).unwrap(), 0);
        let pass_source = &incoming_connection(&scene, "composite", "pass")
            .expect("pass route")
            .from;
        assert_eq!(pass_source.node_id, "image-pass");
        assert_eq!(pass_source.port_id, "pass");
        let texture_source = &incoming_connection(&scene, "sampler", "texture")
            .expect("texture route")
            .from;
        assert_eq!(texture_source.node_id, "sys.route.texture.image-pass");
        assert_eq!(texture_source.port_id, "texture");
    }

    #[test]
    fn materialization_role_is_declared_by_route_not_graph_reachability() {
        let direct_pass = edge("direct", ("pass-a", "pass"), ("composite", "pass"));
        let sampled_pass = edge("sampled", ("pass-b", "pass"), ("consumer", "texture"));
        let explicit_texture = edge("explicit", ("pass-c", "texture"), ("consumer", "texture"));
        let scene = scene(
            vec![
                node("pass-a", "RenderPass"),
                node("pass-b", "RenderPass"),
                node("pass-c", "RenderPass"),
                node("consumer", "TextureSampler"),
                node("composite", "Composite"),
            ],
            vec![direct_pass, sampled_pass, explicit_texture],
        );

        let ids = materialized_texture_output_ids(&scene);
        assert!(!ids.contains("pass-a"));
        assert!(ids.contains("pass-b"));
        assert!(ids.contains("pass-c"));
    }

    #[test]
    fn one_pass_consumed_by_two_targets_gets_two_execution_identities() {
        let mut scene = scene(
            vec![
                node("source", "RenderPass"),
                node("a", "Composite"),
                node("b", "Composite"),
                render_texture("target-a", 100, 50),
                render_texture("target-b", 240, 120),
            ],
            vec![
                edge("layer-a", ("source", "pass"), ("a", "pass")),
                edge("layer-b", ("source", "pass"), ("b", "pass")),
                edge("target-a-edge", ("target-a", "texture"), ("a", "target")),
                edge("target-b-edge", ("target-b", "texture"), ("b", "target")),
            ],
        );

        assert_eq!(plan_consumer_scoped_coercions(&mut scene).unwrap(), 1);
        let a_source = incoming_connection(&scene, "a", "pass")
            .expect("a layer")
            .from
            .node_id
            .clone();
        let b_source = incoming_connection(&scene, "b", "pass")
            .expect("b layer")
            .from
            .node_id
            .clone();
        assert_ne!(a_source, b_source);
        let nodes = scene
            .nodes
            .iter()
            .cloned()
            .map(|node| (node.id.clone(), node))
            .collect::<HashMap<_, _>>();
        assert_eq!(
            downstream_target_sizes(&scene, &nodes, &a_source).unwrap(),
            vec![[100, 50]]
        );
        assert_eq!(
            downstream_target_sizes(&scene, &nodes, &b_source).unwrap(),
            vec![[240, 120]]
        );
        let clone = scene
            .nodes
            .iter()
            .find(|node| node.id != "source" && authored_node_id(node) == "source")
            .expect("consumer-scoped clone");
        assert_eq!(clone.node_type, "RenderPass");
    }
}
