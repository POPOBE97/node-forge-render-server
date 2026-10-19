//! A view of the executed GPU passes, independent of the authored node graph.
//!
//! Resolve reads against the last writer *at that point in execution*. In particular,
//! a texture reused later in the frame must not create a backwards edge or a cycle.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rust_wgpu_fiber::{eframe::wgpu, shader_space::ShaderSpace};

use super::resource_tree::{PassInfo, ResourceSnapshot};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Resource {
    name: String,
    buffer: bool,
}

impl Resource {
    fn texture(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            buffer: false,
        }
    }

    fn buffer(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            buffer: true,
        }
    }

    fn label(&self) -> String {
        if self.buffer {
            format!("{} (buffer)", self.name)
        } else {
            self.name.clone()
        }
    }
}

struct Invocation {
    pass: PassInfo,
    reads: BTreeSet<Resource>,
    writes: BTreeSet<Resource>,
}

#[derive(Debug)]
pub(super) struct GraphNode {
    pub pass: PassInfo,
    pub external_inputs: Vec<String>,
    pub is_output: bool,
}

#[derive(Debug)]
pub(super) struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub resources: BTreeSet<String>,
    /// Only an execution constraint (write-after-write/read), not sampled data.
    pub ordering: bool,
}

#[derive(Default, Debug)]
pub(super) struct PassGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub rows: Vec<Vec<usize>>,
}

/// Match ShaderSpace::render, including repeated invocations and disabled routes.
/// Composition is a strict execution chain in this renderer, so it provides order,
/// not data edges: otherwise every independent branch would appear serialized.
pub(super) fn execution_names(space: &ShaderSpace) -> Vec<String> {
    space
        .composition
        .flatten()
        .into_iter()
        .rev()
        .filter(|dep| {
            space
                .passes
                .inner
                .get(&dep.pass_name)
                .is_some_and(|pass| pass.enabled)
        })
        .map(|dep| dep.pass_name.as_str().to_owned())
        .collect()
}

impl PassGraph {
    pub fn capture(snapshot: &ResourceSnapshot, space: &ShaderSpace) -> Self {
        let metadata: HashMap<_, _> = snapshot
            .passes
            .iter()
            .map(|pass| (pass.name.as_str(), pass))
            .collect();
        let mut invocations = Vec::new();
        for dep in space.composition.flatten().into_iter().rev() {
            let Some(pass) = space
                .passes
                .inner
                .get(&dep.pass_name)
                .filter(|pass| pass.enabled)
            else {
                continue;
            };
            let Some(info) = metadata.get(dep.pass_name.as_str()) else {
                continue;
            };
            let mut reads = BTreeSet::new();
            let mut writes = BTreeSet::new();
            for (entries, _, _) in pass.bindings.values() {
                for (name, entry) in entries.values() {
                    match entry.ty {
                        wgpu::BindingType::Texture { .. } => {
                            reads.insert(Resource::texture(name.as_str()));
                        }
                        wgpu::BindingType::StorageTexture { access, .. } => {
                            let resource = Resource::texture(name.as_str());
                            if access != wgpu::StorageTextureAccess::WriteOnly {
                                reads.insert(resource.clone());
                            }
                            if access != wgpu::StorageTextureAccess::ReadOnly {
                                writes.insert(resource);
                            }
                        }
                        wgpu::BindingType::Buffer { ty, .. } => {
                            let resource = Resource::buffer(name.as_str());
                            reads.insert(resource.clone());
                            if matches!(ty, wgpu::BufferBindingType::Storage { read_only: false }) {
                                writes.insert(resource);
                            }
                        }
                        _ => {}
                    }
                }
            }
            for (name, _, _) in pass.attribute_bindings.values() {
                reads.insert(Resource::buffer(name.as_str()));
            }
            if let Some((name, _)) = &pass.index_binding {
                reads.insert(Resource::buffer(name.as_str()));
            }
            if let Some(name) = &pass.color_attachment {
                let resource = Resource::texture(name.as_str());
                if matches!(pass.load_op, wgpu::LoadOp::Load) {
                    reads.insert(resource.clone());
                }
                writes.insert(resource);
            }
            if let Some(name) = &pass.resolve_target {
                writes.insert(Resource::texture(name.as_str()));
            }
            if let Some(name) = &pass.depth_stencil_attachment {
                writes.insert(Resource::texture(name.as_str()));
            }
            invocations.push(Invocation {
                pass: (*info).clone(),
                reads,
                writes,
            });
        }
        Self::build(invocations, snapshot.final_output_texture.as_deref())
    }

