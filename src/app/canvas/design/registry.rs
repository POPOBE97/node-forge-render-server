use crate::ui::resource_tree::PassDesignTarget;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanvasDesignToolKind {
    MeshGradient,
    IntelligentLight,
}

pub fn tool_kind_for_target(target: &PassDesignTarget) -> Option<CanvasDesignToolKind> {
    match target.node_type.as_str() {
        "MeshGradient" => Some(CanvasDesignToolKind::MeshGradient),
        "IntelligentLight" => Some(CanvasDesignToolKind::IntelligentLight),
        _ => None,
    }
}
