use std::{
    collections::VecDeque,
    sync::mpsc,
    sync::{Arc, Mutex},
    time::Instant,
};

use crossbeam_channel::Receiver;
use rust_wgpu_fiber::{
    ResourceName,
    eframe::{egui, wgpu},
    shader_space::ShaderSpace,
};

use crate::{renderer, ws};

use crate::ui::animation_manager::AnimationManager;
use crate::ui::file_tree_widget::FileTreeState;
use crate::ui::resource_tree::{FileTreeNode, ResourceSnapshot};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum RefImageMode {
    #[default]
    Overlay,
    Diff,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefImageSource {
    Manual,
    SceneNodePath(String),
    SceneNodeDataUrl(String),
    SceneNodeAssetId(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
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
    pub wgpu_texture: wgpu::Texture,
    pub wgpu_texture_view: wgpu::TextureView,
    pub size: [u32; 2],
    pub texture_format: wgpu::TextureFormat,
    pub alpha_mode: RefImageAlphaMode,
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

pub const CANVAS_RADIUS: f32 = 16.0;
pub const OUTER_MARGIN: f32 = 4.0;
pub const SIDEBAR_ANIM_SECS: f64 = crate::ui::debug_sidebar::SIDEBAR_ANIM_SECS;

pub struct AppInit {
    pub shader_space: ShaderSpace,
    pub resolution: [u32; 2],
    pub window_resolution: [u32; 2],
    pub output_texture_name: ResourceName,
    pub scene_output_texture_name: ResourceName,
    pub start: Instant,
    pub passes: Vec<renderer::PassBindings>,
    pub scene_rx: Receiver<ws::SceneUpdate>,
    pub capture_state_rx: Option<Receiver<bool>>,
    pub ws_hub: ws::WsHub,
    pub last_good: Arc<Mutex<Option<crate::dsl::SceneDSL>>>,
    pub uniform_scene: Option<crate::dsl::SceneDSL>,
    pub last_pipeline_signature: Option<[u8; 32]>,
    pub follow_scene_resolution_for_window: bool,
    pub asset_store: crate::asset_store::AssetStore,
}

pub struct App {
    pub shader_space: ShaderSpace,
    pub resolution: [u32; 2],
    pub window_resolution: [u32; 2],
    pub output_texture_name: ResourceName,
    pub scene_output_texture_name: ResourceName,
    pub color_attachment: Option<egui::TextureId>,
    pub start: Instant,
    pub passes: Vec<renderer::PassBindings>,

    pub scene_rx: Receiver<ws::SceneUpdate>,
    pub capture_state_rx: Option<Receiver<bool>>,
    pub ws_hub: ws::WsHub,
    pub last_good: Arc<Mutex<Option<crate::dsl::SceneDSL>>>,
    pub uniform_scene: Option<crate::dsl::SceneDSL>,
    pub last_pipeline_signature: Option<[u8; 32]>,
    pub pipeline_rebuild_count: u64,
    pub uniform_only_update_count: u64,
    pub render_texture_fps_tracker: RenderTextureFpsTracker,

    pub zoom: f32,
    pub zoom_initialized: bool,
    pub min_zoom: Option<f32>,
    pub pan: egui::Vec2,
    pub pan_start: Option<egui::Pos2>,
    pub pan_zoom_start_zoom: f32,
    pub pan_zoom_start_pan: egui::Vec2,
    pub pan_zoom_target_zoom: f32,
    pub pan_zoom_target_pan: egui::Vec2,
    pub canvas_center_prev: Option<egui::Pos2>,
    pub last_sampled: Option<SampledPixel>,
    pub texture_filter: wgpu::FilterMode,
    pub pixel_overlay_dirty: bool,
    pub pixel_overlay_last_request_key: Option<u64>,

    pub follow_scene_resolution_for_window: bool,

    pub window_mode: UiWindowMode,
    pub prev_window_mode: UiWindowMode,
    pub ui_sidebar_factor: f32,
    pub did_startup_sidebar_size: bool,

    pub animations: AnimationManager,

    pub file_tree_state: FileTreeState,
    pub resource_snapshot: Option<ResourceSnapshot>,
    pub resource_tree_nodes: Vec<FileTreeNode>,
    pub resource_snapshot_generation: u64,
    pub preview_texture_name: Option<rust_wgpu_fiber::ResourceName>,
    pub preview_color_attachment: Option<egui::TextureId>,
    pub histogram_renderer: Option<crate::ui::histogram::HistogramRenderer>,
    pub histogram_texture_id: Option<egui::TextureId>,
    pub parade_renderer: Option<crate::ui::parade::ParadeRenderer>,
    pub parade_texture_id: Option<egui::TextureId>,
    pub vectorscope_renderer: Option<crate::ui::vectorscope::VectorscopeRenderer>,
    pub vectorscope_texture_id: Option<egui::TextureId>,
    pub clipping_renderer: Option<crate::ui::clipping_map::ClippingMapRenderer>,
    pub clipping_texture_id: Option<egui::TextureId>,
    pub hdr_preview_clamp_enabled: bool,
    pub hdr_clamp_renderer: Option<crate::ui::hdr_clamp::HdrClampRenderer>,
    pub hdr_clamp_texture_id: Option<egui::TextureId>,
    pub analysis_tab: AnalysisTab,
    pub clip_enabled: bool,
    pub clipping_settings: ClippingSettings,
    pub analysis_dirty: bool,
    pub clipping_dirty: bool,
    pub analysis_source_is_diff: bool,
    pub analysis_source_key: Option<u64>,
    pub ref_image: Option<RefImageState>,
    pub diff_renderer: Option<crate::ui::diff_renderer::DiffRenderer>,
    pub diff_texture_id: Option<egui::TextureId>,
    pub diff_metric_mode: DiffMetricMode,
    pub diff_stats: Option<DiffStats>,
    pub diff_dirty: bool,
    pub last_diff_request_key: Option<u64>,
    pub last_diff_stats_request_key: Option<u64>,
    pub last_histogram_request_key: Option<u64>,
    pub last_parade_request_key: Option<u64>,
    pub last_vectorscope_request_key: Option<u64>,
    pub last_clipping_request_key: Option<u64>,
    pub scene_uses_time: bool,
    pub capture_redraw_active: bool,
    pub scene_redraw_pending: bool,
    pub scene_reference_image_path: Option<String>,
    pub scene_reference_image_data_url: Option<String>,
    pub scene_reference_image_asset_id: Option<String>,
    pub scene_reference_image_alpha_mode: Option<RefImageAlphaMode>,
    pub reference_alpha_mode: RefImageAlphaMode,
    pub asset_store: crate::asset_store::AssetStore,
    pub last_auto_reference_attempt: Option<String>,
    pub time_updates_enabled: bool,
    pub time_value_secs: f32,
    pub time_last_raw_secs: f32,
    pub shift_was_down: bool,
    pub pending_view_reset: bool,
    pub viewport_indicator_manager: crate::ui::viewport_indicators::ViewportIndicatorManager,
    pub viewport_operation_indicator: ViewportOperationIndicator,
    pub viewport_operation_job_rx: Option<mpsc::Receiver<(u64, bool)>>,
    pub viewport_operation_last_visual: Option<ViewportOperationIndicatorVisual>,
    pub viewport_operation_request_id: u64,
}

pub(super) fn scene_uses_time(scene: &crate::dsl::SceneDSL) -> bool {
    scene
        .nodes
        .iter()
        .any(|node| matches!(node.node_type.as_str(), "TimeInput" | "Time"))
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
impl App {
    pub fn from_init(init: AppInit) -> Self {
        let initial_scene_uses_time = init.uniform_scene.as_ref().is_some_and(scene_uses_time);
        let initial_scene_reference_image_path = init
            .uniform_scene
            .as_ref()
            .and_then(scene_reference_image_path);
        let initial_scene_reference_image_data_url = init
            .uniform_scene
            .as_ref()
            .and_then(scene_reference_image_data_url);
        let initial_scene_reference_image_asset_id = init
            .uniform_scene
            .as_ref()
            .and_then(scene_reference_image_asset_id);
        let initial_scene_reference_image_alpha_mode = init
            .uniform_scene
            .as_ref()
            .and_then(scene_reference_image_alpha_mode);
        Self {
            shader_space: init.shader_space,
            resolution: init.resolution,
            window_resolution: init.window_resolution,
            output_texture_name: init.output_texture_name,
            scene_output_texture_name: init.scene_output_texture_name,
            color_attachment: None,
            start: init.start,
            passes: init.passes,
            scene_rx: init.scene_rx,
            capture_state_rx: init.capture_state_rx,
            ws_hub: init.ws_hub,
            last_good: init.last_good,
            uniform_scene: init.uniform_scene,
            last_pipeline_signature: init.last_pipeline_signature,
            pipeline_rebuild_count: 0,
            uniform_only_update_count: 0,
            render_texture_fps_tracker: RenderTextureFpsTracker::default(),
            zoom: 1.0,
            zoom_initialized: false,
            min_zoom: None,
            pan: egui::Vec2::ZERO,
            pan_start: None,
            pan_zoom_start_zoom: 1.0,
            pan_zoom_start_pan: egui::Vec2::ZERO,
            pan_zoom_target_zoom: 1.0,
            pan_zoom_target_pan: egui::Vec2::ZERO,
            canvas_center_prev: None,
            last_sampled: None,
            texture_filter: wgpu::FilterMode::Nearest,
            pixel_overlay_dirty: true,
            pixel_overlay_last_request_key: None,
            follow_scene_resolution_for_window: init.follow_scene_resolution_for_window,
            window_mode: UiWindowMode::Sidebar,
            prev_window_mode: UiWindowMode::Sidebar,
            ui_sidebar_factor: 1.0,
            did_startup_sidebar_size: false,
            animations: AnimationManager::default(),
            file_tree_state: FileTreeState::default(),
            resource_snapshot: None,
            resource_tree_nodes: Vec::new(),
            resource_snapshot_generation: u64::MAX,
            preview_texture_name: None,
            preview_color_attachment: None,
            histogram_renderer: None,
            histogram_texture_id: None,
            parade_renderer: None,
            parade_texture_id: None,
            vectorscope_renderer: None,
            vectorscope_texture_id: None,
            clipping_renderer: None,
            clipping_texture_id: None,
            hdr_preview_clamp_enabled: false,
            hdr_clamp_renderer: None,
            hdr_clamp_texture_id: None,
            analysis_tab: AnalysisTab::Histogram,
            clip_enabled: false,
            clipping_settings: ClippingSettings::default(),
            analysis_dirty: true,
            clipping_dirty: true,
            analysis_source_is_diff: false,
            analysis_source_key: None,
            ref_image: None,
            diff_renderer: None,
            diff_texture_id: None,
            diff_metric_mode: DiffMetricMode::AE,
            diff_stats: None,
            diff_dirty: false,
            last_diff_request_key: None,
            last_diff_stats_request_key: None,
            last_histogram_request_key: None,
            last_parade_request_key: None,
            last_vectorscope_request_key: None,
            last_clipping_request_key: None,
            scene_uses_time: initial_scene_uses_time,
            capture_redraw_active: false,
            scene_redraw_pending: true,
            scene_reference_image_path: initial_scene_reference_image_path,
            scene_reference_image_data_url: initial_scene_reference_image_data_url,
            scene_reference_image_asset_id: initial_scene_reference_image_asset_id,
            scene_reference_image_alpha_mode: initial_scene_reference_image_alpha_mode,
            reference_alpha_mode: initial_scene_reference_image_alpha_mode.unwrap_or_default(),
            asset_store: init.asset_store,
            last_auto_reference_attempt: None,
            time_updates_enabled: true,
            time_value_secs: 0.0,
            time_last_raw_secs: 0.0,
            shift_was_down: false,
            pending_view_reset: false,
            viewport_indicator_manager: Default::default(),
            viewport_operation_indicator: ViewportOperationIndicator::Hidden,
            viewport_operation_job_rx: None,
            viewport_operation_last_visual: None,
            viewport_operation_request_id: 0,
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
                })
                .collect(),
            connections: Vec::new(),
            outputs: None,
            groups: Vec::new(),
            assets: HashMap::new(),
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
}
