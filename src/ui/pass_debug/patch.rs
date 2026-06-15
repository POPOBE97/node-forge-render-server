use serde::{Deserialize, Serialize};

const SHORTWIRE_DIFF_CONTEXT_LINES: usize = 3;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct ShortwireHunk {
    pub(crate) old_start: usize,
    pub(crate) old_lines: Vec<String>,
    pub(crate) new_lines: Vec<String>,
    pub(crate) context_before: Vec<String>,
    pub(crate) context_after: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShortwireDiffRowKind {
    Context,
    Added,
    Removed,
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShortwireDiffRow {
    pub(crate) kind: ShortwireDiffRowKind,
    pub(crate) old_line: Option<usize>,
    pub(crate) new_line: Option<usize>,
    pub(crate) text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShortwireDiffView {
    pub(crate) rows: Vec<ShortwireDiffRow>,
    old_line_count: usize,
    new_line_count: usize,
}

impl ShortwireDiffView {
    pub(crate) fn to_display_text(&self) -> String {
        if self.rows.is_empty() {
            return "No changes\n".to_string();
        }

        let old_width = self.old_line_count.max(1).to_string().len();
        let new_width = self.new_line_count.max(1).to_string().len();
        let mut text = String::new();
        for row in &self.rows {
            if row.kind == ShortwireDiffRowKind::Separator {
                text.push_str(&format!(
                    "{:old_width$} {:new_width$}   ...\n",
                    "",
                    "",
                    old_width = old_width,
                    new_width = new_width
                ));
                continue;
            }

            let old_line = row
                .old_line
                .map(|line| line.to_string())
                .unwrap_or_default();
            let new_line = row
                .new_line
                .map(|line| line.to_string())
                .unwrap_or_default();
            let prefix = match row.kind {
                ShortwireDiffRowKind::Context => " ",
                ShortwireDiffRowKind::Added => "+",
                ShortwireDiffRowKind::Removed => "-",
                ShortwireDiffRowKind::Separator => unreachable!(),
            };
            text.push_str(&format!(
                "{old_line:>old_width$} {new_line:>new_width$} {prefix} {}\n",
                row.text,
                old_width = old_width,
                new_width = new_width
            ));
        }
        text
    }
}

#[derive(Clone, Debug)]
pub(crate) enum HunkApplyError {
    HunkNotFound { hunk_index: usize },
    VerificationFailed { hunk_index: usize },
}

impl std::fmt::Display for HunkApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HunkApplyError::HunkNotFound { hunk_index } => {
                write!(f, "hunk {hunk_index}: could not locate target position")
            }
            HunkApplyError::VerificationFailed { hunk_index } => {
                write!(
                    f,
                    "hunk {hunk_index}: old lines do not match at resolved position"
                )
            }
        }
    }
}

pub(crate) fn compute_hunks(base: &str, edited: &str) -> Vec<ShortwireHunk> {
    use similar::TextDiff;

    if base == edited {
        return Vec::new();
    }

    let diff = TextDiff::from_lines(base, edited);
    let base_lines: Vec<&str> = base.lines().collect();
    let mut hunks = Vec::new();

    for group in diff.grouped_ops(SHORTWIRE_DIFF_CONTEXT_LINES) {
        for op in &group {
            match op {
                similar::DiffOp::Equal { .. } => {}
                similar::DiffOp::Delete {
                    old_index, old_len, ..
                }
                | similar::DiffOp::Replace {
                    old_index, old_len, ..
                } => {
                    let old_start = *old_index;
                    let old_lines_slice: Vec<String> = base_lines[old_start..old_start + old_len]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();

                    let new_lines_slice: Vec<String> = match op {
                        similar::DiffOp::Replace {
                            new_index, new_len, ..
                        } => {
                            let edited_lines: Vec<&str> = edited.lines().collect();
                            edited_lines[*new_index..*new_index + new_len]
                                .iter()
                                .map(|s| s.to_string())
                                .collect()
                        }
                        _ => Vec::new(),
                    };

                    let context_before: Vec<String> = base_lines
                        [old_start.saturating_sub(SHORTWIRE_DIFF_CONTEXT_LINES)..old_start]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    let after_end = (old_start + old_len).min(base_lines.len());
                    let context_after: Vec<String> = base_lines[after_end
                        ..(after_end + SHORTWIRE_DIFF_CONTEXT_LINES).min(base_lines.len())]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();

                    hunks.push(ShortwireHunk {
                        old_start,
                        old_lines: old_lines_slice,
                        new_lines: new_lines_slice,
                        context_before,
                        context_after,
                    });
                }
                similar::DiffOp::Insert {
                    old_index,
                    new_index,
                    new_len,
                } => {
                    let edited_lines: Vec<&str> = edited.lines().collect();
                    let new_lines_slice: Vec<String> = edited_lines
                        [*new_index..*new_index + new_len]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();

                    let context_before: Vec<String> = base_lines
                        [old_index.saturating_sub(SHORTWIRE_DIFF_CONTEXT_LINES)..*old_index]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    let context_after: Vec<String> = base_lines[*old_index
                        ..(*old_index + SHORTWIRE_DIFF_CONTEXT_LINES).min(base_lines.len())]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();

                    hunks.push(ShortwireHunk {
                        old_start: *old_index,
                        old_lines: Vec::new(),
                        new_lines: new_lines_slice,
                        context_before,
                        context_after,
                    });
                }
            }
        }
    }
    hunks
}

