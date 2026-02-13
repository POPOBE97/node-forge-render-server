use rust_wgpu_fiber::eframe::egui;

use crate::app::{AnalysisTab, ClippingSettings, DiffMetricMode, DiffStats, RefImageMode};

use super::file_tree_widget::FileTreeState;
use super::resource_tree::{FileTreeNode, NodeKind};

fn node_forge_icon_texture(ctx: &egui::Context) -> egui::TextureHandle {
    let id = egui::Id::new("ui.debug_sidebar.node_forge_icon.texture");
    if let Some(tex) = ctx.memory(|mem| mem.data.get_temp::<egui::TextureHandle>(id)) {
        return tex;
    }

    let bytes = include_bytes!("../../assets/icons/node-forge-icon.png");
    let image = image::load_from_memory(bytes)
        .expect("decode node-forge-icon.png")
        .to_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    let rgba = image.into_raw();
    let color_image = egui::ColorImage::from_rgba_unmultiplied(size, &rgba);
    let tex = ctx.load_texture(
        "ui.debug_sidebar.node_forge_icon",
        color_image,
        egui::TextureOptions::LINEAR,
    );

    ctx.memory_mut(|mem| {
        mem.data.insert_temp(id, tex.clone());
    });

    tex
}

pub const SIDEBAR_WIDTH: f32 = 340.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 260.0;
/// Maximum sidebar width: 2/3 of the available window width.
fn sidebar_max_width(ctx: &egui::Context) -> f32 {
    let screen_w = ctx.screen_rect().width();
    (screen_w * 2.0 / 3.0).max(SIDEBAR_MIN_WIDTH)
}
pub const SIDEBAR_ANIM_SECS: f64 = 0.25;

const SIDEBAR_RESIZE_HANDLE_W: f32 = 8.0;
const SIDEBAR_DIVIDER_COLOR: egui::Color32 = egui::Color32::from_gray(32);

fn tight_divider(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.min.x, rect.center().y),
            egui::pos2(rect.max.x, rect.center().y),
        ],
        egui::Stroke::new(1.0, egui::Color32::from_gray(48)),
    );
}

fn sidebar_width_id() -> egui::Id {
    egui::Id::new("ui.debug_sidebar.width")
}

fn sidebar_resize_start_width_id() -> egui::Id {
    egui::Id::new("ui.debug_sidebar.resize_start_width")
}

fn sidebar_resize_handle_id() -> egui::Id {
    egui::Id::new("ui.debug_sidebar.resize_handle")
}

pub fn sidebar_width(ctx: &egui::Context) -> f32 {
    ctx.memory(|mem| mem.data.get_temp::<f32>(sidebar_width_id()))
        .unwrap_or(SIDEBAR_WIDTH)
        .clamp(SIDEBAR_MIN_WIDTH, sidebar_max_width(ctx))
}

/// Action returned from the sidebar to the app.
#[derive(Clone, Debug)]
pub enum SidebarAction {
    /// User clicked a readable texture — preview it in the canvas.
    PreviewTexture(String),
    /// Clear the preview (user clicked a non-texture node).
    ClearPreview,
    /// Update reference overlay opacity.
    SetReferenceOpacity(f32),
    /// Toggle reference display mode.
    ToggleReferenceMode,
    /// Clear loaded reference image.
    ClearReference,
    /// Set current diff metric mode.
    SetDiffMetricMode(DiffMetricMode),
    /// Switch current analysis tab.
    SetAnalysisTab(AnalysisTab),
    /// Set clipping shadow threshold.
    SetClippingShadowThreshold(f32),
    /// Set clipping highlight threshold.
    SetClippingHighlightThreshold(f32),
}

#[derive(Clone, Debug)]
pub struct ReferenceSidebarState {
    pub mode: RefImageMode,
    pub opacity: f32,
    pub diff_metric_mode: DiffMetricMode,
    pub diff_stats: Option<DiffStats>,
}

#[derive(Clone, Copy, Debug)]
pub struct AnalysisSidebarState {
    pub tab: AnalysisTab,
    pub clipping: ClippingSettings,
    pub source_is_diff: bool,
}

