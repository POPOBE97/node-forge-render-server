use crate::ui::resource_tree::PassDesignTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasDesignToolKind {
    MeshGradient,
}

pub fn tool_kind_for_target(target: &PassDesignTarget) -> Option<CanvasDesignToolKind> {
    match target.node_type.as_str() {
        "MeshGradient" => Some(CanvasDesignToolKind::MeshGradient),
        _ => None,
    }
}
