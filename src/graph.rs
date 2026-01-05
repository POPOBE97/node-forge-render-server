use std::collections::{HashMap, HashSet, VecDeque};

use anyhow::{bail, Result};

use crate::dsl::SceneDSL;

pub fn topo_sort(scene: &SceneDSL) -> Result<Vec<String>> {
    let mut indeg: HashMap<&str, usize> = scene
        .nodes
        .iter()
        .map(|n| (n.id.as_str(), 0usize))
        .collect();

    let mut outgoing: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in &scene.connections {
        if !indeg.contains_key(c.from.node_id.as_str()) || !indeg.contains_key(c.to.node_id.as_str()) {
            bail!(
                "connection references missing node: {} -> {}",
                c.from.node_id,
                c.to.node_id
            );
        }
        *indeg.get_mut(c.to.node_id.as_str()).unwrap() += 1;
        outgoing
            .entry(c.from.node_id.as_str())
            .or_default()
            .push(c.to.node_id.as_str());
    }

    let mut q: VecDeque<&str> = indeg
        .iter()
        .filter_map(|(id, d)| if *d == 0 { Some(*id) } else { None })
        .collect();
    let mut order: Vec<String> = Vec::with_capacity(scene.nodes.len());

    while let Some(n) = q.pop_front() {
        order.push(n.to_string());
        if let Some(nexts) = outgoing.get(n) {
            for m in nexts {
                let entry = indeg.get_mut(m).unwrap();
                *entry -= 1;
                if *entry == 0 {
                    q.push_back(m);
                }
            }
        }
    }

    if order.len() != scene.nodes.len() {
        bail!("cycle detected in graph (cannot topologically sort)");
    }
    Ok(order)
}

pub fn upstream_reachable(scene: &SceneDSL, start: &str) -> HashSet<String> {
    let mut incoming: HashMap<&str, Vec<&str>> = HashMap::new();
    for c in &scene.connections {
        incoming
            .entry(c.to.node_id.as_str())
            .or_default()
            .push(c.from.node_id.as_str());
    }

    let mut visited: HashSet<String> = HashSet::new();
    let mut stack: Vec<&str> = vec![start];
    while let Some(n) = stack.pop() {
        if !visited.insert(n.to_string()) {
            continue;
        }
        if let Some(prevs) = incoming.get(n) {
            for p in prevs {
                stack.push(p);
            }
        }
    }
    visited
}
