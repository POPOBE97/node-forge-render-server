//! Deduplicates identical render pass subgraphs across expanded group instances.
//!
//! After `expand_group_instances()`, multiple GroupInstance expansions may create
//! structurally identical pass nodes (RenderPass, Downsample, GuassianBlurPass, Composite)
//! with the same shader logic but different node IDs. This module detects such
//! duplicates using recursive Merkle content signatures and merges them so that
//! only one canonical copy exists.
//!
//! The canonical copy is renamed to `sys.group.{groupId}/{originalId}` and all
//! downstream connections are rewired to reference the canonical pass.

use std::collections::{BTreeMap, HashMap, HashSet};

use std::hash::{DefaultHasher, Hash, Hasher};

use crate::dsl::SceneDSL;

/// Pass node types that produce render passes and can be deduplicated.
const PASS_NODE_TYPES: &[&str] = &["RenderPass", "Downsample", "GuassianBlurPass", "Composite"];

fn is_pass_node(node_type: &str) -> bool {
    PASS_NODE_TYPES.contains(&node_type)
}

/// Compute a deterministic content signature for a node, recursing into upstream
/// dependencies. The signature captures:
///   - node_type
///   - all params (except __dedup_* metadata), sorted by key
///   - for each connected input: (to_port_id, from_port_id, signature(upstream_node))
///
/// Two nodes with the same signature produce identical shader output regardless
/// of their node IDs.
fn compute_node_signature(
    node_id: &str,
    nodes_by_id: &HashMap<String, &crate::dsl::Node>,
    // Map: (to_node_id, to_port_id) → Vec<(from_node_id, from_port_id)>
    incoming: &HashMap<(String, String), Vec<(String, String)>>,
    memo: &mut HashMap<String, u64>,
) -> u64 {
    if let Some(&cached) = memo.get(node_id) {
        return cached;
    }

    // Sentinel to break cycles (shouldn't happen in a valid DAG, but be safe).
    memo.insert(node_id.to_string(), 0);

    let Some(node) = nodes_by_id.get(node_id) else {
        return 0;
    };

    let mut hasher = DefaultHasher::new();

    // 1) node_type
    node.node_type.hash(&mut hasher);

    // 2) params (excluding dedup metadata), sorted by key for determinism
    let params: BTreeMap<&String, &serde_json::Value> = node
        .params
        .iter()
        .filter(|(k, _)| !k.starts_with("__dedup_"))
        .collect();
    for (k, v) in &params {
        k.hash(&mut hasher);
        // Serialize to canonical JSON string for deterministic hashing
        let s = serde_json::to_string(v).unwrap_or_default();
        s.hash(&mut hasher);
    }

    // 3) Input connections, sorted by to_port_id for determinism.
    // Collect ports from BOTH the node's declared inputs AND the actual incoming
    // connections. Schema-defined ports (e.g. RenderPass.geometry, .material) may
    // not appear in the DSL node's `inputs` array, so relying only on `node.inputs`
    // would miss them and cause structurally different passes to look identical.
    let mut input_ports: HashSet<String> = node.inputs.iter().map(|p| p.id.clone()).collect();
    for ((to_nid, to_port), _) in incoming.iter() {
        if to_nid == node_id {
            input_ports.insert(to_port.clone());
        }
    }
    let mut input_ports: Vec<String> = input_ports.into_iter().collect();
    input_ports.sort();

    for port_id in &input_ports {
        let key = (node_id.to_string(), port_id.clone());
        if let Some(sources) = incoming.get(&key) {
            // Sort sources for determinism (usually only one per port)
            let mut sorted_sources: Vec<&(String, String)> = sources.iter().collect();
            sorted_sources.sort();
            for (from_node_id, from_port_id) in sorted_sources {
                port_id.hash(&mut hasher);
                from_port_id.hash(&mut hasher);
                let upstream_sig =
                    compute_node_signature(from_node_id, nodes_by_id, incoming, memo);
                upstream_sig.hash(&mut hasher);
            }
        }
    }

    // Also hash input_bindings sourceBinding info (they carry implicit connections)
    let mut bindings: Vec<(&str, &str, &str)> = Vec::new();
    for b in &node.input_bindings {
        if let Some(ref sb) = b.source_binding {
            bindings.push((
                b.port_id.as_str(),
                sb.node_id.as_str(),
                sb.output_port_id.as_str(),
            ));
        }
    }
    bindings.sort();
    for (port_id, src_node_id, src_port_id) in bindings {
        "binding".hash(&mut hasher);
        port_id.hash(&mut hasher);
        src_port_id.hash(&mut hasher);
        let upstream_sig = compute_node_signature(src_node_id, nodes_by_id, incoming, memo);
        upstream_sig.hash(&mut hasher);
    }

    let sig = hasher.finish();
    memo.insert(node_id.to_string(), sig);
    sig
}

