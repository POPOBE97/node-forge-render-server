use rust_wgpu_fiber::eframe::egui;
use std::cell::RefCell;
use std::hash::Hash;

use crate::android_reference::AndroidReferenceStatus;
use crate::app::{
    AnalysisTab, ClippingSettings, DiffMetricMode, DiffStats, QualifierChannel, QualifierSettings,
    RefImageMode, ResourcePoolInfo, TestMode, display_metrics,
};

use super::button::{
    self, ButtonGroupPosition, ButtonOptions, ButtonSize, ButtonVariant, ButtonVisualOverride,
};
use super::components::number_slider;
use super::components::radio_button_group::{self, RadioButtonOption};
use super::components::two_column_section;
use super::design_tokens::{self, TextRole};
use super::file_tree_widget::FileTreeState;
use super::resource_tree::{FileTreeNode, NodeKind, PassDesignTarget};

pub const SIDEBAR_WIDTH: f32 = 340.0;
pub const SIDEBAR_MIN_WIDTH: f32 = 260.0;
/// Maximum sidebar width: 2/3 of the available window width.
fn sidebar_max_width(ctx: &egui::Context) -> f32 {
    let screen_w = ctx.content_rect().width();
    (screen_w * 2.0 / 3.0).max(SIDEBAR_MIN_WIDTH)
}
pub const SIDEBAR_ANIM_SECS: f64 = 0.25;

const SIDEBAR_RESIZE_HANDLE_W: f32 = 8.0;
const SIDEBAR_RESIZE_CONTENT_GUTTER_W: f32 = SIDEBAR_RESIZE_HANDLE_W + 1.0;
const SIDEBAR_DIVIDER_COLOR: egui::Color32 = egui::Color32::from_gray(32);
const ANALYSIS_PANEL_ASPECT: f32 = 400.0 / 768.0;
const SIDEBAR_GRID_COLUMNS: usize = 4;
const SIDEBAR_GRID_GAP: f32 = 8.0;
const SIDEBAR_GRID_LABEL_GAP: f32 = 4.0;
const SIDEBAR_GRID_ROW_GAP: f32 = 8.0;
const SIDEBAR_CONTENT_SIDE_PADDING: i8 = 16;

const SECTION_TOP_PADDING: f32 = 4.0;
const SECTION_BOTTOM_PADDING: f32 = 8.0;

fn tight_divider(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    ui.painter().line_segment(
        [
            egui::pos2(rect.min.x, rect.center().y),
            egui::pos2(rect.max.x, rect.center().y),
        ],
        egui::Stroke::new(design_tokens::LINE_THICKNESS_1, design_tokens::white(10)),
    );
}

fn section_divider(ui: &mut egui::Ui) {
    // first section's buttom padding -> divider -> next section's top padding -> content
    ui.add_space(SECTION_BOTTOM_PADDING);
    tight_divider(ui);
    ui.add_space(SECTION_TOP_PADDING);
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
    number_slider::slider_with_value(
        ui,
        id_source,
        value,
        min,
        max,
        number_slider::NumberSliderConfig::new(sidebar_background_color()).formatter(formatter),
    )
}

fn slider_with_editable_value(
    ui: &mut egui::Ui,
    id_source: impl Hash + Clone,
    value: &mut f32,
    min: f32,
    max: f32,
    step: f32,
    formatter: Option<&dyn Fn(f32) -> String>,
) -> bool {
    number_slider::slider_with_editable_value(
        ui,
        id_source,
        value,
        min,
        max,
        step,
        number_slider::NumberSliderConfig::new(sidebar_background_color()).formatter(formatter),
    )
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
    /// Open a render pass shader debug window.
    OpenPassDebug(String),
    /// Open a pass-specific design window.
    OpenPassDesign(PassDesignTarget),
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
    /// Start Android USB mirroring as a live reference source.
    StartAndroidReferenceUsb,
    /// Stop Android USB reference mirroring.
    StopAndroidReference,
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
    /// Enable/disable qualifier overlay.
    SetQualifierEnabled(bool),
    /// Set the qualifier range for a single channel.
    SetQualifierRange {
        channel: QualifierChannel,
        min: f32,
        max: f32,
    },
    /// Switch test mode (Single / Matrix).
    SetTestMode(TestMode),
    /// Toggle a resource pool's selection in matrix mode.
    ToggleMatrixPool(String),
    /// Set the maximum visible columns per matrix row. `0` disables wrapping.
    SetMatrixMaxRowCols(usize),
    /// Show/hide matrix row and column labels.
    SetMatrixLabelsVisible(bool),
    /// Set the target display PPI used for physical-size preview.
    SetDisplayPpi(f32),
}

