use rust_wgpu_fiber::eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

use crate::ui::{
    components::{
        number_slider,
        radio_button_group::{RadioButtonOption, radio_button_group},
    },
    design_tokens::{self, FontWeight, TextRole},
};

const POPUP_WIDTH: f32 = 216.0;
const CONTENT_WIDTH: f32 = 200.0;
const PICKER_HEIGHT: f32 = 150.0;
const STRIP_HEIGHT: f32 = 14.0;
const POPOVER_GAP: f32 = 6.0;
const POPOVER_PADDING: i8 = 6;
const ROW_LABEL_WIDTH: f32 = 20.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorSpace {
    Rgb,
    Hsv,
    Hsl,
    Lab,
    Oklch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorInputFormat {
    Hex,
    Int,
}

#[derive(Clone, Debug)]
pub struct ColorPopoverState {
    pub space: ColorSpace,
    pub input_format: ColorInputFormat,
    cached_hue: f32,
    hex_buffer: String,
    last_color: Option<[u8; 4]>,
}

impl Default for ColorPopoverState {
    fn default() -> Self {
        Self {
            space: ColorSpace::Hsv,
            input_format: ColorInputFormat::Hex,
            cached_hue: 0.0,
            hex_buffer: "#ffffff".to_string(),
            last_color: None,
        }
    }
}

impl ColorPopoverState {
    pub fn sync_from_color(&mut self, color: Color32) {
        let key = color.to_srgba_unmultiplied();
        if self.last_color == Some(key) {
            return;
        }
        self.set_color_cache(color);
    }

    fn set_color_cache(&mut self, color: Color32) {
        let [r, g, b] = color_to_rgb01(color);
        let (h, s, _) = rgb_to_hsv(r, g, b);
        if s > 0.01 {
            self.cached_hue = h;
        }
        self.hex_buffer = color_to_hex(color);
        self.last_color = Some(color.to_srgba_unmultiplied());
    }
}

#[derive(Clone, Debug)]
pub struct ColorPopoverConfig<'a> {
    pub title: Option<&'a str>,
    pub allow_alpha: bool,
}

