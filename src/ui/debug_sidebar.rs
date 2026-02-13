use rust_wgpu_fiber::eframe::egui;
use std::cell::RefCell;

use crate::app::{AnalysisTab, ClippingSettings, DiffMetricMode, DiffStats, RefImageMode};

use super::components::radio_button_group::{self, RadioButtonOption};
use super::components::two_column_section;
use super::components::value_slider;
use super::design_tokens::{self, TextRole};
use super::file_tree_widget::FileTreeState;
use super::resource_tree::{FileTreeNode, NodeKind};

pub const SIDEBAR_WIDTH: f32 = 340.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 260.0;
/// Maximum sidebar width: 2/3 of the available window width.
fn sidebar_max_width(ctx: &egui::Context) -> f32 {
    let screen_w = ctx.content_rect().width();
    (screen_w * 2.0 / 3.0).max(SIDEBAR_MIN_WIDTH)
}
pub const SIDEBAR_ANIM_SECS: f64 = 0.25;

const SIDEBAR_RESIZE_HANDLE_W: f32 = 8.0;
const SIDEBAR_DIVIDER_COLOR: egui::Color32 = egui::Color32::from_gray(32);
const ANALYSIS_PANEL_ASPECT: f32 = 400.0 / 768.0;

fn tight_divider(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.min.x, rect.center().y),
            egui::pos2(rect.max.x, rect.center().y),
        ],
        egui::Stroke::new(design_tokens::LINE_THICKNESS_1, design_tokens::white(20)),
    );
}

fn value_label(ui: &mut egui::Ui, value: impl Into<String>) {
    ui.label(design_tokens::rich_text(value, TextRole::ValueLabel));
}

fn mode_options() -> [RadioButtonOption<'static, RefImageMode>; 2] {
    [
        RadioButtonOption {
            value: RefImageMode::Overlay,
            label: "Over",
        },
        RadioButtonOption {
            value: RefImageMode::Diff,
            label: "Diff",
        },
    ]
}

fn diff_metric_options() -> [RadioButtonOption<'static, DiffMetricMode>; 5] {
    [
        RadioButtonOption {
            value: DiffMetricMode::E,
            label: "E",
        },
        RadioButtonOption {
            value: DiffMetricMode::AE,
            label: "AE",
        },
        RadioButtonOption {
            value: DiffMetricMode::SE,
            label: "SE",
        },
        RadioButtonOption {
            value: DiffMetricMode::RAE,
            label: "RAE",
        },
        RadioButtonOption {
            value: DiffMetricMode::RSE,
            label: "RSE",
        },
    ]
}

fn analysis_tab_options() -> [RadioButtonOption<'static, AnalysisTab>; 3] {
    [
        RadioButtonOption {
            value: AnalysisTab::Histogram,
            label: "Histogram",
        },
        RadioButtonOption {
            value: AnalysisTab::Parade,
            label: "Parade",
        },
        RadioButtonOption {
            value: AnalysisTab::Vectorscope,
            label: "Vectorscope",
        },
    ]
}

fn bool_options() -> [RadioButtonOption<'static, bool>; 2] {
    [
        RadioButtonOption {
            value: false,
            label: "Off",
        },
        RadioButtonOption {
            value: true,
            label: "On",
        },
    ]
}

fn sidebar_width_id() -> egui::Id {
    egui::Id::new("ui.debug_sidebar.width")
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
    /// Enable/disable clipping overlay.
    SetClipEnabled(bool),
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
    pub clip_enabled: bool,
}

