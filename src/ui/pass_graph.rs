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

#[derive(Clone, Debug)]
pub(super) struct GraphNode {
    pub pass: PassInfo,
    pub external_inputs: Vec<String>,
    pub is_output: bool,
    pub blur_group: Option<String>,
    /// Empty for individual passes; populated only in the collapsed presentation.
    pub collapsed_passes: Vec<PassInfo>,
}

impl GraphNode {
    pub fn passes(&self) -> impl Iterator<Item = &PassInfo> {
        self.collapsed_passes
            .iter()
            .chain(self.collapsed_passes.is_empty().then_some(&self.pass))
    }

    pub fn blur_label(&self) -> &'static str {
        if self.passes().any(|pass| pass.name.starts_with("sys.cb.")) {
            "Chromatic blur"
        } else if self.pass.name.starts_with("sys.gb.") {
            "Gradient blur"
        } else {
            "Gaussian blur"
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct GraphEdge {
    pub from: usize,
    pub to: usize,
    pub resources: BTreeSet<String>,
    /// Only an execution constraint (write-after-write/read), not sampled data.
    pub ordering: bool,
}

#[derive(Clone, Default, Debug)]
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
                blur_group: None,
                collapsed_passes: Vec::new(),
            });
        }
        // Only the final producer represents the presented output.
        if let Some(output) = output.and_then(|name| writers.get(&Resource::texture(name))) {
            graph.nodes[*output].is_output = true;
        }
        graph.edges = edges.into_values().collect();
        graph.assign_blur_groups();
        graph.rows = graph.layout_rows();
        graph
    }

    fn assign_blur_groups(&mut self) {
        // Only contract consecutive invocations. Contracting across an unrelated
        // consumer can turn A -> consumer -> B into a false group-level cycle.
        let mut occurrences = HashMap::<String, usize>::new();
        let mut start = 0;
        while start < self.nodes.len() {
            let Some(owner) = blur_owner(&self.nodes[start].pass.name) else {
                start += 1;
                continue;
            };
            let owner = owner.to_owned();
            let end = (start + 1..self.nodes.len())
                .find(|&i| blur_owner(&self.nodes[i].pass.name) != Some(owner.as_str()))
                .unwrap_or(self.nodes.len());
            let occurrence = occurrences.entry(owner.clone()).or_default();
            let key = format!("{owner}#{occurrence}");
            *occurrence += 1;
            if end - start > 1 {
                for node in &mut self.nodes[start..end] {
                    node.blur_group = Some(key.clone());
                }
            }
            start = end;
        }
    }

    /// Project the exact execution graph for display; the original stays intact
    /// for dependency inspection. Parallel edges are merged, never invented.
    pub fn presentation(&self, expanded: &BTreeSet<String>, show_ordering: bool) -> Self {
        let mut view = Self::default();
        let mut mapping = vec![0; self.nodes.len()];
        let mut index = 0;
        while index < self.nodes.len() {
            let node = &self.nodes[index];
            let end = if let Some(key) = node
                .blur_group
                .as_ref()
                .filter(|key| !expanded.contains(*key))
            {
                (index + 1..self.nodes.len())
                    .find(|&i| self.nodes[i].blur_group.as_ref() != Some(key))
                    .unwrap_or(self.nodes.len())
            } else {
                index + 1
            };
            mapping[index..end].fill(view.nodes.len());
            let mut visible = self.nodes[end - 1].clone();
            if end - index > 1 {
                visible.collapsed_passes = self.nodes[index..end]
                    .iter()
                    .map(|node| node.pass.clone())
                    .collect();
                visible.pass.display_label = Some(
                    blur_owner(&node.pass.name)
                        .unwrap_or(&node.pass.name)
                        .to_owned(),
                );
                visible.external_inputs = self.nodes[index..end]
                    .iter()
                    .flat_map(|node| node.external_inputs.iter().cloned())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
                visible.is_output = self.nodes[index..end].iter().any(|node| node.is_output);
            }
            view.nodes.push(visible);
            index = end;
        }
        let mut edges = BTreeMap::<(usize, usize), GraphEdge>::new();
        for edge in &self.edges {
            if edge.ordering && !show_ordering {
                continue;
            }
            let (from, to) = (mapping[edge.from], mapping[edge.to]);
            if from == to {
                continue;
            }
            let visible = edges.entry((from, to)).or_insert_with(|| GraphEdge {
                from,
                to,
                resources: BTreeSet::new(),
                ordering: true,
            });
            visible.resources.extend(edge.resources.iter().cloned());
            visible.ordering &= edge.ordering;
        }
        view.edges = edges.into_values().collect();
        view.rows = view.layout_rows();
        view
    }

    fn layout_rows(&self) -> Vec<Vec<usize>> {
        let mut levels = vec![0; self.nodes.len()];
        let mut parents = vec![Vec::new(); self.nodes.len()];
        let mut children = vec![Vec::new(); self.nodes.len()];
        for edge in &self.edges {
            parents[edge.to].push(edge.from);
            children[edge.from].push(edge.to);
        }
        for (index, incoming) in parents.iter().enumerate() {
            levels[index] = incoming
                .iter()
                .map(|&parent| levels[parent] + 1)
                .max()
                .unwrap_or(0);
        }
        // Place shorter branches near their first consumer. In particular, an
        // image used only by the last overlay need not send a wire from row zero
        // past the entire graph. Execution numbers remain unchanged.
        for index in (0..self.nodes.len()).rev() {
            if let Some(level) = children[index].iter().map(|&child| levels[child] - 1).min() {
                levels[index] = level;
            }
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

/// The assembler's internal stage names, parsed from the right because authored
/// node IDs may contain dots. Composite draws are deliberately kept separate.
fn blur_owner(name: &str) -> Option<&str> {
    // ChromaticBlur runs its color stages and then the gradient-blur pyramid
    // under the same authored owner. Treat that whole consecutive run as one
    // expandable effect, including all mip inputs to its final resolve.
    if let Some(name) = name.strip_prefix("sys.cb.") {
        let (owner, stage) = name.strip_suffix(".pass")?.rsplit_once('.')?;
        return matches!(stage, "radial" | "rgb").then_some(owner);
    }
    if let Some(name) = name.strip_prefix("sys.gb.") {
        let (owner, stage) = name.strip_suffix(".pass")?.rsplit_once('.')?;
        return (matches!(stage, "pad" | "final")
            || stage
                .strip_prefix("mip")
                .is_some_and(|level| level.parse::<u32>().is_ok()))
        .then_some(owner);
    }
    let name = name.strip_prefix("sys.blur.")?;
    let name = if let Some((base, route)) = name.rsplit_once(".route") {
        if route.parse::<u32>().is_ok() {
            base
        } else {
            name
        }
    } else {
        name
    };
    let name = name.strip_suffix(".pass")?;
    if let Some(owner) = name.strip_suffix(".src") {
        return Some(owner);
    }
    for stage in [".h.ds", ".v.ds", ".upsample_bilinear.ds", ".ds."] {
        if let Some((owner, factor)) = name.rsplit_once(stage)
            && factor.parse::<u32>().is_ok()
        {
            return Some(owner);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doubao_blur_groups_match_active_render_plan() {
        assert_blur_groups_match_active_render_plan("doubao-voice-interaction");
    }

    #[test]
    fn chromatic_blur_groups_match_active_render_plan() {
        let graph = assert_blur_groups_match_active_render_plan("blur-chromatic");
        assert!(graph.nodes.iter().any(|node| {
            node.blur_label() == "Chromatic blur" && node.collapsed_passes.len() == 10
        }));
    }

    fn assert_blur_groups_match_active_render_plan(case: &str) -> PassGraph {
        use crate::renderer::{
            ShaderSpacePresentationMode,
            render_plan::{
                planner::RenderPlanner,
                types::{PlanBuildOptions, PlanningGpuCaps},
            },
        };
        let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "tests/fixtures/render/editor-examples/{case}/scene.nforge"
        ));
        let (scene, assets) = crate::asset_store::load_from_nforge(&archive).unwrap();
        let plan = RenderPlanner::new(PlanBuildOptions {
            gpu_caps: PlanningGpuCaps::default(),
            presentation_mode: ShaderSpacePresentationMode::UiHdrNative,
            debug_dump_wgsl_dir: None,
        })
        .plan(&scene, Some(&assets), None)
        .unwrap();
        let resources = &plan.resources;
        let mut invocations = Vec::new();
        for spec in &resources.image_prepasses {
            invocations.push(pass(
                spec.pass_name.as_str(),
                &[spec.src_texture.as_str()],
                spec.dst_texture.as_str(),
            ));
        }
        let disabled: BTreeSet<_> = resources
            .gaussian_blur_bundles
            .values()
            .flat_map(|bundle| {
                bundle
                    .current
                    .route_passes
                    .iter()
                    .filter(|(factor, _)| **factor != bundle.current.active_factor)
                    .flat_map(|(_, passes)| passes.iter().map(|name| name.as_str()))
            })
            .collect();
        for name in &resources.composite_passes {
            if disabled.contains(name.as_str()) {
                continue;
            }
            let spec = resources
                .render_pass_specs
                .iter()
                .find(|spec| spec.name == *name)
                .unwrap();
            let reads: Vec<_> = spec
                .texture_bindings
                .iter()
                .map(|binding| binding.texture.as_str())
                .collect();
            let mut invocation = pass(name.as_str(), &reads, spec.target_texture.as_str());
            if matches!(spec.color_load_op, wgpu::LoadOp::Load) {
                invocation
                    .reads
                    .insert(Resource::texture(spec.target_texture.as_str()));
            }
            if let Some(resolve) = &spec.resolve_target {
                invocation
                    .writes
                    .insert(Resource::texture(resolve.as_str()));
            }
            invocation.pass.target_size = resources
                .textures
                .iter()
                .find(|tex| tex.name == spec.target_texture)
                .map(|tex| (tex.size[0], tex.size[1]));
            invocations.push(invocation);
        }
        let graph = PassGraph::build(invocations, Some(plan.present_output_texture.as_str()));
        let collapsed = graph.presentation(&BTreeSet::new(), false);
        assert!(collapsed.nodes.len() < graph.nodes.len());
        assert!(collapsed.edges.iter().all(|edge| edge.from < edge.to));
        assert_eq!(
            collapsed
                .nodes
                .iter()
                .map(|node| node.collapsed_passes.len().max(1))
                .sum::<usize>(),
            graph.nodes.len()
        );
        for bundle in resources.gaussian_blur_bundles.values() {
            let active = &bundle.current.route_passes[&bundle.current.active_factor];
            let active: BTreeSet<_> = active.iter().map(|name| name.as_str()).collect();
            assert!(
                collapsed.nodes.iter().any(|node| {
                    node.collapsed_passes
                        .iter()
                        .map(|pass| pass.name.as_str())
                        .collect::<BTreeSet<_>>()
                        == active
                }),
                "the active blur route must form one expandable group"
            );
        }
        collapsed
    }

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
    fn collapsed_blur_preserves_boundary_dependencies_and_expands_losslessly() {
        let graph = PassGraph::build(
            vec![
                pass("source", &[], "image"),
                pass("mask", &[], "mask"),
                pass(
                    "sys.blur.group/blur.with.dots.ds.4.pass.route4",
                    &["image"],
                    "small",
                ),
                pass(
                    "sys.blur.group/blur.with.dots.h.ds4.pass.route4",
                    &["small"],
                    "h",
                ),
                pass(
                    "sys.blur.group/blur.with.dots.v.ds4.pass.route4",
                    &["h", "mask"],
                    "v",
                ),
                pass(
                    "sys.blur.group/blur.with.dots.upsample_bilinear.ds4.pass.route4",
                    &["v"],
                    "out",
                ),
                pass("consumer", &["h", "out"], "screen"),
            ],
            Some("screen"),
        );
        let collapsed = graph.presentation(&BTreeSet::new(), true);
        assert_eq!(collapsed.nodes.len(), 4);
        assert_eq!(collapsed.nodes[2].collapsed_passes.len(), 4);
        assert!(collapsed.nodes[3].is_output);
        assert_eq!(
            collapsed
                .edges
                .iter()
                .map(|e| (e.from, e.to))
                .collect::<Vec<_>>(),
            [(0, 2), (1, 2), (2, 3)]
        );
        assert_eq!(
            collapsed.edges[2].resources,
            BTreeSet::from(["h".into(), "out".into()])
        );
        let expanded = graph.presentation(
            &BTreeSet::from([collapsed.nodes[2].blur_group.clone().unwrap()]),
            true,
        );
        assert_eq!(expanded.nodes.len(), graph.nodes.len());
        assert_eq!(
            expanded
                .edges
                .iter()
                .map(|e| (&e.resources, e.from, e.to, e.ordering))
                .collect::<Vec<_>>(),
            graph
                .edges
                .iter()
                .map(|e| (&e.resources, e.from, e.to, e.ordering))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn chromatic_blur_collapses_color_stages_and_mips_as_one_effect() {
        let owner = "group/ChromaticBlur.with.dots";
        let mut invocations = vec![pass("source", &[], "image"), pass("mask", &[], "mask")];
        let stages = [
            "cb.radial",
            "cb.rgb",
            "gb.pad",
            "gb.mip1",
            "gb.mip2",
            "gb.mip3",
            "gb.mip4",
            "gb.mip5",
            "gb.mip6",
            "gb.final",
        ];
        let mut textures = vec!["image".to_owned()];
        for stage in stages {
            let (prefix, stage) = stage.split_once('.').unwrap();
            let target = format!("sys.{prefix}.{owner}.{stage}");
            let reads = if stage == "final" {
                textures
                    .iter()
                    .skip(2)
                    .map(String::as_str)
                    .chain(["mask"])
                    .collect::<Vec<_>>()
            } else {
                vec![textures.last().unwrap().as_str()]
            };
            invocations.push(pass(&format!("{target}.pass"), &reads, &target));
            textures.push(target);
        }
        invocations.push(pass("present", &[textures.last().unwrap()], "screen"));
        let graph = PassGraph::build(invocations, Some("screen"));
        let collapsed = graph.presentation(&BTreeSet::new(), false);
        assert_eq!(graph.nodes.len(), 13);
        assert_eq!(collapsed.nodes.len(), 4);
        let blur = &collapsed.nodes[2];
        assert_eq!(blur.blur_label(), "Chromatic blur");
        assert_eq!(blur.pass.display_label.as_deref(), Some(owner));
        assert_eq!(blur.collapsed_passes.len(), 10);
        assert_eq!(
            collapsed
                .edges
                .iter()
                .map(|edge| (edge.from, edge.to))
                .collect::<Vec<_>>(),
            [(0, 2), (1, 2), (2, 3)]
        );
        let expanded =
            graph.presentation(&BTreeSet::from([blur.blur_group.clone().unwrap()]), true);
        assert_eq!(expanded.nodes.len(), graph.nodes.len());
        assert_eq!(
            expanded
                .edges
                .iter()
                .map(|e| (e.from, e.to, &e.resources, e.ordering))
                .collect::<Vec<_>>(),
            graph
                .edges
                .iter()
                .map(|e| (e.from, e.to, &e.resources, e.ordering))
                .collect::<Vec<_>>()
        );
        assert!(expanded.nodes.last().unwrap().is_output);
        assert_eq!(blur_owner("sys.gb.group/effect.to.root.compose.pass"), None);
        assert_eq!(blur_owner("sys.gb.group/effect.mipmap.pass"), None);
    }

    fn assert_layout_clear(graph: &PassGraph, columns: usize) {
        use super::super::pass_graph_layout::{GraphLayout, LayoutMode};
        let layout = GraphLayout::new(graph, LayoutMode::Compact, columns);
        for (index, rect) in layout.rects.iter().enumerate() {
            assert!(rect.is_finite());
            assert!(
                layout.rects[index + 1..]
                    .iter()
                    .all(|other| !rect.intersects(*other))
            );
        }
        assert_eq!(layout.paths.len(), graph.edges.len());
        for path in &layout.paths {
            for segment in path.windows(2) {
                let [a, b] = [segment[0], segment[1]];
                assert!(a.is_finite() && b.is_finite() && a != b);
                assert!(a.x == b.x || a.y == b.y, "edges must remain orthogonal");
                for rect in &layout.rects {
                    let rect = rect.shrink(0.5);
                    let crosses = if a.x == b.x {
                        a.x > rect.left()
                            && a.x < rect.right()
                            && a.y.min(b.y) < rect.bottom()
                            && a.y.max(b.y) > rect.top()
                    } else {
                        a.y > rect.top()
                            && a.y < rect.bottom()
                            && a.x.min(b.x) < rect.right()
                            && a.x.max(b.x) > rect.left()
                    };
                    assert!(!crosses, "edge {a:?} -> {b:?} crosses card {rect:?}");
                }
            }
        }
    }

    #[test]
    fn compact_layout_folds_long_chains_and_routes_all_skip_dependencies() {
        use super::super::pass_graph_layout::{GraphLayout, LayoutMode};
        use rust_wgpu_fiber::eframe::egui;
        let names: Vec<_> = (0..13).map(|i| format!("pass{i}")).collect();
        let graph = PassGraph::build(
            names
                .iter()
                .enumerate()
                .map(|(index, name)| {
                    let reads: Vec<_> = names[..index].iter().map(String::as_str).collect();
                    pass(name, &reads, name)
                })
                .collect(),
            Some("pass12"),
        );
        for columns in 1..=13 {
            assert_layout_clear(&graph, columns);
        }
        let available = egui::vec2(1120.0, 600.0);
        let columns = GraphLayout::columns_for(&graph, available);
        let compact = GraphLayout::new(&graph, LayoutMode::Compact, columns);
        let vertical = GraphLayout::new(&graph, LayoutMode::Vertical, 1);
        let fit =
            |bounds: egui::Rect| (available.x / bounds.width()).min(available.y / bounds.height());
        assert!(columns > 1 && columns < 13);
        assert!(fit(compact.bounds) > fit(vertical.bounds) * 2.0);
    }

    #[test]
    fn compact_layout_preserves_parallel_branches_and_avoids_cards_at_row_turns() {
        let graph = PassGraph::build(
            vec![
                pass("source", &[], "base"),
                pass("left", &["base"], "left"),
                pass("middle", &["base"], "middle"),
                pass("right", &["base"], "right"),
                pass("join", &["left", "middle", "right"], "join"),
                pass("a", &["join", "base"], "a"),
                pass("b", &["join"], "b"),
                pass("final", &["a", "b", "base", "middle"], "out"),
            ],
            Some("out"),
        );
        assert_eq!(graph.rows[1].len(), 3);
        for columns in 1..=graph.rows.len() {
            assert_layout_clear(&graph, columns);
        }
    }

    #[test]
    fn blur_groups_do_not_swallow_external_consumers_or_compose_draws() {
        let graph = PassGraph::build(
            vec![
                pass("sys.blur.effect.h.ds1.pass", &["source"], "h"),
                pass("consumer", &["h"], "side"),
                pass("sys.blur.effect.v.ds1.pass", &["h", "side"], "v"),
                pass("sys.blur.effect.to.root.compose.pass", &["v"], "screen"),
            ],
            Some("screen"),
        );
        assert_eq!(graph.presentation(&BTreeSet::new(), false).nodes.len(), 4);
        assert!(graph.nodes.iter().all(|node| node.blur_group.is_none()));
    }

    #[test]
    fn hiding_reuse_order_keeps_load_dependencies_and_independent_branches() {
        let mut overlay = pass("overlay", &["shared"], "shared");
        overlay.pass.sampled_textures.clear(); // Attachment Load, not texture sampling.
        let graph = PassGraph::build(
            vec![
                pass("first", &[], "shared"),
                pass("consumer", &["shared"], "side"),
                pass("clear", &[], "shared"),
                overlay,
            ],
            Some("shared"),
        );
        let data = graph.presentation(&BTreeSet::new(), false);
        assert_eq!(
            data.edges
                .iter()
                .map(|e| (e.from, e.to))
                .collect::<Vec<_>>(),
            [(0, 1), (2, 3)]
        );
        assert_eq!(data.rows, [vec![0, 2], vec![1, 3]]);
        assert_eq!(
            graph.presentation(&BTreeSet::new(), true).edges.len(),
            graph.edges.len()
        );
    }

    #[test]
    fn late_image_input_is_placed_near_its_consumer() {
        let graph = PassGraph::build(
            vec![
                pass("late image", &[], "image"),
                pass("base", &[], "base"),
                pass("effect", &["base"], "effect"),
                pass("overlay", &["effect", "image"], "screen"),
            ],
            Some("screen"),
        );
        assert_eq!(graph.nodes[0].pass.order_index, 0);
        assert_eq!(graph.rows[0], [1]);
        assert!(graph.rows[1].contains(&0));
        assert_eq!(graph.rows[2], [3]);
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