pub fn show_in_rect(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    ui_sidebar_factor: f32,
    animation_just_finished_opening: bool,
    clip_rect: egui::Rect,
    sidebar_rect: egui::Rect,
    mut canvas_only_button: impl FnMut(&mut egui::Ui) -> bool,
    mut toggle_canvas_only: impl FnMut(),
    histogram_texture_id: Option<egui::TextureId>,
    parade_texture_id: Option<egui::TextureId>,
    vectorscope_texture_id: Option<egui::TextureId>,
    analysis: AnalysisSidebarState,
    reference: Option<&ReferenceSidebarState>,
    tree_nodes: &[FileTreeNode],
    file_tree_state: &mut FileTreeState,
) -> Option<SidebarAction> {
    if ui_sidebar_factor <= 0.0 {
        return None;
    }

    let sidebar_bg = crate::color::lab(7.78201, -0.000_014_901_2, 0.0);
    let mut sidebar_action: Option<SidebarAction> = None;

    // Only allow resize once fully open and stable; during animation we want a deterministic width.
    let can_resize = ui_sidebar_factor >= 1.0 && !animation_just_finished_opening;
    if can_resize {
        let handle_rect = egui::Rect::from_min_max(
            egui::pos2(
                sidebar_rect.max.x - SIDEBAR_RESIZE_HANDLE_W,
                sidebar_rect.min.y,
            ),
            sidebar_rect.max,
        );
        let response = ui.interact(
            handle_rect,
            sidebar_resize_handle_id(),
            egui::Sense::click_and_drag(),
        );
        let response = response.on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if response.drag_started() {
            let w = sidebar_width(ctx);
            ctx.memory_mut(|mem| {
                mem.data.insert_temp(sidebar_resize_start_width_id(), w);
            });
        }
        if response.dragged() {
            let current_w = sidebar_width(ctx);
            let next = (current_w + response.drag_delta().x)
                .clamp(SIDEBAR_MIN_WIDTH, sidebar_max_width(ctx));
            ctx.memory_mut(|mem| {
                mem.data.insert_temp(sidebar_width_id(), next);
            });
        }

        // Subtle divider to indicate draggable edge.
        ui.painter().line_segment(
            [
                egui::pos2(sidebar_rect.max.x - 0.5, sidebar_rect.min.y),
                egui::pos2(sidebar_rect.max.x - 0.5, sidebar_rect.max.y),
            ],
            egui::Stroke::new(1.0, SIDEBAR_DIVIDER_COLOR),
        );
    }

    ui.allocate_ui_at_rect(sidebar_rect, |ui| {
        ui.set_clip_rect(clip_rect);

        // Ensure the sidebar background covers the full reserved panel height,
        // even when the inner contents don't consume all vertical space.
        ui.painter()
            .rect_filled(clip_rect, egui::CornerRadius::ZERO, sidebar_bg);

        let content_rect = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            ui.set_clip_rect(content_rect);
            if ui_sidebar_factor > 0.01 {
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: 2,
                        right: 6,
                        top: 4,
                        bottom: 4,
                    })
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            if let Some(reference) = reference {
                                ui.horizontal(|ui| {
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new("Reference Controls")
                                            .size(11.0)
                                            .color(egui::Color32::from_gray(170)),
                                    );
                                });
                                ui.add_space(4.0);

                                let mut opacity = reference.opacity;
                                let opacity_resp = ui.add(
                                    egui::Slider::new(&mut opacity, 0.0..=1.0)
                                        .text("Opacity")
                                        .clamping(egui::SliderClamping::Always),
                                );
                                if opacity_resp.changed() {
                                    sidebar_action =
                                        Some(SidebarAction::SetReferenceOpacity(opacity));
                                }

                                let mode_text = match reference.mode {
                                    RefImageMode::Overlay => "Mode: Overlay",
                                    RefImageMode::Diff => "Mode: Abs Diff",
                                };
                                if ui.button(mode_text).clicked() {
                                    sidebar_action = Some(SidebarAction::ToggleReferenceMode);
                                }

                                ui.horizontal_wrapped(|ui| {
                                    ui.add_space(2.0);
                                    for metric in [
                                        DiffMetricMode::E,
                                        DiffMetricMode::AE,
                                        DiffMetricMode::SE,
                                        DiffMetricMode::RAE,
                                        DiffMetricMode::RSE,
                                    ] {
                                        let selected = reference.diff_metric_mode == metric;
                                        let button =
                                            egui::Button::new(metric.label()).selected(selected);
                                        if ui.add(button).clicked() {
                                            sidebar_action =
                                                Some(SidebarAction::SetDiffMetricMode(metric));
                                        }
                                    }
                                });

                                ui.label(
                                    egui::RichText::new("Shift: toggle Overlay / Abs Diff")
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(140)),
                                );

                                if ui.button("Clear Reference").clicked() {
                                    sidebar_action = Some(SidebarAction::ClearReference);
                                }

                                tight_divider(ui);
                            }

                            let analysis_title = format!(
                                "Analysis{}",
                                if analysis.source_is_diff {
                                    " (Diff Source)"
                                } else {
                                    ""
                                }
                            );
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(analysis_title)
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(170)),
                                );
                            });
                            ui.add_space(4.0);

                            ui.horizontal_wrapped(|ui| {
                                ui.add_space(2.0);
                                for tab in [
                                    AnalysisTab::Histogram,
                                    AnalysisTab::Parade,
                                    AnalysisTab::Vectorscope,
                                    AnalysisTab::Clipping,
                                ] {
                                    let selected = analysis.tab == tab;
                                    let button = egui::Button::new(tab.label()).selected(selected);
                                    if ui.add(button).clicked() && !selected {
                                        sidebar_action = Some(SidebarAction::SetAnalysisTab(tab));
                                    }
                                }
                            });

                            if matches!(analysis.tab, AnalysisTab::Clipping) {
                                let mut shadow_threshold = analysis.clipping.shadow_threshold;
                                let shadow_resp = ui.add(
                                    egui::Slider::new(&mut shadow_threshold, 0.0..=0.25)
                                        .text("Shadow <= ")
                                        .clamping(egui::SliderClamping::Always),
                                );
                                if shadow_resp.changed() {
                                    sidebar_action = Some(
                                        SidebarAction::SetClippingShadowThreshold(shadow_threshold),
                                    );
                                }

                                let mut highlight_threshold = analysis.clipping.highlight_threshold;
                                let highlight_resp = ui.add(
                                    egui::Slider::new(&mut highlight_threshold, 0.75..=1.0)
                                        .text("Highlight >= ")
                                        .clamping(egui::SliderClamping::Always),
                                );
                                if highlight_resp.changed() {
                                    sidebar_action =
                                        Some(SidebarAction::SetClippingHighlightThreshold(
                                            highlight_threshold,
                                        ));
                                }
                            }

                            let (selected_texture_id, image_aspect) = match analysis.tab {
                                AnalysisTab::Histogram => (histogram_texture_id, 400.0 / 768.0),
                                AnalysisTab::Parade => (parade_texture_id, 400.0 / 768.0),
                                AnalysisTab::Vectorscope => (vectorscope_texture_id, 1.0),
                                AnalysisTab::Clipping => (None, 400.0 / 768.0),
                            };

                            if matches!(analysis.tab, AnalysisTab::Clipping) {
                                ui.horizontal(|ui| {
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "Clipping 结果已直接叠加到主预览（右上角可见状态指示）",
                                        )
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(130)),
                                    );
                                });
                            } else {
                                let analysis_border_color =
                                    egui::Color32::from_rgba_unmultiplied(255, 255, 255, 26);
                                egui::Frame::new()
                                    .outer_margin(egui::Margin {
                                        left: 4,
                                        right: 0,
                                        top: 4,
                                        bottom: 4,
                                    })
                                    .stroke(egui::Stroke::new(1.0, analysis_border_color))
                                    .corner_radius(egui::CornerRadius::same(4))
                                    .show(ui, |ui| {
                                        let width = ui.available_width();
                                        let size = egui::vec2(width, width * image_aspect);
                                        if let Some(texture_id) = selected_texture_id {
                                            let image = egui::Image::new(
                                                egui::load::SizedTexture::new(texture_id, size),
                                            )
                                            .corner_radius(egui::CornerRadius::same(4));
                                            ui.add(image);
                                        } else {
                                            ui.set_min_size(size);
                                            ui.centered_and_justified(|ui| {
                                                ui.label(
                                                    egui::RichText::new("No analysis data")
                                                        .size(11.0)
                                                        .color(egui::Color32::from_gray(130)),
                                                );
                                            });
                                        }
                                    });
                            }

                            tight_divider(ui);

                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("资源树")
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(170)),
                                );
                            });
                            ui.add_space(4.0);

                            let tree_response = super::file_tree_widget::show_file_tree(
                                ui,
                                tree_nodes,
                                file_tree_state,
                            );

                            if let Some(texture_name) = tree_response.copied_texture_name.as_ref() {
                                ui.ctx().copy_text(texture_name.clone());
                            }

                            // Translate clicks into SidebarActions.
                            if let Some(clicked) = tree_response.clicked {
                                match &clicked.kind {
                                    NodeKind::Pass {
                                        target_texture: Some(tex_name),
                                    } => {
                                        sidebar_action =
                                            Some(SidebarAction::PreviewTexture(tex_name.clone()));
                                    }
                                    _ => {
                                        sidebar_action = Some(SidebarAction::ClearPreview);
                                    }
                                }
                            }
                        });
                    }); // Frame padding
            }
        });
    });

    sidebar_action
}
