use rust_wgpu_fiber::eframe::egui;
use std::cell::RefCell;
use std::hash::Hash;

use crate::app::{AnalysisTab, ClippingSettings, DiffMetricMode, DiffStats, RefImageMode};

use super::button::{
    self, ButtonOptions, ButtonSize, ButtonVariant, ButtonVisualOverride,
    ButtonGroupPosition,
};
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
const SIDEBAR_GRID_COLUMNS: usize = 4;
const SIDEBAR_GRID_GAP: f32 = 8.0;
const SIDEBAR_GRID_LABEL_GAP: f32 = 4.0;
const SIDEBAR_GRID_ROW_GAP: f32 = 8.0;
const SIDEBAR_SLIDER_VALUE_GAP: f32 = 0.0;
const SIDEBAR_SECTION_DIVIDER_GAP: f32 = 8.0;
const VALUE_LABEL_TEXT_PADDING_X: f32 = 4.0;
const VALUE_LABEL_DIVIDER_WIDTH: f32 = 1.0;
const SIDEBAR_CONTENT_SIDE_PADDING: i8 = 16;

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

fn section_divider(ui: &mut egui::Ui) {
    ui.add_space(SIDEBAR_SECTION_DIVIDER_GAP);
    tight_divider(ui);
    ui.add_space(SIDEBAR_SECTION_DIVIDER_GAP);
}

fn with_sidebar_content_padding(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::NONE
        .inner_margin(egui::Margin {
            left: SIDEBAR_CONTENT_SIDE_PADDING,
            right: SIDEBAR_CONTENT_SIDE_PADDING,
            top: 0,
            bottom: 0,
        })
        .show(ui, body);
}

fn fixed_value_label_width(ui: &egui::Ui) -> f32 {
    let text_style = design_tokens::text_style(TextRole::ValueLabel);
    let label_font = design_tokens::font_id(text_style.size, text_style.weight);
    let text_width = ui
        .painter()
        .layout_no_wrap("100%".to_string(), label_font, text_style.color)
        .size()
        .x
        .ceil();
    text_width + VALUE_LABEL_TEXT_PADDING_X * 2.0 + VALUE_LABEL_DIVIDER_WIDTH
}

fn right_only_radius(px: u8) -> egui::CornerRadius {
    let canonical = (px.clamp(2, 24) / 2) * 2;
    egui::CornerRadius {
        nw: 0,
        ne: canonical,
        sw: 0,
        se: canonical,
    }
}

fn sidebar_background_color() -> egui::Color32 {
    crate::color::lab(7.78201, -0.000_014_901_2, 0.0)
}

fn sidebar_group_cell(ui: &mut egui::Ui, label: &str, body: impl FnOnce(&mut egui::Ui)) {
    sidebar_grid_label(ui, label);
    ui.add_space(SIDEBAR_GRID_LABEL_GAP);
    body(ui);
}

fn sidebar_grid_label(ui: &mut egui::Ui, label: &str) {
    ui.label(design_tokens::rich_text(label, TextRole::AttributeTitle));
}

fn sidebar_grid_column_width(available_width: f32) -> f32 {
    let total_gap = SIDEBAR_GRID_GAP * (SIDEBAR_GRID_COLUMNS.saturating_sub(1) as f32);
    ((available_width - total_gap).max(0.0)) / SIDEBAR_GRID_COLUMNS as f32
}

fn sidebar_grid_span_width(column_width: f32, span: usize) -> f32 {
    let clamped_span = span.clamp(1, SIDEBAR_GRID_COLUMNS);
    column_width * clamped_span as f32 + SIDEBAR_GRID_GAP * (clamped_span.saturating_sub(1) as f32)
}

struct SidebarGridRow<'a> {
    ui: &'a mut egui::Ui,
    column_width: f32,
    next_col: usize,
}

impl SidebarGridRow<'_> {
    fn place(&mut self, col_start: usize, col_span: usize, body: impl FnOnce(&mut egui::Ui)) {
        if self.next_col > SIDEBAR_GRID_COLUMNS {
            return;
        }
        // Child UIs can mutate spacing; force the row gap before each placement.
        self.ui.spacing_mut().item_spacing.x = SIDEBAR_GRID_GAP;

        let col_start = col_start.max(self.next_col).clamp(1, SIDEBAR_GRID_COLUMNS);
        if col_start > self.next_col {
            let spacer_span = col_start - self.next_col;
            self.ui.allocate_ui_with_layout(
                egui::vec2(sidebar_grid_span_width(self.column_width, spacer_span), 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |_ui| {},
            );
        }

        let max_span = SIDEBAR_GRID_COLUMNS.saturating_sub(col_start - 1).max(1);
        let col_span = col_span.clamp(1, max_span);
        self.ui.allocate_ui_with_layout(
            egui::vec2(sidebar_grid_span_width(self.column_width, col_span), 0.0),
            egui::Layout::top_down(egui::Align::Min),
            body,
        );
        self.next_col = col_start + col_span;
    }
}

