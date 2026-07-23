use super::dependency::{AccessProjection, DefinitionEnv, ExprRef};
use super::*;

pub(super) fn expr_ref_scope(expr_ref: &ExprRef) -> &str {
    match expr_ref {
        ExprRef::Global(_) => "module",
        ExprRef::Function { scope, .. } => scope.as_str(),
    }
}

pub(super) fn operation_edge(
    inherited_edge: Option<String>,
    operation: impl Into<String>,
) -> Option<String> {
    inherited_edge.or_else(|| Some(operation.into()))
}

pub(super) fn distinct_definition_source_range(
    definition_source_range: Option<PassDebugSourceRange>,
    source_range: Option<PassDebugSourceRange>,
) -> Option<PassDebugSourceRange> {
    definition_source_range.filter(|definition_range| Some(*definition_range) != source_range)
}

pub(super) fn merge_definition_envs<I>(envs: I) -> DefinitionEnv
where
    I: IntoIterator<Item = DefinitionEnv>,
{
    let mut merged = DefinitionEnv::new();
    for env in envs {
        merge_definition_env_into(&mut merged, env);
    }
    merged
}

pub(super) fn merge_definition_env_into(target: &mut DefinitionEnv, source: DefinitionEnv) {
    for (target_id, definitions) in source {
        let target_definitions = target.entry(target_id).or_default();
        for definition_id in definitions {
            if !target_definitions.contains(&definition_id) {
                target_definitions.push(definition_id);
            }
        }
    }
}

pub(super) fn dependency_target_node_label(
    target: &PassDebugDependencyTarget,
    display_label: Option<&str>,
) -> String {
    let target_label = format!("{} ({})", target.label, target.kind);
    let Some(display_label) = display_label
        .map(str::trim)
        .filter(|label| !label.is_empty() && *label != target.name.as_str())
    else {
        return target_label;
    };
    format!("{display_label} -> {target_label}")
}

pub(super) fn swizzle_pattern_label(
    size: naga::VectorSize,
    pattern: &[SwizzleComponent; 4],
) -> String {
    pattern
        .iter()
        .take(vector_size_len(size))
        .filter_map(|component| swizzle_component_label(*component))
        .collect::<Vec<_>>()
        .join("")
}

pub(super) fn vector_size_len(size: naga::VectorSize) -> usize {
    size as u8 as usize
}

pub(super) fn swizzle_component_for_index(index: u32) -> Option<&'static str> {
    match index {
        0 => Some("x"),
        1 => Some("y"),
        2 => Some("z"),
        3 => Some("w"),
        _ => None,
    }
}

pub(super) fn swizzle_component_index(component: SwizzleComponent) -> Option<u32> {
    match component {
        SwizzleComponent::X => Some(0),
        SwizzleComponent::Y => Some(1),
        SwizzleComponent::Z => Some(2),
        SwizzleComponent::W => Some(3),
    }
}

pub(super) fn swizzle_component_label(component: SwizzleComponent) -> Option<&'static str> {
    match component {
        SwizzleComponent::X => Some("x"),
        SwizzleComponent::Y => Some("y"),
        SwizzleComponent::Z => Some("z"),
        SwizzleComponent::W => Some("w"),
    }
}

pub(super) fn dedupe_projection(projection: AccessProjection) -> AccessProjection {
    projection
        .into_iter()
        .fold(Vec::new(), |mut deduped, component| {
            if !deduped.contains(&component) {
                deduped.push(component);
            }
            deduped
        })
}

pub(super) fn combine_access_projection(
    access_projection: Option<&AccessProjection>,
    projection: &AccessProjection,
) -> AccessProjection {
    match access_projection {
        Some(access_projection) => dedupe_projection(
            projection
                .iter()
                .filter_map(|component| access_projection.get(*component as usize).copied())
                .collect(),
        ),
        None => dedupe_projection(projection.clone()),
    }
}

pub(super) fn target_source_range(
    targets: &[PassDebugDependencyTarget],
    target_id: &str,
) -> Option<PassDebugSourceRange> {
    targets
        .iter()
        .find(|target| target.id == target_id)
        .and_then(|target| target.source_range)
}

pub(super) fn source_range_from_span(
    source: &str,
    span: naga::Span,
) -> Option<PassDebugSourceRange> {
    span.to_range()
        .and_then(|range| source_range_from_byte_range(source, range))
}

pub(super) fn source_range_from_byte_range(
    source: &str,
    range: Range<usize>,
) -> Option<PassDebugSourceRange> {
    if range.start >= range.end
        || range.end > source.len()
        || !source.is_char_boundary(range.start)
        || !source.is_char_boundary(range.end)
    {
        return None;
    }
    let start = u32::try_from(range.start).ok()?;
    let end = u32::try_from(range.end).ok()?;
    let location = naga::Span::new(start, end).location(source);
    Some(PassDebugSourceRange {
        start_byte: range.start,
        end_byte: range.end,
        line: location.line_number,
        column: location.line_position,
    })
}

