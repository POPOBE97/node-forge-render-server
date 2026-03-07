use rust_wgpu_fiber::{ResourceName, eframe::egui};

use crate::app::{
    frame::commands::AppCommand,
    types::{AnalysisTab, DiffMetricMode},
};

#[derive(Clone, Debug)]
pub enum CanvasAction {
    SetPreviewTexture(ResourceName),
    ClearPreviewTexture,
    ToggleHdrClamp,
    TogglePause,
    ResetView,
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
