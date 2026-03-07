use std::sync::Arc;

use rust_wgpu_fiber::{ResourceName, eframe::egui, eframe::wgpu};

use crate::{
    app::types::{
        AnalysisTab, ClippingSettings, DiffMetricMode, DiffStats, RefImageAlphaMode, RefImageState,
        SampledPixel, ViewportOperationIndicatorVisual,
    },
    ui::{self, viewport_indicators::ViewportIndicatorManager},
};

use super::{ops::ClipboardCopyState, pixel_overlay::PixelOverlayCache};

pub struct CanvasState {
    pub viewport: CanvasViewportState,
    pub display: CanvasDisplayState,
    pub analysis: CanvasAnalysisState,
    pub reference: CanvasReferenceState,
    pub interactions: CanvasInteractionState,
    pub async_ops: CanvasAsyncOps,
    pub invalidation: CanvasInvalidation,
    pub viewport_indicator_manager: ViewportIndicatorManager,
}

impl CanvasState {
    pub fn new(
        scene_desired: Option<ReferenceDesiredSource>,
        scene_alpha_mode: Option<RefImageAlphaMode>,
    ) -> Self {
        Self {
            viewport: CanvasViewportState::default(),
            display: CanvasDisplayState::default(),
            analysis: CanvasAnalysisState::default(),
            reference: CanvasReferenceState {
                scene_desired,
                scene_alpha_mode,
                alpha_mode: scene_alpha_mode.unwrap_or_default(),
                ..Default::default()
            },
            interactions: CanvasInteractionState::default(),
            async_ops: CanvasAsyncOps::default(),
            invalidation: CanvasInvalidation::default(),
            viewport_indicator_manager: ViewportIndicatorManager::default(),
        }
    }
}

#[derive(Default)]
pub struct CanvasViewportState {
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
    pub pending_view_reset: bool,
    pub last_sampled: Option<SampledPixel>,
}

pub struct CanvasDisplayState {
    pub texture_filter: wgpu::FilterMode,
    pub color_attachment: Option<egui::TextureId>,
    pub preview_texture_name: Option<ResourceName>,
    pub preview_color_attachment: Option<egui::TextureId>,
    pub hdr_preview_clamp_enabled: bool,
    pub hdr_clamp_renderer: Option<ui::hdr_clamp::HdrClampRenderer>,
    pub hdr_clamp_texture_id: Option<egui::TextureId>,
    pub deferred_texture_frees: Vec<egui::TextureId>,
    pub pixel_overlay_cache: Option<Arc<PixelOverlayCache>>,
    pub pixel_overlay_last_request_key: Option<u64>,
}

impl Default for CanvasDisplayState {
    fn default() -> Self {
        Self {
            texture_filter: wgpu::FilterMode::Nearest,
            color_attachment: None,
            preview_texture_name: None,
            preview_color_attachment: None,
            hdr_preview_clamp_enabled: false,
            hdr_clamp_renderer: None,
            hdr_clamp_texture_id: None,
            deferred_texture_frees: Vec::new(),
            pixel_overlay_cache: None,
            pixel_overlay_last_request_key: None,
        }
    }
}