pub(super) fn source_line_byte_range(source: &str, byte_index: usize) -> Option<Range<usize>> {
    if byte_index > source.len() || !source.is_char_boundary(byte_index) {
        return None;
    }
    let start = source[..byte_index]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let end = source[byte_index..]
        .find('\n')
        .map(|relative| byte_index + relative)
        .unwrap_or(source.len());
    (start < end).then_some(start..end)
}

pub(super) fn find_identifier_range(
    source: &str,
    range: Range<usize>,
    name: &str,
) -> Option<PassDebugSourceRange> {
    find_identifier_occurrence_range(source, range, name, 0)
}

pub(super) fn find_last_identifier_range(
    source: &str,
    range: Range<usize>,
    name: &str,
) -> Option<PassDebugSourceRange> {
    if name.is_empty() || range.start >= range.end || range.end > source.len() {
        return None;
    }

    let haystack = &source[range.clone()];
    let mut offset = 0;
    let mut last = None;
    while let Some(relative) = haystack[offset..].find(name) {
        let start = range.start + offset + relative;
        let end = start + name.len();
        if is_identifier_start_boundary(source, start) && is_identifier_end_boundary(source, end) {
            last = source_range_from_byte_range(source, start..end);
        }
        offset += relative + name.len();
    }
    last
}

pub(super) fn find_identifier_occurrence_range(
    source: &str,
    range: Range<usize>,
    name: &str,
    occurrence_index: usize,
) -> Option<PassDebugSourceRange> {
    if name.is_empty() || range.start >= range.end || range.end > source.len() {
        return None;
    }

    let haystack = &source[range.clone()];
    let mut offset = 0;
    let mut seen = 0usize;
    while let Some(relative) = haystack[offset..].find(name) {
        let start = range.start + offset + relative;
        let end = start + name.len();
        if is_identifier_start_boundary(source, start) && is_identifier_end_boundary(source, end) {
            if seen == occurrence_index {
                return source_range_from_byte_range(source, start..end);
            }
            seen += 1;
        }
        offset += relative + name.len();
    }
    None
}

pub(super) fn find_global_identifier_range(
    source: &str,
    name: &str,
) -> Option<PassDebugSourceRange> {
    find_identifier_range(source, 0..source.len(), name)
}

pub(super) fn find_argument_identifier_range(
    source: &str,
    scope: &str,
    name: &str,
) -> Option<PassDebugSourceRange> {
    let function_range = find_function_range(source, scope)?;
    let signature_end = source[function_range.clone()]
        .find('{')
        .map(|offset| function_range.start + offset)
        .unwrap_or(function_range.end);
    find_identifier_range(source, function_range.start..signature_end, name)
}

pub(super) fn find_keyword_identifier_in_scope(
    source: &str,
    scope: &str,
    keyword: &str,
    name: &str,
) -> Option<PassDebugSourceRange> {
    let function_range = find_function_range(source, scope)?;
    find_keyword_identifier_range(source, function_range, keyword, name)
}

pub(super) fn find_keyword_identifier_range(
    source: &str,
    range: Range<usize>,
    keyword: &str,
    name: &str,
) -> Option<PassDebugSourceRange> {
    if keyword.is_empty() || name.is_empty() || range.end > source.len() {
        return None;
    }

    let haystack = &source[range.clone()];
    let mut offset = 0;
    while let Some(relative) = haystack[offset..].find(keyword) {
        let keyword_start = range.start + offset + relative;
        let keyword_end = keyword_start + keyword.len();
        if is_identifier_start_boundary(source, keyword_start)
            && is_identifier_end_boundary(source, keyword_end)
        {
            let mut name_start = keyword_end;
            while name_start < range.end {
                let byte = source.as_bytes()[name_start];
                if byte.is_ascii_whitespace() {
                    name_start += 1;
                } else {
                    break;
                }
            }
            let name_end = name_start + name.len();
            if name_end <= range.end
                && &source[name_start..name_end] == name
                && is_identifier_start_boundary(source, name_start)
                && is_identifier_end_boundary(source, name_end)
            {
                return source_range_from_byte_range(source, name_start..name_end);
            }
        }
        offset += relative + keyword.len();
    }
    None
}

pub(super) fn find_store_lhs_identifier_range(
    source: &str,
    scope: &str,
    name: &str,
    occurrence_index: usize,
) -> Option<PassDebugSourceRange> {
    let range = find_function_range(source, scope)?;
    if name.is_empty() || range.end > source.len() {
        return None;
    }

    let haystack = &source[range.clone()];
    let mut offset = 0;
    let mut seen = 0usize;
    while let Some(relative) = haystack[offset..].find(name) {
        let start = range.start + offset + relative;
        let end = start + name.len();
        if is_identifier_start_boundary(source, start)
            && is_identifier_end_boundary(source, end)
            && store_assignment_operator_start(source, end, range.end).is_some()
        {
            if seen == occurrence_index {
                return source_range_from_byte_range(source, start..end);
            }
            seen += 1;
        }
        offset += relative + name.len();
    }
    None
}

