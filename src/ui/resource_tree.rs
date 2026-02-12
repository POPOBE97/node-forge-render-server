//! Resource snapshot extraction and file-tree model for the sidebar inspector.
//!
//! Reads live data from `ShaderSpace` and produces a lightweight
//! `ResourceSnapshot` that the UI can display without holding GPU pool locks.
//!
//! The tree is a **true render-graph dependency tree** — a pass B lists pass A
//! as a child only when B samples a texture that A renders to.

use std::collections::{HashMap, HashSet};

use rust_wgpu_fiber::{
    eframe::wgpu,
    shader_space::ShaderSpace,
};

use crate::renderer::PassBindings;

// ---------------------------------------------------------------------------
// Snapshot types
// ---------------------------------------------------------------------------

/// Lightweight info for a single render pass, including its sampled textures.
#[derive(Clone, Debug)]
pub struct PassInfo {
    pub name: String,
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
}

impl ResourceSnapshot {
    /// Capture a snapshot from the live `ShaderSpace` and pass bindings.
    ///
    /// This locks `buffers` once, iterates all pools, and returns owned data
    /// so the UI can render without holding any locks.
    pub fn capture(ss: &ShaderSpace, _pass_bindings: &[PassBindings]) -> Self {
        // --- Passes ---
        let mut passes: Vec<PassInfo> = ss
            .passes
            .inner
            .iter()
            .map(|(name, pass)| {
                let is_compute = matches!(
                    pass.pipeline,
                    rust_wgpu_fiber::pass::Pipeline::Compute(_)
                );

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

                let target_texture = pass
                    .color_attachment
                    .as_ref()
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
            for (pass_info, (_name, pass)) in
                passes.iter_mut().zip(ss.passes.inner.iter())
            {
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
                                let buf_size = fish
                                    .wgpu_buffer
                                    .as_ref()
                                    .map(|b| b.size())
                                    .unwrap_or(0);
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
                                let buf_size = fish
                                    .wgpu_buffer
                                    .as_ref()
                                    .map(|b| b.size())
                                    .unwrap_or(0);
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

        passes.sort_by(|a, b| a.name.cmp(&b.name));

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
        }
    }
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
    Pass { target_texture: Option<String> },
    Texture,
    Buffer,
    Sampler,
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
    ///     ├── passC  →  512×512 RGBA8     (root pass, renders to final output)
    ///     │   ├── passA  →  256×256 RGBA8  (passC samples passA's target)
    ///     │   └── passB  →  256×256 RGBA8  (passC samples passB's target)
    ///     │       └── passA  →  …          (passB also samples passA's target)
    ///   Buffers (folder)
    ///   Samplers (folder)
    pub fn to_tree(&self) -> Vec<FileTreeNode> {
        let mut roots = Vec::new();

        // ── Dependency graph ──
        let dep_children = build_dependency_tree(&self.passes);
        roots.push(FileTreeNode {
            id: "section.deps".into(),
            label: format!("Dependencies ({})", self.passes.len()),
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
                label: truncate_name(&b.name, 32),
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
                label: truncate_name(&s.name, 32),
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

/// Build a true render-graph dependency tree from pass info.
///
/// A dependency edge exists between pass B → pass A when B samples a texture
/// that is pass A's `color_attachment`.  Passes with no downstream consumers
/// are tree roots (i.e. they appear at the top level).
fn build_dependency_tree(passes: &[PassInfo]) -> Vec<FileTreeNode> {
    // Map: texture_name → pass that renders to it.
    let texture_to_pass: HashMap<&str, &PassInfo> = passes
        .iter()
        .filter_map(|p| p.target_texture.as_deref().map(|t| (t, p)))
        .collect();

    // Map: pass_name → PassInfo for quick lookup.
    let pass_by_name: HashMap<&str, &PassInfo> = passes
        .iter()
        .map(|p| (p.name.as_str(), p))
        .collect();

    // For each pass, find its upstream dependencies (passes whose targets it samples).
    let mut deps: HashMap<&str, Vec<&str>> = HashMap::new();
    // Track which passes are depended on (i.e. are NOT roots).
    let mut has_parent: HashSet<&str> = HashSet::new();

    for pass in passes {
        let mut upstream: Vec<&str> = Vec::new();
        for sampled_tex in &pass.sampled_textures {
            if let Some(producing_pass) = texture_to_pass.get(sampled_tex.as_str()) {
                if producing_pass.name != pass.name {
                    upstream.push(producing_pass.name.as_str());
                    has_parent.insert(producing_pass.name.as_str());
                }
            }
        }
        upstream.sort();
        upstream.dedup();
        deps.insert(pass.name.as_str(), upstream);
    }

    // Root passes: those not depended on by any other pass.
    let mut root_names: Vec<&str> = passes
        .iter()
        .map(|p| p.name.as_str())
        .filter(|n| !has_parent.contains(n))
        .collect();
    root_names.sort();

    // Build tree nodes recursively (with visited set to avoid infinite loops in cycles).
    fn build_node(
        pass: &PassInfo,
        deps: &HashMap<&str, Vec<&str>>,
        pass_by_name: &HashMap<&str, &PassInfo>,
        visited: &mut HashSet<String>,
    ) -> FileTreeNode {

        let mut children = Vec::new();
        if let Some(upstream_names) = deps.get(pass.name.as_str()) {
            for &up_name in upstream_names {
                if visited.contains(up_name) {
                    // Cycle — show as leaf with indicator.
                    children.push(FileTreeNode {
                        id: format!("pass.{}.cycle", up_name),
                        label: format!("{} ↻", pass_basename(up_name)),
                        icon: TreeIcon::Pass,
                        kind: NodeKind::Pass {
                            target_texture: pass_by_name
                                .get(up_name)
                                .and_then(|p| p.target_texture.clone()),
                        },
                        detail: None,
                        children: vec![],
                    });
                    continue;
                }
                if let Some(up_pass) = pass_by_name.get(up_name) {
                    visited.insert(up_name.to_string());
                    children.push(build_node(up_pass, deps, pass_by_name, visited));
                    visited.remove(up_name);
                }
            }
        }

        let label = {
            let base = pass_basename(&pass.name);
            if !pass.is_compute && pass.instance_count > 1 {
                format!("{} (×{})", base, pass.instance_count)
            } else {
                base
            }
        };

        FileTreeNode {
            id: format!("pass.{}", pass.name),
            label,
            icon: TreeIcon::Pass,
            kind: NodeKind::Pass {
                target_texture: pass.target_texture.clone(),
            },
            detail: None,
            children,
        }
    }

    let mut result = Vec::new();
    for &root_name in &root_names {
        if let Some(pass) = pass_by_name.get(root_name) {
            let mut visited = HashSet::new();
            visited.insert(root_name.to_string());
            result.push(build_node(pass, &deps, &pass_by_name, &mut visited));
        }
    }
    result
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn truncate_name(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("…{}", &s[s.len() - (max - 1)..])
    }
}

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
