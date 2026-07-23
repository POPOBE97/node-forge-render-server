//! Resource snapshot extraction and file-tree model for the sidebar inspector.
//!
//! Reads live data from `ShaderSpace` and produces a lightweight
//! `ResourceSnapshot` that the UI can display without holding GPU pool locks.
//!
//! The tree is a **target-centric render-graph tree**:
//! - top-level nodes are render targets (textures)
//! - children are passes that write to that target (in execution order)
//! - each writer lists sampled-texture ancestry as texture -> producing-pass chains.

use std::collections::{HashMap, HashSet};

use rust_wgpu_fiber::{eframe::wgpu, shader_space::ShaderSpace};

use crate::{
    dsl::{Node, SceneDSL},
    renderer::{PassBindings, render_plan::resource_naming::readable_pass_name_for_node},
};

// ---------------------------------------------------------------------------
// Snapshot types
// ---------------------------------------------------------------------------

/// Lightweight info for a single render pass, including its sampled textures.
#[derive(Clone, Debug)]
pub struct PassInfo {
    pub name: String,
    /// Optional UI-only label. The exact resource `name` remains the stable
    /// identity used by pass debug and GPU resource lookup.
    pub display_label: Option<String>,
    pub source_node_id: Option<String>,
    pub source_node_type: Option<String>,
    /// Monotonic execution order from the ShaderSpace composition.
    pub order_index: usize,
    pub target_texture: Option<String>,
    /// Target texture dimensions (if known).
    pub target_size: Option<(u32, u32)>,
    /// Target texture format (if known).
    pub target_format: Option<String>,
    pub is_compute: bool,
    /// Names of textures this pass samples (via bind groups).
    pub sampled_textures: Vec<String>,
    /// Number of GPU instances per draw call (from instance-step-mode vertex buffer).
    pub instance_count: u32,
    /// Number of vertices (or indices) per draw call.
    pub vertex_count: u32,
    /// For compute passes: total workgroup dispatches.
    pub workgroup_count: u32,
}

/// Lightweight info for a single buffer.
#[derive(Clone, Debug)]
pub struct BufferNodeInfo {
    pub name: String,
    pub size: u64,
    pub usage_label: String,
}

/// Lightweight info for a sampler.
#[derive(Clone, Debug)]
pub struct SamplerNodeInfo {
    pub name: String,
}

/// Complete point-in-time snapshot of all GPU resources.
#[derive(Clone, Debug, Default)]
pub struct ResourceSnapshot {
    pub passes: Vec<PassInfo>,
    pub buffers: Vec<BufferNodeInfo>,
    pub samplers: Vec<SamplerNodeInfo>,
    pub final_output_texture: Option<String>,
}