impl Default for ColorPopoverConfig<'_> {
    fn default() -> Self {
        Self {
            title: None,
            allow_alpha: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ColorPopoverResponse {
    pub changed: bool,
    pub close_requested: bool,
}

pub fn show_color_popover(
    ctx: &egui::Context,
    id: egui::Id,
    anchor_rect: Rect,
    state: &mut ColorPopoverState,
    color: &mut Color32,
    config: ColorPopoverConfig<'_>,
) -> ColorPopoverResponse {
    state.sync_from_color(*color);

    let mut changed = false;
    let popup_size = popup_size(state, &config);
    let popup_pos = popup_position(anchor_rect, ctx.content_rect(), popup_size);

    let area_output = egui::Area::new(id.with("color-popover-area"))
        .order(egui::Order::Foreground)
        .fixed_pos(popup_pos)
        .show(ctx, |ui| {
            egui::Frame::new()
                .fill(popover_bg())
                .stroke(egui::Stroke::new(
                    design_tokens::LINE_THICKNESS_1,
                    design_tokens::black(60),
                ))
                .corner_radius(design_tokens::radius(
                    design_tokens::BORDER_RADIUS_SMALL as u8,
                ))
                .inner_margin(egui::Margin::same(POPOVER_PADDING))
                .show(ui, |ui| {
                    ui.set_width(CONTENT_WIDTH);
                    ui.spacing_mut().item_spacing = egui::vec2(0.0, 6.0);
                    if let Some(title) = config.title {
                        ui.label(design_tokens::rich_text(title, TextRole::AttributeTitle));
                    }

                    show_space_tabs(ui, id, state);
                    match state.space {
                        ColorSpace::Hsv | ColorSpace::Hsl => {
                            changed |= show_area_picker(ui, id, state, color);
                            changed |= show_hue_strip(ui, id, state, color);
                        }
                        ColorSpace::Rgb => {
                            changed |= show_rgb_sliders(ui, id, state, color);
                        }
                        ColorSpace::Lab => {
                            changed |= show_lab_sliders(ui, id, state, color);
                        }
                        ColorSpace::Oklch => {
                            changed |= show_oklch_sliders(ui, id, state, color);
                        }
                    }

                    if config.allow_alpha {
                        changed |= show_alpha_strip(ui, id, state, color);
                    }
                    changed |= show_input_row(ui, id, state, color, config.allow_alpha);
                });
        });

    let close_requested = ctx.input(|input| {
        input.key_pressed(egui::Key::Escape)
            || (input.pointer.any_pressed()
                && input.pointer.interact_pos().is_some_and(|pos| {
                    !area_output.response.rect.contains(pos)
                        && !anchor_rect.expand(8.0).contains(pos)
                }))
    });

    ColorPopoverResponse {
        changed,
        close_requested,
    }
}

fn popup_size(state: &ColorPopoverState, config: &ColorPopoverConfig<'_>) -> Vec2 {
    let title_h = if config.title.is_some() { 20.0 } else { 0.0 };
    let body_h = match state.space {
        ColorSpace::Hsv | ColorSpace::Hsl => 250.0,
        ColorSpace::Rgb => 170.0,
        ColorSpace::Lab | ColorSpace::Oklch => 170.0,
    };
    let alpha_h = if config.allow_alpha { 20.0 } else { 0.0 };
    Vec2::new(POPUP_WIDTH, title_h + body_h + alpha_h)
}

fn popup_position(anchor_rect: Rect, bounds: Rect, popup_size: Vec2) -> Pos2 {
    let right = Pos2::new(anchor_rect.right() + POPOVER_GAP, anchor_rect.top());
    let left = Pos2::new(
        anchor_rect.left() - popup_size.x - POPOVER_GAP,
        anchor_rect.top(),
    );
    let mut pos = if right.x + popup_size.x <= bounds.right() - POPOVER_GAP {
        right
    } else {
        left
    };
    pos.x = pos.x.clamp(
        bounds.left() + POPOVER_GAP,
        bounds.right() - popup_size.x - POPOVER_GAP,
    );
    pos.y = pos.y.clamp(
        bounds.top() + POPOVER_GAP,
        bounds.bottom() - popup_size.y - POPOVER_GAP,
    );
    pos
}

fn show_space_tabs(ui: &mut egui::Ui, id: egui::Id, state: &mut ColorPopoverState) {
    const OPTIONS: [RadioButtonOption<'static, ColorSpace>; 5] = [
        RadioButtonOption {
            value: ColorSpace::Rgb,
            label: "RGB",
        },
        RadioButtonOption {
            value: ColorSpace::Hsv,
            label: "HSV",
        },
        RadioButtonOption {
            value: ColorSpace::Hsl,
            label: "HSL",
        },
        RadioButtonOption {
            value: ColorSpace::Lab,
            label: "LAB",
        },
        RadioButtonOption {
            value: ColorSpace::Oklch,
            label: "OKLCH",
        },
    ];
    radio_button_group(ui, id.with("space-tabs"), &mut state.space, &OPTIONS);
}

fn show_area_picker(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
) -> bool {
    let [r, g, b] = color_to_rgb01(*color);
    let (_, x_frac, y_value) = match state.space {
        ColorSpace::Hsl => {
            let (_, s, l) = rgb_to_hsl(r, g, b);
            (state.cached_hue, s, l)
        }
        _ => {
            let (_, s, v) = rgb_to_hsv(r, g, b);
            (state.cached_hue, s, v)
        }
    };
    let y_frac = 1.0 - y_value;

    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(CONTENT_WIDTH, PICKER_HEIGHT),
        Sense::click_and_drag(),
    );
    let response = response.on_hover_cursor(egui::CursorIcon::Crosshair);
    let painter = ui.painter_at(rect);
    draw_sv_area(&painter, rect, state.space, state.cached_hue);

    let crosshair = Pos2::new(
        rect.left() + x_frac.clamp(0.0, 1.0) * rect.width(),
        rect.top() + y_frac.clamp(0.0, 1.0) * rect.height(),
    );
    painter.circle_stroke(crosshair, 6.0, Stroke::new(2.0_f32, Color32::WHITE));
    painter.circle_stroke(
        crosshair,
        7.0,
        Stroke::new(1.0_f32, design_tokens::black(60)),
    );

    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let x = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let y = ((pointer.y - rect.top()) / rect.height()).clamp(0.0, 1.0);
        let rgb = match state.space {
            ColorSpace::Hsl => hsl_to_rgb(state.cached_hue, x, 1.0 - y),
            _ => hsv_to_rgb(state.cached_hue, x, 1.0 - y),
        };
        return apply_color(id, state, color, rgb_to_color(rgb, color.a()));
    }
    false
}

