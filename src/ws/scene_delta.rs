use std::collections::{HashMap, HashSet, VecDeque};

use serde_json::Value;

use crate::dsl::{Connection, GroupDSL, Metadata, Node, SceneDSL};

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SceneDelta {
    pub version: String,
    pub nodes: SceneDeltaNodes,
    pub connections: SceneDeltaConnections,
    #[serde(default)]
    pub outputs: Option<HashMap<String, String>>,
    // Groups are currently only sent in full `scene_update` messages.
    // Keep this optional for forward-compatibility if editors start sending deltas.
    #[serde(default)]
    pub groups: Option<Vec<GroupDSL>>,
    /// Optional state-machine patch:
    /// - `None`: field absent in delta (leave cache unchanged)
    /// - `Some(None)`: explicit `null` (clear state machine)
    /// - `Some(Some(sm))`: replace state machine
    #[serde(default, rename = "stateMachine", alias = "state_machine")]
    pub state_machine: Option<Option<crate::state_machine::types::StateMachine>>,
    /// Asset metadata added/updated by this delta (upsert semantics).
    #[serde(rename = "assetsAdded", default)]
    pub assets_added: Option<HashMap<String, crate::dsl::AssetEntry>>,
    /// Asset ids removed by this delta.
    #[serde(rename = "assetsRemoved", default)]
    pub assets_removed: Option<Vec<String>>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SceneDeltaNodes {
    pub added: Vec<Node>,
    pub updated: Vec<Node>,
    pub removed: Vec<String>,
}

#[derive(Debug, Clone, serde::Deserialize)]
pub struct SceneDeltaConnections {
    pub added: Vec<Connection>,
    pub updated: Vec<Connection>,
    pub removed: Vec<String>,
}

pub type SceneOutputs = HashMap<String, String>;
pub type SceneCacheNodesById = HashMap<String, Node>;
pub type SceneCacheConnectionsById = HashMap<String, Connection>;

#[derive(Debug, Clone)]
pub struct SceneCache {
    pub version: String,
    pub metadata: Metadata,
    pub nodes_by_id: SceneCacheNodesById,
    pub connections_by_id: SceneCacheConnectionsById,
    pub outputs: SceneOutputs,
    pub groups: Vec<GroupDSL>,
    pub assets: HashMap<String, crate::dsl::AssetEntry>,
    pub state_machine: Option<crate::state_machine::types::StateMachine>,
}

impl SceneCache {
    pub fn from_scene_update(scene: &SceneDSL) -> Self {
        let mut cache = Self {
            version: scene.version.clone(),
            metadata: scene.metadata.clone(),
            nodes_by_id: HashMap::new(),
            connections_by_id: HashMap::new(),
            outputs: scene.outputs.clone().unwrap_or_default(),
            groups: scene.groups.clone(),
            assets: scene.assets.clone(),
            state_machine: scene.state_machine.clone(),
        };
        apply_scene_update(&mut cache, scene);
        cache
    }
}

pub fn apply_scene_update(cache: &mut SceneCache, scene: &SceneDSL) {
    cache.version = scene.version.clone();
    cache.metadata = scene.metadata.clone();
    cache.groups = scene.groups.clone();
    cache.assets = scene.assets.clone();
    cache.state_machine = scene.state_machine.clone();

    cache.nodes_by_id.clear();
    for node in &scene.nodes {
        cache.nodes_by_id.insert(node.id.clone(), node.clone());
    }

    cache.connections_by_id.clear();
    for conn in &scene.connections {
        cache
            .connections_by_id
            .insert(conn.id.clone(), conn.clone());
    }

    cache.outputs = scene.outputs.clone().unwrap_or_default();
}

pub fn apply_scene_delta(cache: &mut SceneCache, delta: &SceneDelta) {
    for connection_id in &delta.connections.removed {
        cache.connections_by_id.remove(connection_id);
    }

    for node_id in &delta.nodes.removed {
        cache.nodes_by_id.remove(node_id);
    }

    for node in &delta.nodes.added {
        cache.nodes_by_id.insert(node.id.clone(), node.clone());
    }
    for node in &delta.nodes.updated {
        let mut merged = node.clone();
        let has_label = merged
            .params
            .get("label")
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.trim().is_empty());
        if !has_label {
            if let Some(prev) = cache.nodes_by_id.get(&merged.id) {
                if let Some(prev_label) = prev
                    .params
                    .get("label")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.trim().is_empty())
                {
                    merged
                        .params
                        .insert("label".to_string(), Value::String(prev_label.to_string()));
                }
            }
        }
        cache.nodes_by_id.insert(merged.id.clone(), merged);
    }

    for conn in &delta.connections.added {
        cache
            .connections_by_id
            .insert(conn.id.clone(), conn.clone());
    }
    for conn in &delta.connections.updated {
        cache
            .connections_by_id
            .insert(conn.id.clone(), conn.clone());
    }

    if let Some(outputs) = delta.outputs.as_ref() {
        cache.outputs = outputs.clone();
    }

    if let Some(groups) = delta.groups.as_ref() {
        cache.groups = groups.clone();
    }

    if let Some(state_machine) = delta.state_machine.as_ref() {
        cache.state_machine = state_machine.clone();
    }

    // Merge asset metadata carried by the delta.
    if let Some(assets) = delta.assets_added.as_ref() {
        for (id, entry) in assets {
            cache.assets.insert(id.clone(), entry.clone());
        }
    }

    if let Some(asset_ids) = delta.assets_removed.as_ref() {
        for asset_id in asset_ids {
            cache.assets.remove(asset_id);
        }
    }
}