impl ResourceSnapshot {
    /// Capture a snapshot from the live `ShaderSpace` and pass bindings.
    ///
    /// This locks `buffers` once, iterates all pools, and returns owned data
    /// so the UI can render without holding any locks.
    pub fn capture(
        ss: &ShaderSpace,
        _pass_bindings: &[PassBindings],
        final_output_texture: Option<&str>,
        scene: Option<&SceneDSL>,
    ) -> Self {
        let pass_sources = scene.map(pass_source_metadata_by_pass).unwrap_or_default();
        let pass_display_labels = scene.map(pass_display_labels_by_pass).unwrap_or_default();
        let mut execution_order = ss.composition.flatten();
        execution_order.reverse();
        let execution_order_by_pass: HashMap<String, usize> = execution_order
            .into_iter()
            .enumerate()
            .map(|(index, dependency)| (dependency.pass_name.as_str().to_string(), index))
            .collect();
        // --- Passes ---
        let mut passes: Vec<PassInfo> = ss
            .passes
            .inner
            .iter()
            .enumerate()
            .map(|(registry_index, (name, pass))| {
                let order_index = execution_order_by_pass
                    .get(name.as_str())
                    .copied()
                    .unwrap_or(registry_index);
                let (source_node_id, source_node_type) = pass_sources
                    .get(name.as_str())
                    .cloned()
                    .unwrap_or((None, None));
                let is_compute =
                    matches!(pass.pipeline, rust_wgpu_fiber::pass::Pipeline::Compute(_));

                // Collect sampled textures from bind group entries.
                let mut sampled_textures = Vec::new();
                for (_group_id, (entries, _, _)) in &pass.bindings {
                    for (_binding_id, (res_name, entry)) in entries {
                        if matches!(entry.ty, wgpu::BindingType::Texture { .. }) {
                            sampled_textures.push(res_name.as_str().to_string());
                        }
                    }
                }
                sampled_textures.sort();
                sampled_textures.dedup();

                // For MSAA render passes, preview/dependency consumers should use the
                // single-sample resolve target (when present), not the multisampled
                // color attachment that cannot be sampled by egui.
                let target_texture = pass
                    .resolve_target
                    .as_ref()
                    .or(pass.color_attachment.as_ref())
                    .map(|r| r.as_str().to_string());

                // Look up target texture info.
                let (target_size, target_format) = target_texture
                    .as_deref()
                    .and_then(|tn| ss.texture_info(tn))
                    .map(|info| {
                        (
                            Some((info.size.width, info.size.height)),
                            Some(format!("{:?}", info.format)),
                        )
                    })
                    .unwrap_or((None, None));

                PassInfo {
                    name: name.as_str().to_string(),
                    display_label: pass_display_labels.get(name.as_str()).cloned(),
                    source_node_id,
                    source_node_type,
                    order_index,
                    target_texture,
                    target_size,
                    target_format,
                    is_compute,
                    sampled_textures,
                    instance_count: 0,  // filled below from buffers
                    vertex_count: 0,    // filled below from buffers
                    workgroup_count: 0, // filled below for compute
                }
            })
            .collect();

        // Compute per-pass draw metrics from attribute bindings + buffer pool,
        // mirroring the logic in ShaderSpace::render().
        {
            let buffers_ok = ss.buffers.lock().ok();
            for (pass_info, (_name, pass)) in passes.iter_mut().zip(ss.passes.inner.iter()) {
                if pass_info.is_compute {
                    pass_info.workgroup_count =
                        pass.workgroup[0] * pass.workgroup[1] * pass.workgroup[2];
                } else {
                    let mut num_instance: u32 = 1;
                    let mut num_vertices: u32 = 3;

                    if let Some(ref bufs) = buffers_ok {
                        for (_location, (buffer_name, step_mode, vertex_attribute)) in
                            pass.attribute_bindings.iter()
                        {
                            let stride: u64 =
                                vertex_attribute.iter().map(|a| a.format.size()).sum();
                            if stride == 0 {
                                continue;
                            }
                            if let Some(fish) = bufs.get(buffer_name.as_str()) {
                                let buf_size =
                                    fish.wgpu_buffer.as_ref().map(|b| b.size()).unwrap_or(0);
                                let num = (buf_size / stride) as u32;
                                match step_mode {
                                    wgpu::VertexStepMode::Vertex => num_vertices = num,
                                    wgpu::VertexStepMode::Instance => num_instance = num,
                                }
                            }
                        }

                        // Check index buffer for indexed draw calls.
                        if let Some((buffer_name, format)) = pass.index_binding.as_ref() {
                            if let Some(fish) = bufs.get(buffer_name.as_str()) {
                                let buf_size =
                                    fish.wgpu_buffer.as_ref().map(|b| b.size()).unwrap_or(0);
                                let index_stride = match format {
                                    wgpu::IndexFormat::Uint16 => 2u64,
                                    wgpu::IndexFormat::Uint32 => 4u64,
                                };
                                num_vertices = (buf_size / index_stride) as u32;
                            }
                        }
                    }

                    pass_info.instance_count = num_instance;
                    pass_info.vertex_count = num_vertices;
                }
            }
        }

        // --- Buffers ---
        let mut buffers: Vec<BufferNodeInfo> = Vec::new();
        if let Ok(bufs) = ss.buffers.lock() {
            buffers = bufs
                .iter()
                .map(|(name, fish)| {
                    let (size, usage) = match &fish.wgpu_buffer_desc {
                        rust_wgpu_fiber::pool::buffer_pool::BufferPoolFishDescriptor::Sized {
                            size,
                            usage,
                        } => (*size, *usage),
                        rust_wgpu_fiber::pool::buffer_pool::BufferPoolFishDescriptor::Init {
                            contents,
                            usage,
                        } => (contents.len() as u64, *usage),
                    };
                    let actual_size = fish.wgpu_buffer.as_ref().map(|b| b.size()).unwrap_or(size);
                    BufferNodeInfo {
                        name: name.as_str().to_string(),
                        size: actual_size,
                        usage_label: format_buffer_usage(usage),
                    }
                })
                .collect();
            buffers.sort_by(|a, b| a.name.cmp(&b.name));
        }

        // --- Samplers ---
        let mut samplers: Vec<SamplerNodeInfo> = ss
            .samplers
            .iter()
            .map(|(name, _)| SamplerNodeInfo {
                name: name.as_str().to_string(),
            })
            .collect();
        samplers.sort_by(|a, b| a.name.cmp(&b.name));

        ResourceSnapshot {
            passes,
            buffers,
            samplers,
            final_output_texture: final_output_texture.map(ToOwned::to_owned),
        }
    }
}