pub fn show_in_rect(
    ctx: &egui::Context,
    ui: &mut egui::Ui,
    ui_sidebar_factor: f32,
    animation_just_finished_opening: bool,
    clip_rect: egui::Rect,
    sidebar_rect: egui::Rect,
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
        if response.dragged() {
            let current_w = sidebar_width(ctx);
            let next = (current_w + response.drag_delta().x)
                .clamp(SIDEBAR_MIN_WIDTH, sidebar_max_width(ctx));
            ctx.memory_mut(|mem| {
                mem.data.insert_temp(sidebar_width_id(), next);
            });
        }

        ui.painter().line_segment(
            [
                egui::pos2(sidebar_rect.max.x - 0.5, sidebar_rect.min.y),
                egui::pos2(sidebar_rect.max.x - 0.5, sidebar_rect.max.y),
            ],
            egui::Stroke::new(design_tokens::LINE_THICKNESS_1, SIDEBAR_DIVIDER_COLOR),
        );
    }

    ui.scope_builder(egui::UiBuilder::new().max_rect(sidebar_rect), |ui| {
        ui.set_clip_rect(clip_rect);
        ui.painter()
            .rect_filled(clip_rect, egui::CornerRadius::ZERO, sidebar_bg);

        let content_rect = ui.available_rect_before_wrap();
        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            ui.set_clip_rect(content_rect);
            if ui_sidebar_factor > 0.01 {
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: 4,
                        right: 8,
                        top: 6,
                        bottom: 6,
                    })
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            show_ref_section(ui, reference, &mut sidebar_action);
                            tight_divider(ui);
                            show_clip_section(ui, analysis, &mut sidebar_action);
                            tight_divider(ui);
                            show_infographics_section(
                                ui,
                                analysis.tab,
                                histogram_texture_id,
                                parade_texture_id,
                                vectorscope_texture_id,
                                &mut sidebar_action,
                            );
                            tight_divider(ui);
                            show_resource_tree_section(
                                ui,
                                tree_nodes,
                                file_tree_state,
                                &mut sidebar_action,
                            );
                        });
                    });
            }
        });
    });

    sidebar_action
}

fn show_ref_section(
    ui: &mut egui::Ui,
    reference: Option<&ReferenceSidebarState>,
    sidebar_action: &mut Option<SidebarAction>,
) {
    two_column_section::section(ui, "Ref", |ui| {
        if let Some(reference) = reference {
            let row_action = RefCell::new(None);
            two_column_section::row(
                ui,
                |ui| {
                    two_column_section::cell(ui, "Mode", |ui| {
                        let mut mode = reference.mode;
                        if radio_button_group::radio_button_group(
                            ui,
                            "ui.debug_sidebar.ref.mode",
                            &mut mode,
                            &mode_options(),
                        ) && mode != reference.mode
                        {
                            *row_action.borrow_mut() = Some(SidebarAction::ToggleReferenceMode);
                        }
                    });
                },
                |ui| match reference.mode {
                    RefImageMode::Overlay => {
                        two_column_section::cell(ui, "Mix", |ui| {
                            let mut opacity = reference.opacity;
                            let slider = value_slider::value_slider(
                                ui,
                                "ui.debug_sidebar.ref.opacity",
                                &mut opacity,
                                0.0,
                                1.0,
                                Some(&|v| format!("{:.0}%", v * 100.0)),
                            );
                            if slider.changed {
                                *row_action.borrow_mut() =
                                    Some(SidebarAction::SetReferenceOpacity(opacity));
                            }
                        });
                    }
                    RefImageMode::Diff => {
                        two_column_section::cell(ui, "Metrice", |ui| {
                            let mut metric = reference.diff_metric_mode;
                            if radio_button_group::radio_button_group(
                                ui,
                                "ui.debug_sidebar.ref.metric",
                                &mut metric,
                                &diff_metric_options(),
                            ) && metric != reference.diff_metric_mode
                            {
                                *row_action.borrow_mut() =
                                    Some(SidebarAction::SetDiffMetricMode(metric));
                            }
                        });
                    }
                },
            );
            if let Some(action) = row_action.into_inner() {
                *sidebar_action = Some(action);
            }

            ui.add_space(6.0);
            let row_action = RefCell::new(None);
            two_column_section::row(
                ui,
                |ui| {
                    two_column_section::cell(ui, "Quick", |ui| {
                        ui.horizontal(|ui| {
                            if quick_value_button(ui, "1", (reference.opacity - 0.0).abs() < 1e-6)
                                .clicked()
                            {
                                *row_action.borrow_mut() =
                                    Some(SidebarAction::SetReferenceOpacity(0.0));
                            }
                            if quick_value_button(ui, "2", (reference.opacity - 1.0).abs() < 1e-6)
                                .clicked()
                            {
                                *row_action.borrow_mut() =
                                    Some(SidebarAction::SetReferenceOpacity(1.0));
                            }
                        });
                    });
                },
                |ui| {
                    two_column_section::cell(ui, "Action", |ui| {
                        if ui
                            .add(
                                egui::Button::new(design_tokens::rich_text(
                                    "Clear",
                                    TextRole::ValueLabel,
                                ))
                                .fill(design_tokens::white(15))
                                .corner_radius(design_tokens::radius(4))
                                .stroke(egui::Stroke::NONE),
                            )
                            .clicked()
                        {
                            *row_action.borrow_mut() = Some(SidebarAction::ClearReference);
                        }
                    });
                },
            );
            if let Some(action) = row_action.into_inner() {
                *sidebar_action = Some(action);
            }

            if let Some(stats) = reference.diff_stats {
                ui.add_space(6.0);
                ui.label(design_tokens::rich_text(
                    format!(
                        "min {:.4}  max {:.4}  avg {:.4}",
                        stats.min, stats.max, stats.avg
                    ),
                    TextRole::AttributeTitle,
                ));
            }

            ui.add_space(4.0);
            ui.label(design_tokens::rich_text(
                "Shift toggles Over/Diff. Keys 1/2 set opacity 0%/100%.",
                TextRole::AttributeTitle,
            ));
        } else {
            ui.label(design_tokens::rich_text(
                "Drop a reference image to enable comparison controls.",
                TextRole::InactiveItemTitle,
            ));
        }
    });

    ui.add_space(8.0);
}

