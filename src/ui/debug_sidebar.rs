use rust_wgpu_fiber::eframe::egui;

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
pub const SIDEBAR_MAX_WIDTH: f32 = 460.0;
pub const SIDEBAR_ANIM_SECS: f64 = 0.25;

const SIDEBAR_RESIZE_HANDLE_W: f32 = 8.0;

const CARD_RADIUS: f32 = 12.0;

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
        .clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH)
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
) {
    if ui_sidebar_factor <= 0.0 {
        return;
    }

    let sidebar_bg = crate::color::lab(7.78201, -0.000_014_901_2, 0.0);

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
            let start_w = ctx
                .memory(|mem| mem.data.get_temp::<f32>(sidebar_resize_start_width_id()))
                .unwrap_or_else(|| sidebar_width(ctx));
            let next =
                (start_w + response.drag_delta().x).clamp(SIDEBAR_MIN_WIDTH, SIDEBAR_MAX_WIDTH);
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
            egui::Stroke::new(1.0, egui::Color32::from_gray(32)),
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
                    .inner_margin(egui::Margin::same(8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            egui::Frame::NONE
                                .inner_margin(egui::Margin::same(6))
                                .show(ui, |ui| {
                                    let icon = node_forge_icon_texture(ctx);
                                    ui.add(
                                        egui::Image::new((icon.id(), egui::vec2(20.0, 20.0)))
                                            .corner_radius(egui::CornerRadius::same(4)),
                                    );
                                    ui.add_space(6.0);
                                    crate::ui::typography::label(ui, "Node Forge", 600.0, 16.0);
                                });

                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if canvas_only_button(ui) {
                                        toggle_canvas_only();
                                    }
                                },
                            );
                        });
                    });
                ui.separator();

                egui::ScrollArea::vertical().show(ui, |ui| {
                    for idx in 0..3 {
                        let card_width = ui.available_size_before_wrap().x;
                        egui::Frame::default()
                            .fill(egui::Color32::from_gray(24))
                            .inner_margin(egui::Margin::same(12))
                            .corner_radius(egui::CornerRadius::same(CARD_RADIUS as u8))
                            .show(ui, |ui| {
                                ui.set_max_width(card_width);
                                ui.label(egui::RichText::new(format!(
                                    "Placeholder card {}",
                                    idx + 1
                                )));
                                ui.add_space(6.0);
                                ui.label("TODO: debug content");
                            });

                        if idx != 2 {
                            ui.add_space(10.0);
                        }
                    }
                });
            }
        });
    });
}