fn pass_source_metadata_by_pass(
    scene: &SceneDSL,
) -> HashMap<String, (Option<String>, Option<String>)> {
    let mut out = HashMap::new();
    for node in &scene.nodes {
        if node.node_type == "MeshGradient" {
            out.insert(
                format!("sys.mesh_gradient.{}.pass", node.id),
                (Some(node.id.clone()), Some(node.node_type.clone())),
            );
        } else if node.node_type == "IntelligentLight" {
            out.insert(
                format!("sys.ilight.{}.pass", node.id),
                (Some(node.id.clone()), Some(node.node_type.clone())),
            );
        }
    }
    out
}

fn pass_display_labels_by_pass(scene: &SceneDSL) -> HashMap<String, String> {
    scene
        .nodes
        .iter()
        .filter(|node| node.node_type == "RenderPass")
        .filter_map(|node| {
            grouped_render_pass_display_label(node).map(|label| {
                (
                    readable_pass_name_for_node(node).as_str().to_string(),
                    label,
                )
            })
        })
        .collect()
}

fn grouped_render_pass_display_label(node: &Node) -> Option<String> {
    let group_label = ["__group_instance_label", "__group_name", "__group_id"]
        .iter()
        .find_map(|key| node.params.get(*key).and_then(|value| value.as_str()))
        .map(str::trim)
        .filter(|label| !label.is_empty())?;
    let pass_label = ["__node_label", "label", "name", "title", "headerLabel"]
        .iter()
        .find_map(|key| node.params.get(*key).and_then(|value| value.as_str()))
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .unwrap_or("Render Pass");

    Some(format!("{group_label} / {pass_label}"))
}

// ---------------------------------------------------------------------------
// File-tree model
// ---------------------------------------------------------------------------

/// Visual icon kind for a tree node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TreeIcon {
    FolderOpen,
    FolderClosed,
    Pass,
    Texture,
    Buffer,
    Sampler,
}

/// Semantic kind of the node — determines click behaviour.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Folder,
    /// A render pass. `target_texture` is the name of its color attachment (if any).
    Pass {
        pass_name: String,
        target_texture: Option<String>,
        target_size: Option<(u32, u32)>,
        source_node_id: Option<String>,
        source_node_type: Option<String>,
    },
    Texture {
        texture_name: String,
    },
    Buffer,
    Sampler,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PassDesignTarget {
    pub node_id: String,
    pub node_type: String,
    pub pass_name: String,
    pub target_texture: Option<String>,
    pub target_size: Option<(u32, u32)>,
}

