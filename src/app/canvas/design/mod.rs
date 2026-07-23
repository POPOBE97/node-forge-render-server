pub mod intelligent_light;
pub mod interaction;
pub mod mesh_gradient;
pub mod registry;
pub mod state;

use rust_wgpu_fiber::{
    ResourceName,
    eframe::{egui, wgpu},
};

pub use interaction::{
    DesignInteractionClaims, DesignOverlayInput, DesignOverlayOutput, DesignOverlayStatus,
};
use registry::{CanvasDesignToolKind, tool_kind_for_target};
pub use state::{CanvasDesignSession, CanvasDesignState, CanvasDesignToolState};
use state::{IntelligentLightDesignState, MeshGradientDesignState};

use crate::ui::resource_tree::PassDesignTarget;

pub fn enter_session(
    current: Option<CanvasDesignSession>,
    target: PassDesignTarget,
    previous_preview_texture: Option<ResourceName>,
    previous_texture_filter: wgpu::FilterMode,
) -> Option<CanvasDesignSession> {
    let tool_kind = tool_kind_for_target(&target)?;
    let (previous_preview_texture, previous_texture_filter, already_owns_preview) = current
        .map(|session| {
            (
                session.previous_preview_texture,
                session.previous_texture_filter,
                session.owns_preview_texture,
            )
        })
        .unwrap_or((previous_preview_texture, previous_texture_filter, false));
    let owns_preview_texture = already_owns_preview || target.target_texture.is_some();
    let session_id = format!("design:{}:{}", target.node_id, target.pass_name);
    let tool = match tool_kind {
        CanvasDesignToolKind::MeshGradient => {
            CanvasDesignToolState::MeshGradient(MeshGradientDesignState::default())
        }
        CanvasDesignToolKind::IntelligentLight => {
            CanvasDesignToolState::IntelligentLight(IntelligentLightDesignState::default())
        }
    };

    Some(CanvasDesignSession {
        target,
        session_id,
        previous_preview_texture,
        previous_texture_filter,
        owns_preview_texture,
        tool,
    })
}

pub struct HandleEscapeResult {
    pub consumed: bool,
    pub patches: Vec<crate::protocol::DesignParamPatchPayload>,
}

pub fn handle_escape(session: &mut CanvasDesignSession) -> HandleEscapeResult {
    match &mut session.tool {
        CanvasDesignToolState::MeshGradient(state) => {
            if state.color_popover_point.is_some() {
                state.color_popover_point = None;
                HandleEscapeResult {
                    consumed: true,
                    patches: Vec::new(),
                }
            } else {
                HandleEscapeResult {
                    consumed: false,
                    patches: Vec::new(),
                }
            }
        }
        CanvasDesignToolState::IntelligentLight(state) => {
            if state.color_popover_zone.is_some() {
                let patch = intelligent_light::cancel_color_edit(
                    &session.target,
                    session.session_id.as_str(),
                    state,
                );
                HandleEscapeResult {
                    consumed: true,
                    patches: patch.into_iter().collect(),
                }
            } else {
                HandleEscapeResult {
                    consumed: false,
                    patches: Vec::new(),
                }
            }
        }
    }
}

pub fn sync_session_target_from_snapshot(
    session: &mut CanvasDesignSession,
    resource_snapshot: Option<&crate::ui::resource_tree::ResourceSnapshot>,
) -> bool {
    let before = session.target.target_texture.clone();
    let Some(pass) = resource_snapshot.and_then(|snapshot| {
        snapshot
            .passes
            .iter()
            .find(|pass| pass.name == session.target.pass_name)
    }) else {
        return false;
    };

    if let Some(target_size) = pass.target_size {
        session.target.target_size = Some(target_size);
    }
    if pass.target_texture.is_some() {
        session.target.target_texture = pass.target_texture.clone();
    }

    before != session.target.target_texture
}

pub fn show_active_overlay(
    ui: &mut egui::Ui,
    ctx: &egui::Context,
    session: &mut CanvasDesignSession,
    input: DesignOverlayInput<'_>,
) -> DesignOverlayOutput {
    match &mut session.tool {
        CanvasDesignToolState::MeshGradient(state) => mesh_gradient::show_overlay(
            ui,
            ctx,
            &session.target,
            session.session_id.as_str(),
            state,
            input,
        ),
        CanvasDesignToolState::IntelligentLight(state) => intelligent_light::show_overlay(
            ui,
            ctx,
            &session.target,
            session.session_id.as_str(),
            state,
            input,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::resource_tree::{PassInfo, ResourceSnapshot};

    #[test]
    fn sync_target_from_snapshot_uses_live_pass_target_size() {
        let target = PassDesignTarget {
            node_id: "mesh".to_string(),
            node_type: "MeshGradient".to_string(),
            pass_name: "sys.mesh_gradient.mesh.pass".to_string(),
            target_texture: None,
            target_size: Some((1, 1)),
        };
        let mut session = enter_session(None, target, None, wgpu::FilterMode::Nearest).unwrap();
        let snapshot = ResourceSnapshot {
            passes: vec![PassInfo {
                name: "sys.mesh_gradient.mesh.pass".to_string(),
                display_label: None,
                source_node_id: Some("mesh".to_string()),
                source_node_type: Some("MeshGradient".to_string()),
                order_index: 0,
                target_texture: Some("rt.mesh".to_string()),
                target_size: Some((1280, 720)),
                target_format: None,
                is_compute: false,
                sampled_textures: Vec::new(),
                instance_count: 0,
                vertex_count: 0,
                workgroup_count: 0,
            }],
            buffers: Vec::new(),
            samplers: Vec::new(),
            final_output_texture: None,
        };

        assert!(sync_session_target_from_snapshot(
            &mut session,
            Some(&snapshot)
        ));

        assert_eq!(session.target.target_texture.as_deref(), Some("rt.mesh"));
        assert_eq!(session.target.target_size, Some((1280, 720)));
    }

    #[test]
    fn replacement_session_keeps_original_preview_restore() {
        let first_target = PassDesignTarget {
            node_id: "mesh1".to_string(),
            node_type: "MeshGradient".to_string(),
            pass_name: "pass1".to_string(),
            target_texture: Some("rt.mesh1".to_string()),
            target_size: None,
        };
        let second_target = PassDesignTarget {
            node_id: "mesh2".to_string(),
            node_type: "MeshGradient".to_string(),
            pass_name: "pass2".to_string(),
            target_texture: Some("rt.mesh2".to_string()),
            target_size: None,
        };
        let first = enter_session(
            None,
            first_target,
            Some(ResourceName::from("before.design")),
            wgpu::FilterMode::Nearest,
        )
        .unwrap();
        let second =
            enter_session(Some(first), second_target, None, wgpu::FilterMode::Linear).unwrap();

        assert_eq!(
            second
                .previous_preview_texture
                .as_ref()
                .map(|name| name.as_str()),
            Some("before.design")
        );
        assert!(second.owns_preview_texture);
    }

    #[test]
    fn intelligent_light_targets_open_design_session() {
        let target = PassDesignTarget {
            node_id: "ilight".to_string(),
            node_type: "IntelligentLight".to_string(),
            pass_name: "sys.ilight.ilight.pass".to_string(),
            target_texture: Some("rt.ilight".to_string()),
            target_size: Some((640, 480)),
        };
        let session = enter_session(None, target, None, wgpu::FilterMode::Nearest)
            .expect("ilight design session");

        assert!(matches!(
            session.tool,
            CanvasDesignToolState::IntelligentLight(_)
        ));
    }
}
