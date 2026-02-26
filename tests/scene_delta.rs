use std::collections::HashMap;

use node_forge_render_server::{
    dsl::{AssetEntry, Connection, Endpoint, Metadata, Node, SceneDSL},
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
        input_bindings: Vec::new(),
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
        groups: Vec::new(),
        assets: Default::default(),
    }
}

fn asset_entry(path: &str, name: &str, mime: &str, size: u64) -> AssetEntry {
    AssetEntry {
        path: path.to_string(),
        original_name: name.to_string(),
        mime_type: mime.to_string(),
        size: Some(size),
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
        groups: Vec::new(),
        assets: Default::default(),
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
        groups: None,
        assets_added: None,
        assets_removed: None,
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
fn scene_delta_applies_asset_manifest_add_update_and_remove() {
    let mut scene = base_scene();
    scene.assets.insert(
        "asset-a".to_string(),
        asset_entry("assets/asset-a.png", "asset-a.png", "image/png", 12),
    );
    let mut cache = SceneCache::from_scene_update(&scene);

    let delta_add_update = SceneDelta {
        version: "1.0".to_string(),
        nodes: SceneDeltaNodes {
            added: vec![],
            updated: vec![],
            removed: vec![],
        },
        connections: SceneDeltaConnections {
            added: vec![],
            updated: vec![],
            removed: vec![],
        },
        outputs: None,
        groups: None,
        assets_added: Some(HashMap::from([
            (
                "asset-a".to_string(),
                asset_entry("assets/asset-a.v2.png", "asset-a.v2.png", "image/png", 24),
            ),
            (
                "asset-b".to_string(),
                asset_entry(
                    "assets/asset-b.glb",
                    "asset-b.glb",
                    "model/gltf-binary",
                    100,
                ),
            ),
        ])),
        assets_removed: None,
    };

    apply_scene_delta(&mut cache, &delta_add_update);
    assert_eq!(cache.assets.len(), 2);
    assert_eq!(
        cache.assets.get("asset-a").map(|v| v.path.as_str()),
        Some("assets/asset-a.v2.png")
    );
    assert!(cache.assets.contains_key("asset-b"));

    let delta_remove = SceneDelta {
        version: "1.0".to_string(),
        nodes: SceneDeltaNodes {
            added: vec![],
            updated: vec![],
            removed: vec![],
        },
        connections: SceneDeltaConnections {
            added: vec![],
            updated: vec![],
            removed: vec![],
        },
        outputs: None,
        groups: None,
        assets_added: None,
        assets_removed: Some(vec!["asset-a".to_string()]),
    };

    apply_scene_delta(&mut cache, &delta_remove);
    assert!(!cache.assets.contains_key("asset-a"));
    assert!(cache.assets.contains_key("asset-b"));
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