/// A single node in the file-tree.
#[derive(Clone, Debug)]
pub struct FileTreeNode {
    pub id: String,
    pub label: String,
    pub icon: TreeIcon,
    pub kind: NodeKind,
    pub detail: Option<String>,
    pub children: Vec<FileTreeNode>,
}

impl ResourceSnapshot {
    /// Build the file-tree from this snapshot.
    ///
    /// Structure:
    ///   Dependencies (root folder)
    ///     ├── <target texture>
    ///     │   ├── <writer pass #1>
    ///     │   │   └── <sampled texture>
    ///     │   │       └── <producing pass>
    ///     │   ├── <writer pass #2>
    ///   Buffers (folder)
    ///   Samplers (folder)
    pub fn to_tree(&self) -> Vec<FileTreeNode> {
        let mut roots = Vec::new();
        let total_draw_calls = self.passes.iter().filter(|pass| !pass.is_compute).count();

        // ── Dependency graph ──
        let dep_children =
            build_dependency_tree(&self.passes, self.final_output_texture.as_deref());
        roots.push(FileTreeNode {
            id: "section.deps".into(),
            label: format!("Pass Dependencies ({total_draw_calls} DCs)"),
            icon: TreeIcon::FolderClosed,
            kind: NodeKind::Folder,
            detail: None,
            children: dep_children,
        });

        // ── Buffers ──
        let buf_children: Vec<FileTreeNode> = self
            .buffers
            .iter()
            .map(|b| FileTreeNode {
                id: format!("buf.{}", b.name),
                label: b.name.clone(),
                icon: TreeIcon::Buffer,
                kind: NodeKind::Buffer,
                detail: Some(format!("{} {}", format_bytes(b.size), b.usage_label)),
                children: vec![],
            })
            .collect();
        roots.push(FileTreeNode {
            id: "section.buffers".into(),
            label: format!("Buffers ({})", buf_children.len()),
            icon: TreeIcon::FolderClosed,
            kind: NodeKind::Folder,
            detail: None,
            children: buf_children,
        });

        // ── Samplers ──
        let sam_children: Vec<FileTreeNode> = self
            .samplers
            .iter()
            .map(|s| FileTreeNode {
                id: format!("sam.{}", s.name),
                label: s.name.clone(),
                icon: TreeIcon::Sampler,
                kind: NodeKind::Sampler,
                detail: None,
                children: vec![],
            })
            .collect();
        roots.push(FileTreeNode {
            id: "section.samplers".into(),
            label: format!("Samplers ({})", sam_children.len()),
            icon: TreeIcon::FolderClosed,
            kind: NodeKind::Folder,
            detail: None,
            children: sam_children,
        });

        roots
    }
}

// ---------------------------------------------------------------------------
// Dependency graph builder
// ---------------------------------------------------------------------------