fn show_hue_strip(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(CONTENT_WIDTH, STRIP_HEIGHT),
        Sense::click_and_drag(),
    );
    let response = response.on_hover_cursor(egui::CursorIcon::Crosshair);
    let painter = ui.painter_at(rect);
    draw_hue_strip(&painter, rect);
    draw_strip_indicator(&painter, rect, (state.cached_hue / 360.0).clamp(0.0, 1.0));

    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        state.cached_hue = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0) * 360.0;
        let [r, g, b] = color_to_rgb01(*color);
        let rgb = match state.space {
            ColorSpace::Hsl => {
                let (_, s, l) = rgb_to_hsl(r, g, b);
                hsl_to_rgb(state.cached_hue, s, l)
            }
            _ => {
                let (_, s, v) = rgb_to_hsv(r, g, b);
                hsv_to_rgb(state.cached_hue, s, v)
            }
        };
        return apply_color(id, state, color, rgb_to_color(rgb, color.a()));
    }
    false
}

fn show_alpha_strip(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
) -> bool {
    let show_label = matches!(
        state.space,
        ColorSpace::Rgb | ColorSpace::Lab | ColorSpace::Oklch
    );
    let strip_width = if show_label {
        CONTENT_WIDTH - ROW_LABEL_WIDTH
    } else {
        CONTENT_WIDTH
    };
    if show_label {
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let label_font = design_tokens::font_id(design_tokens::FONT_SIZE_9, FontWeight::Medium);
            let label_rect = ui
                .allocate_exact_size(egui::vec2(ROW_LABEL_WIDTH, STRIP_HEIGHT), Sense::hover())
                .0;
            ui.painter().text(
                label_rect.center(),
                egui::Align2::CENTER_CENTER,
                "A",
                label_font,
                design_tokens::white(60),
            );
            changed = show_alpha_strip_body(ui, id, state, color, strip_width);
        });
        return changed;
    }
    show_alpha_strip_body(ui, id, state, color, strip_width)
}

fn show_alpha_strip_body(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
    strip_width: f32,
) -> bool {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(strip_width, STRIP_HEIGHT),
        Sense::click_and_drag(),
    );
    let response = response.on_hover_cursor(egui::CursorIcon::Crosshair);
    let painter = ui.painter_at(rect);
    draw_checkerboard(&painter, rect);
    draw_alpha_overlay(&painter, rect, *color);
    draw_strip_indicator(&painter, rect, color.a() as f32 / 255.0);

    if (response.clicked() || response.dragged())
        && let Some(pointer) = response.interact_pointer_pos()
    {
        let alpha = ((pointer.x - rect.left()) / rect.width()).clamp(0.0, 1.0);
        let next =
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), float_to_u8(alpha));
        return apply_color(id, state, color, next);
    }
    false
}

fn show_rgb_sliders(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
) -> bool {
    let mut rgb = color_to_rgb01(*color);
    let mut changed = false;
    changed |= slider_row(ui, id.with("rgb-r"), "R", &mut rgb[0], 0.0, 1.0, 0.001, 3);
    changed |= slider_row(ui, id.with("rgb-g"), "G", &mut rgb[1], 0.0, 1.0, 0.001, 3);
    changed |= slider_row(ui, id.with("rgb-b"), "B", &mut rgb[2], 0.0, 1.0, 0.001, 3);
    if changed {
        return apply_color(id, state, color, rgb_to_color(rgb, color.a()));
    }
    false
}

fn show_lab_sliders(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
) -> bool {
    let [r, g, b] = color_to_rgb01(*color);
    let mut lab = rgb_to_lab(r, g, b);
    let mut changed = false;
    changed |= slider_row(ui, id.with("lab-l"), "L*", &mut lab[0], 0.0, 100.0, 0.1, 1);
    changed |= slider_row(
        ui,
        id.with("lab-a"),
        "a*",
        &mut lab[1],
        -150.0,
        150.0,
        0.1,
        1,
    );
    changed |= slider_row(
        ui,
        id.with("lab-b"),
        "b*",
        &mut lab[2],
        -150.0,
        150.0,
        0.1,
        1,
    );
    if changed {
        let rgb = lab_to_rgb(lab[0], lab[1], lab[2]);
        return apply_color(id, state, color, rgb_to_color(rgb, color.a()));
    }
    false
}

