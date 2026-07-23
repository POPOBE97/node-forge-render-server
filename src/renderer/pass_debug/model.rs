use serde::Serialize;

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugSourceRange {
    pub start_byte: usize,
    pub end_byte: usize,
    pub line: u32,
    pub column: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugAstNode {
    pub label: String,
    pub target_id: Option<String>,
    pub role: Option<String>,
    pub source_range: Option<PassDebugSourceRange>,
    pub children: Vec<PassDebugAstNode>,
}

impl PassDebugAstNode {
    pub(super) fn leaf(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            target_id: None,
            role: None,
            source_range: None,
            children: Vec::new(),
        }
    }

    pub(super) fn branch(label: impl Into<String>, children: Vec<Self>) -> Self {
        Self {
            label: label.into(),
            target_id: None,
            role: None,
            source_range: None,
            children,
        }
    }

    pub(super) fn with_source_range(mut self, source_range: Option<PassDebugSourceRange>) -> Self {
        self.source_range = source_range;
        self
    }

    pub(super) fn with_target_range(
        mut self,
        target_id: impl Into<String>,
        role: impl Into<String>,
        source_range: Option<PassDebugSourceRange>,
    ) -> Self {
        self.target_id = Some(target_id.into());
        self.role = Some(role.into());
        self.source_range = source_range;
        self
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugDependencyTarget {
    pub id: String,
    pub name: String,
    pub label: String,
    pub scope: String,
    pub kind: String,
    pub source_range: Option<PassDebugSourceRange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct PassDebugDependencyNode {
    pub label: String,
    pub edge_label: Option<String>,
    pub display_label: Option<String>,
    pub source_range: Option<PassDebugSourceRange>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub definition_source_range: Option<PassDebugSourceRange>,
    pub target_id: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    pub reference: bool,
    pub children: Vec<PassDebugDependencyNode>,
}

impl PassDebugDependencyNode {
    pub(super) fn leaf(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            edge_label: None,
            display_label: None,
            source_range: None,
            definition_source_range: None,
            target_id: None,
            reference: false,
            children: Vec::new(),
        }
    }

    pub(super) fn branch(label: impl Into<String>, children: Vec<Self>) -> Self {
        Self {
            children,
            ..Self::leaf(label)
        }
    }

    pub(super) fn target(
        label: impl Into<String>,
        target_id: impl Into<String>,
        children: Vec<Self>,
    ) -> Self {
        Self {
            target_id: Some(target_id.into()),
            children,
            ..Self::leaf(label)
        }
    }

    pub(super) fn target_reference(label: impl Into<String>, target_id: impl Into<String>) -> Self {
        Self::target(label, target_id, Vec::new()).with_reference()
    }

    pub(super) fn with_reference(mut self) -> Self {
        self.reference = true;
        self
    }

    pub(super) fn with_edge_label(mut self, edge_label: Option<String>) -> Self {
        self.edge_label = edge_label;
        self
    }

    pub(super) fn with_display_label(mut self, display_label: Option<String>) -> Self {
        self.display_label = display_label;
        self
    }

    pub(super) fn with_source_range(mut self, source_range: Option<PassDebugSourceRange>) -> Self {
        self.source_range = source_range;
        self
    }

    pub(super) fn with_definition_source_range(
        mut self,
        definition_source_range: Option<PassDebugSourceRange>,
    ) -> Self {
        self.definition_source_range = definition_source_range;
        self
    }
}