/// Build a target-centric dependency tree from pass info.
///
/// A writer pass is grouped under the texture it writes to. For each sampled
/// texture used by that pass, we attach a texture child and (when resolvable)
/// the pass that produced that sampled texture earlier in execution order.
fn build_dependency_tree(
    passes: &[PassInfo],
    final_output_texture: Option<&str>,
) -> Vec<FileTreeNode> {
    // Map: texture_name -> all pass writers for that target, in execution order.
    let mut writers_by_target: HashMap<&str, Vec<&PassInfo>> = HashMap::new();
    for pass in passes {
        if let Some(target) = pass.target_texture.as_deref() {
            writers_by_target.entry(target).or_default().push(pass);
        }
    }

    let Some(root_target) = final_output_texture else {
        return Vec::new();
    };

    let mut root_writers = writers_by_target.remove(root_target).unwrap_or_default();
    root_writers.sort_by_key(|p| p.order_index);

    fn pass_label(pass: &PassInfo) -> String {
        let base = pass
            .display_label
            .clone()
            .unwrap_or_else(|| pass_basename(&pass.name));
        if !pass.is_compute && pass.instance_count > 1 {
            format!("{} (×{})", base, pass.instance_count)
        } else {
            base
        }
    }

    /// Return all passes that write to `target`, sorted by execution order.
    fn producing_passes_for_texture<'a>(target: &str, passes: &'a [PassInfo]) -> Vec<&'a PassInfo> {
        let mut writers: Vec<&PassInfo> = passes
            .iter()
            .filter(|p| p.target_texture.as_deref() == Some(target))
            .collect();
        writers.sort_by_key(|p| p.order_index);
        writers
    }

    fn build_pass_node(
        pass: &PassInfo,
        passes: &[PassInfo],
        visited: &mut HashSet<String>,
    ) -> FileTreeNode {
        let mut texture_children: Vec<FileTreeNode> = Vec::new();

        for sampled_tex in &pass.sampled_textures {
            let producers = producing_passes_for_texture(sampled_tex.as_str(), passes);
            let mut sampled_children: Vec<FileTreeNode> = Vec::new();

            for producer in &producers {
                if visited.contains(&producer.name) {
                    sampled_children.push(FileTreeNode {
                        id: format!("pass.{}.cycle", producer.name),
                        label: format!("{} ↻", pass_basename(&producer.name)),
                        icon: TreeIcon::Pass,
                        kind: NodeKind::Pass {
                            pass_name: producer.name.clone(),
                            target_texture: producer.target_texture.clone(),
                            target_size: producer.target_size,
                            source_node_id: producer.source_node_id.clone(),
                            source_node_type: producer.source_node_type.clone(),
                        },
                        detail: None,
                        children: vec![],
                    });
                } else {
                    visited.insert(producer.name.clone());
                    sampled_children.push(build_pass_node(producer, passes, visited));
                    visited.remove(&producer.name);
                }
            }

            texture_children.push(FileTreeNode {
                id: format!("texdep.{}.{}", pass.name, sampled_tex),
                label: sampled_tex.clone(),
                icon: TreeIcon::Texture,
                kind: NodeKind::Texture {
                    texture_name: sampled_tex.clone(),
                },
                detail: None,
                children: sampled_children,
            });
        }

        FileTreeNode {
            id: format!("pass.{}", pass.name),
            label: pass_label(pass),
            icon: TreeIcon::Pass,
            kind: NodeKind::Pass {
                pass_name: pass.name.clone(),
                target_texture: pass.target_texture.clone(),
                target_size: pass.target_size,
                source_node_id: pass.source_node_id.clone(),
                source_node_type: pass.source_node_type.clone(),
            },
            detail: None,
            children: texture_children,
        }
    }

    let mut writer_nodes: Vec<FileTreeNode> = Vec::new();
    for writer in root_writers {
        let mut visited = HashSet::new();
        visited.insert(writer.name.clone());
        writer_nodes.push(build_pass_node(writer, passes, &mut visited));
    }

    vec![FileTreeNode {
        id: format!("target.{root_target}"),
        label: root_target.to_string(),
        icon: TreeIcon::Texture,
        kind: NodeKind::Texture {
            texture_name: root_target.to_string(),
        },
        detail: None,
        children: writer_nodes,
    }]
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Extract a short display name for a pass by stripping the common `.pass`
/// suffix and any `sys.` prefix, keeping only the distinctive segments.
fn pass_basename(name: &str) -> String {
    let s = name.strip_suffix(".pass").unwrap_or(name);
    let s = s.strip_prefix("sys.").unwrap_or(s);
    s.to_string()
}

fn format_bytes(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / 1024.0)
    } else {
        format!("{} B", bytes)
    }
}