/// Hover state from the timeline panel.
#[derive(Clone, Debug)]
pub struct TimelineHover {
    /// Index of the hovered frame in the timeline buffer.
    pub frame_index: usize,
}

/// Structured result returned from the sidebar each frame.
#[derive(Clone, Debug, Default)]
pub struct SidebarResult {
    /// Persistent action (e.g. tab switch, preview texture, reference ops).
    pub action: Option<SidebarAction>,
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
    pub qualifier: QualifierSettings,
    pub qualifier_enabled: bool,
}

pub struct TestModeSidebarState<'a> {
    pub mode: TestMode,
    pub resource_pools: &'a [ResourcePoolInfo],
    pub selected_pool_ids: &'a [String],
    pub max_row_cols: usize,
    pub show_labels: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct DisplaySidebarState {
    pub ppi: f32,
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
    display: DisplaySidebarState,
    android_reference: AndroidReferenceStatus,
    reference: Option<&ReferenceSidebarState>,
    test_mode_state: TestModeSidebarState<'_>,
    tree_nodes: &[FileTreeNode],
    file_tree_state: &mut FileTreeState,
) -> SidebarResult {
    if ui_sidebar_factor <= 0.0 {
        return SidebarResult::default();
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

        let mut content_rect = ui.available_rect_before_wrap();
        if can_resize {
            // Keep the scroll area and its scrollbar out of the resize handle strip.
            content_rect.max.x =
                (content_rect.max.x - SIDEBAR_RESIZE_CONTENT_GUTTER_W).max(content_rect.min.x);
        }
        ui.scope_builder(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            ui.set_clip_rect(content_rect);
            if ui_sidebar_factor > 0.01 {
                egui::Frame::NONE
                    .inner_margin(egui::Margin {
                        left: 0,
                        right: 0,
                        top: SECTION_TOP_PADDING as i8,
                        bottom: SECTION_BOTTOM_PADDING as i8,
                    })
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            with_sidebar_content_padding(ui, |ui| {
                                show_test_mode_section(ui, &test_mode_state, &mut sidebar_action);
                            });
                            section_divider(ui);
                            with_sidebar_content_padding(ui, |ui| {
                                show_display_section(ui, display, &mut sidebar_action);
                            });
                            section_divider(ui);
                            with_sidebar_content_padding(ui, |ui| {
                                show_ref_section(
                                    ui,
                                    reference,
                                    &android_reference,
                                    &mut sidebar_action,
                                );
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

    SidebarResult {
        action: sidebar_action,
    }
}

fn show_display_section(
    ui: &mut egui::Ui,
    display: DisplaySidebarState,
    sidebar_action: &mut Option<SidebarAction>,
) {
    two_column_section::section(ui, "Display", |ui| {
        sidebar_grid_row(ui, |row| {
            row.place(1, 4, |ui| {
                sidebar_group_cell(ui, "PPI", |ui| {
                    let mut ppi = display.ppi;
                    let formatter = |v: f32| format!("{:.0}", v);
                    let changed = slider_with_editable_value(
                        ui,
                        "ui.debug_sidebar.display.ppi",
                        &mut ppi,
                        display_metrics::MIN_DISPLAY_PPI,
                        display_metrics::MAX_DISPLAY_PPI,
                        1.0,
                        Some(&formatter),
                    );
                    if changed {
                        *sidebar_action = Some(SidebarAction::SetDisplayPpi(ppi));
                    }
                });
            });
        });
    });
}

fn show_ref_section(
    ui: &mut egui::Ui,
    reference: Option<&ReferenceSidebarState>,
    android_reference: &AndroidReferenceStatus,
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
            sidebar_grid_row(ui, |row| {
                row.place(1, 2, |ui| {
                    sidebar_group_cell(ui, "Source", |ui| {
                        let label = if android_reference.running {
                            "Stop USB"
                        } else {
                            "USB"
                        };
                        let tooltip = if android_reference.running {
                            "Stop Android USB reference"
                        } else {
                            "Use Android USB reference"
                        };
                        let response = button::button(
                            ui,
                            ButtonOptions {
                                label,
                                tooltip: Some(tooltip),
                                variant: if android_reference.running {
                                    ButtonVariant::Outline
                                } else {
                                    ButtonVariant::Ghost
                                },
                                size: ButtonSize::Small,
                                enabled: true,
                                icon: None,
                                icon_kind: None,
                                visual_override: None,
                                group_position: ButtonGroupPosition::Single,
                            },
                        );
                        if response.clicked() {
                            *row_action.borrow_mut() = Some(if android_reference.running {
                                SidebarAction::StopAndroidReference
                            } else {
                                SidebarAction::StartAndroidReferenceUsb
                            });
                        }
                    });
                });
                row.place(3, 2, |ui| {
                    sidebar_group_cell(ui, "Status", |ui| {
                        let status = if let Some(size) = android_reference.size {
                            format!(
                                "{}x{} · {:.0} fps · {} frames",
                                size[0],
                                size[1],
                                android_reference.fps,
                                android_reference.frame_count
                            )
                        } else if let Some(error) = android_reference.last_error.as_ref() {
                            error.clone()
                        } else {
                            android_reference.label.clone()
                        };
                        ui.label(design_tokens::rich_text(
                            status.as_str(),
                            if android_reference.running {
                                TextRole::ActiveItemTitle
                            } else {
                                TextRole::InactiveItemTitle
                            },
                        ));
                    });
                });
            });
            ui.add_space(SIDEBAR_GRID_ROW_GAP);
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

        {
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
                                        let image_size = if matches!(tab, AnalysisTab::Vectorscope)
                                        {
                                            let side = panel_size.x.min(panel_size.y);
                                            egui::vec2(side, side)
                                        } else {
                                            panel_size
                                        };
                                        let image = egui::Image::new(
                                            egui::load::SizedTexture::new(texture_id, image_size),
                                        )
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
        } // end analysis texture block
    });
}

fn test_mode_options() -> [RadioButtonOption<'static, TestMode>; 2] {
    [
        RadioButtonOption {
            value: TestMode::Single,
            label: "Single",
        },
        RadioButtonOption {
            value: TestMode::Matrix,
            label: "Matrix",
        },
    ]
}

fn selected_matrix_col_count(state: &TestModeSidebarState<'_>) -> usize {
    state
        .selected_pool_ids
        .get(1)
        .or_else(|| state.selected_pool_ids.first())
        .and_then(|id| state.resource_pools.iter().find(|p| p.node_id == *id))
        .map(|pool| pool.item_count)
        .unwrap_or(0)
}

fn numbered_checkbox(ui: &mut egui::Ui, order: Option<usize>, label: &str) -> egui::Response {
    let spacing = ui.spacing().icon_spacing;
    let icon_width = ui.spacing().icon_width;
    let total_extra = icon_width + spacing;
    let text_style = design_tokens::text_style(TextRole::ActiveItemTitle);
    let font = design_tokens::font_id(text_style.size, text_style.weight);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, text_style.color);
    let desired_size = egui::vec2(
        total_extra + galley.size().x,
        icon_width.max(galley.size().y),
    );
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());

    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let box_size = icon_width * 0.8;
        let box_rect = egui::Rect::from_center_size(
            egui::pos2(rect.min.x + icon_width * 0.5, rect.center().y),
            egui::vec2(box_size, box_size),
        );
        ui.painter().rect(
            box_rect,
            egui::CornerRadius::same(2),
            if order.is_some() {
                visuals.bg_fill
            } else {
                egui::Color32::TRANSPARENT
            },
            visuals.bg_stroke,
            egui::StrokeKind::Outside,
        );
        if let Some(idx) = order {
            let num = format!("{}", idx + 1);
            ui.painter().text(
                box_rect.center(),
                egui::Align2::CENTER_CENTER,
                num,
                egui::FontId::new(box_size * 0.75, egui::FontFamily::Monospace),
                visuals.text_color(),
            );
        }
        let text_pos = egui::pos2(
            rect.min.x + total_extra,
            rect.center().y - galley.size().y * 0.5,
        );
        ui.painter()
            .galley(text_pos, galley, egui::Color32::PLACEHOLDER);
    }

    response
}