fn show_oklch_sliders(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
) -> bool {
    let [r, g, b] = color_to_rgb01(*color);
    let mut oklch = rgb_to_oklch(r, g, b);
    let mut changed = false;
    changed |= slider_row(
        ui,
        id.with("oklch-l"),
        "L",
        &mut oklch[0],
        0.0,
        1.0,
        0.001,
        3,
    );
    changed |= slider_row(
        ui,
        id.with("oklch-c"),
        "C",
        &mut oklch[1],
        0.0,
        0.4,
        0.001,
        3,
    );
    changed |= slider_row(
        ui,
        id.with("oklch-h"),
        "H",
        &mut oklch[2],
        0.0,
        360.0,
        0.1,
        1,
    );
    if changed {
        let rgb = oklch_to_rgb(oklch[0], oklch[1], oklch[2]);
        return apply_color(id, state, color, rgb_to_color(rgb, color.a()));
    }
    false
}

fn slider_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    label: &str,
    value: &mut f32,
    min: f32,
    max: f32,
    step: f32,
    decimals: usize,
) -> bool {
    let mut changed = false;
    let label_font = design_tokens::font_id(design_tokens::FONT_SIZE_9, FontWeight::Medium);
    let slider_width = (CONTENT_WIDTH - ROW_LABEL_WIDTH).max(0.0);
    let formatter = |value: f32| format!("{value:.decimals$}");
    let value_label_width =
        number_slider::value_label_width_for(ui, slider_value_sample(min, max, decimals).as_str());

    ui.allocate_ui_with_layout(
        egui::vec2(CONTENT_WIDTH, design_tokens::CONTROL_ROW_HEIGHT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let label_rect = ui
                .allocate_exact_size(
                    egui::vec2(ROW_LABEL_WIDTH, design_tokens::CONTROL_ROW_HEIGHT),
                    Sense::hover(),
                )
                .0;
            ui.painter().text(
                label_rect.center(),
                egui::Align2::CENTER_CENTER,
                label,
                label_font.clone(),
                design_tokens::white(60),
            );
            ui.allocate_ui_with_layout(
                egui::vec2(slider_width, design_tokens::CONTROL_ROW_HEIGHT),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    changed = number_slider::slider_with_editable_value(
                        ui,
                        id,
                        value,
                        min,
                        max,
                        step,
                        number_slider::NumberSliderConfig::new(popover_bg())
                            .formatter(Some(&formatter))
                            .value_label_width(value_label_width),
                    );
                },
            );
        },
    );

    changed
}

fn slider_value_sample(min: f32, max: f32, decimals: usize) -> String {
    let min = format!("{min:.decimals$}");
    let max = format!("{max:.decimals$}");
    if min.len() > max.len() { min } else { max }
}

fn show_input_row(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
    allow_alpha: bool,
) -> bool {
    const OPTIONS: [RadioButtonOption<'static, ColorInputFormat>; 2] = [
        RadioButtonOption {
            value: ColorInputFormat::Hex,
            label: "HEX",
        },
        RadioButtonOption {
            value: ColorInputFormat::Int,
            label: "INT",
        },
    ];

    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.allocate_ui_with_layout(
            egui::vec2(62.0, design_tokens::CONTROL_ROW_HEIGHT),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                radio_button_group(ui, id.with("format"), &mut state.input_format, &OPTIONS);
            },
        );
        match state.input_format {
            ColorInputFormat::Hex => {
                changed |= show_hex_input(ui, id, state, color);
            }
            ColorInputFormat::Int => {
                changed |= show_int_inputs(ui, id, state, color, allow_alpha);
            }
        }
    });
    changed
}

fn show_hex_input(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
) -> bool {
    let width = (CONTENT_WIDTH - 68.0).max(0.0);
    let mut changed = false;
    egui::Frame::new()
        .fill(design_tokens::RESOURCE_ACTIVE_BG)
        .corner_radius(design_tokens::radius(
            design_tokens::BORDER_RADIUS_SMALL as u8,
        ))
        .inner_margin(egui::Margin::symmetric(8, 0))
        .show(ui, |ui| {
            ui.set_width(width);
            let response = ui.add_sized(
                egui::vec2(width, design_tokens::CONTROL_ROW_HEIGHT),
                egui::TextEdit::singleline(&mut state.hex_buffer)
                    .id(id.with("hex-input"))
                    .font(design_tokens::font_id(
                        design_tokens::FONT_SIZE_13,
                        FontWeight::Normal,
                    ))
                    .text_color(design_tokens::white(90))
                    .desired_width(width),
            );
            if response.changed()
                && let Some(next) = parse_hex_color(state.hex_buffer.as_str())
            {
                changed |= apply_color(id, state, color, next);
            }
        });
    changed
}