fn is_value_driven_input_node_type(node_type: &str) -> bool {
    matches!(
        node_type,
        "BoolInput" | "FloatInput" | "IntInput" | "Vector2Input" | "Vector3Input" | "ColorInput"
    )
}

fn is_uniform_param_key(key: &str) -> bool {
    matches!(key, "value" | "x" | "y" | "z" | "w" | "v")
}

fn node_params_changed_only_uniform_keys(
    prev: &HashMap<String, Value>,
    next: &HashMap<String, Value>,
) -> bool {
    let mut saw_change = false;
    for (key, after) in next {
        let before = prev.get(key);
        if before != Some(after) {
            saw_change = true;
            if !is_uniform_param_key(key) {
                return false;
            }
        }
    }

    saw_change
}

pub(crate) fn delta_updates_only_uniform_values(cache: &SceneCache, delta: &SceneDelta) -> bool {
    if delta.nodes.updated.is_empty() {
        return false;
    }

    if !delta.nodes.added.is_empty()
        || !delta.nodes.removed.is_empty()
        || !delta.connections.added.is_empty()
        || !delta.connections.updated.is_empty()
        || !delta.connections.removed.is_empty()
        || delta.outputs.is_some()
        || delta.groups.is_some()
        || delta.state_machine.is_some()
        || delta.assets_added.is_some()
        || delta.assets_removed.is_some()
    {
        return false;
    }

    for updated in &delta.nodes.updated {
        let Some(prev) = cache.nodes_by_id.get(&updated.id) else {
            return false;
        };
        if prev.node_type != updated.node_type {
            return false;
        }
        if !is_value_driven_input_node_type(updated.node_type.as_str()) {
            return false;
        }
        if !node_params_changed_only_uniform_keys(&prev.params, &updated.params) {
            return false;
        }
        if uniform_delta_change_affects_geometry_allocation(cache, &updated.id) {
            return false;
        }
    }

    true
}

fn is_geometry_allocation_sink(node_type: &str, port_id: &str) -> bool {
    matches!(
        (node_type, port_id),
        ("Rect2DGeometry", "size")
            | ("RenderPass", "camera")
            | ("GuassianBlurPass", "camera")
            | ("GradientBlur", "camera")
            | ("Downsample", "targetSize")
            | ("Downsample", "camera")
            | ("Upsample", "camera")
            | ("Upsample", "targetSize")
            | ("Composite", "camera")
            | ("SetTransform", "matrix")
            | ("TransformGeometry", "matrix")
            | ("RenderTexture", "width")
            | ("RenderTexture", "height")
    )
}

fn uniform_delta_change_affects_geometry_allocation(
    cache: &SceneCache,
    updated_node_id: &str,
) -> bool {
    let mut queue: VecDeque<String> = VecDeque::from([updated_node_id.to_string()]);
    let mut visited: HashSet<String> = HashSet::new();

    while let Some(from_id) = queue.pop_front() {
        if !visited.insert(from_id.clone()) {
            continue;
        }

        for conn in cache.connections_by_id.values() {
            if conn.from.node_id != from_id {
                continue;
            }

            if let Some(dst_node) = cache.nodes_by_id.get(&conn.to.node_id) {
                if is_geometry_allocation_sink(
                    dst_node.node_type.as_str(),
                    conn.to.port_id.as_str(),
                ) {
                    return true;
                }
            }
            queue.push_back(conn.to.node_id.clone());
        }
    }

    false
}