fn show_test_mode_section(
    ui: &mut egui::Ui,
    state: &TestModeSidebarState<'_>,
    sidebar_action: &mut Option<SidebarAction>,
) {
    two_column_section::section(ui, "Test Mode", |ui| {
        sidebar_grid_row(ui, |row| {
            row.place(1, 4, |ui| {
                sidebar_group_cell(ui, "Mode", |ui| {
                    let mut mode = state.mode;
                    if radio_button_group::radio_button_group(
                        ui,
                        "ui.debug_sidebar.test_mode.mode",
                        &mut mode,
                        &test_mode_options(),
                    ) && mode != state.mode
                    {
                        *sidebar_action = Some(SidebarAction::SetTestMode(mode));
                    }
                });
            });
        });

        if state.mode == TestMode::Matrix && !state.resource_pools.is_empty() {
            let selected_col_count = selected_matrix_col_count(state);
            ui.add_space(SIDEBAR_GRID_ROW_GAP);
            sidebar_grid_row(ui, |row| {
                row.place(1, 3, |ui| {
                    sidebar_group_cell(ui, "Max Cols", |ui| {
                        let can_wrap = selected_col_count > 1;
                        ui.add_enabled_ui(can_wrap, |ui| {
                            let max_slider_value = selected_col_count.saturating_sub(1).max(1);
                            let visible_max_cols = if state.max_row_cols == 0
                                || state.max_row_cols >= selected_col_count
                            {
                                0
                            } else {
                                state.max_row_cols
                            };
                            let mut max_cols = visible_max_cols as f32;
                            let formatter = |v: f32| {
                                let rounded = v.round() as usize;
                                if rounded == 0 {
                                    "Off".to_string()
                                } else {
                                    format!("{rounded}")
                                }
                            };
                            let changed = slider_with_editable_value(
                                ui,
                                "ui.debug_sidebar.matrix.max_cols",
                                &mut max_cols,
                                0.0,
                                max_slider_value as f32,
                                1.0,
                                Some(&formatter),
                            );
                            if changed {
                                let next = max_cols.round() as usize;
                                let next = if next == 0 || next >= selected_col_count {
                                    0
                                } else {
                                    next
                                };
                                *sidebar_action = Some(SidebarAction::SetMatrixMaxRowCols(next));
                            }
                        });
                    });
                });
                row.place(4, 1, |ui| {
                    sidebar_group_cell(ui, "Labels", |ui| {
                        let (tooltip, variant, icon, visual_override) = if state.show_labels {
                            (
                                "Hide matrix labels",
                                ButtonVariant::Outline,
                                button::ButtonIcon::Eye,
                                Some(ButtonVisualOverride {
                                    bg: design_tokens::indicator_success_bg(),
                                    hover_bg: design_tokens::indicator_success_bg(),
                                    active_bg: design_tokens::indicator_success_bg(),
                                    text: design_tokens::indicator_success_fg(),
                                    border: design_tokens::indicator_success_border(),
                                }),
                            )
                        } else {
                            (
                                "Show matrix labels",
                                ButtonVariant::Ghost,
                                button::ButtonIcon::EyeOff,
                                None,
                            )
                        };
                        let response = button::button(
                            ui,
                            ButtonOptions {
                                label: "",
                                tooltip: Some(tooltip),
                                variant,
                                size: ButtonSize::Default,
                                enabled: true,
                                icon: None,
                                icon_kind: Some(icon),
                                visual_override,
                                group_position: button::ButtonGroupPosition::Single,
                            },
                        );
                        if response.clicked() {
                            *sidebar_action =
                                Some(SidebarAction::SetMatrixLabelsVisible(!state.show_labels));
                        }
                    });
                });
            });

            ui.add_space(SIDEBAR_GRID_ROW_GAP);
            sidebar_grid_label(ui, "Pools");
            ui.add_space(SIDEBAR_GRID_LABEL_GAP);

            let num_selected = state.selected_pool_ids.len();
            for pool in state.resource_pools {
                let order_index = state
                    .selected_pool_ids
                    .iter()
                    .position(|id| id == &pool.node_id);
                let is_selected = order_index.is_some();
                let can_toggle = is_selected || num_selected < 2;

                ui.add_enabled_ui(can_toggle, |ui| {
                    let label = format!("{} ({} items)", pool.label, pool.item_count);
                    if numbered_checkbox(ui, order_index, &label).clicked() {
                        *sidebar_action =
                            Some(SidebarAction::ToggleMatrixPool(pool.node_id.clone()));
                    }
                });
            }
        } else if state.mode == TestMode::Matrix && state.resource_pools.is_empty() {
            ui.add_space(SIDEBAR_GRID_ROW_GAP);
            ui.label(design_tokens::rich_text(
                "No resource pools in scene",
                TextRole::InactiveItemTitle,
            ));
        }
    });
}

fn show_resource_tree_section(
    ui: &mut egui::Ui,
    tree_nodes: &[FileTreeNode],
    file_tree_state: &mut FileTreeState,
    sidebar_action: &mut Option<SidebarAction>,
) {
    two_column_section::section(ui, "Resource Tree", |ui| {
        let tree_response = egui::ScrollArea::horizontal()
            .id_salt("ui.debug_sidebar.resource_tree.scroll_x")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                super::file_tree_widget::show_file_tree(ui, tree_nodes, file_tree_state)
            })
            .inner;

        if let Some(texture_name) = tree_response.copied_texture_name.as_ref() {
            ui.ctx().copy_text(texture_name.clone());
        }

        if let Some(pass_name) = tree_response.open_pass_debug {
            *sidebar_action = Some(SidebarAction::OpenPassDebug(pass_name));
            return;
        }

        if let Some(target) = tree_response.open_pass_design {
            *sidebar_action = Some(SidebarAction::OpenPassDesign(target));
            return;
        }

        if let Some(clicked) = tree_response.clicked {
            match &clicked.kind {
                NodeKind::Texture { texture_name } => {
                    *sidebar_action = Some(SidebarAction::PreviewTexture(texture_name.clone()));
                }
                _ => {}
            }
        }
    });
}
