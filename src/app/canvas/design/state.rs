use std::collections::HashMap;

use rust_wgpu_fiber::ResourceName;
use serde_json::Value;

use crate::ui::{color_popover::ColorPopoverState, resource_tree::PassDesignTarget};

#[derive(Default)]
pub struct CanvasDesignState {
    pub active: Option<CanvasDesignSession>,
}

#[derive(Clone, Debug)]
pub struct CanvasDesignSession {
    pub target: PassDesignTarget,
    pub session_id: String,
    pub previous_preview_texture: Option<ResourceName>,
    pub owns_preview_texture: bool,
    pub tool: CanvasDesignToolState,
}

#[derive(Clone, Debug)]
pub enum CanvasDesignToolState {
    MeshGradient(MeshGradientDesignState),
}

#[derive(Clone, Debug)]
pub struct MeshGradientDesignState {
    pub selected_point: usize,
    pub active_drag_point: Option<usize>,
    pub color_popover_point: Option<usize>,
    pub color_popover_state: ColorPopoverState,
    pub optimistic_params: HashMap<String, Value>,
}

impl Default for MeshGradientDesignState {
    fn default() -> Self {
        Self {
            selected_point: 4,
            active_drag_point: None,
            color_popover_point: None,
            color_popover_state: ColorPopoverState::default(),
            optimistic_params: HashMap::new(),
        }
    }
}