pub(super) fn store_assignment_operator_start(
    source: &str,
    mut index: usize,
    end: usize,
) -> Option<usize> {
    index = skip_ascii_whitespace(source, index, end);
    while index < end {
        match source.as_bytes()[index] {
            b'.' => {
                index += 1;
                while index < end {
                    let byte = source.as_bytes()[index];
                    if byte.is_ascii_alphanumeric() || byte == b'_' {
                        index += 1;
                    } else {
                        break;
                    }
                }
                index = skip_ascii_whitespace(source, index, end);
            }
            b'[' => {
                index = skip_bracketed_source(source, index, end)?;
                index = skip_ascii_whitespace(source, index, end);
            }
            _ => break,
        }
    }

    let bytes = source.as_bytes();
    match bytes.get(index).copied()? {
        b'=' if bytes.get(index + 1) != Some(&b'=') => Some(index),
        b'+' | b'-' | b'*' | b'/' | b'%' | b'&' | b'|' | b'^'
            if bytes.get(index + 1) == Some(&b'=') =>
        {
            Some(index)
        }
        b'<' if bytes.get(index + 1) == Some(&b'<') && bytes.get(index + 2) == Some(&b'=') => {
            Some(index)
        }
        b'>' if bytes.get(index + 1) == Some(&b'>') && bytes.get(index + 2) == Some(&b'=') => {
            Some(index)
        }
        _ => None,
    }
}

pub(super) fn skip_ascii_whitespace(source: &str, mut index: usize, end: usize) -> usize {
    while index < end && source.as_bytes()[index].is_ascii_whitespace() {
        index += 1;
    }
    index
}

pub(super) fn skip_bracketed_source(source: &str, open: usize, end: usize) -> Option<usize> {
    if source.as_bytes().get(open) != Some(&b'[') {
        return None;
    }
    let mut depth = 0usize;
    for index in open..end {
        match source.as_bytes()[index] {
            b'[' => depth += 1,
            b']' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index + 1);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn find_function_range(source: &str, scope: &str) -> Option<Range<usize>> {
    let mut offset = 0;
    while let Some(relative) = source[offset..].find("fn") {
        let fn_start = offset + relative;
        let fn_end = fn_start + 2;
        if !is_identifier_start_boundary(source, fn_start)
            || !is_identifier_end_boundary(source, fn_end)
        {
            offset = fn_end;
            continue;
        }

        let mut name_start = fn_end;
        while name_start < source.len() && source.as_bytes()[name_start].is_ascii_whitespace() {
            name_start += 1;
        }
        let name_end = name_start + scope.len();
        if name_end <= source.len()
            && &source[name_start..name_end] == scope
            && is_identifier_start_boundary(source, name_start)
            && is_identifier_end_boundary(source, name_end)
        {
            let body_start = source[name_end..]
                .find('{')
                .map(|relative| name_end + relative);
            let function_end = body_start
                .and_then(|start| find_matching_brace(source, start))
                .map(|end| end + 1)
                .unwrap_or(source.len());
            return Some(fn_start..function_end);
        }

        offset = fn_end;
    }
    None
}

pub(super) fn find_matching_brace(source: &str, open_brace: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, byte) in source.as_bytes().iter().enumerate().skip(open_brace) {
        match *byte {
            b'{' => depth += 1,
            b'}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn find_enclosed_arguments_range(
    source: &str,
    range: Range<usize>,
) -> Option<Range<usize>> {
    if range.start >= range.end || range.end > source.len() {
        return None;
    }
    let open = source.as_bytes()[range.clone()]
        .iter()
        .position(|byte| *byte == b'(')
        .map(|relative| range.start + relative)?;
    let mut depth = 0usize;
    for index in open..range.end {
        match source.as_bytes()[index] {
            b'(' => depth += 1,
            b')' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Some(open + 1..index);
                }
            }
            _ => {}
        }
    }
    None
}

pub(super) fn is_identifier_start_boundary(source: &str, byte_index: usize) -> bool {
    if byte_index > source.len() {
        return false;
    }
    !byte_index
        .checked_sub(1)
        .and_then(|index| source.as_bytes().get(index))
        .copied()
        .map(is_wgsl_identifier_byte)
        .unwrap_or(false)
}

pub(super) fn is_identifier_end_boundary(source: &str, byte_index: usize) -> bool {
    if byte_index > source.len() {
        return false;
    }
    !source
        .as_bytes()
        .get(byte_index)
        .copied()
        .map(is_wgsl_identifier_byte)
        .unwrap_or(false)
}

pub(super) fn is_wgsl_identifier_byte(byte: u8) -> bool {
    byte == b'_' || byte.is_ascii_alphanumeric()
}