    fn build(invocations: Vec<Invocation>, output: Option<&str>) -> Self {
        let mut graph = Self::default();
        let mut writers = BTreeMap::<Resource, usize>::new();
        let mut readers = BTreeMap::<Resource, BTreeSet<usize>>::new();
        let mut edges = BTreeMap::<(usize, usize), GraphEdge>::new();
        for (index, mut invocation) in invocations.into_iter().enumerate() {
            let mut link = |from, resource: String, ordering| {
                if from == index {
                    return;
                }
                let edge = edges.entry((from, index)).or_insert_with(|| GraphEdge {
                    from,
                    to: index,
                    resources: BTreeSet::new(),
                    ordering,
                });
                edge.resources.insert(resource);
                edge.ordering &= ordering;
            };
            let mut external_inputs = Vec::new();
            for resource in &invocation.reads {
                if let Some(&writer) = writers.get(resource) {
                    link(writer, resource.label(), false);
                } else {
                    external_inputs.push(resource.label());
                }
                readers.entry(resource.clone()).or_default().insert(index);
            }
            for resource in &invocation.writes {
                if let Some(writer) = writers.insert(resource.clone(), index) {
                    link(writer, resource.label(), true);
                }
                // Reusing a target must wait for consumers of its preceding contents.
                for reader in readers.remove(resource).unwrap_or_default() {
                    link(reader, resource.label(), true);
                }
            }
            invocation.pass.order_index = index;
            graph.nodes.push(GraphNode {
                pass: invocation.pass,
                external_inputs,
                is_output: false,
            });
        }
        // Only the final producer represents the presented output.
        if let Some(output) = output.and_then(|name| writers.get(&Resource::texture(name))) {
            graph.nodes[*output].is_output = true;
        }
        graph.edges = edges.into_values().collect();
        graph.rows = graph.layout_rows();
        graph
    }

    fn layout_rows(&self) -> Vec<Vec<usize>> {
        let mut levels = vec![0; self.nodes.len()];
        let mut parents = vec![Vec::new(); self.nodes.len()];
        for edge in &self.edges {
            parents[edge.to].push(edge.from);
        }
        for (index, incoming) in parents.iter().enumerate() {
            levels[index] = incoming
                .iter()
                .map(|&parent| levels[parent] + 1)
                .max()
                .unwrap_or(0);
        }
        let mut rows = vec![Vec::new(); levels.iter().max().map_or(0, |level| level + 1)];
        for (index, &level) in levels.iter().enumerate() {
            rows[level].push(index);
        }
        let mut positions = vec![0.0; self.nodes.len()];
        for row in &mut rows {
            // Keep branches near their parents; stable ties retain execution order.
            row.sort_by(|&a, &b| {
                let center = |index: usize| {
                    let incoming = &parents[index];
                    if incoming.is_empty() {
                        index as f32
                    } else {
                        incoming
                            .iter()
                            .map(|&parent| positions[parent])
                            .sum::<f32>()
                            / incoming.len() as f32
                    }
                };
                center(a).total_cmp(&center(b))
            });
            for (column, &index) in row.iter().enumerate() {
                positions[index] = column as f32 - (row.len() as f32 - 1.0) / 2.0;
            }
        }
        rows
    }

