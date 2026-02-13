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
const ANALYSIS_PANEL_ASPECT: f32 = 400.0 / 768.0;

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

fn shadcn_tabs_trigger(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    // Match dependencies-tree row typography tokens:
    // - regular row: 12px, weight 400, gray(190)
    // - emphasized row: 12px, weight 600, gray(220)
    let (text_color, font_id) = if selected {
        (
            egui::Color32::from_gray(220),
            egui::FontId::new(
                12.0,
                crate::ui::typography::mi_sans_family_for_weight(600.0),
            ),
        )
    } else {
        (
            egui::Color32::from_gray(190),
            egui::FontId::new(
                12.0,
                crate::ui::typography::mi_sans_family_for_weight(400.0),
            ),
        )
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_id, text_color);
    let desired_size = egui::vec2(galley.size().x + 16.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);

    if ui.is_rect_visible(rect) {
        let fill = if selected {
            egui::Color32::from_rgb(46, 46, 50)
        } else {
            egui::Color32::TRANSPARENT
        };
        let stroke = if selected {
            egui::Stroke::new(
                1.0,
                egui::Color32::from_rgba_unmultiplied(255, 255, 255, 26),
            )
        } else {
            egui::Stroke::NONE
        };

        ui.painter().rect(
            rect,
            egui::CornerRadius::same(6),
            fill,
            stroke,
            egui::StrokeKind::Inside,
        );
        ui.painter().galley(
            egui::pos2(
                rect.center().x - galley.size().x * 0.5,
                rect.center().y - galley.size().y * 0.5 - 0.25,
            ),
            galley,
            egui::Color32::PLACEHOLDER,
        );
    }

    response
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
                            let clipping_active = matches!(analysis.tab, AnalysisTab::Clipping);
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new("Reference + Clipping")
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(170)),
                                );
                            });
                            ui.add_space(4.0);

                            if let Some(reference) = reference {
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
                            } else {
                                ui.horizontal(|ui| {
                                    ui.add_space(4.0);
                                    ui.label(
                                        egui::RichText::new(
                                            "Drop a reference image to enable diff controls.",
                                        )
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(130)),
                                    );
                                });
                            }

                            ui.horizontal_wrapped(|ui| {
                                ui.add_space(2.0);
                                let clipping_button =
                                    egui::Button::new(AnalysisTab::Clipping.label())
                                        .selected(clipping_active);
                                if ui.add(clipping_button).clicked() {
                                    sidebar_action =
                                        Some(SidebarAction::SetAnalysisTab(AnalysisTab::Clipping));
                                }
                            });

                            let mut shadow_threshold = analysis.clipping.shadow_threshold;
                            let shadow_resp = ui.add(
                                egui::Slider::new(&mut shadow_threshold, 0.0..=0.25)
                                    .text("Shadow <= ")
                                    .clamping(egui::SliderClamping::Always),
                            );
                            if shadow_resp.changed() {
                                sidebar_action =
                                    Some(SidebarAction::SetClippingShadowThreshold(shadow_threshold));
                            }

                            let mut highlight_threshold = analysis.clipping.highlight_threshold;
                            let highlight_resp = ui.add(
                                egui::Slider::new(&mut highlight_threshold, 0.75..=1.0)
                                    .text("Highlight >= ")
                                    .clamping(egui::SliderClamping::Always),
                            );
                            if highlight_resp.changed() {
                                sidebar_action = Some(SidebarAction::SetClippingHighlightThreshold(
                                    highlight_threshold,
                                ));
                            }

                            if clipping_active {
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
                            }

                            tight_divider(ui);

                            let scopes_title = format!(
                                "InfoGraphics{}",
                                if analysis.source_is_diff {
                                    " (Diff Source)"
                                } else {
                                    ""
                                }
                            );
                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                ui.label(
                                    egui::RichText::new(scopes_title)
                                        .size(11.0)
                                        .color(egui::Color32::from_gray(170)),
                                );
                            });
                            // Match the same title -> first-item rhythm as the 资源树 section.
                            ui.add_space(4.0);

                            ui.horizontal(|ui| {
                                ui.add_space(4.0);
                                let tabs = [
                                    AnalysisTab::Histogram,
                                    AnalysisTab::Parade,
                                    AnalysisTab::Vectorscope,
                                ];
                                let trigger_gap = 2.0;
                                let container_inner_margin = 2.0;
                                let tabs_content_width: f32 = tabs
                                    .iter()
                                    .map(|tab| {
                                        let selected = analysis.tab == *tab;
                                        let (_, font_id) = if selected {
                                            (
                                                egui::Color32::from_gray(220),
                                                egui::FontId::new(
                                                    12.0,
                                                    crate::ui::typography::mi_sans_family_for_weight(
                                                        600.0,
                                                    ),
                                                ),
                                            )
                                        } else {
                                            (
                                                egui::Color32::from_gray(190),
                                                egui::FontId::new(
                                                    12.0,
                                                    crate::ui::typography::mi_sans_family_for_weight(
                                                        400.0,
                                                    ),
                                                ),
                                            )
                                        };
                                        let galley = ui.painter().layout_no_wrap(
                                            tab.label().to_owned(),
                                            font_id,
                                            egui::Color32::PLACEHOLDER,
                                        );
                                        galley.size().x + 16.0
                                    })
                                    .sum::<f32>()
                                    + trigger_gap * (tabs.len().saturating_sub(1) as f32);
                                let tabs_width = (tabs_content_width + container_inner_margin * 2.0)
                                    .min(ui.available_width());
                                ui.scope(|ui| {
                                    ui.set_width(tabs_width);
                                    egui::Frame::new()
                                        .fill(egui::Color32::from_rgb(39, 39, 42))
                                        .corner_radius(egui::CornerRadius::same(4))
                                        .inner_margin(egui::Margin::same(
                                            container_inner_margin as i8,
                                        ))
                                        .show(ui, |ui| {
                                            egui::ScrollArea::horizontal()
                                                .id_salt(
                                                    "ui.debug_sidebar.infographics_tabs.scroll",
                                                )
                                                .auto_shrink([false, true])
                                                .show(ui, |ui| {
                                                    ui.with_layout(
                                                        egui::Layout::left_to_right(
                                                            egui::Align::Center,
                                                        ),
                                                        |ui| {
                                                            ui.spacing_mut().item_spacing.x =
                                                                trigger_gap;
                                                            for tab in tabs {
                                                                let selected = analysis.tab == tab;
                                                                if shadcn_tabs_trigger(
                                                                    ui,
                                                                    tab.label(),
                                                                    selected,
                                                                )
                                                                .clicked()
                                                                    && !selected
                                                                {
                                                                    sidebar_action = Some(
                                                                        SidebarAction::SetAnalysisTab(tab),
                                                                    );
                                                                }
                                                            }
                                                        },
                                                    );
                                                });
                                        });
                                });
                            });

                            let selected_texture_id = match analysis.tab {
                                AnalysisTab::Histogram => histogram_texture_id,
                                AnalysisTab::Parade => parade_texture_id,
                                AnalysisTab::Vectorscope => vectorscope_texture_id,
                                AnalysisTab::Clipping => None,
                            };

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
                                    let panel_size =
                                        egui::vec2(width, width * ANALYSIS_PANEL_ASPECT);
                                    ui.allocate_ui_with_layout(
                                        panel_size,
                                        egui::Layout::centered_and_justified(
                                            egui::Direction::LeftToRight,
                                        ),
                                        |ui| {
                                            if let Some(texture_id) = selected_texture_id {
                                                let image_size = if matches!(
                                                    analysis.tab,
                                                    AnalysisTab::Vectorscope
                                                ) {
                                                    let side = panel_size.x.min(panel_size.y);
                                                    egui::vec2(side, side)
                                                } else {
                                                    panel_size
                                                };
                                                let image = egui::Image::new(
                                                    egui::load::SizedTexture::new(
                                                        texture_id,
                                                        image_size,
                                                    ),
                                                )
                                                .corner_radius(egui::CornerRadius::same(4));
                                                ui.add_sized(image_size, image);
                                            } else {
                                                let empty_text = if clipping_active {
                                                    "Clipping active: pick an InfoGraphics tab to view scopes"
                                                } else {
                                                    "No analysis data"
                                                };
                                                ui.label(
                                                    egui::RichText::new(empty_text)
                                                        .size(11.0)
                                                        .color(egui::Color32::from_gray(130)),
                                                );
                                            }
                                        },
                                    );
                                });

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