fn show_clip_section(
    ui: &mut egui::Ui,
    analysis: AnalysisSidebarState,
    sidebar_action: &mut Option<SidebarAction>,
) {
    two_column_section::section(ui, "Clip", |ui| {
        two_column_section::row(
            ui,
            |ui| {
                two_column_section::cell(ui, "Enable", |ui| {
                    let mut enabled = analysis.clip_enabled;
                    if radio_button_group::radio_button_group(
                        ui,
                        "ui.debug_sidebar.clip.enable",
                        &mut enabled,
                        &bool_options(),
                    ) && enabled != analysis.clip_enabled
                    {
                        *sidebar_action = Some(SidebarAction::SetClipEnabled(enabled));
                    }
                });
            },
            |ui| {
                two_column_section::cell(ui, "State", |ui| {
                    value_label(ui, if analysis.clip_enabled { "On" } else { "Off" });
                });
            },
        );

        ui.add_space(6.0);
        let row_action = RefCell::new(None);
        two_column_section::row(
            ui,
            |ui| {
                two_column_section::cell(ui, "Shadow <=", |ui| {
                    ui.add_enabled_ui(analysis.clip_enabled, |ui| {
                        let mut shadow = analysis.clipping.shadow_threshold;
                        let slider = value_slider::value_slider(
                            ui,
                            "ui.debug_sidebar.clip.shadow",
                            &mut shadow,
                            0.0,
                            0.25,
                            Some(&|v| format!("{:.3}", v)),
                        );
                        if slider.changed {
                            *row_action.borrow_mut() =
                                Some(SidebarAction::SetClippingShadowThreshold(shadow));
                        }
                    });
                });
            },
            |ui| {
                two_column_section::cell(ui, "Highlight >=", |ui| {
                    ui.add_enabled_ui(analysis.clip_enabled, |ui| {
                        let mut highlight = analysis.clipping.highlight_threshold;
                        let slider = value_slider::value_slider(
                            ui,
                            "ui.debug_sidebar.clip.highlight",
                            &mut highlight,
                            0.75,
                            1.0,
                            Some(&|v| format!("{:.3}", v)),
                        );
                        if slider.changed {
                            *row_action.borrow_mut() =
                                Some(SidebarAction::SetClippingHighlightThreshold(highlight));
                        }
                    });
                });
            },
        );
        if let Some(action) = row_action.into_inner() {
            *sidebar_action = Some(action);
        }
    });

    ui.add_space(8.0);
}