/// Collect all upstream node IDs reachable from `start` (exclusive of `start` itself).
fn upstream_subgraph(
    start: &str,
    incoming: &HashMap<(String, String), Vec<(String, String)>>,
    nodes_by_id: &HashMap<String, &crate::dsl::Node>,
) -> HashSet<String> {
    let mut visited = HashSet::new();
    let mut stack = vec![start.to_string()];
    while let Some(nid) = stack.pop() {
        if !visited.insert(nid.clone()) {
            continue;
        }
        if let Some(node) = nodes_by_id.get(nid.as_str()) {
            for port in &node.inputs {
                let key = (nid.clone(), port.id.clone());
                if let Some(sources) = incoming.get(&key) {
                    for (from_id, _) in sources {
                        stack.push(from_id.clone());
                    }
                }
            }
            // Also follow input_bindings
            for b in &node.input_bindings {
                if let Some(ref sb) = b.source_binding {
                    stack.push(sb.node_id.clone());
                }
            }
        }
    }
    visited.remove(start);
    visited
}

/// Deduplicate identical pass subgraphs in the expanded scene.
///
/// Returns a report: `(deduped_pass_count, canonical_mapping)` where
/// `canonical_mapping` maps removed node IDs to their canonical replacement.
pub(crate) fn dedup_identical_passes(scene: &mut SceneDSL) -> DedupReport {
    // Build lookup structures.
    let nodes_by_id: HashMap<String, &crate::dsl::Node> =
        scene.nodes.iter().map(|n| (n.id.clone(), n)).collect();

    // Build incoming connection index: (to_node_id, to_port_id) → [(from_node_id, from_port_id)]
    let mut incoming: HashMap<(String, String), Vec<(String, String)>> = HashMap::new();
    for c in &scene.connections {
        incoming
            .entry((c.to.node_id.clone(), c.to.port_id.clone()))
            .or_default()
            .push((c.from.node_id.clone(), c.from.port_id.clone()));
    }

    // 1) Compute signatures for all pass nodes.
    let mut memo: HashMap<String, u64> = HashMap::new();
    let mut pass_sigs: Vec<(String, u64)> = Vec::new();

    for node in &scene.nodes {
        if is_pass_node(&node.node_type) {
            let sig = compute_node_signature(&node.id, &nodes_by_id, &incoming, &mut memo);
            pass_sigs.push((node.id.clone(), sig));
        }
    }

    // 2) Group pass nodes by signature.
    let mut groups_by_sig: HashMap<u64, Vec<String>> = HashMap::new();
    for (nid, sig) in &pass_sigs {
        groups_by_sig.entry(*sig).or_default().push(nid.clone());
    }

    // 3) For each group with >1 member, pick canonical and build redirect map.
    let mut canonical_map: HashMap<String, String> = HashMap::new(); // removed → canonical
    let mut nodes_to_remove: HashSet<String> = HashSet::new();

    for (_sig, mut members) in groups_by_sig {
        if members.len() <= 1 {
            continue;
        }

        // Sort for determinism; pick the shortest path as canonical (simpler ID).
        members.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));

        let canonical_original = &members[0];

        // Derive canonical name from dedup metadata when both fields exist.
        // For non-group-generated passes (e.g. sys.auto.fullscreen.pass.*), keep
        // the original canonical ID to avoid unstable sys.group.unknown renames.
        let canonical_name = if let Some(node) = nodes_by_id.get(canonical_original.as_str()) {
            let group_id = node.params.get("__dedup_group_id").and_then(|v| v.as_str());
            let original_id = node
                .params
                .get("__dedup_original_id")
                .and_then(|v| v.as_str());
            match (group_id, original_id) {
                (Some(group_id), Some(original_id)) => {
                    format!("sys.group.{group_id}/{original_id}")
                }
                _ => canonical_original.clone(),
            }
        } else {
            canonical_original.clone()
        };

        // Map old canonical name → new canonical name (if different)
        if *canonical_original != canonical_name {
            canonical_map.insert(canonical_original.clone(), canonical_name.clone());
        }

        // Map all duplicates → canonical name
        for dup in &members[1..] {
            canonical_map.insert(dup.clone(), canonical_name.clone());
            nodes_to_remove.insert(dup.clone());
        }
    }

    if canonical_map.is_empty() {
        return DedupReport {
            deduped_passes: 0,
            removed_nodes: 0,
        };
    }

    // 4) Also rename the upstream subgraphs exclusively owned by canonical passes.
    // For each canonical pass (the one being kept), rename its exclusive upstream
    // nodes to use the sys.group. prefix as well.
    let mut upstream_rename_map: HashMap<String, String> = HashMap::new();

    // Collect canonical passes (values in canonical_map that aren't in nodes_to_remove)
    let canonical_passes: HashSet<String> = canonical_map.values().cloned().collect();

    // For each canonical pass, find its upstream subgraph
    for canonical_name in &canonical_passes {
        // Find the original node ID that maps to this canonical name
        let original_id = canonical_map
            .iter()
            .find(|(_, v)| *v == canonical_name)
            .map(|(k, _)| k.clone())
            .unwrap_or_else(|| canonical_name.clone());

        // Get the node that will become canonical
        let source_id = if nodes_by_id.contains_key(&original_id) {
            original_id.clone()
        } else {
            continue;
        };

        let upstream = upstream_subgraph(&source_id, &incoming, &nodes_by_id);

        for up_id in &upstream {
            if nodes_to_remove.contains(up_id) || canonical_map.contains_key(up_id) {
                continue; // Already handled
            }
            // Derive new name using dedup metadata
            if let Some(node) = nodes_by_id.get(up_id.as_str()) {
                if let (Some(group_id), Some(orig_id)) = (
                    node.params.get("__dedup_group_id").and_then(|v| v.as_str()),
                    node.params
                        .get("__dedup_original_id")
                        .and_then(|v| v.as_str()),
                ) {
                    let new_name = format!("sys.group.{group_id}/{orig_id}");
                    if *up_id != new_name {
                        upstream_rename_map.insert(up_id.clone(), new_name);
                    }
                }
            }
        }
    }

    // 5) Find all upstream nodes exclusively owned by duplicate (removed) passes.
    // These need to be removed too.
    // First, collect upstream of ALL removed passes.
    let mut dup_upstream: HashSet<String> = HashSet::new();
    for dup_id in &nodes_to_remove {
        let upstream = upstream_subgraph(dup_id, &incoming, &nodes_by_id);
        dup_upstream.extend(upstream);
    }

    // Simpler approach: a node is "orphaned" if it's ONLY used by removed nodes.
    // Build outgoing index: from_node_id → [(to_node_id, ...)]
    let mut outgoing: HashMap<String, Vec<String>> = HashMap::new();
    for c in &scene.connections {
        outgoing
            .entry(c.from.node_id.clone())
            .or_default()
            .push(c.to.node_id.clone());
    }

    // A node in dup_upstream is "orphaned" if ALL its downstream consumers are
    // either in nodes_to_remove or themselves orphaned.
    // We solve this iteratively.
    let mut orphaned: HashSet<String> = HashSet::new();
    let mut changed = true;
    while changed {
        changed = false;
        for up_id in &dup_upstream {
            if orphaned.contains(up_id) || nodes_to_remove.contains(up_id) {
                continue;
            }
            // Check if the canonical pass's upstream rename targets this node
            if upstream_rename_map.contains_key(up_id) {
                continue; // This node belongs to the canonical pass, keep it
            }
            if canonical_map.contains_key(up_id) {
                continue; // This is a canonical node
            }
            // Check all consumers
            if let Some(consumers) = outgoing.get(up_id) {
                let all_removed = consumers
                    .iter()
                    .all(|c| nodes_to_remove.contains(c) || orphaned.contains(c));
                if all_removed {
                    orphaned.insert(up_id.clone());
                    changed = true;
                }
            }
        }
    }
    nodes_to_remove.extend(orphaned);

    // 6) Merge canonical_map and upstream_rename_map into a single rewrite map.
    let mut rewrite_map: HashMap<String, String> = HashMap::new();
    for (old, new) in &canonical_map {
        if !nodes_to_remove.contains(old) {
            // This is a rename (canonical pass getting sys.group. prefix)
            rewrite_map.insert(old.clone(), new.clone());
        }
    }
    for (old, new) in &upstream_rename_map {
        rewrite_map.insert(old.clone(), new.clone());
    }
    // Also add the duplicate→canonical mapping for connection rewriting
    for (dup, canonical) in &canonical_map {
        rewrite_map.insert(dup.clone(), canonical.clone());
    }

    let deduped_passes = nodes_to_remove
        .iter()
        .filter(|id| {
            nodes_by_id
                .get(id.as_str())
                .is_some_and(|n| is_pass_node(&n.node_type))
        })
        .count();

    // 7) Rename nodes that are being kept but need new IDs.
    for node in &mut scene.nodes {
        if nodes_to_remove.contains(&node.id) {
            continue; // Will be removed
        }
        if let Some(new_id) = rewrite_map.get(&node.id) {
            node.id = new_id.clone();
        }
        // Rewrite input_bindings source references
        for b in &mut node.input_bindings {
            if let Some(ref mut sb) = b.source_binding {
                if let Some(new_id) = rewrite_map.get(&sb.node_id) {
                    sb.node_id = new_id.clone();
                }
            }
        }
    }

    // 8) Rewrite all connections.
    for conn in &mut scene.connections {
        if let Some(new_id) = rewrite_map.get(&conn.from.node_id) {
            conn.from.node_id = new_id.clone();
        }
        if let Some(new_id) = rewrite_map.get(&conn.to.node_id) {
            conn.to.node_id = new_id.clone();
        }
    }

    // 9) Remove duplicate nodes.
    scene.nodes.retain(|n| !nodes_to_remove.contains(&n.id));

    // 10) Deduplicate connections (after rewriting, multiple connections may
    // have the same from/to endpoints).
    let mut seen_conns: HashSet<(String, String, String, String)> = HashSet::new();
    scene.connections.retain(|c| {
        // Also remove connections that reference removed nodes
        if nodes_to_remove.contains(&c.from.node_id) || nodes_to_remove.contains(&c.to.node_id) {
            return false;
        }
        let key = (
            c.from.node_id.clone(),
            c.from.port_id.clone(),
            c.to.node_id.clone(),
            c.to.port_id.clone(),
        );
        seen_conns.insert(key)
    });

    // 11) Clean up __dedup_* metadata from all remaining nodes.
    for node in &mut scene.nodes {
        node.params.remove("__dedup_group_id");
        node.params.remove("__dedup_original_id");
    }

    DedupReport {
        deduped_passes,
        removed_nodes: nodes_to_remove.len(),
    }
}

