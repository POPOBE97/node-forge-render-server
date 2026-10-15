use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(super) enum ScaleMode {
    #[default]
    Fit,
    Physical(f32),
}

#[derive(Clone, Debug, Default)]
pub(super) struct ImageTransform {
    pub scale: ScaleMode,
    pub pan: egui::Vec2,
    pub turns: u8,
    warp: Option<geometry::WarpCache>,
}

impl ImageTransform {
    pub fn fit(&mut self) {
        self.scale = ScaleMode::Fit;
        self.pan = egui::Vec2::ZERO;
    }
    pub fn original(&mut self) {
        self.scale = ScaleMode::Physical(1.0);
        self.pan = egui::Vec2::ZERO;
    }
    pub fn rotate(&mut self) {
        self.turns = (self.turns + 1) % 4;
        self.pan = egui::Vec2::ZERO;
    }
    pub fn rotated_size(&self, size: [u32; 2]) -> egui::Vec2 {
        if self.turns % 2 == 1 {
            egui::vec2(size[1] as f32, size[0] as f32)
        } else {
            egui::vec2(size[0] as f32, size[1] as f32)
        }
    }
    pub fn factor(&self, size: [u32; 2], rect: egui::Rect, ppp: f32) -> f32 {
        match self.scale {
            ScaleMode::Fit => {
                let s = self.rotated_size(size);
                ((rect.width() - 48.).max(1.) / s.x).min((rect.height() - 48.).max(1.) / s.y)
            }
            ScaleMode::Physical(scale) => scale / ppp,
        }
    }
    pub fn image_rect(&self, size: [u32; 2], rect: egui::Rect, ppp: f32) -> egui::Rect {
        egui::Rect::from_center_size(
            rect.center() + self.pan,
            self.rotated_size(size) * self.factor(size, rect, ppp),
        )
    }
    pub fn zoom_at(
        &mut self,
        delta: f32,
        pointer: egui::Pos2,
        size: [u32; 2],
        rect: egui::Rect,
        ppp: f32,
    ) {
        let before = self.factor(size, rect, ppp);
        let next = (before * delta * ppp).clamp(0.02, 32.) / ppp;
        self.pan = pointer - rect.center() - (pointer - rect.center() - self.pan) * (next / before);
        self.scale = ScaleMode::Physical(next * ppp);
    }
    pub fn draw(
        &mut self,
        ui: &mut egui::Ui,
        view: &ImageView,
        correction: Option<&DisplayGeometry>,
    ) -> egui::Response {
        let size = correction.map(|g| g.screen_size).unwrap_or(view.size);
        let (rect, response) =
            ui.allocate_exact_size(ui.available_size(), egui::Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        if response.hovered() {
            let zoom = ui.input(|i| i.zoom_delta());
            if zoom != 1.0 {
                if let Some(pos) = response.hover_pos() {
                    self.zoom_at(zoom, pos, size, rect, ppp);
                }
            } else {
                let delta = ui.input(|i| i.smooth_scroll_delta);
                if delta != egui::Vec2::ZERO {
                    let factor = self.factor(size, rect, ppp);
                    self.scale = ScaleMode::Physical(factor * ppp);
                    self.pan += delta;
                }
            }
        }
        if response.dragged_by(egui::PointerButton::Middle) {
            let factor = self.factor(size, rect, ppp);
            self.scale = ScaleMode::Physical(factor * ppp);
            self.pan += ui.input(|i| i.pointer.delta());
        }
        let image_rect = self.image_rect(size, rect, ppp);
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        if let Some(g) = correction {
            let mesh = geometry::WarpCache::mesh(&mut self.warp, g, view);
            painter.add(egui::Shape::mesh(geometry::place_mesh(
                mesh,
                view.texture.id(),
                image_rect,
                self.turns,
            )));
            return response;
        }
        let points = [
            image_rect.left_top(),
            image_rect.right_top(),
            image_rect.right_bottom(),
            image_rect.left_bottom(),
        ];
        let uv = [
            egui::pos2(0., 0.),
            egui::pos2(1., 0.),
            egui::pos2(1., 1.),
            egui::pos2(0., 1.),
        ];
        let mut mesh = egui::Mesh::with_texture(view.texture.id());
        for (index, pos) in points.into_iter().enumerate() {
            mesh.vertices.push(egui::epaint::Vertex {
                pos,
                uv: uv[(index + 4 - self.turns as usize) % 4],
                color: Color32::WHITE,
            });
        }
        mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
        painter.add(egui::Shape::mesh(mesh));
        response
    }
}