fn sidebar_grid_row(ui: &mut egui::Ui, body: impl FnOnce(&mut SidebarGridRow<'_>)) {
    let column_width = sidebar_grid_column_width(ui.available_width());
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = SIDEBAR_GRID_GAP;
        let mut row = SidebarGridRow {
            ui,
            column_width,
            next_col: 1,
        };
        body(&mut row);
    });
}

fn slider_with_value(
    ui: &mut egui::Ui,
    id_source: impl Hash,
    value: &mut f32,
    min: f32,
    max: f32,
    formatter: Option<&dyn Fn(f32) -> String>,
) -> bool {
    let mut changed = false;
    let mut formatted_value = formatter
        .map(|f| f(*value))
        .unwrap_or_else(|| format!("{:.3}", *value));
    let label_width = fixed_value_label_width(ui);
    let slider_width = (ui.available_width() - SIDEBAR_SLIDER_VALUE_GAP - label_width).max(0.0);
    let text_style = design_tokens::text_style(TextRole::ValueLabel);
    let label_font = design_tokens::font_id(text_style.size, text_style.weight);
    let sidebar_bg = sidebar_background_color();

    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), design_tokens::CONTROL_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = SIDEBAR_SLIDER_VALUE_GAP;
            ui.allocate_ui_with_layout(
                egui::vec2(slider_width, design_tokens::CONTROL_ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.allocate_ui_with_layout(
                        egui::vec2(slider_width, value_slider::VALUE_SLIDER_HEIGHT),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            let out = value_slider::value_slider(
                                ui, id_source, value, min, max, formatter,
                            );
                            changed = out.changed;
                            formatted_value = out.formatted_value;
                        },
                    );
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(label_width, design_tokens::CONTROL_ROW_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    let (label_rect, label_response) = ui.allocate_exact_size(
                        egui::vec2(label_width, value_slider::VALUE_SLIDER_HEIGHT),
                        egui::Sense::hover(),
                    );
                    let label_border_stroke = if label_response.hovered() {
                        egui::Stroke::new(
                            design_tokens::LINE_THICKNESS_05,
                            design_tokens::white(20),
                        )
                    } else {
                        egui::Stroke::NONE
                    };
                    let painter = ui.painter_at(label_rect);
                    painter.rect(
                        label_rect,
                        right_only_radius(design_tokens::BORDER_RADIUS_SMALL as u8),
                        design_tokens::RESOURCE_ACTIVE_BG,
                        label_border_stroke,
                        egui::StrokeKind::Inside,
                    );
                    painter.line_segment(
                        [
                            egui::pos2(label_rect.left(), label_rect.top()),
                            egui::pos2(label_rect.left(), label_rect.bottom()),
                        ],
                        egui::Stroke::new(VALUE_LABEL_DIVIDER_WIDTH, sidebar_bg),
                    );

                    let text_rect = egui::Rect::from_min_max(
                        egui::pos2(
                            label_rect.left()
                                + VALUE_LABEL_DIVIDER_WIDTH
                                + VALUE_LABEL_TEXT_PADDING_X,
                            label_rect.top(),
                        ),
                        egui::pos2(
                            label_rect.right() - VALUE_LABEL_TEXT_PADDING_X,
                            label_rect.bottom(),
                        ),
                    );
                    let galley = painter.layout_no_wrap(
                        formatted_value.clone(),
                        label_font.clone(),
                        text_style.color,
                    );
                    let text_pos = egui::pos2(
                        text_rect.center().x - galley.size().x * 0.5,
                        text_rect.center().y - galley.size().y * 0.5 - 0.25,
                    );
                    painter.galley(text_pos, galley, text_style.color);
                },
            );
        },
    );

    changed
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
    /// Open system picker to load/replace reference image.
    PickReferenceImage,
    /// Remove current reference image.
    RemoveReferenceImage,
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
    pub name: String,
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

    let sidebar_bg = sidebar_background_color();
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
                        left: 0,
                        right: 0,
                        top: 16,
                        bottom: 6,
                    })
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            with_sidebar_content_padding(ui, |ui| {
                                show_ref_section(ui, reference, &mut sidebar_action);
                            });
                            section_divider(ui);
                            with_sidebar_content_padding(ui, |ui| {
                                show_clip_section(ui, analysis, &mut sidebar_action);
                            });
                            section_divider(ui);
                            with_sidebar_content_padding(ui, |ui| {
                                show_infographics_section(
                                    ui,
                                    analysis.tab,
                                    histogram_texture_id,
                                    parade_texture_id,
                                    vectorscope_texture_id,
                                    &mut sidebar_action,
                                );
                            });
                            section_divider(ui);
                            with_sidebar_content_padding(ui, |ui| {
                                show_resource_tree_section(
                                    ui,
                                    tree_nodes,
                                    file_tree_state,
                                    &mut sidebar_action,
                                );
                            });
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
    let has_reference = reference.is_some();
    let reference_state = reference.cloned().unwrap_or(ReferenceSidebarState {
        name: String::new(),
        mode: RefImageMode::Overlay,
        opacity: 0.5,
        diff_metric_mode: DiffMetricMode::default(),
        diff_stats: None,
    });
    let ref_action = RefCell::new(None);
    two_column_section::section_with_header_action(
        ui,
        "Ref",
        |ui| {
            let tooltip = if has_reference {
                "Replace reference image"
            } else {
                "Load reference image"
            };

            let response = button::group_button(
                ui,
                button::GroupButtonOptions {
                    primary: ButtonOptions {
                        label: if has_reference {
                            reference_state.name.as_str()
                        } else {
                            ""
                        },
                        tooltip: Some(tooltip),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        enabled: true,
                        icon: None,
                        icon_kind: Some(button::ButtonIcon::Folder),
                        visual_override: None,
                        group_position: ButtonGroupPosition::Single,
                    },
                    secondary: has_reference.then_some(ButtonOptions {
                        label: "",
                        tooltip: Some("Remove reference image"),
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        enabled: true,
                        icon: None,
                        icon_kind: Some(button::ButtonIcon::Trash),
                        visual_override: None,
                        group_position: ButtonGroupPosition::Single,
                    }),
                    behavior: button::GroupButtonBehavior {
                        draw_group_hover_border: has_reference,
                        truncate_primary_middle: has_reference,
                    },
                },
            );

            if response.primary.clicked() {
                *ref_action.borrow_mut() = Some(SidebarAction::PickReferenceImage);
            }
            if let Some(delete_resp) = response.secondary
                && delete_resp.clicked()
            {
                *ref_action.borrow_mut() = Some(SidebarAction::RemoveReferenceImage);
            }
        },
        |ui| {
        let row_action = RefCell::new(None);
        ui.add_enabled_ui(has_reference, |ui| {
            sidebar_grid_row(ui, |row| {
                row.place(1, 2, |ui| {
                    sidebar_group_cell(ui, "Mode", |ui| {
                        let mut mode = reference_state.mode;
                        if radio_button_group::radio_button_group(
                            ui,
                            "ui.debug_sidebar.ref.mode",
                            &mut mode,
                            &mode_options(),
                        ) && mode != reference_state.mode
                        {
                            *row_action.borrow_mut() = Some(SidebarAction::ToggleReferenceMode);
                        }
                    });
                });
                match reference_state.mode {
                    RefImageMode::Overlay => {
                        row.place(3, 2, |ui| {
                            sidebar_group_cell(ui, "Mix", |ui| {
                                let mut opacity = reference_state.opacity;
                                let changed = slider_with_value(
                                    ui,
                                    "ui.debug_sidebar.ref.opacity",
                                    &mut opacity,
                                    0.0,
                                    1.0,
                                    Some(&|v| format!("{:.0}%", v * 100.0)),
                                );
                                if changed {
                                    *row_action.borrow_mut() =
                                        Some(SidebarAction::SetReferenceOpacity(opacity));
                                }
                            });
                        });
                    }
                    RefImageMode::Diff => {
                        row.place(3, 2, |ui| {
                            sidebar_group_cell(ui, "Metrice", |ui| {
                                let mut metric = reference_state.diff_metric_mode;
                                if radio_button_group::radio_button_group(
                                    ui,
                                    "ui.debug_sidebar.ref.metric",
                                    &mut metric,
                                    &diff_metric_options(),
                                ) && metric != reference_state.diff_metric_mode
                                {
                                    *row_action.borrow_mut() =
                                        Some(SidebarAction::SetDiffMetricMode(metric));
                                }
                            });
                        });
                    }
                }
            });
        });
        if let Some(action) = row_action.into_inner() {
            *ref_action.borrow_mut() = Some(action);
        }
    },
    );
    if let Some(action) = ref_action.into_inner() {
        *sidebar_action = Some(action);
    }
}