fn show_int_inputs(
    ui: &mut egui::Ui,
    id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
    allow_alpha: bool,
) -> bool {
    let [mut r, mut g, mut b, mut a] = color.to_srgba_unmultiplied();
    let mut changed = false;
    let component_count = if allow_alpha { 4.0 } else { 3.0 };
    let cell_w = ((CONTENT_WIDTH - 68.0) - (component_count - 1.0) * 2.0) / component_count;

    ui.spacing_mut().item_spacing.x = 2.0;
    changed |= int_component(ui, id.with("r-int"), &mut r, cell_w);
    changed |= int_component(ui, id.with("g-int"), &mut g, cell_w);
    changed |= int_component(ui, id.with("b-int"), &mut b, cell_w);
    if allow_alpha {
        changed |= int_component(ui, id.with("a-int"), &mut a, cell_w);
    }
    if changed {
        return apply_color(
            id,
            state,
            color,
            Color32::from_rgba_unmultiplied(r, g, b, a),
        );
    }
    false
}

fn int_component(ui: &mut egui::Ui, id: egui::Id, value: &mut u8, width: f32) -> bool {
    let mut numeric = *value as i32;
    let response = egui::Frame::new()
        .fill(design_tokens::RESOURCE_ACTIVE_BG)
        .corner_radius(design_tokens::radius(
            design_tokens::BORDER_RADIUS_SMALL as u8,
        ))
        .inner_margin(egui::Margin::symmetric(2, 0))
        .show(ui, |ui| {
            ui.push_id(id, |ui| {
                ui.add_sized(
                    egui::vec2(width, design_tokens::CONTROL_ROW_HEIGHT),
                    egui::DragValue::new(&mut numeric)
                        .range(0..=255)
                        .speed(1)
                        .custom_formatter(|value, _| format!("{:.0}", value))
                        .custom_parser(|text| text.parse::<f64>().ok()),
                )
            })
            .inner
        })
        .inner;
    let next = numeric.clamp(0, 255) as u8;
    let changed = response.changed() && next != *value;
    if changed {
        *value = next;
    }
    changed
}

fn draw_sv_area(painter: &egui::Painter, rect: Rect, space: ColorSpace, hue: f32) {
    let cols = 32;
    let rows = 24;
    for row in 0..rows {
        let y0 = rect.top() + rect.height() * row as f32 / rows as f32;
        let y1 = rect.top() + rect.height() * (row + 1) as f32 / rows as f32;
        let y = row as f32 / (rows - 1) as f32;
        for col in 0..cols {
            let x0 = rect.left() + rect.width() * col as f32 / cols as f32;
            let x1 = rect.left() + rect.width() * (col + 1) as f32 / cols as f32;
            let x = col as f32 / (cols - 1) as f32;
            let rgb = match space {
                ColorSpace::Hsl => hsl_to_rgb(hue, x, 1.0 - y),
                _ => hsv_to_rgb(hue, x, 1.0 - y),
            };
            painter.rect_filled(
                Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1 + 0.5, y1 + 0.5)),
                egui::CornerRadius::ZERO,
                rgb_to_color(rgb, u8::MAX),
            );
        }
    }
    painter.rect_stroke(
        rect,
        design_tokens::radius(design_tokens::BORDER_RADIUS_SMALL as u8),
        Stroke::new(design_tokens::LINE_THICKNESS_1, design_tokens::black(50)),
        egui::StrokeKind::Inside,
    );
}

fn draw_hue_strip(painter: &egui::Painter, rect: Rect) {
    let segments = 48;
    for index in 0..segments {
        let t0 = index as f32 / segments as f32;
        let t1 = (index + 1) as f32 / segments as f32;
        let segment = Rect::from_min_max(
            Pos2::new(rect.left() + rect.width() * t0, rect.top()),
            Pos2::new(rect.left() + rect.width() * t1 + 0.5, rect.bottom()),
        );
        painter.rect_filled(
            segment,
            egui::CornerRadius::ZERO,
            rgb_to_color(hsv_to_rgb(t0 * 360.0, 1.0, 1.0), u8::MAX),
        );
    }
    painter.rect_stroke(
        rect,
        design_tokens::radius(design_tokens::BORDER_RADIUS_SMALL as u8),
        Stroke::new(design_tokens::LINE_THICKNESS_1, design_tokens::black(50)),
        egui::StrokeKind::Inside,
    );
}