#[derive(Debug, Default)]
pub(crate) struct DedupReport {
    pub deduped_passes: usize,
    pub removed_nodes: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{Connection, Endpoint, Metadata, Node, NodePort};

    fn make_node(id: &str, node_type: &str, params: HashMap<String, serde_json::Value>) -> Node {
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params,
            inputs: vec![
                NodePort {
                    id: "material".to_string(),
                    name: None,
                    port_type: None,
                },
                NodePort {
                    id: "source".to_string(),
                    name: None,
                    port_type: None,
                },
                NodePort {
                    id: "geometry".to_string(),
                    name: None,
                    port_type: None,
                },
            ],
            outputs: vec![NodePort {
                id: "output".to_string(),
                name: None,
                port_type: None,
            }],
            input_bindings: Vec::new(),
        }
    }

    fn make_conn(
        id: &str,
        from_node: &str,
        from_port: &str,
        to_node: &str,
        to_port: &str,
    ) -> Connection {
        Connection {
            id: id.to_string(),
            from: Endpoint {
                node_id: from_node.to_string(),
                port_id: from_port.to_string(),
            },
            to: Endpoint {
                node_id: to_node.to_string(),
                port_id: to_port.to_string(),
            },
        }
    }

    #[test]
    fn test_identical_passes_are_deduped() {
        let mut params = HashMap::new();
        params.insert("__dedup_group_id".to_string(), serde_json::json!("myGroup"));
        params.insert("__dedup_original_id".to_string(), serde_json::json!("RP_1"));
        params.insert("blend".to_string(), serde_json::json!("add"));

        let mut params2 = params.clone();
        params2.insert("__dedup_original_id".to_string(), serde_json::json!("RP_1"));

        let mut scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                make_node("inst_a/RP_1", "RenderPass", params.clone()),
                make_node("inst_b/RP_1", "RenderPass", params2),
                make_node("downstream", "MathClosure", HashMap::new()),
            ],
            connections: vec![
                make_conn("c1", "inst_a/RP_1", "output", "downstream", "in_0"),
                make_conn("c2", "inst_b/RP_1", "output", "downstream", "in_1"),
            ],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
        };

        let report = dedup_identical_passes(&mut scene);
        assert!(
            report.deduped_passes >= 1,
            "expected at least 1 deduped pass, got {}",
            report.deduped_passes
        );

        // Should have canonical + downstream = 2 nodes
        let pass_nodes: Vec<_> = scene
            .nodes
            .iter()
            .filter(|n| n.node_type == "RenderPass")
            .collect();
        assert_eq!(
            pass_nodes.len(),
            1,
            "expected 1 RenderPass after dedup, got {}",
            pass_nodes.len()
        );
        assert!(
            pass_nodes[0].id.starts_with("sys.group."),
            "canonical should have sys.group. prefix: {}",
            pass_nodes[0].id
        );

        // Both connections should now point to the canonical pass
        for c in &scene.connections {
            if c.to.node_id == "downstream" {
                assert_eq!(c.from.node_id, pass_nodes[0].id);
            }
        }
    }

    #[test]
    fn test_different_params_not_deduped() {
        let mut params_a = HashMap::new();
        params_a.insert("__dedup_group_id".to_string(), serde_json::json!("g"));
        params_a.insert("__dedup_original_id".to_string(), serde_json::json!("RP"));
        params_a.insert("blend".to_string(), serde_json::json!("add"));

        let mut params_b = HashMap::new();
        params_b.insert("__dedup_group_id".to_string(), serde_json::json!("g"));
        params_b.insert("__dedup_original_id".to_string(), serde_json::json!("RP"));
        params_b.insert("blend".to_string(), serde_json::json!("multiply"));

        let mut scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                make_node("a/RP", "RenderPass", params_a),
                make_node("b/RP", "RenderPass", params_b),
            ],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
        };

        let report = dedup_identical_passes(&mut scene);
        assert_eq!(
            report.deduped_passes, 0,
            "different params should not be deduped"
        );
        assert_eq!(scene.nodes.len(), 2);
    }

    #[test]
    fn test_non_group_pass_keeps_canonical_id() {
        let mut scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                make_node(
                    "sys.auto.fullscreen.pass.edge_1",
                    "RenderPass",
                    HashMap::new(),
                ),
                make_node(
                    "sys.auto.fullscreen.pass.edge_2",
                    "RenderPass",
                    HashMap::new(),
                ),
                make_node("downstream", "MathClosure", HashMap::new()),
            ],
            connections: vec![
                make_conn(
                    "c1",
                    "sys.auto.fullscreen.pass.edge_1",
                    "output",
                    "downstream",
                    "in_0",
                ),
                make_conn(
                    "c2",
                    "sys.auto.fullscreen.pass.edge_2",
                    "output",
                    "downstream",
                    "in_1",
                ),
            ],
            outputs: None,
            groups: Vec::new(),
            assets: Default::default(),
        };

        let report = dedup_identical_passes(&mut scene);
        assert_eq!(report.deduped_passes, 1);

        let pass_nodes: Vec<_> = scene
            .nodes
            .iter()
            .filter(|n| n.node_type == "RenderPass")
            .collect();
        assert_eq!(pass_nodes.len(), 1);
        assert!(
            pass_nodes[0].id.starts_with("sys.auto.fullscreen.pass."),
            "canonical non-group pass should keep sys.auto.* id, got {}",
            pass_nodes[0].id
        );
        assert!(
            !pass_nodes[0].id.starts_with("sys.group.unknown/"),
            "non-group pass should not be renamed to sys.group.unknown/*"
        );
    }
}
