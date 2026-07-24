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
    pub animation_playing: bool,
    pub canvas_rect: egui::Rect,
    pub image_rect: egui::Rect,
    pub display_resolution: [u32; 2],
    pub pointer_response: &'a egui::Response,
}

pub(super) fn project_local_point(
    position: [f32; 2],
    rect: egui::Rect,
    local_size: [f32; 2],
    camera: [f32; 16],
) -> egui::Pos2 {
    let clip_x = camera[0] * position[0] + camera[4] * position[1] + camera[12];
    let clip_y = camera[1] * position[0] + camera[5] * position[1] + camera[13];
    let clip_w = camera[3] * position[0] + camera[7] * position[1] + camera[15];
    if clip_w.is_finite() && clip_w.abs() > f32::EPSILON {
        let ndc_x = clip_x / clip_w;
        let ndc_y = clip_y / clip_w;
        if ndc_x.is_finite() && ndc_y.is_finite() {
            return egui::Pos2::new(
                rect.left() + (ndc_x + 1.0) * 0.5 * rect.width(),
                rect.top() + (1.0 - ndc_y) * 0.5 * rect.height(),
            );
        }
    }

    egui::Pos2::new(
        rect.left() + position[0] / local_size[0].max(1.0) * rect.width(),
        rect.bottom() - position[1] / local_size[1].max(1.0) * rect.height(),
    )
}

pub(super) fn unproject_screen_point(
    pos: egui::Pos2,
    rect: egui::Rect,
    local_size: [f32; 2],
    camera: [f32; 16],
) -> [f32; 2] {
    let ndc_x = (pos.x - rect.left()) / rect.width() * 2.0 - 1.0;
    let ndc_y = 1.0 - (pos.y - rect.top()) / rect.height() * 2.0;
    let a = camera[0] - ndc_x * camera[3];
    let b = camera[4] - ndc_x * camera[7];
    let c = camera[1] - ndc_y * camera[3];
    let d = camera[5] - ndc_y * camera[7];
    let rhs_x = ndc_x * camera[15] - camera[12];
    let rhs_y = ndc_y * camera[15] - camera[13];
    let determinant = a * d - b * c;
    if determinant.is_finite() && determinant.abs() > f32::EPSILON {
        return [
            (rhs_x * d - b * rhs_y) / determinant,
            (a * rhs_y - rhs_x * c) / determinant,
        ];
    }

    [
        (pos.x - rect.left()) / rect.width() * local_size[0].max(1.0),
        (rect.bottom() - pos.y) / rect.height() * local_size[1].max(1.0),
    ]
}