fn draw_alpha_overlay(painter: &egui::Painter, rect: Rect, color: Color32) {
    let segments = 48;
    for index in 0..segments {
        let t0 = index as f32 / segments as f32;
        let t1 = (index + 1) as f32 / segments as f32;
        let segment = Rect::from_min_max(
            Pos2::new(rect.left() + rect.width() * t0, rect.top()),
            Pos2::new(rect.left() + rect.width() * t1 + 0.5, rect.bottom()),
        );
        painter.rect_filled(
            segment,
            egui::CornerRadius::ZERO,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), float_to_u8(t0)),
        );
    }
    painter.rect_stroke(
        rect,
        design_tokens::radius(design_tokens::BORDER_RADIUS_SMALL as u8),
        Stroke::new(design_tokens::LINE_THICKNESS_1, design_tokens::black(50)),
        egui::StrokeKind::Inside,
    );
}

fn draw_checkerboard(painter: &egui::Painter, rect: Rect) {
    let size = 5.0;
    let cols = (rect.width() / size).ceil() as i32;
    let rows = (rect.height() / size).ceil() as i32;
    for row in 0..rows {
        for col in 0..cols {
            let dark = (row + col) % 2 == 0;
            let color = if dark {
                design_tokens::black(20)
            } else {
                design_tokens::white(10)
            };
            let cell = Rect::from_min_max(
                Pos2::new(
                    rect.left() + col as f32 * size,
                    rect.top() + row as f32 * size,
                ),
                Pos2::new(
                    (rect.left() + (col + 1) as f32 * size).min(rect.right()),
                    (rect.top() + (row + 1) as f32 * size).min(rect.bottom()),
                ),
            );
            painter.rect_filled(cell, egui::CornerRadius::ZERO, color);
        }
    }
}

fn draw_strip_indicator(painter: &egui::Painter, rect: Rect, t: f32) {
    let x = rect.left() + rect.width() * t.clamp(0.0, 1.0);
    let indicator = Rect::from_center_size(
        Pos2::new(x, rect.center().y),
        egui::vec2(4.0, rect.height()),
    );
    painter.rect_filled(indicator, design_tokens::radius(2), Color32::WHITE);
    painter.rect_stroke(
        indicator,
        design_tokens::radius(2),
        Stroke::new(1.0_f32, design_tokens::black(60)),
        egui::StrokeKind::Inside,
    );
}

fn apply_color(
    _id: egui::Id,
    state: &mut ColorPopoverState,
    color: &mut Color32,
    next: Color32,
) -> bool {
    if *color == next {
        return false;
    }
    *color = next;
    state.set_color_cache(next);
    true
}

fn popover_bg() -> Color32 {
    Color32::from_rgb(0x25, 0x25, 0x25)
}

fn color_to_rgb01(color: Color32) -> [f32; 3] {
    [
        color.r() as f32 / 255.0,
        color.g() as f32 / 255.0,
        color.b() as f32 / 255.0,
    ]
}

fn rgb_to_color(rgb: [f32; 3], alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(
        float_to_u8(rgb[0]),
        float_to_u8(rgb[1]),
        float_to_u8(rgb[2]),
        alpha,
    )
}

fn float_to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn color_to_hex(color: Color32) -> String {
    let [r, g, b, a] = color.to_srgba_unmultiplied();
    if a == u8::MAX {
        format!("#{:02x}{:02x}{:02x}", r, g, b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a)
    }
}

