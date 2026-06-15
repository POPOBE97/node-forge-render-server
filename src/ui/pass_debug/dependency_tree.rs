use std::collections::{HashMap, HashSet};

use crate::renderer::{PassDebugDependencyNode, PassDebugSource, PassDebugSourceRange};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PassDebugDependencyRow {
    pub(crate) depth: usize,
    pub(crate) row_key: String,
    pub(crate) parent_row_key: Option<String>,
    pub(crate) label: String,
    pub(crate) relation_path: String,
    pub(crate) target_id: Option<String>,
    pub(crate) source_range: Option<PassDebugSourceRange>,
    pub(crate) source_jump_range: Option<PassDebugSourceRange>,
    pub(crate) selectable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PassDebugTreeClick {
    pub(crate) row_key: Option<String>,
    pub(crate) target_id: Option<String>,
    pub(crate) source_range: Option<PassDebugSourceRange>,
    pub(crate) toggle_row_key: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct DependencyTreeState {
    pub(crate) rows: Vec<PassDebugDependencyRow>,
    pub(crate) focused_target_id: Option<String>,
    pub(crate) focused_row_key: Option<String>,
    pub(crate) root_target_id: Option<String>,
    pub(crate) expanded_row_keys: HashSet<String>,
    pub(crate) pending_editor_jump: Option<PassDebugSourceRange>,
    pub(crate) pending_reveal_row_key: Option<String>,
    pub(crate) rows_generation: u64,
    pub(crate) expansion_generation: u64,
    pub(crate) filter_text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct DependencyTreeStateChange {
    pub(crate) rows_changed: bool,
    pub(crate) visibility_changed: bool,
}

impl DependencyTreeStateChange {
    fn mark_rows_changed(&mut self) {
        self.rows_changed = true;
    }

    fn mark_visibility_changed(&mut self) {
        self.visibility_changed = true;
    }

    fn merge(&mut self, other: DependencyTreeStateChange) {
        self.rows_changed |= other.rows_changed;
        self.visibility_changed |= other.visibility_changed;
    }
}

impl DependencyTreeState {
    pub(crate) fn refresh_from_source(
        &mut self,
        source: Option<&PassDebugSource>,
    ) -> DependencyTreeStateChange {
        let mut change = self.ensure_navigation_targets(source);
        self.rows = source
            .and_then(|source| {
                self.root_target_id.as_ref().and_then(|target_id| {
                    source
                        .dependency_trees
                        .get(target_id)
                        .map(|tree| flatten_dependency_tree(tree, source))
                })
            })
            .unwrap_or_default();
        self.rows_generation = self.rows_generation.wrapping_add(1);
        change.mark_rows_changed();
        self.ensure_focused_row();
        self.prune_expansion(&mut change);
        self.ensure_root_expanded(&mut change);
        change
    }

    pub(crate) fn focus_target(
        &mut self,
        source: Option<&PassDebugSource>,
        target_id: impl Into<String>,
        jump_editor: bool,
    ) -> DependencyTreeStateChange {
        let target_id = target_id.into();
        let Some(source) = source else {
            return DependencyTreeStateChange::default();
        };
        self.focus_target_inner(source, target_id, jump_editor)
    }

    pub(crate) fn focus_target_from_editor(
        &mut self,
        source: Option<&PassDebugSource>,
        target_id: impl Into<String>,
    ) -> DependencyTreeStateChange {
        let target_id = target_id.into();
        let Some(source) = source else {
            return DependencyTreeStateChange::default();
        };
        let mut change = self.focus_target_inner(source, target_id.clone(), false);
        if let Some(row_key) = self.shortest_row_key_for_target(&target_id) {
            self.focused_row_key = Some(row_key.clone());
            self.pending_reveal_row_key = Some(row_key.clone());
            change.merge(self.reveal_row_key(&row_key, true));
        }
        change
    }

    pub(crate) fn focus_tree_click(
        &mut self,
        source: Option<&PassDebugSource>,
        click: PassDebugTreeClick,
    ) -> DependencyTreeStateChange {
        if let Some(row_key) = click.toggle_row_key {
            self.toggle_row_expanded(&row_key)
        } else if let Some(row_key) = click.row_key {
            let jump_override = click.source_range;
            let change = self.focus_row_key(row_key, jump_override.is_none(), false);
            if let Some(source_range) = jump_override {
                self.pending_editor_jump = Some(source_range);
            }
            change
        } else if let Some(target_id) = click.target_id {
            self.focus_target(source, target_id, true)
        } else {
            if let Some(source_range) = click.source_range {
                self.pending_editor_jump = Some(source_range);
            }
            DependencyTreeStateChange::default()
        }
    }

    pub(crate) fn focus_row_key(
        &mut self,
        row_key: impl Into<String>,
        jump_editor: bool,
        reveal_row: bool,
    ) -> DependencyTreeStateChange {
        let row_key = row_key.into();
        let Some(row) = self.rows.iter().find(|row| row.row_key == row_key).cloned() else {
            return DependencyTreeStateChange::default();
        };

        let mut change = DependencyTreeStateChange::default();
        self.focused_row_key = Some(row_key.clone());
        if reveal_row {
            self.pending_reveal_row_key = Some(row_key.clone());
            change.merge(self.reveal_row_key(&row_key, false));
        }
        if let Some(target_id) = row.target_id {
            self.focused_target_id = Some(target_id);
        }
        if jump_editor {
            self.pending_editor_jump = row.source_range;
        }
        change
    }

    pub(crate) fn focus_target_at_char_index(
        &mut self,
        source: Option<&PassDebugSource>,
        draft_source: &str,
        char_index: usize,
    ) -> DependencyTreeStateChange {
        let byte_index = char_index_to_byte_index(draft_source, char_index);
        let matching_dependency_row_key = self
            .rows
            .iter()
            .filter_map(|row| {
                let range = row.source_range?;
                if range.start_byte <= byte_index && byte_index < range.end_byte {
                    Some((
                        range.end_byte.saturating_sub(range.start_byte),
                        row.depth,
                        row.row_key.clone(),
                    ))
                } else {
                    None
                }
            })
            .min_by(
                |(left_len, left_depth, left_key), (right_len, right_depth, right_key)| {
                    right_depth
                        .cmp(left_depth)
                        .then_with(|| left_len.cmp(right_len))
                        .then_with(|| left_key.cmp(right_key))
                },
            )
            .map(|(_, _, row_key)| row_key);
        if let Some(row_key) = matching_dependency_row_key {
            return self.focus_row_key(row_key, false, false);
        }

        let matching_target_id = source.and_then(|source| {
            source
                .dependency_targets
                .iter()
                .find(|target| {
                    target
                        .source_range
                        .map(|range| range.start_byte <= byte_index && byte_index < range.end_byte)
                        .unwrap_or(false)
                })
                .map(|target| target.id.clone())
        });

        if let Some(target_id) = matching_target_id {
            return self.focus_target_from_editor(source, target_id);
        }

        let matching_target_id =
            identifier_at_char_index(draft_source, char_index).and_then(|identifier| {
                source.and_then(|source| {
                    source
                        .dependency_targets
                        .iter()
                        .find(|target| target.name == identifier)
                        .map(|target| target.id.clone())
                })
            });
        if let Some(target_id) = matching_target_id {
            return self.focus_target_from_editor(source, target_id);
        }

        DependencyTreeStateChange::default()
    }

    pub(crate) fn focused_source_range(
        &self,
        source: Option<&PassDebugSource>,
    ) -> Option<PassDebugSourceRange> {
        if let Some(row_source_range) = self
            .focused_row_key
            .as_deref()
            .and_then(|row_key| dependency_row_source_range(&self.rows, row_key))
        {
            return Some(row_source_range);
        }

        let source = source?;
        let focused_target_id = self.focused_target_id.as_deref()?;
        source
            .dependency_targets
            .iter()
            .find(|target| target.id == focused_target_id)
            .and_then(|target| target.source_range)
    }

    pub(crate) fn focus_is_in_root(&self) -> bool {
        if self.focused_target_id.is_none() {
            return true;
        }
        let Some(row_key) = self.focused_row_key.as_deref() else {
            return false;
        };
        self.rows.iter().any(|row| row.row_key == row_key)
    }

    pub(crate) fn expandable_row_keys(&self) -> HashSet<String> {
        dependency_expandable_row_keys(&self.rows)
    }

    pub(crate) fn visible_row_indices(&self) -> Vec<usize> {
        visible_dependency_row_indices(&self.rows, &self.expanded_row_keys)
    }

    pub(crate) fn focus_path_row_keys(&self) -> Vec<String> {
        let Some(row_key) = self.focused_row_key.as_deref() else {
            return Vec::new();
        };
        dependency_path_for_row_key(&self.rows, row_key)
    }

    pub(crate) fn consume_reveal_row_key(&mut self) -> Option<String> {
        let row_key = self.pending_reveal_row_key.clone()?;
        if self.rows.iter().any(|row| row.row_key == row_key) {
            self.pending_reveal_row_key = None;
            Some(row_key)
        } else {
            None
        }
    }

    pub(crate) fn take_pending_editor_jump(&mut self) -> Option<PassDebugSourceRange> {
        self.pending_editor_jump.take()
    }

    pub(crate) fn shortest_row_key_for_target(&self, target_id: &str) -> Option<String> {
        shortest_dependency_row_key_for_target(&self.rows, target_id)
    }

    fn ensure_navigation_targets(
        &mut self,
        source: Option<&PassDebugSource>,
    ) -> DependencyTreeStateChange {
        let mut change = DependencyTreeStateChange::default();
        let Some(source) = source else {
            self.focused_target_id = None;
            self.focused_row_key = None;
            self.root_target_id = None;
            if !self.expanded_row_keys.is_empty() {
                self.expanded_row_keys.clear();
                self.expansion_generation = self.expansion_generation.wrapping_add(1);
                change.mark_visibility_changed();
            }
            self.pending_editor_jump = None;
            self.pending_reveal_row_key = None;
            return change;
        };

        let next_root_target_id = source
            .dependency_root_target_id
            .clone()
            .filter(|target_id| target_exists(source, Some(target_id)))
            .or_else(|| {
                source
                    .dependency_targets
                    .first()
                    .map(|target| target.id.clone())
            });
        let focused_target_exists = target_exists(source, self.focused_target_id.as_deref());
        let fallback_focus_target_id = next_root_target_id.clone().or_else(|| {
            source
                .dependency_targets
                .first()
                .map(|target| target.id.clone())
        });
        if self.root_target_id != next_root_target_id {
            self.root_target_id = next_root_target_id;
            self.expanded_row_keys.clear();
            self.expansion_generation = self.expansion_generation.wrapping_add(1);
            change.mark_visibility_changed();
        }

        if !focused_target_exists {
            self.focused_target_id = fallback_focus_target_id;
        }
        change
    }

    fn focus_target_inner(
        &mut self,
        source: &PassDebugSource,
        target_id: String,
        jump_editor: bool,
    ) -> DependencyTreeStateChange {
        let mut change = DependencyTreeStateChange::default();
        if let Some(source_range) = source
            .dependency_targets
            .iter()
            .find(|target| target.id == target_id)
            .and_then(|target| target.source_range)
        {
            self.focused_target_id = Some(target_id.clone());
            if let Some(row_key) = self.shortest_row_key_for_target(&target_id) {
                self.focused_row_key = Some(row_key.clone());
                self.pending_reveal_row_key = Some(row_key.clone());
                change.merge(self.reveal_row_key(&row_key, false));
            } else {
                self.focused_row_key = None;
            }
            if jump_editor {
                self.pending_editor_jump = Some(source_range);
            }
        } else if source
            .dependency_targets
            .iter()
            .any(|target| target.id == target_id)
        {
            self.focused_target_id = Some(target_id.clone());
            if let Some(row_key) = self.shortest_row_key_for_target(&target_id) {
                self.focused_row_key = Some(row_key.clone());
                self.pending_reveal_row_key = Some(row_key.clone());
                change.merge(self.reveal_row_key(&row_key, false));
            } else {
                self.focused_row_key = None;
            }
        }
        change
    }

    fn ensure_focused_row(&mut self) {
        let focused_row_exists = self
            .focused_row_key
            .as_deref()
            .map(|row_key| self.rows.iter().any(|row| row.row_key == row_key))
            .unwrap_or(false);
        if focused_row_exists {
            return;
        }

        self.focused_row_key = self
            .focused_target_id
            .as_deref()
            .and_then(|target_id| self.shortest_row_key_for_target(target_id));
    }

    fn ensure_root_expanded(&mut self, change: &mut DependencyTreeStateChange) {
        if let Some(root_row_key) = self.rows.first().map(|row| row.row_key.clone())
            && self.expanded_row_keys.insert(root_row_key)
        {
            self.expansion_generation = self.expansion_generation.wrapping_add(1);
            change.mark_visibility_changed();
        }
    }

    fn prune_expansion(&mut self, change: &mut DependencyTreeStateChange) {
        let expandable_row_keys = self.expandable_row_keys();
        let before_len = self.expanded_row_keys.len();
        self.expanded_row_keys
            .retain(|row_key| expandable_row_keys.contains(row_key));
        if self.expanded_row_keys.len() != before_len {
            self.expansion_generation = self.expansion_generation.wrapping_add(1);
            change.mark_visibility_changed();
        }
    }

    pub(crate) fn toggle_row_expanded(&mut self, row_key: &str) -> DependencyTreeStateChange {
        let expandable_row_keys = self.expandable_row_keys();
        if !expandable_row_keys.contains(row_key) {
            return DependencyTreeStateChange::default();
        }
        if !self.expanded_row_keys.remove(row_key) {
            self.expanded_row_keys.insert(row_key.to_string());
        }
        self.expansion_generation = self.expansion_generation.wrapping_add(1);
        DependencyTreeStateChange {
            rows_changed: false,
            visibility_changed: true,
        }
    }

    pub(crate) fn reveal_row_key(
        &mut self,
        row_key: &str,
        collapse_to_path: bool,
    ) -> DependencyTreeStateChange {
        let path = dependency_path_for_row_key(&self.rows, row_key);
        if path.is_empty() {
            return DependencyTreeStateChange::default();
        }
        let expandable_row_keys = self.expandable_row_keys();
        let ancestor_keys = path
            .iter()
            .take(path.len().saturating_sub(1))
            .filter(|row_key| expandable_row_keys.contains(*row_key))
            .cloned()
            .collect::<HashSet<_>>();
        let before = self.expanded_row_keys.clone();
        if collapse_to_path {
            self.expanded_row_keys = ancestor_keys;
        } else {
            self.expanded_row_keys.extend(ancestor_keys);
        }

        let mut change = DependencyTreeStateChange::default();
        if self.expanded_row_keys != before {
            self.expansion_generation = self.expansion_generation.wrapping_add(1);
            change.mark_visibility_changed();
        }
        self.ensure_root_expanded(&mut change);
        change
    }
}

pub(crate) trait PassDebugTreeRow {
    fn depth(&self) -> usize;
    fn row_key(&self) -> Option<&str>;
    fn label(&self) -> &str;
    fn relation_path(&self) -> Option<&str>;
    fn target_id(&self) -> Option<&str>;
    fn source_range(&self) -> Option<PassDebugSourceRange>;
    fn source_jump_range(&self) -> Option<PassDebugSourceRange>;
    fn selectable(&self) -> bool;
}

impl PassDebugTreeRow for PassDebugDependencyRow {
    fn depth(&self) -> usize {
        self.depth
    }

    fn row_key(&self) -> Option<&str> {
        Some(&self.row_key)
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn relation_path(&self) -> Option<&str> {
        if self.relation_path.is_empty() {
            None
        } else {
            Some(&self.relation_path)
        }
    }

    fn target_id(&self) -> Option<&str> {
        self.target_id.as_deref()
    }

    fn source_range(&self) -> Option<PassDebugSourceRange> {
        self.source_range
    }

    fn source_jump_range(&self) -> Option<PassDebugSourceRange> {
        self.source_jump_range
    }

    fn selectable(&self) -> bool {
        self.selectable
    }
}

pub(crate) fn target_exists(source: &PassDebugSource, target_id: Option<&str>) -> bool {
    let Some(target_id) = target_id else {
        return false;
    };
    source
        .dependency_targets
        .iter()
        .any(|target| target.id == target_id)
}

pub(crate) fn flatten_dependency_tree(
    root: &PassDebugDependencyNode,
    source: &PassDebugSource,
) -> Vec<PassDebugDependencyRow> {
    let mut rows = Vec::new();
    push_dependency_rows(
        root,
        source,
        0,
        None,
        &mut vec![0],
        &mut Vec::new(),
        &mut HashSet::new(),
        &mut rows,
    );
    rows
}

fn push_dependency_rows(
    node: &PassDebugDependencyNode,
    source: &PassDebugSource,
    depth: usize,
    parent_row_key: Option<String>,
    path: &mut Vec<usize>,
    relation_path: &mut Vec<String>,
    reference_stack: &mut HashSet<String>,
    rows: &mut Vec<PassDebugDependencyRow>,
) {
    if node.target_id.is_some() {
        let row_key = dependency_row_key(path);
        let relation_path_text = relation_path.join(" / ");
        let target_id = node.target_id.clone();
        let label = dependency_target_row_label(
            source,
            target_id.as_deref(),
            &node.label,
            node.display_label.as_deref(),
            node.edge_label.as_deref(),
        );
        let target_range = target_id
            .as_deref()
            .and_then(|target_id| target_source_range(source, target_id));
        let source_range = node.source_range;
        let definition_source_range = node
            .definition_source_range
            .or_else(|| source_range.is_none().then_some(target_range).flatten());
        let source_jump_range = definition_source_range
            .filter(|definition_source_range| source_range != Some(*definition_source_range));
        rows.push(PassDebugDependencyRow {
            depth,
            row_key: row_key.clone(),
            parent_row_key,
            label,
            relation_path: relation_path_text,
            target_id: target_id.clone(),
            source_range,
            source_jump_range,
            selectable: true,
        });
        let reference_children = node
            .reference
            .then(|| target_id.as_deref())
            .flatten()
            .and_then(|target_id| {
                if reference_stack.insert(target_id.to_string()) {
                    source
                        .dependency_trees
                        .get(target_id)
                        .map(|tree| (target_id.to_string(), tree.children.as_slice()))
                } else {
                    None
                }
            });
        let children = reference_children
            .as_ref()
            .map(|(_, children)| *children)
            .unwrap_or_else(|| node.children.as_slice());
        for (index, child) in children.iter().enumerate() {
            path.push(index);
            let mut child_relation_path = Vec::new();
            push_dependency_rows(
                child,
                source,
                depth + 1,
                Some(row_key.clone()),
                path,
                &mut child_relation_path,
                reference_stack,
                rows,
            );
            path.pop();
        }
        if let Some((target_id, _)) = reference_children {
            reference_stack.remove(&target_id);
        }
    } else {
        let relation_label = compact_dependency_relation_label(&node.label);
        if !relation_label.is_empty() {
            relation_path.push(relation_label);
        }
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            push_dependency_rows(
                child,
                source,
                depth,
                parent_row_key.clone(),
                path,
                relation_path,
                reference_stack,
                rows,
            );
            path.pop();
        }
        if !relation_path.is_empty() {
            relation_path.pop();
        }
    }
}

fn dependency_target_row_label(
    source: &PassDebugSource,
    target_id: Option<&str>,
    fallback_label: &str,
    display_label: Option<&str>,
    edge_label: Option<&str>,
) -> String {
    let fallback_label = clean_debug_tree_row_label(fallback_label);
    let base_label = display_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            target_id
                .and_then(|target_id| {
                    source
                        .dependency_targets
                        .iter()
                        .find(|target| target.id == target_id)
                })
                .map(|target| target.name.clone())
        })
        .unwrap_or_else(|| fallback_label.clone());
    let status = ["[cycle]", "[depth limit]"]
        .into_iter()
        .find(|status| fallback_label.contains(status));
    let mut label = match edge_label.map(str::trim).filter(|edge| !edge.is_empty()) {
        Some(edge) => format!("{base_label} ({edge})"),
        None => base_label,
    };
    if let Some(status) = status {
        label.push(' ');
        label.push_str(status);
    }
    label
}

fn clean_debug_tree_row_label(label: &str) -> String {
    let Some(stripped) = strip_leading_naga_handle(label.trim_start()) else {
        return label.to_string();
    };
    stripped.trim_start().to_string()
}

fn strip_leading_naga_handle(label: &str) -> Option<&str> {
    let rest = label.strip_prefix('[')?;
    let (handle, after_handle) = rest.split_once(']')?;
    if handle.is_empty() || !handle.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(after_handle.strip_prefix(':').unwrap_or(after_handle))
}

fn dependency_row_key(path: &[usize]) -> String {
    path.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

fn compact_dependency_relation_label(label: &str) -> String {
    let label = clean_debug_tree_row_label(label);
    let label = label.trim();
    if let Some(rest) = label.strip_prefix('[')
        && let Some((edge, after_edge)) = rest.split_once(']')
    {
        return format!("{edge}{}", after_edge).trim().to_string();
    }
    label.to_string()
}

fn target_source_range(source: &PassDebugSource, target_id: &str) -> Option<PassDebugSourceRange> {
    source
        .dependency_targets
        .iter()
        .find(|target| target.id == target_id)
        .and_then(|target| target.source_range)
}

pub(crate) fn dependency_path_for_row_key(
    rows: &[PassDebugDependencyRow],
    row_key: &str,
) -> Vec<String> {
    if !rows.iter().any(|row| row.row_key == row_key) {
        return Vec::new();
    }
    let row_parent_by_key = rows
        .iter()
        .map(|row| (row.row_key.as_str(), row.parent_row_key.as_deref()))
        .collect::<HashMap<_, _>>();
    let mut path = Vec::new();
    let mut current = Some(row_key);
    while let Some(row_key) = current {
        path.push(row_key.to_string());
        current = row_parent_by_key.get(row_key).copied().flatten();
    }
    path.reverse();
    path
}

pub(crate) fn dependency_expandable_row_keys(rows: &[PassDebugDependencyRow]) -> HashSet<String> {
    rows.iter()
        .filter_map(|row| row.parent_row_key.clone())
        .collect()
}

pub(crate) fn visible_dependency_row_indices(
    rows: &[PassDebugDependencyRow],
    expanded_row_keys: &HashSet<String>,
) -> Vec<usize> {
    let mut visible_rows = Vec::new();
    let mut hidden_depth: Option<usize> = None;
    for (row_index, row) in rows.iter().enumerate() {
        if let Some(depth) = hidden_depth {
            if row.depth > depth {
                continue;
            }
            hidden_depth = None;
        }

        visible_rows.push(row_index);
        if rows
            .iter()
            .any(|child| child.parent_row_key.as_deref() == Some(row.row_key.as_str()))
            && !expanded_row_keys.contains(&row.row_key)
        {
            hidden_depth = Some(row.depth);
        }
    }
    visible_rows
}

pub(crate) fn filtered_dependency_row_indices(
    rows: &[PassDebugDependencyRow],
    filter_text: &str,
) -> Option<Vec<usize>> {
    if filter_text.is_empty() {
        return None;
    }

    let filter_lower = filter_text.to_lowercase();
    let matched_row_keys = rows
        .iter()
        .filter(|row| row.label.to_lowercase().contains(&filter_lower))
        .map(|row| row.row_key.clone())
        .collect::<HashSet<_>>();

    let mut keep_row_keys = matched_row_keys.clone();
    for row_key in &matched_row_keys {
        let mut current = rows
            .iter()
            .find(|row| &row.row_key == row_key)
            .and_then(|row| row.parent_row_key.clone());
        while let Some(parent_key) = current {
            if !keep_row_keys.insert(parent_key.clone()) {
                break;
            }
            current = rows
                .iter()
                .find(|row| row.row_key == parent_key)
                .and_then(|row| row.parent_row_key.clone());
        }
    }

    Some(
        rows.iter()
            .enumerate()
            .filter(|(_, row)| keep_row_keys.contains(&row.row_key))
            .map(|(index, _)| index)
            .collect(),
    )
}

pub(crate) fn shortest_dependency_row_key_for_target(
    rows: &[PassDebugDependencyRow],
    target_id: &str,
) -> Option<String> {
    rows.iter()
        .filter(|row| row.target_id.as_deref() == Some(target_id))
        .map(|row| (row.depth, row.row_key.clone()))
        .min_by(|(left_depth, left_key), (right_depth, right_key)| {
            left_depth
                .cmp(right_depth)
                .then_with(|| left_key.cmp(right_key))
        })
        .map(|(_, row_key)| row_key)
}

pub(crate) fn dependency_row_source_range(
    rows: &[PassDebugDependencyRow],
    row_key: &str,
) -> Option<PassDebugSourceRange> {
    rows.iter()
        .find(|row| row.row_key == row_key)
        .and_then(|row| row.source_range)
}

pub(crate) fn identifier_at_char_index(source: &str, char_index: usize) -> Option<String> {
    let byte_index = char_index_to_byte_index(source, char_index);
    if source.is_empty() || byte_index > source.len() {
        return None;
    }

    let mut start = byte_index.min(source.len());
    while start > 0 {
        let Some((prev_index, ch)) = source[..start].char_indices().next_back() else {
            break;
        };
        if is_wgsl_identifier_continue(ch) {
            start = prev_index;
        } else {
            break;
        }
    }

    let mut end = byte_index.min(source.len());
    while end < source.len() {
        let Some(ch) = source[end..].chars().next() else {
            break;
        };
        if is_wgsl_identifier_continue(ch) {
            end += ch.len_utf8();
        } else {
            break;
        }
    }

    if start == end {
        return None;
    }
    let identifier = &source[start..end];
    identifier
        .chars()
        .next()
        .filter(|ch| is_wgsl_identifier_start(*ch))
        .map(|_| identifier.to_string())
}

fn is_wgsl_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_wgsl_identifier_continue(ch: char) -> bool {
    is_wgsl_identifier_start(ch) || ch.is_ascii_digit()
}

pub(crate) fn char_index_to_byte_index(source: &str, char_index: usize) -> usize {
    source
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(source.len())
}

pub(crate) fn byte_index_to_char_index(source: &str, byte_index: usize) -> usize {
    source[..byte_index.min(source.len())].chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn range(start_byte: usize, end_byte: usize) -> PassDebugSourceRange {
        PassDebugSourceRange {
            start_byte,
            end_byte,
            line: 0,
            column: 0,
        }
    }

    #[test]
    fn flatten_dependency_tree_uses_canonical_targets_and_stable_row_keys() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
@fragment
fn fs_main() -> @location(0) f32 {
    let a = 1.0;
    let b = a + 2.0;
    return b;
}
"#,
        );
        let root_id = source
            .dependency_root_target_id
            .as_deref()
            .expect("root target");
        let root = source.dependency_trees.get(root_id).expect("root tree");

        let rows = flatten_dependency_tree(root, &source);

        assert!(!rows.is_empty());
        assert_eq!(rows[0].row_key, "0");
        assert!(rows.iter().any(|row| row.label == "b"));
        assert!(
            dependency_path_for_row_key(&rows, rows.last().unwrap().row_key.as_str())
                .first()
                .is_some_and(|row_key| row_key == "0")
        );
    }

    #[test]
    fn identifier_lookup_respects_wgsl_identifier_boundaries() {
        let source = "let alpha_1 = beta2 + 3.0;";
        let alpha_index = source.find("pha").unwrap();
        let digit_index = source.find("1").unwrap();
        let beta_index = source.find("eta").unwrap();
        let number_index = source.find("3.0").unwrap();

        assert_eq!(
            identifier_at_char_index(source, alpha_index),
            Some("alpha_1".to_string())
        );
        assert_eq!(
            identifier_at_char_index(source, digit_index),
            Some("alpha_1".to_string())
        );
        assert_eq!(
            identifier_at_char_index(source, beta_index),
            Some("beta2".to_string())
        );
        assert_eq!(identifier_at_char_index(source, number_index), None);
    }

    #[test]
    fn char_and_byte_indices_round_trip_multibyte_text() {
        let source = "aé中b";
        let b_char_index = 3;
        let b_byte_index = source.find('b').unwrap();

        assert_eq!(char_index_to_byte_index(source, b_char_index), b_byte_index);
        assert_eq!(byte_index_to_char_index(source, b_byte_index), b_char_index);
        assert_eq!(char_index_to_byte_index(source, usize::MAX), source.len());
        assert_eq!(
            byte_index_to_char_index(source, usize::MAX),
            source.chars().count()
        );
    }

    #[test]
    fn row_selectors_find_expandable_visible_and_shortest_rows() {
        let rows = vec![
            PassDebugDependencyRow {
                depth: 0,
                row_key: "0".to_string(),
                parent_row_key: None,
                label: "root".to_string(),
                relation_path: String::new(),
                target_id: Some("root".to_string()),
                source_range: Some(range(0, 4)),
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 1,
                row_key: "0/0".to_string(),
                parent_row_key: Some("0".to_string()),
                label: "deep".to_string(),
                relation_path: String::new(),
                target_id: Some("shared".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 0,
                row_key: "1".to_string(),
                parent_row_key: None,
                label: "shallow".to_string(),
                relation_path: String::new(),
                target_id: Some("shared".to_string()),
                source_range: Some(range(10, 16)),
                source_jump_range: None,
                selectable: true,
            },
        ];

        assert_eq!(
            dependency_expandable_row_keys(&rows),
            HashSet::from(["0".to_string()])
        );
        assert_eq!(
            visible_dependency_row_indices(&rows, &HashSet::new()),
            vec![0, 2]
        );
        assert_eq!(
            visible_dependency_row_indices(&rows, &HashSet::from(["0".to_string()])),
            vec![0, 1, 2]
        );
        assert_eq!(
            shortest_dependency_row_key_for_target(&rows, "shared").as_deref(),
            Some("1")
        );
        assert_eq!(dependency_row_source_range(&rows, "1"), Some(range(10, 16)));
    }

    #[test]
    fn filter_selector_keeps_matching_rows_and_ancestors() {
        let rows = vec![
            PassDebugDependencyRow {
                depth: 0,
                row_key: "0".to_string(),
                parent_row_key: None,
                label: "root".to_string(),
                relation_path: String::new(),
                target_id: Some("root".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 1,
                row_key: "0/0".to_string(),
                parent_row_key: Some("0".to_string()),
                label: "lighting".to_string(),
                relation_path: String::new(),
                target_id: Some("lighting".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 2,
                row_key: "0/0/0".to_string(),
                parent_row_key: Some("0/0".to_string()),
                label: "final_alpha".to_string(),
                relation_path: String::new(),
                target_id: Some("alpha".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 0,
                row_key: "1".to_string(),
                parent_row_key: None,
                label: "unrelated".to_string(),
                relation_path: String::new(),
                target_id: Some("other".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
        ];

        assert_eq!(filtered_dependency_row_indices(&rows, ""), None);
        assert_eq!(
            filtered_dependency_row_indices(&rows, "ALPHA"),
            Some(vec![0, 1, 2])
        );
        assert_eq!(
            filtered_dependency_row_indices(&rows, "missing"),
            Some(Vec::new())
        );
    }
}
