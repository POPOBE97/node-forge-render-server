use std::{
    sync::{Arc, Mutex},
    time::Instant,
};

use crossbeam_channel::Receiver;
use rust_wgpu_fiber::{
    ResourceName,
    eframe::{egui, wgpu},
    shader_space::ShaderSpace,
};

use crate::{renderer, ws};

use crate::ui::animation_manager::AnimationManager;
use crate::ui::file_tree_widget::FileTreeState;
use crate::ui::resource_tree::{FileTreeNode, ResourceSnapshot};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiWindowMode {
    #[default]
    Sidebar,
    CanvasOnly,
}

#[derive(Clone, Copy, Debug)]
pub struct SampledPixel {
    pub x: u32,
    pub y: u32,
    pub rgba: [u8; 4],
}

pub const CANVAS_RADIUS: f32 = 16.0;
pub const OUTER_MARGIN: f32 = 4.0;
pub const SIDEBAR_ANIM_SECS: f64 = crate::ui::debug_sidebar::SIDEBAR_ANIM_SECS;

pub struct AppInit {
    pub shader_space: ShaderSpace,
    pub resolution: [u32; 2],
    pub window_resolution: [u32; 2],
    pub output_texture_name: ResourceName,
    pub start: Instant,
    pub passes: Vec<renderer::PassBindings>,
    pub scene_rx: Receiver<ws::SceneUpdate>,
    pub ws_hub: ws::WsHub,
    pub last_good: Arc<Mutex<Option<crate::dsl::SceneDSL>>>,
    pub uniform_scene: Option<crate::dsl::SceneDSL>,
    pub last_pipeline_signature: Option<[u8; 32]>,
    pub follow_scene_resolution_for_window: bool,
}

pub struct App {
    pub shader_space: ShaderSpace,
    pub resolution: [u32; 2],
    pub window_resolution: [u32; 2],
    pub output_texture_name: ResourceName,
    pub color_attachment: Option<egui::TextureId>,
    pub start: Instant,
    pub passes: Vec<renderer::PassBindings>,

    pub scene_rx: Receiver<ws::SceneUpdate>,
    pub ws_hub: ws::WsHub,
    pub last_good: Arc<Mutex<Option<crate::dsl::SceneDSL>>>,
    pub uniform_scene: Option<crate::dsl::SceneDSL>,
    pub last_pipeline_signature: Option<[u8; 32]>,
    pub pipeline_rebuild_count: u64,
    pub uniform_only_update_count: u64,

    pub zoom: f32,
    pub zoom_initialized: bool,
    pub min_zoom: Option<f32>,
    pub pan: egui::Vec2,
    pub pan_start: Option<egui::Pos2>,
    pub pan_zoom_start_zoom: f32,
    pub pan_zoom_start_pan: egui::Vec2,
    pub pan_zoom_target_zoom: f32,
    pub pan_zoom_target_pan: egui::Vec2,
    pub canvas_center_prev: Option<egui::Pos2>,
    pub last_sampled: Option<SampledPixel>,
    pub texture_filter: wgpu::FilterMode,

    pub follow_scene_resolution_for_window: bool,

    pub window_mode: UiWindowMode,
    pub prev_window_mode: UiWindowMode,
    pub ui_sidebar_factor: f32,
    pub did_startup_sidebar_size: bool,

    pub animations: AnimationManager,

    // --- Resource tree sidebar state ---
    pub file_tree_state: FileTreeState,
    pub resource_snapshot: Option<ResourceSnapshot>,
    pub resource_tree_nodes: Vec<FileTreeNode>,
    /// Last pipeline rebuild count when we cached the resource snapshot.
    pub resource_snapshot_generation: u64,
    /// Texture to preview in canvas (None = show main output).
    pub preview_texture_name: Option<rust_wgpu_fiber::ResourceName>,
    /// Registered egui TextureId for preview texture.
    pub preview_color_attachment: Option<egui::TextureId>,
    /// Request one-shot reset of canvas pan/zoom on next canvas frame.
    pub pending_view_reset: bool,
}

impl App {
    pub fn from_init(init: AppInit) -> Self {
        Self {
            shader_space: init.shader_space,
            resolution: init.resolution,
            window_resolution: init.window_resolution,
            output_texture_name: init.output_texture_name,
            color_attachment: None,
            start: init.start,
            passes: init.passes,
            scene_rx: init.scene_rx,
            ws_hub: init.ws_hub,
            last_good: init.last_good,
            uniform_scene: init.uniform_scene,
            last_pipeline_signature: init.last_pipeline_signature,
            pipeline_rebuild_count: 0,
            uniform_only_update_count: 0,
            zoom: 1.0,
            zoom_initialized: false,
            min_zoom: None,
            pan: egui::Vec2::ZERO,
            pan_start: None,
            pan_zoom_start_zoom: 1.0,
            pan_zoom_start_pan: egui::Vec2::ZERO,
            pan_zoom_target_zoom: 1.0,
            pan_zoom_target_pan: egui::Vec2::ZERO,
            canvas_center_prev: None,
            last_sampled: None,
            texture_filter: wgpu::FilterMode::Nearest,
            follow_scene_resolution_for_window: init.follow_scene_resolution_for_window,
            window_mode: UiWindowMode::Sidebar,
            prev_window_mode: UiWindowMode::Sidebar,
            ui_sidebar_factor: 1.0,
            did_startup_sidebar_size: false,
            animations: AnimationManager::default(),
            file_tree_state: FileTreeState::default(),
            resource_snapshot: None,
            resource_tree_nodes: Vec::new(),
            resource_snapshot_generation: u64::MAX,
            preview_texture_name: None,
            preview_color_attachment: None,
            pending_view_reset: false,
        }
    }
}
