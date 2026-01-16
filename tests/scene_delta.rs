use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{Connection, Endpoint, Metadata, Node, SceneDSL},
    ws::{
        SceneCache, SceneDelta, SceneDeltaConnections, SceneDeltaNodes, apply_scene_delta,
        apply_scene_update, materialize_scene_dsl, prune_invalid_connections,
    },
};

fn node(id: &str) -> Node {
    Node {
        id: id.to_string(),
        node_type: "Test".to_string(),
        params: HashMap::new(),
        inputs: vec![],
        outputs: Vec::new(),
    }
}

fn conn(id: &str, from: &str, to: &str) -> Connection {
    Connection {
        id: id.to_string(),
        from: Endpoint {
            node_id: from.to_string(),
            port_id: "out".to_string(),
        },
        to: Endpoint {
            node_id: to.to_string(),
            port_id: "in".to_string(),
        },
    }
}

fn base_scene() -> SceneDSL {
    SceneDSL {
        version: "1.0".to_string(),
        metadata: Metadata {
            name: "base".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![node("a"), node("b")],
        connections: vec![conn("c1", "a", "b")],
        outputs: Some(HashMap::from([(String::from("main"), String::from("b"))])),
    }
}

#[test]
fn scene_update_replaces_cache() {
    let scene1 = base_scene();
    let mut cache = SceneCache::from_scene_update(&scene1);

    assert_eq!(cache.nodes_by_id.len(), 2);
    assert_eq!(cache.connections_by_id.len(), 1);
    assert_eq!(cache.outputs.get("main").map(String::as_str), Some("b"));

    let scene2 = SceneDSL {
        version: "1.0".to_string(),
        metadata: Metadata {
            name: "next".to_string(),
            created: None,
            modified: None,
        },
        nodes: vec![node("x")],
        connections: vec![],
        outputs: None,
    };

    apply_scene_update(&mut cache, &scene2);

    assert_eq!(cache.metadata.name, "next");
    assert_eq!(cache.nodes_by_id.len(), 1);
    assert!(cache.nodes_by_id.contains_key("x"));
    assert_eq!(cache.connections_by_id.len(), 0);
    assert!(cache.outputs.is_empty());
}

#[test]
fn scene_delta_applies_in_correct_order_and_preserves_outputs_when_missing() {
    let scene = base_scene();
    let mut cache = SceneCache::from_scene_update(&scene);

    // delta removes c1 and node a, then adds c2 (b->c) and node c.
    let delta = SceneDelta {
        version: "1.0".to_string(),
        nodes: SceneDeltaNodes {
            added: vec![node("c")],
            updated: vec![],
            removed: vec!["a".to_string()],
        },
        connections: SceneDeltaConnections {
            added: vec![conn("c2", "b", "c")],
            updated: vec![],
            removed: vec!["c1".to_string()],
        },
        outputs: None,
    };

    apply_scene_delta(&mut cache, &delta);

    assert!(!cache.nodes_by_id.contains_key("a"));
    assert!(cache.nodes_by_id.contains_key("b"));
    assert!(cache.nodes_by_id.contains_key("c"));

    assert!(!cache.connections_by_id.contains_key("c1"));
    assert!(cache.connections_by_id.contains_key("c2"));

    // outputs stays unchanged when delta.outputs missing
    assert_eq!(cache.outputs.get("main").map(String::as_str), Some("b"));
}

#[test]
fn prune_invalid_connections_removes_dangling_edges() {
    let mut scene = base_scene();
    scene.connections.push(conn("dangling", "a", "missing"));

    let mut cache = SceneCache::from_scene_update(&scene);
    prune_invalid_connections(&mut cache);

    assert!(cache.connections_by_id.contains_key("c1"));
    assert!(!cache.connections_by_id.contains_key("dangling"));
}

#[test]
fn materialize_scene_dsl_roundtrips_cache() {
    let scene = base_scene();
    let cache = SceneCache::from_scene_update(&scene);
    let materialized = materialize_scene_dsl(&cache);

    assert_eq!(materialized.version, "1.0");
    assert_eq!(materialized.metadata.name, "base");
    assert_eq!(materialized.nodes.len(), 2);
    assert_eq!(materialized.connections.len(), 1);
    assert_eq!(
        materialized
            .outputs
            .unwrap()
            .get("main")
            .map(String::as_str),
        Some("b")
    );
}
