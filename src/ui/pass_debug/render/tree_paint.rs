use rust_wgpu_fiber::eframe::egui;

use crate::ui::pass_debug::shortwire::{ShortwireDotInfo, ShortwireDotStatus};

const TREE_ROW_SOURCE_JUMP_LABEL: &str = "src";
const TREE_ROW_SOURCE_JUMP_HORIZONTAL_PADDING: f32 = 5.0;
const TREE_ROW_SOURCE_JUMP_VERTICAL_PADDING: f32 = 2.0;

pub(crate) fn source_jump_button_size(ui: &egui::Ui, font_id: &egui::FontId) -> egui::Vec2 {
    let text_color = ui.visuals().text_color();
    let label_size = ui
        .painter()
        .layout_no_wrap(
            TREE_ROW_SOURCE_JUMP_LABEL.to_string(),
            font_id.clone(),
            text_color,
        )
        .size();
    egui::vec2(
        label_size.x + TREE_ROW_SOURCE_JUMP_HORIZONTAL_PADDING * 2.0,
        label_size.y + TREE_ROW_SOURCE_JUMP_VERTICAL_PADDING * 2.0,
    )
}

pub(crate) fn paint_tree_toggle_symbol(
    ui: &egui::Ui,
    rect: egui::Rect,
    symbol: &str,
    hovered: bool,
    font_id: &egui::FontId,
) {
    let symbol_color = if hovered {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let symbol_galley =
        ui.painter()
            .layout_no_wrap(symbol.to_string(), font_id.clone(), symbol_color);
    let symbol_pos = egui::pos2(
        rect.center().x - symbol_galley.size().x * 0.5,
        rect.center().y - symbol_galley.size().y * 0.5,
    );
    ui.painter().galley(symbol_pos, symbol_galley, symbol_color);
}

pub(crate) fn paint_source_jump_button(
    ui: &egui::Ui,
    rect: egui::Rect,
    hovered: bool,
    font_id: &egui::FontId,
) {
    let fill = if hovered {
        source_jump_button_hover_bg(ui)
    } else {
        source_jump_button_bg(ui)
    };
    let text_color = if hovered {
        tree_highlight_text_color(ui)
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    let galley = ui.painter().layout_no_wrap(
        TREE_ROW_SOURCE_JUMP_LABEL.to_string(),
        font_id.clone(),
        text_color,
    );
    let text_pos = egui::pos2(
        rect.center().x - galley.size().x * 0.5,
        rect.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(text_pos, galley, text_color);
}

pub(crate) fn dependency_path_color(ui: &egui::Ui, index: usize, len: usize) -> egui::Color32 {
    let t = if len <= 1 {
        1.0
    } else {
        index as f32 / (len - 1) as f32
    };
    let (start, end) = if ui.visuals().dark_mode {
        (
            egui::Color32::from_rgba_unmultiplied(96, 165, 250, 26),
            egui::Color32::from_rgba_unmultiplied(245, 158, 11, 38),
        )
    } else {
        (
            egui::Color32::from_rgba_unmultiplied(37, 99, 235, 20),
            egui::Color32::from_rgba_unmultiplied(180, 83, 9, 28),
        )
    };
    lerp_color(start, end, t)
}

pub(crate) fn tree_selected_row_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(44, 58, 76)
    } else {
        egui::Color32::from_rgb(218, 231, 248)
    }
}

pub(crate) fn tree_hovered_row_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 10)
    }
}

fn source_jump_button_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(148, 163, 184, 30)
    } else {
        egui::Color32::from_rgba_unmultiplied(71, 85, 105, 20)
    }
}

fn source_jump_button_hover_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(96, 165, 250, 62)
    } else {
        egui::Color32::from_rgba_unmultiplied(37, 99, 235, 42)
    }
}

pub(crate) fn tree_highlight_text_color(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(238, 242, 247)
    } else {
        egui::Color32::from_rgb(20, 31, 46)
    }
}

pub(crate) fn shortwire_dot_hover_text(dot_info: ShortwireDotInfo) -> String {
    match (dot_info.max_ae, dot_info.sample_count) {
        (Some(max_ae), Some(sample_count)) => {
            format!(
                "Shortwire diff: max AE {max_ae:.6} ({:.2}/255), threshold < 2/255, n {sample_count}",
                max_ae * 255.0
            )
        }
        _ => "Shortwire diff: pending".to_string(),
    }
}

pub(crate) fn shortwire_dot_color(
    ui: &egui::Ui,
    status: ShortwireDotStatus,
    alpha: f32,
) -> egui::Color32 {
    let base = match (status, ui.visuals().dark_mode) {
        (ShortwireDotStatus::PendingDiff, true) => egui::Color32::from_rgb(250, 204, 21),
        (ShortwireDotStatus::PendingDiff, false) => egui::Color32::from_rgb(202, 138, 4),
        (ShortwireDotStatus::Passing, true) => egui::Color32::from_rgb(74, 222, 128),
        (ShortwireDotStatus::Passing, false) => egui::Color32::from_rgb(22, 163, 74),
        (ShortwireDotStatus::Failing, true) => egui::Color32::from_rgb(248, 113, 113),
        (ShortwireDotStatus::Failing, false) => egui::Color32::from_rgb(220, 38, 38),
    };
    let [r, g, b, _] = base.to_srgba_unmultiplied();
    egui::Color32::from_rgba_unmultiplied(r, g, b, (220.0 * alpha) as u8)
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let [ar, ag, ab, aa] = a.to_srgba_unmultiplied();
    let [br, bg, bb, ba] = b.to_srgba_unmultiplied();
    let lerp = |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t).round() as u8 };
    egui::Color32::from_rgba_unmultiplied(lerp(ar, br), lerp(ag, bg), lerp(ab, bb), lerp(aa, ba))
}