pub fn prune_invalid_connections(cache: &mut SceneCache) {
    cache.connections_by_id.retain(|_, conn| {
        cache.nodes_by_id.contains_key(&conn.from.node_id)
            && cache.nodes_by_id.contains_key(&conn.to.node_id)
    });
}

pub fn has_dangling_connection_references(cache: &SceneCache) -> bool {
    cache.connections_by_id.values().any(|conn| {
        !cache.nodes_by_id.contains_key(&conn.from.node_id)
            || !cache.nodes_by_id.contains_key(&conn.to.node_id)
    })
}

pub fn materialize_scene_dsl(cache: &SceneCache) -> SceneDSL {
    SceneDSL {
        version: cache.version.clone(),
        metadata: cache.metadata.clone(),
        nodes: cache.nodes_by_id.values().cloned().collect(),
        connections: cache.connections_by_id.values().cloned().collect(),
        outputs: Some(cache.outputs.clone()),
        groups: cache.groups.clone(),
        assets: cache.assets.clone(),
        state_machine: cache.state_machine.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{Connection, Endpoint, Metadata, SceneDSL};
    use serde_json::json;

    fn node(id: &str, node_type: &str, params: serde_json::Value) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params: serde_json::from_value(params).unwrap_or_default(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: Vec::new(),
        }
    }

    fn base_scene() -> SceneDSL {
        SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node(
                    "FloatInput_1",
                    "FloatInput",
                    json!({"value": 0.5, "min": 0.0}),
                ),
                node("MathAdd_1", "MathAdd", json!({})),
            ],
            connections: vec![Connection {
                id: "c1".to_string(),
                from: Endpoint {
                    node_id: "FloatInput_1".to_string(),
                    port_id: "value".to_string(),
                },
                to: Endpoint {
                    node_id: "MathAdd_1".to_string(),
                    port_id: "a".to_string(),
                },
            }],
            outputs: Some(std::collections::HashMap::new()),
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
        }
    }

    #[test]
    fn delta_updates_only_uniform_values_accepts_float_value_change() {
        let scene = base_scene();
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node(
                    "FloatInput_1",
                    "FloatInput",
                    json!({"value": 0.75, "min": 0.0}),
                )],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            state_machine: None,
            assets_added: None,
            assets_removed: None,
        };
        assert!(delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_accepts_partial_param_patch() {
        let scene = base_scene();
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node("FloatInput_1", "FloatInput", json!({"value": 0.9}))],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            state_machine: None,
            assets_added: None,
            assets_removed: None,
        };
        assert!(delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_rejects_structural_connection_changes() {
        let scene = base_scene();
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node(
                    "FloatInput_1",
                    "FloatInput",
                    json!({"value": 0.75, "min": 0.0}),
                )],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: vec![Connection {
                    id: "c2".to_string(),
                    from: Endpoint {
                        node_id: "FloatInput_1".to_string(),
                        port_id: "value".to_string(),
                    },
                    to: Endpoint {
                        node_id: "MathAdd_1".to_string(),
                        port_id: "b".to_string(),
                    },
                }],
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            state_machine: None,
            assets_added: None,
            assets_removed: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_rejects_non_uniform_param_change() {
        let scene = base_scene();
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node(
                    "FloatInput_1",
                    "FloatInput",
                    json!({"value": 0.75, "min": -1.0}),
                )],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            state_machine: None,
            assets_added: None,
            assets_removed: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_rejects_state_machine_patch() {
        let scene = base_scene();
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node("FloatInput_1", "FloatInput", json!({"value": 0.75}))],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            state_machine: Some(None),
            assets_added: None,
            assets_removed: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_rejects_geometry_allocation_sensitive_change() {
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("v2", "Vector2Input", json!({"x": 108.0, "y": 240.0})),
                node("rect", "Rect2DGeometry", json!({})),
                node("pass", "RenderPass", json!({})),
                node("comp", "Composite", json!({})),
                node("rt", "RenderTexture", json!({"width": 400, "height": 400})),
            ],
            connections: vec![
                Connection {
                    id: "c1".to_string(),
                    from: Endpoint {
                        node_id: "v2".to_string(),
                        port_id: "vector".to_string(),
                    },
                    to: Endpoint {
                        node_id: "rect".to_string(),
                        port_id: "size".to_string(),
                    },
                },
                Connection {
                    id: "c2".to_string(),
                    from: Endpoint {
                        node_id: "rect".to_string(),
                        port_id: "geometry".to_string(),
                    },
                    to: Endpoint {
                        node_id: "pass".to_string(),
                        port_id: "geometry".to_string(),
                    },
                },
                Connection {
                    id: "c3".to_string(),
                    from: Endpoint {
                        node_id: "pass".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "comp".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
                Connection {
                    id: "c4".to_string(),
                    from: Endpoint {
                        node_id: "rt".to_string(),
                        port_id: "texture".to_string(),
                    },
                    to: Endpoint {
                        node_id: "comp".to_string(),
                        port_id: "target".to_string(),
                    },
                },
            ],
            outputs: Some(std::collections::HashMap::new()),
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
        };
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node("v2", "Vector2Input", json!({"x": 54.0, "y": 120.0}))],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            state_machine: None,
            assets_added: None,
            assets_removed: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }

    #[test]
    fn delta_updates_only_uniform_values_rejects_camera_chain_change() {
        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                node("v3", "Vector3Input", json!({"x": 0.0, "y": 0.0, "z": 10.0})),
                node(
                    "cam",
                    "PerspectiveCamera",
                    json!({
                        "target": {"x": 0.0, "y": 0.0, "z": 0.0},
                        "up": {"x": 0.0, "y": 1.0, "z": 0.0},
                        "fovY": 60.0,
                        "aspect": 1.0,
                        "near": 0.1,
                        "far": 100.0
                    }),
                ),
                node("rect", "Rect2DGeometry", json!({})),
                node("pass", "RenderPass", json!({})),
                node("comp", "Composite", json!({})),
                node("rt", "RenderTexture", json!({"width": 400, "height": 400})),
            ],
            connections: vec![
                Connection {
                    id: "c1".to_string(),
                    from: Endpoint {
                        node_id: "v3".to_string(),
                        port_id: "vector".to_string(),
                    },
                    to: Endpoint {
                        node_id: "cam".to_string(),
                        port_id: "position".to_string(),
                    },
                },
                Connection {
                    id: "c2".to_string(),
                    from: Endpoint {
                        node_id: "rect".to_string(),
                        port_id: "geometry".to_string(),
                    },
                    to: Endpoint {
                        node_id: "pass".to_string(),
                        port_id: "geometry".to_string(),
                    },
                },
                Connection {
                    id: "c3".to_string(),
                    from: Endpoint {
                        node_id: "cam".to_string(),
                        port_id: "camera".to_string(),
                    },
                    to: Endpoint {
                        node_id: "pass".to_string(),
                        port_id: "camera".to_string(),
                    },
                },
                Connection {
                    id: "c4".to_string(),
                    from: Endpoint {
                        node_id: "pass".to_string(),
                        port_id: "pass".to_string(),
                    },
                    to: Endpoint {
                        node_id: "comp".to_string(),
                        port_id: "pass".to_string(),
                    },
                },
                Connection {
                    id: "c5".to_string(),
                    from: Endpoint {
                        node_id: "rt".to_string(),
                        port_id: "texture".to_string(),
                    },
                    to: Endpoint {
                        node_id: "comp".to_string(),
                        port_id: "target".to_string(),
                    },
                },
            ],
            outputs: Some(std::collections::HashMap::new()),
            groups: Vec::new(),
            assets: Default::default(),
            state_machine: None,
        };
        let cache = SceneCache::from_scene_update(&scene);
        let delta = SceneDelta {
            version: "1.0".to_string(),
            nodes: SceneDeltaNodes {
                added: Vec::new(),
                updated: vec![node(
                    "v3",
                    "Vector3Input",
                    json!({"x": 5.0, "y": 0.0, "z": 10.0}),
                )],
                removed: Vec::new(),
            },
            connections: SceneDeltaConnections {
                added: Vec::new(),
                updated: Vec::new(),
                removed: Vec::new(),
            },
            outputs: None,
            groups: None,
            state_machine: None,
            assets_added: None,
            assets_removed: None,
        };
        assert!(!delta_updates_only_uniform_values(&cache, &delta));
    }
}
