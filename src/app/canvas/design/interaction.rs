use rust_wgpu_fiber::eframe::egui;

use crate::{
    dsl::SceneDSL, protocol::DesignParamPatchPayload, ui::resource_tree::ResourceSnapshot,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DesignOverlayStatus {
    Active,
    Stale(String),
}

impl Default for DesignOverlayStatus {
    fn default() -> Self {
        Self::Active
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DesignInteractionClaims {
    pub primary_pointer: bool,
    pub suppress_pixel_sampling: bool,
    pub suppress_reference_drag: bool,
    pub suppress_analysis_overlays: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DesignOverlayOutput {
    pub patches: Vec<DesignParamPatchPayload>,
    pub claims: DesignInteractionClaims,
    pub status: DesignOverlayStatus,
}

pub struct DesignOverlayInput<'a> {
    pub scene: Option<&'a SceneDSL>,
    pub resource_snapshot: Option<&'a ResourceSnapshot>,
    pub editor_connected: bool,
    pub canvas_rect: egui::Rect,
    pub image_rect: egui::Rect,
    pub display_resolution: [u32; 2],
    pub pointer_response: &'a egui::Response,
}