fn parse_hex_color(value: &str) -> Option<Color32> {
    let raw = value.trim().strip_prefix('#').unwrap_or(value.trim());
    match raw.len() {
        3 => {
            let r = u8::from_str_radix(&raw[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&raw[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&raw[2..3], 16).ok()? * 17;
            Some(Color32::from_rgb(r, g, b))
        }
        4 => {
            let r = u8::from_str_radix(&raw[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&raw[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&raw[2..3], 16).ok()? * 17;
            let a = u8::from_str_radix(&raw[3..4], 16).ok()? * 17;
            Some(Color32::from_rgba_unmultiplied(r, g, b, a))
        }
        6 => Some(Color32::from_rgb(
            u8::from_str_radix(&raw[0..2], 16).ok()?,
            u8::from_str_radix(&raw[2..4], 16).ok()?,
            u8::from_str_radix(&raw[4..6], 16).ok()?,
        )),
        8 => Some(Color32::from_rgba_unmultiplied(
            u8::from_str_radix(&raw[0..2], 16).ok()?,
            u8::from_str_radix(&raw[2..4], 16).ok()?,
            u8::from_str_radix(&raw[4..6], 16).ok()?,
            u8::from_str_radix(&raw[6..8], 16).ok()?,
        )),
        _ => None,
    }
}

fn rgb_to_hsv(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let v = max;
    let d = max - min;
    let s = if max <= f32::EPSILON { 0.0 } else { d / max };
    if d <= f32::EPSILON {
        return (0.0, 0.0, v);
    }
    let h = if (max - r).abs() <= f32::EPSILON {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() <= f32::EPSILON {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h * 360.0, s, v)
}

fn hsv_to_rgb(h: f32, s: f32, v: f32) -> [f32; 3] {
    let h = positive_mod(h, 360.0) / 60.0;
    let c = v * s;
    let x = c * (1.0 - ((h % 2.0) - 1.0).abs());
    let m = v - c;
    let (r1, g1, b1) = if h < 1.0 {
        (c, x, 0.0)
    } else if h < 2.0 {
        (x, c, 0.0)
    } else if h < 3.0 {
        (0.0, c, x)
    } else if h < 4.0 {
        (0.0, x, c)
    } else if h < 5.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    [r1 + m, g1 + m, b1 + m]
}

fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) * 0.5;
    if (max - min).abs() <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let d = max - min;
    let s = if l > 0.5 {
        d / (2.0 - max - min)
    } else {
        d / (max + min)
    };
    let h = if (max - r).abs() <= f32::EPSILON {
        ((g - b) / d + if g < b { 6.0 } else { 0.0 }) / 6.0
    } else if (max - g).abs() <= f32::EPSILON {
        ((b - r) / d + 2.0) / 6.0
    } else {
        ((r - g) / d + 4.0) / 6.0
    };
    (h * 360.0, s, l)
}

fn hsl_to_rgb(h: f32, s: f32, l: f32) -> [f32; 3] {
    let h = positive_mod(h, 360.0) / 360.0;
    if s <= f32::EPSILON {
        return [l, l, l];
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    [
        hue_to_rgb(p, q, h + 1.0 / 3.0),
        hue_to_rgb(p, q, h),
        hue_to_rgb(p, q, h - 1.0 / 3.0),
    ]
}

fn hue_to_rgb(p: f32, q: f32, mut t: f32) -> f32 {
    if t < 0.0 {
        t += 1.0;
    }
    if t > 1.0 {
        t -= 1.0;
    }
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

fn rgb_to_lab(r: f32, g: f32, b: f32) -> [f32; 3] {
    let [x, y, z] = linear_rgb_to_xyz(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
    let fx = xyz_to_lab_fn(x / 0.95047);
    let fy = xyz_to_lab_fn(y);
    let fz = xyz_to_lab_fn(z / 1.08883);
    [116.0 * fy - 16.0, 500.0 * (fx - fy), 200.0 * (fy - fz)]
}

fn lab_to_rgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;
    let x = 0.95047 * lab_to_xyz_fn(fx);
    let y = lab_to_xyz_fn(fy);
    let z = 1.08883 * lab_to_xyz_fn(fz);
    let [r, g, b] = xyz_to_linear_rgb(x, y, z);
    [
        clamp01(linear_to_srgb(r)),
        clamp01(linear_to_srgb(g)),
        clamp01(linear_to_srgb(b)),
    ]
}

fn rgb_to_oklch(r: f32, g: f32, b: f32) -> [f32; 3] {
    let [l, a, b_lab] =
        linear_rgb_to_oklab(srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b));
    let c = (a * a + b_lab * b_lab).sqrt();
    let mut h = b_lab.atan2(a).to_degrees();
    if h < 0.0 {
        h += 360.0;
    }
    [l, c, h]
}

fn oklch_to_rgb(l: f32, c: f32, h: f32) -> [f32; 3] {
    let h_rad = h.to_radians();
    let a = c * h_rad.cos();
    let b = c * h_rad.sin();
    let [r, g, b] = oklab_to_linear_rgb(l, a, b);
    [
        clamp01(linear_to_srgb(r)),
        clamp01(linear_to_srgb(g)),
        clamp01(linear_to_srgb(b)),
    ]
}

fn linear_rgb_to_xyz(r: f32, g: f32, b: f32) -> [f32; 3] {
    [
        0.4124564 * r + 0.3575761 * g + 0.1804375 * b,
        0.2126729 * r + 0.7151522 * g + 0.072175 * b,
        0.0193339 * r + 0.119192 * g + 0.9503041 * b,
    ]
}

fn xyz_to_linear_rgb(x: f32, y: f32, z: f32) -> [f32; 3] {
    [
        3.2404542 * x - 1.5371385 * y - 0.4985314 * z,
        -0.969266 * x + 1.8760108 * y + 0.041556 * z,
        0.0556434 * x - 0.2040259 * y + 1.0572252 * z,
    ]
}

fn linear_rgb_to_oklab(r: f32, g: f32, b: f32) -> [f32; 3] {
    let l_ = 0.41222147 * r + 0.53633255 * g + 0.05144599 * b;
    let m_ = 0.2119035 * r + 0.6806995 * g + 0.10739696 * b;
    let s_ = 0.08830246 * r + 0.28171885 * g + 0.6299787 * b;
    let l = cbrt(l_);
    let m = cbrt(m_);
    let s = cbrt(s_);
    [
        0.21045426 * l + 0.7936178 * m - 0.004072047 * s,
        1.9779985 * l - 2.4285922 * m + 0.4505937 * s,
        0.025904037 * l + 0.78277177 * m - 0.80867577 * s,
    ]
}

fn oklab_to_linear_rgb(l: f32, a: f32, b: f32) -> [f32; 3] {
    let l_ = l + 0.39633778 * a + 0.21580376 * b;
    let m_ = l - 0.105561346 * a - 0.06385417 * b;
    let s_ = l - 0.08948418 * a - 1.2914855 * b;
    let l = l_ * l_ * l_;
    let m = m_ * m_ * m_;
    let s = s_ * s_ * s_;
    [
        4.0767417 * l - 3.3077116 * m + 0.23096994 * s,
        -1.268438 * l + 2.6097574 * m - 0.34131938 * s,
        -0.0041960863 * l - 0.7034186 * m + 1.7076147 * s,
    ]
}

fn xyz_to_lab_fn(value: f32) -> f32 {
    const EPSILON: f32 = 216.0 / 24389.0;
    const KAPPA: f32 = 24389.0 / 27.0;
    if value > EPSILON {
        cbrt(value)
    } else {
        (KAPPA * value + 16.0) / 116.0
    }
}

fn lab_to_xyz_fn(value: f32) -> f32 {
    const EPSILON: f32 = 216.0 / 24389.0;
    const KAPPA: f32 = 24389.0 / 27.0;
    let cubed = value * value * value;
    if cubed > EPSILON {
        cubed
    } else {
        (116.0 * value - 16.0) / KAPPA
    }
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.0031308 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

fn cbrt(value: f32) -> f32 {
    if value < 0.0 {
        -(-value).powf(1.0 / 3.0)
    } else {
        value.powf(1.0 / 3.0)
    }
}

fn positive_mod(value: f32, modulo: f32) -> f32 {
    ((value % modulo) + modulo) % modulo
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parser_accepts_editor_formats() {
        assert_eq!(
            parse_hex_color("#fc0").unwrap(),
            Color32::from_rgb(255, 204, 0)
        );
        assert_eq!(
            parse_hex_color("#11223344").unwrap(),
            Color32::from_rgba_unmultiplied(17, 34, 51, 68)
        );
        assert_eq!(
            parse_hex_color("abcd").unwrap(),
            Color32::from_rgba_unmultiplied(170, 187, 204, 221)
        );
    }

    #[test]
    fn hsv_round_trip_preserves_primary_color() {
        let (h, s, v) = rgb_to_hsv(1.0, 0.0, 0.0);
        let rgb = hsv_to_rgb(h, s, v);
        assert!((rgb[0] - 1.0).abs() < 1e-6);
        assert!(rgb[1].abs() < 1e-6);
        assert!(rgb[2].abs() < 1e-6);
    }

    #[test]
    fn hsl_round_trip_preserves_primary_color() {
        let (h, s, l) = rgb_to_hsl(0.0, 0.0, 1.0);
        let rgb = hsl_to_rgb(h, s, l);
        assert!(rgb[0].abs() < 1e-6);
        assert!(rgb[1].abs() < 1e-6);
        assert!((rgb[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn oklch_round_trip_stays_in_srgb_range() {
        let [l, c, h] = rgb_to_oklch(0.2, 0.5, 0.9);
        let [r, g, b] = oklch_to_rgb(l, c, h);
        assert!((r - 0.2).abs() < 0.02);
        assert!((g - 0.5).abs() < 0.02);
        assert!((b - 0.9).abs() < 0.02);
    }
}