#[derive(Default)]
pub struct CanvasAnalysisState {
    pub histogram_renderer: Option<ui::histogram::HistogramRenderer>,
    pub histogram_texture_id: Option<egui::TextureId>,
    pub parade_renderer: Option<ui::parade::ParadeRenderer>,
    pub parade_texture_id: Option<egui::TextureId>,
    pub vectorscope_renderer: Option<ui::vectorscope::VectorscopeRenderer>,
    pub vectorscope_texture_id: Option<egui::TextureId>,
    pub clipping_renderer: Option<ui::clipping_map::ClippingMapRenderer>,
    pub clipping_texture_id: Option<egui::TextureId>,
    pub analysis_tab: AnalysisTab,
    pub clip_enabled: bool,
    pub clipping_settings: ClippingSettings,
    pub analysis_source_is_diff: bool,
    pub analysis_source_key: Option<u64>,
    pub diff_renderer: Option<ui::diff_renderer::DiffRenderer>,
    pub diff_texture_id: Option<egui::TextureId>,
    pub diff_metric_mode: DiffMetricMode,
    pub diff_stats: Option<DiffStats>,
    pub last_diff_request_key: Option<u64>,
    pub last_diff_stats_request_key: Option<u64>,
    pub last_histogram_request_key: Option<u64>,
    pub last_parade_request_key: Option<u64>,
    pub last_vectorscope_request_key: Option<u64>,
    pub last_clipping_request_key: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReferenceDesiredSource {
    SceneAsset {
        asset_id: String,
        alpha_mode: RefImageAlphaMode,
    },
    SceneDataUrl {
        data_hash: u64,
        original_data_url: String,
        alpha_mode: RefImageAlphaMode,
    },
    ScenePath {
        path: String,
        alpha_mode: RefImageAlphaMode,
    },
    Manual,
}

impl ReferenceDesiredSource {
    pub fn alpha_mode(&self) -> Option<RefImageAlphaMode> {
        match self {
            Self::SceneAsset { alpha_mode, .. }
            | Self::SceneDataUrl { alpha_mode, .. }
            | Self::ScenePath { alpha_mode, .. } => Some(*alpha_mode),
            Self::Manual => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ReferenceAttemptKey {
    Asset {
        asset_id: String,
        alpha_mode: RefImageAlphaMode,
        asset_store_revision: u64,
    },
    DataUrl {
        data_hash: u64,
        alpha_mode: RefImageAlphaMode,
    },
    Path {
        path: String,
        alpha_mode: RefImageAlphaMode,
    },
}

#[derive(Default)]
pub struct CanvasReferenceState {
    pub ref_image: Option<RefImageState>,
    pub scene_desired: Option<ReferenceDesiredSource>,
    pub desired_override: Option<ReferenceDesiredSource>,
    pub scene_alpha_mode: Option<RefImageAlphaMode>,
    pub alpha_mode: RefImageAlphaMode,
    pub last_attempt_key: Option<ReferenceAttemptKey>,
}

#[derive(Default)]
pub struct CanvasInteractionState {
    pub canvas_event_focus_latched: bool,
    pub interaction_event_seq: u64,
    pub last_synced_animation_state_id: Option<String>,
    pub cached_state_local_times: Vec<(String, f64)>,
    pub cached_transition_blend: Option<f64>,
    pub cached_override_values: Vec<(String, String)>,
    pub last_canvas_rect: Option<egui::Rect>,
}

#[derive(Default)]
pub struct CanvasAsyncOps {
    pub clipboard_copy: ClipboardCopyState,
    pub last_visual: Option<ViewportOperationIndicatorVisual>,
    pub next_request_id: u64,
}

pub struct CanvasInvalidation {
    diff_dirty: bool,
    analysis_dirty: bool,
    clipping_dirty: bool,
    pixel_overlay_dirty: bool,
}

impl Default for CanvasInvalidation {
    fn default() -> Self {
        Self {
            diff_dirty: false,
            analysis_dirty: true,
            clipping_dirty: true,
            pixel_overlay_dirty: true,
        }
    }
}

impl CanvasInvalidation {
    pub fn diff_dirty(&self) -> bool {
        self.diff_dirty
    }

    pub fn analysis_dirty(&self) -> bool {
        self.analysis_dirty
    }

    pub fn clipping_dirty(&self) -> bool {
        self.clipping_dirty
    }

    pub fn pixel_overlay_dirty(&self) -> bool {
        self.pixel_overlay_dirty
    }

    pub fn clear_diff(&mut self) {
        self.diff_dirty = false;
    }

    pub fn clear_analysis(&mut self) {
        self.analysis_dirty = false;
    }

    pub fn clear_clipping(&mut self) {
        self.clipping_dirty = false;
    }

    pub fn clear_pixel_overlay(&mut self) {
        self.pixel_overlay_dirty = false;
    }

    pub fn mark_diff_dirty(&mut self) {
        self.diff_dirty = true;
    }

    pub fn mark_analysis_dirty(&mut self) {
        self.analysis_dirty = true;
    }

    pub fn mark_clipping_dirty(&mut self) {
        self.clipping_dirty = true;
    }

    pub fn mark_pixel_overlay_dirty(&mut self) {
        self.pixel_overlay_dirty = true;
    }

    pub fn scene_redraw_changed(&mut self, has_reference_diff: bool) {
        if has_reference_diff {
            self.diff_dirty = true;
        }
        self.analysis_dirty = true;
        self.clipping_dirty = true;
        self.pixel_overlay_dirty = true;
    }

    pub fn preview_source_changed(&mut self) {
        self.diff_dirty = true;
        self.analysis_dirty = true;
        self.clipping_dirty = true;
        self.pixel_overlay_dirty = true;
    }

    pub fn reference_pixels_changed(&mut self, reference_mode: crate::app::types::RefImageMode) {
        self.diff_dirty = true;
        if matches!(reference_mode, crate::app::types::RefImageMode::Diff) {
            self.analysis_dirty = true;
            self.clipping_dirty = true;
        }
        self.pixel_overlay_dirty = true;
    }

    pub fn reference_removed(&mut self) {
        self.diff_dirty = false;
        self.analysis_dirty = true;
        self.clipping_dirty = true;
        self.pixel_overlay_dirty = true;
    }

    pub fn reference_mode_changed(&mut self) {
        self.diff_dirty = true;
        self.analysis_dirty = true;
        self.clipping_dirty = true;
        self.pixel_overlay_dirty = true;
    }

    pub fn analysis_controls_changed(&mut self) {
        self.diff_dirty = true;
        self.analysis_dirty = true;
        self.clipping_dirty = true;
    }

    pub fn clipping_controls_changed(&mut self) {
        self.clipping_dirty = true;
    }

    pub fn analysis_tab_changed(&mut self) {
        self.analysis_dirty = true;
        self.clipping_dirty = true;
    }

    pub fn time_pause_toggled(&mut self, scene_uses_time: bool, has_reference_diff: bool) {
        if !scene_uses_time {
            return;
        }
        if has_reference_diff {
            self.diff_dirty = true;
        }
        self.analysis_dirty = true;
        self.clipping_dirty = true;
        self.pixel_overlay_dirty = true;
    }
}