fn show_infographics_section(
    ui: &mut egui::Ui,
    tab: AnalysisTab,
    histogram_texture_id: Option<egui::TextureId>,
    parade_texture_id: Option<egui::TextureId>,
    vectorscope_texture_id: Option<egui::TextureId>,
    sidebar_action: &mut Option<SidebarAction>,
) {
    two_column_section::section(ui, "Infographics", |ui| {
        two_column_section::row(
            ui,
            |ui| {
                two_column_section::cell(ui, "Scope", |ui| {
                    let mut selected_tab = tab;
                    if radio_button_group::radio_button_group(
                        ui,
                        "ui.debug_sidebar.infographics.tab",
                        &mut selected_tab,
                        &analysis_tab_options(),
                    ) && selected_tab != tab
                    {
                        *sidebar_action = Some(SidebarAction::SetAnalysisTab(selected_tab));
                    }
                });
            },
            |ui| {
                two_column_section::cell(ui, "View", |ui| {
                    value_label(ui, tab.label());
                });
            },
        );

        ui.add_space(6.0);
        let selected_texture_id = match tab {
            AnalysisTab::Histogram => histogram_texture_id,
            AnalysisTab::Parade => parade_texture_id,
            AnalysisTab::Vectorscope => vectorscope_texture_id,
        };

        let analysis_border_color = design_tokens::white(20);
        egui::Frame::new()
            .outer_margin(egui::Margin {
                left: 0,
                right: 0,
                top: 0,
                bottom: 0,
            })
            .stroke(egui::Stroke::new(
                design_tokens::LINE_THICKNESS_1,
                analysis_border_color,
            ))
            .corner_radius(design_tokens::radius(4))
            .show(ui, |ui| {
                let width = ui.available_width();
                let panel_size = egui::vec2(width, width * ANALYSIS_PANEL_ASPECT);
                ui.allocate_ui_with_layout(
                    panel_size,
                    egui::Layout::centered_and_justified(egui::Direction::LeftToRight),
                    |ui| {
                        if let Some(texture_id) = selected_texture_id {
                            let image_size = if matches!(tab, AnalysisTab::Vectorscope) {
                                let side = panel_size.x.min(panel_size.y);
                                egui::vec2(side, side)
                            } else {
                                panel_size
                            };
                            let image = egui::Image::new(egui::load::SizedTexture::new(
                                texture_id, image_size,
                            ))
                            .corner_radius(design_tokens::radius(4));
                            ui.add_sized(image_size, image);
                        } else {
                            ui.label(design_tokens::rich_text(
                                "No analysis data",
                                TextRole::InactiveItemTitle,
                            ));
                        }
                    },
                );
            });
    });

    ui.add_space(8.0);
}

fn show_resource_tree_section(
    ui: &mut egui::Ui,
    tree_nodes: &[FileTreeNode],
    file_tree_state: &mut FileTreeState,
    sidebar_action: &mut Option<SidebarAction>,
) {
    two_column_section::section(ui, "Resource Tree", |ui| {
        let tree_response =
            super::file_tree_widget::show_file_tree(ui, tree_nodes, file_tree_state);

        if let Some(texture_name) = tree_response.copied_texture_name.as_ref() {
            ui.ctx().copy_text(texture_name.clone());
        }

        if let Some(clicked) = tree_response.clicked {
            match &clicked.kind {
                NodeKind::Pass {
                    target_texture: Some(tex_name),
                } => {
                    *sidebar_action = Some(SidebarAction::PreviewTexture(tex_name.clone()));
                }
                _ => {
                    *sidebar_action = Some(SidebarAction::ClearPreview);
                }
            }
        }
    });
}

fn quick_value_button(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    ui.add(
        egui::Button::new(design_tokens::rich_text(
            label,
            if selected {
                TextRole::ActiveItemTitle
            } else {
                TextRole::InactiveItemTitle
            },
        ))
        .fill(if selected {
            design_tokens::black(80)
        } else {
            design_tokens::white(15)
        })
        .corner_radius(design_tokens::radius(4))
        .stroke(egui::Stroke::NONE),
    )
}