fn show_clip_section(
    ui: &mut egui::Ui,
    analysis: AnalysisSidebarState,
    sidebar_action: &mut Option<SidebarAction>,
) {
    let clip_action = RefCell::new(None);
    two_column_section::section_with_header_action(
        ui,
        "Clip",
        |ui| {
            let (tooltip, variant, visual_override) = if analysis.clip_enabled {
                (
                    "Disable clip",
                    ButtonVariant::Outline,
                    Some(ButtonVisualOverride {
                        bg: design_tokens::indicator_success_bg(),
                        hover_bg: design_tokens::indicator_success_bg(),
                        active_bg: design_tokens::indicator_success_bg(),
                        text: design_tokens::indicator_success_fg(),
                        border: design_tokens::indicator_success_border(),
                    }),
                )
            } else {
                ("Enable clip", ButtonVariant::Ghost, None)
            };
            let response = button::button(
                ui,
                ButtonOptions {
                    label: "",
                    tooltip: Some(tooltip),
                    variant,
                    size: ButtonSize::Small,
                    enabled: true,
                    icon: None,
                    icon_kind: Some(button::ButtonIcon::Eye),
                    visual_override,
                    group_position: button::ButtonGroupPosition::Single,
                },
            );
            if response.clicked() {
                *clip_action.borrow_mut() =
                    Some(SidebarAction::SetClipEnabled(!analysis.clip_enabled));
            }
        },
        |ui| {
            let row_action = RefCell::new(None);
            ui.add_enabled_ui(analysis.clip_enabled, |ui| {
                sidebar_grid_row(ui, |row| {
                    row.place(1, 2, |ui| {
                        sidebar_group_cell(ui, "Shadow", |ui| {
                            let mut shadow = analysis.clipping.shadow_threshold;
                            let changed = slider_with_value(
                                ui,
                                "ui.debug_sidebar.clip.shadow",
                                &mut shadow,
                                0.0,
                                0.25,
                                Some(&|v| format!("{:.0}%", v * 100.0)),
                            );
                            if changed {
                                *row_action.borrow_mut() =
                                    Some(SidebarAction::SetClippingShadowThreshold(shadow));
                            }
                        });
                    });
                    row.place(3, 2, |ui| {
                        sidebar_group_cell(ui, "Highlight", |ui| {
                            let mut highlight = analysis.clipping.highlight_threshold;
                            let changed = slider_with_value(
                                ui,
                                "ui.debug_sidebar.clip.highlight",
                                &mut highlight,
                                0.75,
                                1.0,
                                Some(&|v| format!("{:.0}%", v * 100.0)),
                            );
                            if changed {
                                *row_action.borrow_mut() =
                                    Some(SidebarAction::SetClippingHighlightThreshold(highlight));
                            }
                        });
                    });
                });
            });
            if let Some(action) = row_action.into_inner() {
                *clip_action.borrow_mut() = Some(action);
            }
        },
    );
    if let Some(action) = clip_action.into_inner() {
        *sidebar_action = Some(action);
    }
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
        sidebar_grid_row(ui, |row| {
            row.place(1, 4, |ui| {
                sidebar_group_cell(ui, "Scope", |ui| {
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
            });
        });

        ui.add_space(SIDEBAR_GRID_ROW_GAP);
        let selected_texture_id = match tab {
            AnalysisTab::Histogram => histogram_texture_id,
            AnalysisTab::Parade => parade_texture_id,
            AnalysisTab::Vectorscope => vectorscope_texture_id,
        };

        sidebar_grid_row(ui, |row| {
            row.place(1, 4, |ui| {
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
                    .corner_radius(design_tokens::radius(
                        design_tokens::BORDER_RADIUS_SMALL as u8,
                    ))
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
                                    .corner_radius(design_tokens::radius(
                                        design_tokens::BORDER_RADIUS_SMALL as u8,
                                    ));
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
        });
    });
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
