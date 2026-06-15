use rust_wgpu_fiber::eframe::egui;

use crate::ui::pass_debug::dependency_tree::char_index_to_byte_index;

pub(crate) fn line_boundaries_for_layout(text: &str) -> Vec<(usize, usize)> {
    let mut boundaries = Vec::with_capacity(text.lines().count().saturating_add(1));
    let mut start = 0usize;

    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            boundaries.push((start, idx));
            start = idx + ch.len_utf8();
        }
    }

    if start < text.len() || text.ends_with('\n') || text.is_empty() {
        boundaries.push((start, text.len()));
    }

    boundaries
}

pub(crate) fn highlighted_line_sections_for_layout(
    line: &str,
    theme: &crate::ui::wgsl_highlight::WgslTheme,
) -> Vec<egui::text::LayoutSection> {
    let mut sections = crate::ui::wgsl_highlight::highlight_wgsl_line(line, theme);
    if sections.is_empty() {
        sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: 0..0,
            format: egui::text::TextFormat {
                font_id: theme.font_id.clone(),
                color: theme.default,
                ..Default::default()
            },
        });
    }
    sections
}

pub(crate) fn build_full_layout_job(
    text: &str,
    line_boundaries: &[(usize, usize)],
    line_sections: &[Vec<egui::text::LayoutSection>],
    wrap_width: f32,
    theme: &crate::ui::wgsl_highlight::WgslTheme,
) -> egui::text::LayoutJob {
    let default_fmt = egui::text::TextFormat {
        font_id: theme.font_id.clone(),
        color: theme.default,
        ..Default::default()
    };
    let mut all_sections = Vec::with_capacity(
        line_sections.iter().map(|s| s.len()).sum::<usize>() + line_boundaries.len(),
    );
    for (line_idx, &(start, end)) in line_boundaries.iter().enumerate() {
        for section in &line_sections[line_idx] {
            all_sections.push(egui::text::LayoutSection {
                leading_space: section.leading_space,
                byte_range: (section.byte_range.start + start)..(section.byte_range.end + start),
                format: section.format.clone(),
            });
        }
        if end < text.len() {
            all_sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: end..end + 1,
                format: default_fmt.clone(),
            });
        }
    }
    let last_covered = all_sections.last().map_or(0, |s| s.byte_range.end);
    if last_covered < text.len() {
        all_sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: last_covered..text.len(),
            format: default_fmt,
        });
    }
    egui::text::LayoutJob {
        text: text.to_owned(),
        wrap: egui::text::TextWrapping {
            max_width: wrap_width,
            max_rows: usize::MAX,
            ..Default::default()
        },
        sections: all_sections,
        break_on_newline: true,
        halign: egui::Align::LEFT,
        justify: false,
        first_row_min_height: 0.0,
        round_output_to_gui: true,
    }
}

pub(crate) fn line_start_char_indices_for_layout(
    source: &str,
    line_boundaries: &[(usize, usize)],
) -> Vec<usize> {
    let mut starts = Vec::with_capacity(line_boundaries.len());
    let mut char_index = 0usize;

    for &(start, end) in line_boundaries {
        starts.push(char_index);
        char_index += source[start..end].chars().count();
        if end < source.len() {
            char_index += 1;
        }
    }

    starts
}

pub(crate) fn line_index_at_char_index(
    source: &str,
    char_index: usize,
    line_boundaries: &[(usize, usize)],
) -> Option<usize> {
    let byte_index = char_index_to_byte_index(source, char_index);
    for (line_idx, &(start, end)) in line_boundaries.iter().enumerate() {
        let line_end_exclusive = if end < source.len() { end + 1 } else { end };
        if byte_index >= start && byte_index < line_end_exclusive {
            return Some(line_idx);
        }
    }
    if byte_index == source.len() {
        return line_boundaries.len().checked_sub(1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_boundaries_keep_trailing_empty_line() {
        assert_eq!(line_boundaries_for_layout("a\n"), vec![(0, 1), (2, 2)]);
    }

    #[test]
    fn line_boundaries_keep_consecutive_empty_lines() {
        assert_eq!(
            line_boundaries_for_layout("a\n\nb"),
            vec![(0, 1), (2, 2), (3, 4)]
        );
    }

    #[test]
    fn line_boundaries_include_empty_document_line() {
        assert_eq!(line_boundaries_for_layout(""), vec![(0, 0)]);
    }

    #[test]
    fn line_start_char_indices_track_unicode_and_empty_lines() {
        let source = "é\n\nabc";
        let boundaries = line_boundaries_for_layout(source);

        assert_eq!(
            line_start_char_indices_for_layout(source, &boundaries),
            vec![0, 2, 3]
        );
    }

    #[test]
    fn line_index_at_char_index_treats_line_start_as_next_line() {
        let source = "a\nb";
        let boundaries = line_boundaries_for_layout(source);

        assert_eq!(line_index_at_char_index(source, 0, &boundaries), Some(0));
        assert_eq!(line_index_at_char_index(source, 1, &boundaries), Some(0));
        assert_eq!(line_index_at_char_index(source, 2, &boundaries), Some(1));
        assert_eq!(line_index_at_char_index(source, 3, &boundaries), Some(1));
    }

    #[test]
    fn empty_line_layout_sections_keep_default_font() {
        let theme = crate::ui::wgsl_highlight::WgslTheme::dark(egui::FontId::monospace(14.0));
        let sections = highlighted_line_sections_for_layout("", &theme);

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].byte_range, 0..0);
        assert_eq!(sections[0].format.font_id, theme.font_id);
        assert_eq!(sections[0].format.color, theme.default);
    }
}