pub(crate) fn build_shortwire_diff_view(base: &str, edited: &str) -> ShortwireDiffView {
    use similar::TextDiff;

    let old_lines: Vec<&str> = base.lines().collect();
    let new_lines: Vec<&str> = edited.lines().collect();
    let mut rows = Vec::new();

    if base != edited {
        let diff = TextDiff::from_lines(base, edited);
        for (group_index, group) in diff
            .grouped_ops(SHORTWIRE_DIFF_CONTEXT_LINES)
            .into_iter()
            .enumerate()
        {
            if group_index > 0 {
                rows.push(ShortwireDiffRow {
                    kind: ShortwireDiffRowKind::Separator,
                    old_line: None,
                    new_line: None,
                    text: String::new(),
                });
            }

            for op in group {
                match op {
                    similar::DiffOp::Equal {
                        old_index,
                        new_index,
                        len,
                    } => {
                        for offset in 0..len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Context,
                                old_line: Some(old_index + offset + 1),
                                new_line: Some(new_index + offset + 1),
                                text: old_lines
                                    .get(old_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                    }
                    similar::DiffOp::Delete {
                        old_index, old_len, ..
                    } => {
                        for offset in 0..old_len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Removed,
                                old_line: Some(old_index + offset + 1),
                                new_line: None,
                                text: old_lines
                                    .get(old_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                    }
                    similar::DiffOp::Insert {
                        new_index, new_len, ..
                    } => {
                        for offset in 0..new_len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Added,
                                old_line: None,
                                new_line: Some(new_index + offset + 1),
                                text: new_lines
                                    .get(new_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                    }
                    similar::DiffOp::Replace {
                        old_index,
                        old_len,
                        new_index,
                        new_len,
                    } => {
                        for offset in 0..old_len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Removed,
                                old_line: Some(old_index + offset + 1),
                                new_line: None,
                                text: old_lines
                                    .get(old_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                        for offset in 0..new_len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Added,
                                old_line: None,
                                new_line: Some(new_index + offset + 1),
                                text: new_lines
                                    .get(new_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    ShortwireDiffView {
        rows,
        old_line_count: old_lines.len(),
        new_line_count: new_lines.len(),
    }
}

pub(crate) fn apply_hunks(base: &str, hunks: &[ShortwireHunk]) -> Result<String, HunkApplyError> {
    if hunks.is_empty() {
        return Ok(base.to_string());
    }

    let mut base_lines: Vec<String> = base.lines().map(|s| s.to_string()).collect();

    let mut sorted_indices: Vec<usize> = (0..hunks.len()).collect();
    sorted_indices.sort_by(|a, b| hunks[*b].old_start.cmp(&hunks[*a].old_start));

    for &hunk_index in &sorted_indices {
        let hunk = &hunks[hunk_index];
        let position = locate_hunk_position(&base_lines, hunk, hunk_index)?;

        if !hunk.old_lines.is_empty() {
            if position + hunk.old_lines.len() > base_lines.len() {
                return Err(HunkApplyError::VerificationFailed { hunk_index });
            }
            for (i, old_line) in hunk.old_lines.iter().enumerate() {
                if base_lines[position + i] != *old_line {
                    return Err(HunkApplyError::VerificationFailed { hunk_index });
                }
            }
            base_lines.splice(
                position..position + hunk.old_lines.len(),
                hunk.new_lines.iter().cloned(),
            );
        } else {
            base_lines.splice(position..position, hunk.new_lines.iter().cloned());
        }
    }

    let mut result = base_lines.join("\n");
    if base.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

pub(crate) fn three_way_merge_sources(
    base: &str,
    incoming: &str,
    local: &str,
) -> Result<String, HunkApplyError> {
    if local == base {
        return Ok(incoming.to_string());
    }
    if incoming == base {
        return Ok(local.to_string());
    }

    let local_hunks = compute_hunks(base, local);
    if local_hunks.is_empty() {
        return Ok(incoming.to_string());
    }
    apply_hunks(incoming, &local_hunks)
}

fn locate_hunk_position(
    base_lines: &[String],
    hunk: &ShortwireHunk,
    hunk_index: usize,
) -> Result<usize, HunkApplyError> {
    if verify_hunk_at_position(base_lines, hunk, hunk.old_start) {
        return Ok(hunk.old_start);
    }

    let search_range = 30;
    let start = hunk.old_start.saturating_sub(search_range);
    let end = (hunk.old_start + search_range).min(base_lines.len());

    for offset in 1..=search_range {
        if hunk.old_start + offset < end
            && verify_hunk_at_position(base_lines, hunk, hunk.old_start + offset)
        {
            return Ok(hunk.old_start + offset);
        }
        if hunk.old_start >= offset + start.min(hunk.old_start) {
            let pos = hunk.old_start - offset;
            if pos >= start && verify_hunk_at_position(base_lines, hunk, pos) {
                return Ok(pos);
            }
        }
    }

    Err(HunkApplyError::HunkNotFound { hunk_index })
}

fn verify_hunk_at_position(base_lines: &[String], hunk: &ShortwireHunk, position: usize) -> bool {
    if !hunk.old_lines.is_empty() {
        if position + hunk.old_lines.len() > base_lines.len() {
            return false;
        }
        for (i, old_line) in hunk.old_lines.iter().enumerate() {
            if base_lines[position + i] != *old_line {
                return false;
            }
        }
        if !hunk.context_before.is_empty() {
            let ctx_start = position.saturating_sub(hunk.context_before.len());
            let available = &base_lines[ctx_start..position];
            let expected_suffix =
                &hunk.context_before[hunk.context_before.len().saturating_sub(available.len())..];
            if available.len() >= expected_suffix.len() {
                let tail = &available[available.len() - expected_suffix.len()..];
                if tail.iter().zip(expected_suffix.iter()).any(|(a, b)| a != b) {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    } else {
        if position > base_lines.len() {
            return false;
        }
        if !hunk.context_before.is_empty() {
            let ctx_start = position.saturating_sub(hunk.context_before.len());
            let available = &base_lines[ctx_start..position];
            let expected_suffix =
                &hunk.context_before[hunk.context_before.len().saturating_sub(available.len())..];
            if available.len() >= expected_suffix.len() {
                let tail = &available[available.len() - expected_suffix.len()..];
                if tail.iter().zip(expected_suffix.iter()).any(|(a, b)| a != b) {
                    return false;
                }
            } else {
                return false;
            }
        }
        if !hunk.context_after.is_empty() {
            let available_after =
                &base_lines[position..(position + hunk.context_after.len()).min(base_lines.len())];
            let expected_prefix =
                &hunk.context_after[..hunk.context_after.len().min(available_after.len())];
            if available_after.len() >= expected_prefix.len() {
                if available_after[..expected_prefix.len()]
                    .iter()
                    .zip(expected_prefix.iter())
                    .any(|(a, b)| a != b)
                {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}
