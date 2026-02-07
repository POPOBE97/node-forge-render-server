use rust_wgpu_fiber::eframe::egui::{self, Rect, pos2};

pub fn clamp_zoom(value: f32, min_zoom: f32) -> f32 {
    value.clamp(min_zoom, 100.0)
}

pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn lerp_pos2(a: egui::Pos2, b: egui::Pos2, t: f32) -> egui::Pos2 {
    egui::pos2(lerp(a.x, b.x, t), lerp(a.y, b.y, t))
}

pub fn lerp_vec2(a: egui::Vec2, b: egui::Vec2, t: f32) -> egui::Vec2 {
    egui::vec2(lerp(a.x, b.x, t), lerp(a.y, b.y, t))
}

pub fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    Rect::from_min_max(lerp_pos2(a.min, b.min, t), lerp_pos2(a.max, b.max, t))
}

pub fn cover_uv_rect(dst_size: egui::Vec2, tex_size: egui::Vec2) -> Rect {
    let dst_aspect = (dst_size.x / dst_size.y).max(0.0001);
    let tex_aspect = (tex_size.x / tex_size.y).max(0.0001);

    let (uv_w, uv_h) = if dst_aspect > tex_aspect {
        (1.0, tex_aspect / dst_aspect)
    } else {
        (dst_aspect / tex_aspect, 1.0)
    };

    let size = egui::vec2(uv_w, uv_h);
    Rect::from_center_size(pos2(0.5, 0.5), size)
}

pub fn clamp_uv_rect_into_unit(mut uv: Rect) -> Rect {
    let size = uv.size();
    if size.x > 1.0 || size.y > 1.0 {
        return uv;
    }

    if uv.min.x < 0.0 {
        let d = -uv.min.x;
        uv.min.x += d;
        uv.max.x += d;
    }
    if uv.max.x > 1.0 {
        let d = uv.max.x - 1.0;
        uv.min.x -= d;
        uv.max.x -= d;
    }
    if uv.min.y < 0.0 {
        let d = -uv.min.y;
        uv.min.y += d;
        uv.max.y += d;
    }
    if uv.max.y > 1.0 {
        let d = uv.max.y - 1.0;
        uv.min.y -= d;
        uv.max.y -= d;
    }

    uv
}