    pub fn connected_to(&self, selected: usize) -> BTreeSet<usize> {
        let mut upstream = BTreeSet::from([selected]);
        let mut downstream = upstream.clone();
        for index in (0..=selected).rev() {
            if upstream.contains(&index) {
                upstream.extend(
                    self.edges
                        .iter()
                        .filter(|edge| edge.to == index)
                        .map(|edge| edge.from),
                );
            }
        }
        for index in selected..self.nodes.len() {
            if downstream.contains(&index) {
                downstream.extend(
                    self.edges
                        .iter()
                        .filter(|edge| edge.from == index)
                        .map(|edge| edge.to),
                );
            }
        }
        upstream.extend(downstream);
        upstream
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(name: &str, reads: &[&str], target: &str) -> Invocation {
        Invocation {
            pass: PassInfo {
                name: name.into(),
                display_label: None,
                source_node_id: None,
                source_node_type: None,
                order_index: 0,
                target_texture: Some(target.into()),
                target_size: None,
                target_format: None,
                is_compute: false,
                sampled_textures: reads.iter().map(|s| s.to_string()).collect(),
                instance_count: 1,
                vertex_count: 3,
                workgroup_count: 0,
            },
            reads: reads.iter().map(|s| Resource::texture(*s)).collect(),
            writes: BTreeSet::from([Resource::texture(target)]),
        }
    }

    #[test]
    fn reused_target_reads_preceding_writer_and_orders_overwrite_after_consumers() {
        let graph = PassGraph::build(
            vec![
                pass("first", &["image"], "shared"),
                pass("consumer", &["shared"], "result"),
                pass("overwrite", &[], "shared"),
                pass("present", &["shared", "result"], "screen"),
            ],
            Some("screen"),
        );
        let edges: Vec<_> = graph
            .edges
            .iter()
            .map(|e| (e.from, e.to, e.ordering))
            .collect();
        assert_eq!(
            edges,
            vec![
                (0, 1, false),
                (0, 2, true),
                (1, 2, true),
                (1, 3, false),
                (2, 3, false)
            ]
        );
        assert_eq!(graph.nodes[0].external_inputs, ["image"]);
        assert!(graph.nodes[3].is_output);
        assert_eq!(graph.rows, vec![vec![0], vec![1], vec![2], vec![3]]);
    }

    #[test]
    fn branches_share_one_producer_and_join_without_duplicating_nodes() {
        let graph = PassGraph::build(
            vec![
                pass("source", &[], "base"),
                pass("left", &["base"], "left"),
                pass("right", &["base"], "right"),
                pass("join", &["left", "right"], "out"),
                pass("independent", &[], "other"),
            ],
            Some("out"),
        );
        assert_eq!(graph.nodes.len(), 5);
        assert_eq!(graph.rows, vec![vec![0, 4], vec![1, 2], vec![3]]);
        assert_eq!(graph.connected_to(1), BTreeSet::from([0, 1, 3]));
    }

    #[test]
    fn buffer_dependencies_and_repeated_pass_invocations_remain_distinct() {
        let mut compute = pass("compute", &[], "unused");
        compute.writes = BTreeSet::from([Resource::buffer("vertices")]);
        let mut draw = pass("draw", &["vertices"], "screen");
        draw.reads.insert(Resource::buffer("vertices"));
        let graph = PassGraph::build(
            vec![compute, draw, pass("draw", &["screen"], "screen")],
            Some("screen"),
        );
        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(graph.nodes[1].external_inputs, ["vertices"]); // Texture != buffer.
        assert!(graph.edges[0].resources.contains("vertices (buffer)"));
        assert!(graph.edges.iter().all(|edge| edge.from < edge.to));
        assert!(!graph.nodes[1].is_output);
        assert!(graph.nodes[2].is_output);
    }

    #[test]
    fn future_writer_is_not_a_dependency_and_empty_graph_is_supported() {
        let graph = PassGraph::build(
            vec![
                pass("read", &["history"], "screen"),
                pass("write", &[], "history"),
            ],
            None,
        );
        assert_eq!(graph.nodes[0].external_inputs, ["history"]);
        assert!(graph.edges.iter().all(|edge| edge.ordering));
        assert!(PassGraph::build(vec![], None).rows.is_empty());
    }

    #[test]
    fn capture_filters_disabled_passes_and_uses_resolved_texture_dependencies() {
        use rust_wgpu_fiber::{
            HeadlessRenderer, HeadlessRendererConfig, ResourceName,
            composition::CompositionBuilder, pass::Pass,
        };

        let gpu = HeadlessRenderer::new(HeadlessRendererConfig {
            backends: wgpu::Backends::NOOP,
            ..Default::default()
        })
        .expect("CPU-only test adapter");
        let mut space = gpu.shader_space;
        let source = Pass {
            color_attachment: Some(ResourceName::from("multisampled")),
            resolve_target: Some(ResourceName::from("resolved")),
            ..Default::default()
        };
        let mut consumer = Pass {
            color_attachment: Some(ResourceName::from("screen")),
            ..Default::default()
        };
        consumer.bindings.insert(
            0,
            (
                BTreeMap::from([(
                    0,
                    (
                        ResourceName::from("resolved"),
                        wgpu::BindGroupLayoutEntry {
                            binding: 0,
                            visibility: wgpu::ShaderStages::FRAGMENT,
                            ty: wgpu::BindingType::Texture {
                                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                                view_dimension: wgpu::TextureViewDimension::D2,
                                multisampled: false,
                            },
                            count: None,
                        },
                    ),
                )]),
                None,
                None,
            ),
        );
        space
            .passes
            .inner
            .insert(ResourceName::from("source"), source);
        space.passes.inner.insert(
            ResourceName::from("off"),
            Pass {
                enabled: false,
                ..Default::default()
            },
        );
        space
            .passes
            .inner
            .insert(ResourceName::from("draw"), consumer);
        space.composition = CompositionBuilder::new()
            .pass("source")
            .pass("off")
            .pass("draw")
            .pass("draw")
            .commit();
        let snapshot = ResourceSnapshot {
            passes: vec![
                pass("source", &[], "resolved").pass,
                pass("off", &[], "screen").pass,
                pass("draw", &["resolved"], "screen").pass,
            ],
            final_output_texture: Some("screen".into()),
            ..Default::default()
        };
        assert_eq!(execution_names(&space), ["source", "draw", "draw"]);
        let graph = PassGraph::capture(&snapshot, &space);
        assert_eq!(graph.nodes.len(), 3);
        assert_eq!(
            graph
                .edges
                .iter()
                .map(|e| (e.from, e.to, e.ordering))
                .collect::<Vec<_>>(),
            [(0, 1, false), (0, 2, false), (1, 2, true)]
        );
        assert!(graph.edges[0].resources.contains("resolved"));
        assert!(graph.nodes[2].is_output);
        space
            .passes
            .inner
            .get_mut(&ResourceName::from("draw"))
            .unwrap()
            .enabled = false;
        assert_eq!(execution_names(&space), ["source"]);
    }
}
