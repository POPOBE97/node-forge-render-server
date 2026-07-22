use std::{
    collections::{HashSet, VecDeque, hash_map::DefaultHasher},
    hash::{Hash, Hasher},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use crossbeam_channel::Receiver;
use rust_wgpu_fiber::{
    ResourceName,
    eframe::{egui, wgpu},
    shader_space::{RenderProfile, ShaderSpace},
};
use serde::{Deserialize, Serialize};

use crate::{renderer, ws};

use crate::ui::animation_manager::AnimationManager;
use crate::ui::file_tree_widget::FileTreeState;
use crate::ui::resource_tree::{FileTreeNode, ResourceSnapshot};

use super::canvas::state::{CanvasState, ReferenceDesiredSource};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefImageMode {
    #[default]
    Overlay,
    Diff,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefImageSource {
    Manual,
    AndroidScrcpyUsb(String),
    ShortwireClipboard,
    ShortwirePatch,
    SceneNodePath(String),
    SceneNodeDataUrl(String),
    SceneNodeAssetId(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum RefImageAlphaMode {
    #[default]
    Premultiplied,
    Straight,
}

impl RefImageAlphaMode {
    pub fn short_label(self) -> &'static str {
        match self {
            Self::Premultiplied => "PRE",
            Self::Straight => "STR",
        }
    }
}

fn default_shortwire_reference_opacity() -> f32 {
    0.5
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortwireReferenceImage {
    #[serde(rename = "artifactId")]
    pub artifact_id: String,
    pub name: String,
    pub width: u32,
    pub height: u32,
    #[serde(default)]
    pub alpha_mode: RefImageAlphaMode,
    #[serde(default)]
    pub mode: RefImageMode,
    #[serde(default = "default_shortwire_reference_opacity")]
    pub opacity: f32,
    #[serde(default)]
    pub offset: [f32; 2],
}

#[derive(Clone, Debug, PartialEq)]
pub struct ShortwirePastedReferenceImage {
    pub name: String,
    pub png_bytes: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub alpha_mode: RefImageAlphaMode,
    pub mode: RefImageMode,
    pub opacity: f32,
    pub offset: [f32; 2],
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RefImageTransferMode {
    #[default]
    Srgb,
    Linear,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum DiffMetricMode {
    E,
    #[default]
    AE,
    SE,
    RAE,
    RSE,
}

impl DiffMetricMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::E => "E (signed)",
            Self::AE => "AE",
            Self::SE => "SE",
            Self::RAE => "RAE",
            Self::RSE => "RSE",
        }
    }

    pub fn shader_code(self) -> u32 {
        match self {
            Self::E => 0,
            Self::AE => 1,
            Self::SE => 2,
            Self::RAE => 3,
            Self::RSE => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AnalysisTab {
    #[default]
    Histogram,
    Parade,
    Vectorscope,
}

impl AnalysisTab {
    pub fn label(self) -> &'static str {
        match self {
            Self::Histogram => "Histogram",
            Self::Parade => "Parade",
            Self::Vectorscope => "Vectorscope",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClippingSettings {
    pub shadow_threshold: f32,
    pub highlight_threshold: f32,
}

impl Default for ClippingSettings {
    fn default() -> Self {
        Self {
            shadow_threshold: 0.02,
            highlight_threshold: 0.98,
        }
    }
}

/// Per-channel min/max range for the Qualifier overlay. Pixels whose RGB
/// components all fall inside their respective ranges get highlighted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QualifierSettings {
    pub r_min: f32,
    pub r_max: f32,
    pub g_min: f32,
    pub g_max: f32,
    pub b_min: f32,
    pub b_max: f32,
}

impl Default for QualifierSettings {
    fn default() -> Self {
        Self {
            r_min: 0.0,
            r_max: 1.0,
            g_min: 0.0,
            g_max: 1.0,
            b_min: 0.0,
            b_max: 1.0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum QualifierChannel {
    R,
    G,
    B,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DiffStats {
    pub min: f32,
    pub max: f32,
    pub avg: f32,
    pub rms: f32,
    pub p95_abs: f32,
    pub sample_count: u64,
    pub non_finite_count: u64,
}

pub struct AnalysisSourceDomain<'a> {
    pub texture_name: &'a str,
    pub view: &'a wgpu::TextureView,
    pub size: [u32; 2],
    pub format: wgpu::TextureFormat,
}

pub struct RefImageState {
    pub name: String,
    pub source_linear_rgba: Vec<f32>,
    pub linear_premul_rgba: Vec<f32>,
    pub texture: egui::TextureHandle,
    pub native_texture_id: Option<egui::TextureId>,
    pub wgpu_texture: wgpu::Texture,
    pub wgpu_texture_view: wgpu::TextureView,
    pub size: [u32; 2],
    pub texture_format: wgpu::TextureFormat,
    pub alpha_mode: RefImageAlphaMode,
    #[allow(dead_code)]
    pub transfer_mode: RefImageTransferMode,
    pub offset: egui::Vec2,
    pub mode: RefImageMode,
    pub opacity: f32,
    pub drag_start: Option<egui::Pos2>,
    pub drag_start_offset: egui::Vec2,
    pub source: RefImageSource,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiWindowMode {
    #[default]
    Sidebar,
    CanvasOnly,
}

#[derive(Clone, Copy, Debug)]
pub struct SampledPixel {
    pub x: u32,
    pub y: u32,
    pub rgba: [f32; 4],
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub enum ViewportOperationIndicator {
    Hidden,
    InProgress { started_at: f64, request_id: u64 },
    Success { hide_at: f64 },
    Failure { hide_at: f64 },
}

#[derive(Clone, Copy, Debug)]
pub enum ViewportOperationIndicatorVisual {
    InProgress,
    Success,
    Failure,
}

#[derive(Clone, Debug, Default)]
pub struct RenderTextureFpsTracker {
    scene_redraw_timestamps: VecDeque<f64>,
}

impl RenderTextureFpsTracker {
    const WINDOW_SECS: f64 = 1.0;

    pub fn record_scene_redraw(&mut self, now_secs: f64) {
        self.scene_redraw_timestamps.push_back(now_secs);
        self.prune_stale(now_secs);
    }

    pub fn fps_at(&mut self, now_secs: f64) -> u32 {
        self.prune_stale(now_secs);
        self.scene_redraw_timestamps.len() as u32
    }

    fn prune_stale(&mut self, now_secs: f64) {
        while let Some(oldest) = self.scene_redraw_timestamps.front().copied() {
            if now_secs - oldest > Self::WINDOW_SECS {
                let _ = self.scene_redraw_timestamps.pop_front();
            } else {
                break;
            }
        }
    }
}

#[allow(dead_code)]
pub const CANVAS_RADIUS: f32 = 16.0;
pub const OUTER_MARGIN: f32 = 4.0;
pub const SIDEBAR_ANIM_SECS: f64 = crate::ui::debug_sidebar::SIDEBAR_ANIM_SECS;

pub struct AppInit {
    pub shader_space: ShaderSpace,
    pub resolution: [u32; 2],
    pub window_resolution: [u32; 2],
    pub output_texture_name: ResourceName,
    pub scene_output_texture_name: ResourceName,
    pub export_texture_name: ResourceName,
    /// On-demand SDR encode pass name (UiHdrNative only).
    pub export_encode_pass_name: Option<ResourceName>,
    pub start: Instant,
    pub passes: Vec<renderer::PassBindings>,
    pub scene_rx: Receiver<ws::SceneUpdate>,
    pub capture_state_rx: Option<Receiver<bool>>,
    pub ws_hub: ws::WsHub,
    pub last_good: Arc<Mutex<Option<crate::dsl::SceneDSL>>>,
    pub uniform_scene: Option<crate::dsl::SceneDSL>,
    pub last_pipeline_signature: Option<[u8; 32]>,
    pub follow_scene_resolution_for_window: bool,
    pub force_continuous_redraw: bool,
    pub asset_store: crate::asset_store::AssetStore,
    pub animation_session: Option<crate::animation::AnimationSession>,
    pub pass_debug_sources: std::collections::HashMap<String, renderer::PassDebugSource>,
    pub debug_artifacts: crate::debug_artifacts::DebugArtifactStore,
    pub nforge_path: Option<PathBuf>,
}

pub(super) struct AppCore {
    pub shader_space: ShaderSpace,
    pub resolution: [u32; 2],
    pub window_resolution: [u32; 2],
    pub output_texture_name: ResourceName,
    pub scene_output_texture_name: ResourceName,
    /// Texture for clipboard copy / file export (sRGB-encoded bytes).
    pub export_texture_name: ResourceName,
    /// On-demand SDR encode pass name (UiHdrNative only).
    pub export_encode_pass_name: Option<ResourceName>,
    pub passes: Vec<renderer::PassBindings>,
    pub ws_hub: ws::WsHub,
    pub asset_store: crate::asset_store::AssetStore,
}

pub(super) struct AppRuntime {
    pub start: Instant,
    pub scene_rx: Receiver<ws::SceneUpdate>,
    pub capture_state_rx: Option<Receiver<bool>>,
    pub last_good: Arc<Mutex<Option<crate::dsl::SceneDSL>>>,
    pub uniform_scene: Option<crate::dsl::SceneDSL>,
    pub last_pipeline_signature: Option<[u8; 32]>,
    pub pipeline_rebuild_count: u64,
    pub uniform_only_update_count: u64,
    pub render_texture_fps_tracker: RenderTextureFpsTracker,
    pub follow_scene_resolution_for_window: bool,
    pub scene_uses_time: bool,
    pub capture_redraw_active: bool,
    pub force_continuous_redraw: bool,
    pub scene_redraw_pending: bool,
    pub animation_session: Option<crate::animation::AnimationSession>,
    /// Whether the animation state machine is actively playing.
    /// Defaults to `false`; toggled by `animation_control` WebSocket messages.
    pub animation_playing: bool,
    /// Rolling timeline buffer recording per-frame state-machine snapshots.
    /// `None` when the current scene has no state machine.
    pub timeline_buffer: Option<crate::animation::TimelineBuffer>,
    /// Snapshot of the most recent live `AnimationStep.active_overrides`.
    /// Used to restore the canvas when leaving timeline hover preview.
    pub last_live_overrides:
        Option<std::collections::HashMap<crate::state_machine::OverrideKey, serde_json::Value>>,
    /// Snapshot of uniform_scene param values captured when timeline hover
    /// begins.  Used to restore the scene when the cursor leaves the
    /// timeline, regardless of whether the animation is playing or stopped.
    pub timeline_pre_hover_overrides:
        Option<std::collections::HashMap<crate::state_machine::OverrideKey, serde_json::Value>>,
    /// Whether the timeline hover preview was active last frame.
    /// Used to detect hover-exit transitions (egui has no hover events).
    pub timeline_preview_was_active: bool,
    pub time_updates_enabled: bool,
    /// Previous frame's `time_updates_enabled`, used to detect pause→play
    /// transitions so the advance phase can force-resync uniform_scene.
    pub time_updates_enabled_prev_frame: bool,
    pub time_value_secs: f32,
    pub time_last_raw_secs: f32,
    pub latest_render_profile: Option<RenderProfile>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TestMode {
    #[default]
    Single,
    Matrix,
}

#[derive(Clone, Debug)]
pub struct ResourcePoolInfo {
    pub node_id: String,
    pub label: String,
    pub item_count: usize,
    pub item_names: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct MatrixConfig {
    pub selected_pool_ids: Vec<String>,
    /// Maximum visible columns per logical matrix row. `0` means unlimited.
    pub max_row_cols: usize,
    pub show_labels: bool,
}

impl Default for MatrixConfig {
    fn default() -> Self {
        Self {
            selected_pool_ids: Vec::new(),
            max_row_cols: 0,
            show_labels: true,
        }
    }
}

pub(super) struct AppShell {
    pub window_mode: UiWindowMode,
    pub prev_window_mode: UiWindowMode,
    pub ui_sidebar_factor: f32,
    pub timeline_visible: bool,
    pub did_startup_sidebar_size: bool,
    pub animations: AnimationManager,
    pub file_tree_state: FileTreeState,
    pub resource_snapshot: Option<ResourceSnapshot>,
    pub resource_tree_nodes: Vec<FileTreeNode>,
    pub resource_snapshot_generation: u64,
    pub resource_snapshot_broadcast_generation: u64,
    pub pass_debug_sources: std::collections::HashMap<String, renderer::PassDebugSource>,
    pub pass_debug_sources_revision: u64,
    pub pass_debug_windows: crate::ui::pass_debug_window::PassDebugWindowMap,
    pub pass_shader_overrides: std::collections::HashMap<String, String>,
    pub pending_shortwire_diff_capture:
        Option<crate::ui::pass_debug_window::ShortwireDiffCaptureRequest>,
    pub debug_artifacts: crate::debug_artifacts::DebugArtifactStore,
    pub nforge_path: Option<PathBuf>,
    pub test_mode: TestMode,
    pub matrix_config: MatrixConfig,
    pub resource_pools: Vec<ResourcePoolInfo>,
    pub matrix_state: super::matrix_render::MatrixRenderState,
    pub android_reference: crate::android_reference::AndroidReferenceState,
}

#[derive(Default)]
pub(super) struct InteractionBridgeState {
    pub interaction_event_seq: u64,
    pub pressed_mouse_buttons: HashSet<String>,
    pub last_synced_animation_state_id: Option<String>,
    pub cached_state_local_times: Vec<(String, f64)>,
    pub cached_override_values: Vec<(String, String)>,
}

pub struct App {
    pub(super) core: AppCore,
    pub(super) runtime: AppRuntime,
    pub(super) shell: AppShell,
    pub(super) interaction_bridge: InteractionBridgeState,
    pub(super) canvas: CanvasState,
}

pub(super) fn scene_uses_time(scene: &crate::dsl::SceneDSL) -> bool {
    scene.nodes.iter().any(|node| {
        matches!(node.node_type.as_str(), "TimeInput" | "Time")
            || (node.node_type == "ShaderMaterial"
                && crate::renderer::node_compiler::shader_material::node_uses_time(node))
    })
}

pub(super) fn extract_resource_pools(scene: &crate::dsl::SceneDSL) -> Vec<ResourcePoolInfo> {
    let mut pools = Vec::new();
    let mut seen_origins = std::collections::HashSet::new();

    for node in &scene.nodes {
        if node.node_type != "ResourcePool" {
            continue;
        }
        let origin_id = node
            .params
            .get("__dedup_original_id")
            .and_then(|v| v.as_str())
            .unwrap_or(&node.id);
        if !seen_origins.insert(origin_id.to_owned()) {
            continue;
        }

        let items: Vec<&crate::dsl::NodePort> = node
            .inputs
            .iter()
            .filter(|p| p.id != "selectedIndex")
            .collect();
        let item_count = items.len();
        let item_names: Vec<String> = items
            .iter()
            .enumerate()
            .map(|(i, p)| {
                p.name
                    .as_deref()
                    .filter(|n| !n.is_empty())
                    .unwrap_or(&format!("{}", i))
                    .to_owned()
            })
            .collect();
        let label = node
            .params
            .get("label")
            .and_then(|v| v.as_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| node.id.clone());
        pools.push(ResourcePoolInfo {
            node_id: node.id.clone(),
            label,
            item_count,
            item_names,
        });
    }

    pools
}

pub(super) fn scene_reference_image_path(scene: &crate::dsl::SceneDSL) -> Option<String> {
    scene
        .nodes
        .iter()
        .find(|node| node.node_type.as_str() == "ReferenceImage")
        .and_then(|node| node.params.get("path"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn scene_reference_image_data_url(scene: &crate::dsl::SceneDSL) -> Option<String> {
    scene
        .nodes
        .iter()
        .find(|node| node.node_type.as_str() == "ReferenceImage")
        .and_then(|node| {
            node.params
                .get("dataUrl")
                .or_else(|| node.params.get("dataurl"))
        })
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|data_url| !data_url.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn scene_reference_image_asset_id(scene: &crate::dsl::SceneDSL) -> Option<String> {
    scene
        .nodes
        .iter()
        .find(|node| node.node_type.as_str() == "ReferenceImage")
        .and_then(|node| node.params.get("assetId"))
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

pub(super) fn scene_reference_image_source(scene: &crate::dsl::SceneDSL) -> Option<String> {
    scene
        .nodes
        .iter()
        .find(|node| node.node_type.as_str() == "ReferenceImage")
        .and_then(|node| {
            node.params
                .get("source")
                .or_else(|| node.params.get("sourceType"))
        })
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .map(|source| source.to_ascii_lowercase())
}

pub(super) fn scene_reference_image_alpha_mode(
    scene: &crate::dsl::SceneDSL,
) -> Option<RefImageAlphaMode> {
    let mode = scene
        .nodes
        .iter()
        .find(|node| node.node_type.as_str() == "ReferenceImage")
        .and_then(|node| {
            node.params
                .get("alphaMode")
                .or_else(|| node.params.get("alpha_mode"))
        })
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|mode| !mode.is_empty())?
        .to_ascii_lowercase();

    match mode.as_str() {
        "premultiplied" | "premul" | "pre" => Some(RefImageAlphaMode::Premultiplied),
        "straight" | "str" => Some(RefImageAlphaMode::Straight),
        _ => None,
    }
}

fn hash_key<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

pub(super) fn scene_reference_desired_source(
    scene: &crate::dsl::SceneDSL,
) -> Option<ReferenceDesiredSource> {
    let alpha_mode = scene_reference_image_alpha_mode(scene).unwrap_or_default();
    if matches!(
        scene_reference_image_source(scene).as_deref(),
        Some("scrcpyusb" | "scrcpy_usb" | "scrcpy-usb" | "android")
    ) {
        return Some(ReferenceDesiredSource::AndroidScrcpyUsb { alpha_mode });
    }
    if let Some(asset_id) = scene_reference_image_asset_id(scene) {
        return Some(ReferenceDesiredSource::SceneAsset {
            asset_id,
            alpha_mode,
        });
    }
    if let Some(original_data_url) = scene_reference_image_data_url(scene) {
        return Some(ReferenceDesiredSource::SceneDataUrl {
            data_hash: hash_key(original_data_url.as_str()),
            original_data_url,
            alpha_mode,
        });
    }
    scene_reference_image_path(scene)
        .map(|path| ReferenceDesiredSource::ScenePath { path, alpha_mode })
}

impl App {
    pub fn from_init(init: AppInit) -> Self {
        let initial_scene_uses_time = init.uniform_scene.as_ref().is_some_and(scene_uses_time);
        let initial_scene_reference_desired = init
            .uniform_scene
            .as_ref()
            .and_then(scene_reference_desired_source);
        let initial_scene_reference_image_alpha_mode = init
            .uniform_scene
            .as_ref()
            .and_then(scene_reference_image_alpha_mode);
        let mut debug_artifacts = init.debug_artifacts;
        if debug_artifacts.is_empty()
            && let Some(scene) = init.uniform_scene.as_ref()
        {
            let _ = debug_artifacts.sync_manifest(scene.debug_artifacts.clone());
        }
        Self {
            core: AppCore {
                shader_space: init.shader_space,
                resolution: init.resolution,
                window_resolution: init.window_resolution,
                output_texture_name: init.output_texture_name,
                scene_output_texture_name: init.scene_output_texture_name,
                export_texture_name: init.export_texture_name,
                export_encode_pass_name: init.export_encode_pass_name,
                passes: init.passes,
                ws_hub: init.ws_hub,
                asset_store: init.asset_store,
            },
            runtime: AppRuntime {
                start: init.start,
                scene_rx: init.scene_rx,
                capture_state_rx: init.capture_state_rx,
                last_good: init.last_good,
                uniform_scene: init.uniform_scene,
                last_pipeline_signature: init.last_pipeline_signature,
                pipeline_rebuild_count: 0,
                uniform_only_update_count: 0,
                render_texture_fps_tracker: RenderTextureFpsTracker::default(),
                follow_scene_resolution_for_window: init.follow_scene_resolution_for_window,
                scene_uses_time: initial_scene_uses_time,
                capture_redraw_active: false,
                force_continuous_redraw: init.force_continuous_redraw,
                scene_redraw_pending: true,
                animation_session: init.animation_session,
                animation_playing: false,
                timeline_buffer: None,
                last_live_overrides: None,
                timeline_pre_hover_overrides: None,
                timeline_preview_was_active: false,
                time_updates_enabled: true,
                time_updates_enabled_prev_frame: true,
                time_value_secs: 0.0,
                time_last_raw_secs: 0.0,
                latest_render_profile: None,
            },
            shell: AppShell {
                window_mode: UiWindowMode::Sidebar,
                prev_window_mode: UiWindowMode::Sidebar,
                ui_sidebar_factor: 1.0,
                timeline_visible: true,
                did_startup_sidebar_size: false,
                animations: AnimationManager::default(),
                file_tree_state: FileTreeState::default(),
                resource_snapshot: None,
                resource_tree_nodes: Vec::new(),
                resource_snapshot_generation: u64::MAX,
                resource_snapshot_broadcast_generation: u64::MAX,
                pass_debug_sources: init.pass_debug_sources,
                pass_debug_sources_revision: 0,
                pass_debug_windows: crate::ui::pass_debug_window::PassDebugWindowMap::default(),
                pass_shader_overrides: std::collections::HashMap::new(),
                pending_shortwire_diff_capture: None,
                debug_artifacts,
                nforge_path: init.nforge_path,
                test_mode: TestMode::default(),
                matrix_config: MatrixConfig::default(),
                resource_pools: Vec::new(),
                matrix_state: super::matrix_render::MatrixRenderState::default(),
                android_reference: crate::android_reference::AndroidReferenceState::default(),
            },
            interaction_bridge: InteractionBridgeState::default(),
            canvas: CanvasState::new(
                initial_scene_reference_desired,
                initial_scene_reference_image_alpha_mode,
            ),
        }
    }

    pub(super) fn persist_debug_artifacts_to_source_nforge(&mut self) {
        let Some(nforge_path) = self.shell.nforge_path.clone() else {
            return;
        };
        let manifest = self.shell.debug_artifacts.export_manifest();

        let mut scene_for_archive = None;
        if let Ok(mut guard) = self.runtime.last_good.lock()
            && let Some(scene) = guard.as_mut()
        {
            scene.debug_artifacts = manifest.clone();
            scene_for_archive = Some(scene.clone());
        }
        if scene_for_archive.is_none()
            && let Some(scene) = self.runtime.uniform_scene.as_mut()
        {
            scene.debug_artifacts = manifest.clone();
            scene_for_archive = Some(scene.clone());
        }

        let Some(scene) = scene_for_archive else {
            eprintln!(
                "[debug-artifacts] skipped .nforge persistence; no current scene for {}",
                nforge_path.display()
            );
            return;
        };

        if let Err(error) = crate::asset_store::save_debug_artifacts_to_nforge(
            nforge_path.as_path(),
            &scene,
            &self.shell.debug_artifacts,
        ) {
            eprintln!(
                "[debug-artifacts] failed to persist {}: {error:#}",
                nforge_path.display()
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::{AnalysisTab, ClippingSettings, RenderTextureFpsTracker};
    use crate::dsl::{Metadata, Node, SceneDSL};

    #[test]
    fn analysis_tab_only_contains_infographics_labels() {
        assert_eq!(AnalysisTab::Histogram.label(), "Histogram");
        assert_eq!(AnalysisTab::Parade.label(), "Parade");
        assert_eq!(AnalysisTab::Vectorscope.label(), "Vectorscope");
    }

    #[test]
    fn clipping_settings_defaults_are_in_expected_ranges() {
        let settings = ClippingSettings::default();
        assert!((0.0..=0.25).contains(&settings.shadow_threshold));
        assert!((0.75..=1.0).contains(&settings.highlight_threshold));
    }

    #[test]
    fn render_texture_fps_counts_scene_redraws_within_last_second() {
        let mut tracker = RenderTextureFpsTracker::default();
        tracker.record_scene_redraw(0.0);
        tracker.record_scene_redraw(0.2);
        tracker.record_scene_redraw(0.9);
        assert_eq!(tracker.fps_at(0.9), 3);
    }

    #[test]
    fn render_texture_fps_prunes_scene_redraws_older_than_window() {
        let mut tracker = RenderTextureFpsTracker::default();
        tracker.record_scene_redraw(0.0);
        tracker.record_scene_redraw(0.3);
        tracker.record_scene_redraw(0.6);
        tracker.record_scene_redraw(1.61);
        assert_eq!(tracker.fps_at(1.61), 1);
    }

    #[test]
    fn render_texture_fps_decays_without_new_scene_redraws() {
        let mut tracker = RenderTextureFpsTracker::default();
        tracker.record_scene_redraw(0.0);
        tracker.record_scene_redraw(0.2);
        assert_eq!(tracker.fps_at(0.2), 2);
        assert_eq!(tracker.fps_at(1.25), 0);
    }

    #[test]
    fn render_texture_fps_keeps_exactly_one_second_boundary() {
        let mut tracker = RenderTextureFpsTracker::default();
        tracker.record_scene_redraw(0.0);
        tracker.record_scene_redraw(1.0);
        assert_eq!(tracker.fps_at(1.0), 2);

        tracker.record_scene_redraw(1.000_001);
        assert_eq!(tracker.fps_at(1.000_001), 2);
    }

    fn scene_with_node_types(node_types: &[&str]) -> SceneDSL {
        SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "scene".to_string(),
                created: None,
                modified: None,
            },
            nodes: node_types
                .iter()
                .enumerate()
                .map(|(idx, node_type)| Node {
                    id: format!("node_{idx}"),
                    node_type: (*node_type).to_string(),
                    params: HashMap::new(),
                    inputs: Vec::new(),
                    outputs: Vec::new(),
                    input_bindings: Vec::new(),
                    wgsl_override: None,
                })
                .collect(),
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        }
    }

    #[test]
    fn scene_uses_time_returns_true_for_time_input_node() {
        let scene = scene_with_node_types(&["TimeInput"]);
        assert!(super::scene_uses_time(&scene));
    }

    #[test]
    fn scene_uses_time_returns_true_for_time_node() {
        let scene = scene_with_node_types(&["Time"]);
        assert!(super::scene_uses_time(&scene));
    }

    #[test]
    fn scene_uses_time_returns_false_when_time_nodes_absent() {
        let scene = scene_with_node_types(&["FloatInput", "ColorInput"]);
        assert!(!super::scene_uses_time(&scene));
    }

    #[test]
    fn scene_uses_time_detects_shader_material_system_time() {
        let path =
            std::env::temp_dir().join(format!("node-forge-scene-time-{}.wgsl", std::process::id()));
        std::fs::write(
            &path,
            r#"
fn shader_material(in: ShaderMaterialInput) -> vec4f {
    return vec4f(in.uv, sin(in.time), 1.0);
}
"#,
        )
        .unwrap();
        let mut scene = scene_with_node_types(&["ShaderMaterial"]);
        scene.nodes[0].wgsl_override = Some(path.to_string_lossy().to_string());

        assert!(super::scene_uses_time(&scene));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scene_reference_alpha_mode_accepts_supported_values_case_insensitively() {
        let mut scene = scene_with_node_types(&["ReferenceImage"]);
        scene.nodes[0]
            .params
            .insert("alphaMode".to_string(), serde_json::json!("PrEmUlTiPlIeD"));
        assert_eq!(
            super::scene_reference_image_alpha_mode(&scene),
            Some(super::RefImageAlphaMode::Premultiplied)
        );

        scene.nodes[0]
            .params
            .insert("alphaMode".to_string(), serde_json::json!("straight"));
        assert_eq!(
            super::scene_reference_image_alpha_mode(&scene),
            Some(super::RefImageAlphaMode::Straight)
        );
    }

    #[test]
    fn scene_reference_alpha_mode_returns_none_for_missing_or_invalid_value() {
        let scene = scene_with_node_types(&["ReferenceImage"]);
        assert_eq!(super::scene_reference_image_alpha_mode(&scene), None);

        let mut scene = scene_with_node_types(&["ReferenceImage"]);
        scene.nodes[0]
            .params
            .insert("alphaMode".to_string(), serde_json::json!("unknown"));
        assert_eq!(super::scene_reference_image_alpha_mode(&scene), None);
    }

    #[test]
    fn scene_reference_source_accepts_scrcpy_usb() {
        let mut scene = scene_with_node_types(&["ReferenceImage"]);
        scene.nodes[0]
            .params
            .insert("source".to_string(), serde_json::json!("scrcpyUsb"));
        scene.nodes[0]
            .params
            .insert("alphaMode".to_string(), serde_json::json!("straight"));

        assert!(matches!(
            super::scene_reference_desired_source(&scene),
            Some(super::ReferenceDesiredSource::AndroidScrcpyUsb {
                alpha_mode: super::RefImageAlphaMode::Straight
            })
        ));
    }

    #[test]
    fn extract_resource_pools_deduplicates_by_dedup_original_id() {
        use crate::dsl::NodePort;

        let scene = SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes: vec![
                Node {
                    id: "GroupInstance_70/ResourcePool_69".to_string(),
                    node_type: "ResourcePool".to_string(),
                    params: HashMap::from([
                        (
                            "__dedup_original_id".to_string(),
                            serde_json::json!("ResourcePool_69"),
                        ),
                        ("label".to_string(), serde_json::json!("Resource Pool")),
                    ]),
                    inputs: vec![
                        NodePort {
                            id: "dynamic_1".to_string(),
                            name: Some("input1".to_string()),
                            port_type: Some("color".to_string()),
                        },
                        NodePort {
                            id: "dynamic_2".to_string(),
                            name: Some("input2".to_string()),
                            port_type: Some("color".to_string()),
                        },
                    ],
                    outputs: Vec::new(),
                    input_bindings: Vec::new(),
                    wgsl_override: None,
                },
                Node {
                    id: "GroupInstance_71/ResourcePool_69".to_string(),
                    node_type: "ResourcePool".to_string(),
                    params: HashMap::from([
                        (
                            "__dedup_original_id".to_string(),
                            serde_json::json!("ResourcePool_69"),
                        ),
                        ("label".to_string(), serde_json::json!("Resource Pool")),
                    ]),
                    inputs: vec![
                        NodePort {
                            id: "dynamic_1".to_string(),
                            name: Some("input1".to_string()),
                            port_type: Some("color".to_string()),
                        },
                        NodePort {
                            id: "dynamic_2".to_string(),
                            name: Some("input2".to_string()),
                            port_type: Some("color".to_string()),
                        },
                    ],
                    outputs: Vec::new(),
                    input_bindings: Vec::new(),
                    wgsl_override: None,
                },
            ],
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: HashMap::new(),
            state_machine: None,
            debug_artifacts: None,
        };

        let pools = super::extract_resource_pools(&scene);
        assert_eq!(
            pools.len(),
            1,
            "two instances of the same group ResourcePool should count as one"
        );
        assert_eq!(pools[0].item_count, 2);
        assert_eq!(pools[0].label, "Resource Pool");
    }
}
