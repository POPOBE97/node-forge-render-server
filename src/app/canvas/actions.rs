use rust_wgpu_fiber::{ResourceName, eframe::egui, shader_space::PassCaptureMode};

use crate::app::{
    frame::commands::AppCommand,
    types::{AnalysisTab, DiffMetricMode, QualifierChannel},
};
use crate::ui::resource_tree::PassDesignTarget;

#[derive(Clone, Debug)]
pub enum CanvasAction {
    SetPreviewTexture(ResourceName),
    SetPassCapture(String),
    SetPassCaptureMode(PassCaptureMode),
    ClearPreviewTexture,
    EnterPassDesign(PassDesignTarget),
    ExitPassDesign,
    ToggleHdrClamp,
    ToggleWireframe,
    TogglePause,
    ResetView {
        current_display_ppi: Option<f32>,
    },
    CenterAt1x {
        pixels_per_point: f32,
        current_display_ppi: Option<f32>,
    },
    SetDisplayPpi {
        ppi: f32,
        current_display_ppi: Option<f32>,
        pixels_per_point: f32,
    },
    ToggleSampling,
    ToggleReferenceAlpha,
    ToggleClipping,
    SetClipEnabled(bool),
    ResetReferenceOffset,
    SetReferenceOpacity(f32),
    ToggleReferenceMode,
    SetDiffMetricMode(DiffMetricMode),
    SetAnalysisTab(AnalysisTab),
    SetClippingShadowThreshold(f32),
    SetClippingHighlightThreshold(f32),
    #[allow(dead_code)]
    ToggleQualifier,
    SetQualifierEnabled(bool),
    SetQualifierRange {
        channel: QualifierChannel,
        min: f32,
        max: f32,
    },
    BeginPanDrag(egui::Pos2),
    UpdatePanDrag(egui::Pos2),
    EndPanDrag,
    BeginReferenceDrag(egui::Pos2),
    UpdateReferenceDrag(egui::Pos2),
    EndReferenceDrag,
    ApplyScrollPan(egui::Vec2),
    ApplyZoomAroundPointer {
        pointer_pos: egui::Pos2,
        zoom_delta: f32,
        canvas_rect: egui::Rect,
        image_size: egui::Vec2,
        effective_min_zoom: f32,
        current_display_ppi: Option<f32>,
        pixels_per_point: f32,
    },
    SamplePixel {
        x: u32,
        y: u32,
        rgba: [f32; 4],
    },
    PollClipboardOp {
        now: f64,
    },
}

#[derive(Clone, Debug, Default)]
pub struct CanvasFrameResult {
    pub commands: Vec<AppCommand>,
}