fn format_buffer_usage(usage: wgpu::BufferUsages) -> String {
    let mut parts = Vec::new();
    if usage.contains(wgpu::BufferUsages::VERTEX) {
        parts.push("vtx");
    }
    if usage.contains(wgpu::BufferUsages::INDEX) {
        parts.push("idx");
    }
    if usage.contains(wgpu::BufferUsages::UNIFORM) {
        parts.push("uni");
    }
    if usage.contains(wgpu::BufferUsages::STORAGE) {
        parts.push("sto");
    }
    if usage.contains(wgpu::BufferUsages::COPY_SRC) {
        parts.push("src");
    }
    if usage.contains(wgpu::BufferUsages::COPY_DST) {
        parts.push("dst");
    }
    if parts.is_empty() {
        "–".into()
    } else {
        parts.join("|")
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{
        BufferNodeInfo, PassInfo, ResourceSnapshot, SamplerNodeInfo, pass_display_labels_by_pass,
        pass_source_metadata_by_pass,
    };
    use crate::dsl::{Metadata, Node, SceneDSL};
    use serde_json::json;

    #[test]
    fn long_root_target_label_is_preserved() {
        let root_target = "sys.this.is.a.very.long.root.target.texture.name.present.sdr.srgb";
        let snapshot = ResourceSnapshot {
            passes: vec![],
            buffers: vec![],
            samplers: vec![],
            final_output_texture: Some(root_target.to_string()),
        };

        let tree = snapshot.to_tree();
        assert_eq!(tree[0].children[0].label, root_target);
    }

    #[test]
    fn long_sampled_texture_label_is_preserved() {
        let root_target = "sys.final.output.present.sdr.srgb";
        let sampled_texture =
            "sys.really.long.sampled.texture.name.with.multiple.segments.for.debugging";
        let snapshot = ResourceSnapshot {
            passes: vec![PassInfo {
                name: "sys.compose.pass".to_string(),
                display_label: None,
                source_node_id: None,
                source_node_type: None,
                order_index: 0,
                target_texture: Some(root_target.to_string()),
                target_size: None,
                target_format: None,
                is_compute: false,
                sampled_textures: vec![sampled_texture.to_string()],
                instance_count: 1,
                vertex_count: 3,
                workgroup_count: 0,
            }],
            buffers: vec![],
            samplers: vec![],
            final_output_texture: Some(root_target.to_string()),
        };

        let tree = snapshot.to_tree();
        assert_eq!(
            tree[0].children[0].children[0].children[0].label,
            sampled_texture
        );
    }

    #[test]
    fn long_buffer_and_sampler_labels_are_preserved() {
        let buffer_name = "sys.very.long.params.buffer.name.for.resource.tree.debugging";
        let sampler_name = "sys.very.long.sampler.name.for.resource.tree.debugging";
        let snapshot = ResourceSnapshot {
            passes: vec![],
            buffers: vec![BufferNodeInfo {
                name: buffer_name.to_string(),
                size: 256,
                usage_label: "uni".to_string(),
            }],
            samplers: vec![SamplerNodeInfo {
                name: sampler_name.to_string(),
            }],
            final_output_texture: None,
        };

        let tree = snapshot.to_tree();
        assert_eq!(tree[1].children[0].label, buffer_name);
        assert_eq!(tree[2].children[0].label, sampler_name);
    }

    #[test]
    fn pass_dependencies_label_uses_total_draw_calls() {
        let root_target = "sys.final.output.present.sdr.srgb";
        let snapshot = ResourceSnapshot {
            passes: vec![
                PassInfo {
                    name: "sys.compose.pass".to_string(),
                    display_label: None,
                    source_node_id: None,
                    source_node_type: None,
                    order_index: 0,
                    target_texture: Some(root_target.to_string()),
                    target_size: None,
                    target_format: None,
                    is_compute: false,
                    sampled_textures: vec![],
                    instance_count: 1,
                    vertex_count: 3,
                    workgroup_count: 0,
                },
                PassInfo {
                    name: "sys.grade.pass".to_string(),
                    display_label: None,
                    source_node_id: None,
                    source_node_type: None,
                    order_index: 1,
                    target_texture: Some(root_target.to_string()),
                    target_size: None,
                    target_format: None,
                    is_compute: false,
                    sampled_textures: vec![],
                    instance_count: 1,
                    vertex_count: 3,
                    workgroup_count: 0,
                },
                PassInfo {
                    name: "sys.compute.prepass".to_string(),
                    display_label: None,
                    source_node_id: None,
                    source_node_type: None,
                    order_index: 2,
                    target_texture: None,
                    target_size: None,
                    target_format: None,
                    is_compute: true,
                    sampled_textures: vec![],
                    instance_count: 0,
                    vertex_count: 0,
                    workgroup_count: 1,
                },
            ],
            buffers: vec![],
            samplers: vec![],
            final_output_texture: Some(root_target.to_string()),
        };

        let tree = snapshot.to_tree();
        assert_eq!(tree[0].label, "Pass Dependencies (2 DCs)");
    }

    #[test]
    fn pass_nodes_preserve_exact_pass_name() {
        let root_target = "sys.final.output.present.sdr.srgb";
        let snapshot = ResourceSnapshot {
            passes: vec![PassInfo {
                name: "sys.render.pass.exact.name.pass".to_string(),
                display_label: Some("Light Effect / Render Pass".to_string()),
                source_node_id: None,
                source_node_type: None,
                order_index: 0,
                target_texture: Some(root_target.to_string()),
                target_size: None,
                target_format: None,
                is_compute: false,
                sampled_textures: vec![],
                instance_count: 1,
                vertex_count: 3,
                workgroup_count: 0,
            }],
            buffers: vec![],
            samplers: vec![],
            final_output_texture: Some(root_target.to_string()),
        };

        let tree = snapshot.to_tree();
        let pass_node = &tree[0].children[0].children[0];
        assert_eq!(pass_node.label, "Light Effect / Render Pass");
        assert!(matches!(
            &pass_node.kind,
            super::NodeKind::Pass { pass_name, .. }
                if pass_name == "sys.render.pass.exact.name.pass"
        ));
    }

    #[test]
    fn grouped_render_pass_display_label_uses_instance_and_node_title_labels() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: Metadata {
                name: "metadata test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![Node {
                id: "GroupInstance_32/RenderPass_26".to_string(),
                node_type: "RenderPass".to_string(),
                params: HashMap::from([
                    ("__group_instance_label".to_string(), json!("LightEffect")),
                    ("__node_label".to_string(), json!("Beauty Pass")),
                    ("name".to_string(), json!("Render Pass")),
                ]),
                inputs: vec![],
                outputs: vec![],
                input_bindings: vec![],
                wgsl_override: None,
            }],
            connections: vec![],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };

        let labels = pass_display_labels_by_pass(&scene);
        assert_eq!(
            labels.get("render.pass.pass26.pass"),
            Some(&"LightEffect / Beauty Pass".to_string())
        );
    }

    #[test]
    fn mesh_gradient_pass_source_metadata_maps_to_scene_node() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: Metadata {
                name: "metadata test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![Node {
                id: "MeshGradient_12".to_string(),
                node_type: "MeshGradient".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                outputs: vec![],
                input_bindings: vec![],
                wgsl_override: None,
            }],
            connections: vec![],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };

        let sources = pass_source_metadata_by_pass(&scene);
        assert_eq!(
            sources.get("sys.mesh_gradient.MeshGradient_12.pass"),
            Some(&(
                Some("MeshGradient_12".to_string()),
                Some("MeshGradient".to_string())
            ))
        );
    }

    #[test]
    fn intelligent_light_pass_source_metadata_maps_intrinsic_pass_to_scene_node() {
        let scene = SceneDSL {
            version: "1".to_string(),
            metadata: Metadata {
                name: "metadata test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![Node {
                id: "IntelligentLight_7".to_string(),
                node_type: "IntelligentLight".to_string(),
                params: HashMap::new(),
                inputs: vec![],
                outputs: vec![],
                input_bindings: vec![],
                wgsl_override: None,
            }],
            connections: vec![],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };

        let sources = pass_source_metadata_by_pass(&scene);
        assert_eq!(
            sources.get("sys.ilight.IntelligentLight_7.pass"),
            Some(&(
                Some("IntelligentLight_7".to_string()),
                Some("IntelligentLight".to_string())
            ))
        );
    }
}
