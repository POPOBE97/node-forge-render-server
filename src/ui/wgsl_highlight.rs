use rust_wgpu_fiber::eframe::egui;
use egui::text::{LayoutSection, TextFormat};

#[derive(Clone)]
pub struct WgslTheme {
    pub font_id: egui::FontId,
    pub keyword: egui::Color32,
    pub type_name: egui::Color32,
    pub attribute: egui::Color32,
    pub comment: egui::Color32,
    pub number: egui::Color32,
    pub string: egui::Color32,
    pub punctuation: egui::Color32,
    pub default: egui::Color32,
}

impl WgslTheme {
    pub fn dark(font_id: egui::FontId) -> Self {
        Self {
            font_id,
            keyword: egui::Color32::from_rgb(198, 120, 221),
            type_name: egui::Color32::from_rgb(97, 175, 239),
            attribute: egui::Color32::from_rgb(229, 192, 123),
            comment: egui::Color32::from_rgb(92, 99, 112),
            number: egui::Color32::from_rgb(209, 154, 102),
            string: egui::Color32::from_rgb(152, 195, 121),
            punctuation: egui::Color32::from_rgb(171, 178, 191),
            default: egui::Color32::from_rgb(171, 178, 191),
        }
    }

    pub fn light(font_id: egui::FontId) -> Self {
        Self {
            font_id,
            keyword: egui::Color32::from_rgb(166, 38, 164),
            type_name: egui::Color32::from_rgb(1, 132, 188),
            attribute: egui::Color32::from_rgb(152, 104, 1),
            comment: egui::Color32::from_rgb(160, 161, 167),
            number: egui::Color32::from_rgb(152, 104, 1),
            string: egui::Color32::from_rgb(80, 161, 79),
            punctuation: egui::Color32::from_rgb(56, 58, 66),
            default: egui::Color32::from_rgb(56, 58, 66),
        }
    }
}

pub fn highlight_wgsl_line(line: &str, theme: &WgslTheme) -> Vec<LayoutSection> {
    let mut sections = Vec::new();
    let mut pos = 0;
    let bytes = line.as_bytes();

    while pos < bytes.len() {
        let b = bytes[pos];

        if b == b'/' && pos + 1 < bytes.len() && bytes[pos + 1] == b'/' {
            push_section(&mut sections, pos, bytes.len(), &theme.comment, &theme.font_id);
            break;
        }

        if b == b'@' {
            let start = pos;
            pos += 1;
            while pos < bytes.len() && is_ident_continue(bytes[pos]) {
                pos += 1;
            }
            push_section(&mut sections, start, pos, &theme.attribute, &theme.font_id);
            continue;
        }

        if b == b'"' {
            let start = pos;
            pos += 1;
            while pos < bytes.len() && bytes[pos] != b'"' {
                if bytes[pos] == b'\\' && pos + 1 < bytes.len() {
                    pos += 1;
                }
                pos += 1;
            }
            if pos < bytes.len() {
                pos += 1;
            }
            push_section(&mut sections, start, pos, &theme.string, &theme.font_id);
            continue;
        }

        if is_digit(b) || (b == b'0' && pos + 1 < bytes.len() && (bytes[pos + 1] == b'x' || bytes[pos + 1] == b'X')) {
            let start = pos;
            if b == b'0' && pos + 1 < bytes.len() && (bytes[pos + 1] == b'x' || bytes[pos + 1] == b'X') {
                pos += 2;
                while pos < bytes.len() && is_hex_digit(bytes[pos]) {
                    pos += 1;
                }
            } else {
                while pos < bytes.len() && is_digit(bytes[pos]) {
                    pos += 1;
                }
                if pos < bytes.len() && bytes[pos] == b'.' {
                    pos += 1;
                    while pos < bytes.len() && is_digit(bytes[pos]) {
                        pos += 1;
                    }
                }
                if pos < bytes.len() && (bytes[pos] == b'e' || bytes[pos] == b'E') {
                    pos += 1;
                    if pos < bytes.len() && (bytes[pos] == b'+' || bytes[pos] == b'-') {
                        pos += 1;
                    }
                    while pos < bytes.len() && is_digit(bytes[pos]) {
                        pos += 1;
                    }
                }
            }
            if pos < bytes.len() && is_ident_continue(bytes[pos]) {
                pos += 1;
            }
            push_section(&mut sections, start, pos, &theme.number, &theme.font_id);
            continue;
        }

        if is_ident_start(b) {
            let start = pos;
            pos += 1;
            while pos < bytes.len() && is_ident_continue(bytes[pos]) {
                pos += 1;
            }
            let word = &line[start..pos];
            let color = if is_keyword(word) {
                &theme.keyword
            } else if is_type_name(word) {
                &theme.type_name
            } else {
                &theme.default
            };
            push_section(&mut sections, start, pos, color, &theme.font_id);
            continue;
        }

        if b.is_ascii_whitespace() {
            let start = pos;
            while pos < bytes.len() && bytes[pos].is_ascii_whitespace() {
                pos += 1;
            }
            push_section(&mut sections, start, pos, &theme.default, &theme.font_id);
            continue;
        }

        let start = pos;
        pos += 1;
        push_section(&mut sections, start, pos, &theme.punctuation, &theme.font_id);
    }

    sections
}

fn push_section(
    sections: &mut Vec<LayoutSection>,
    start: usize,
    end: usize,
    color: &egui::Color32,
    font_id: &egui::FontId,
) {
    sections.push(LayoutSection {
        leading_space: 0.0,
        byte_range: start..end,
        format: TextFormat {
            font_id: font_id.clone(),
            color: *color,
            ..Default::default()
        },
    });
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

fn is_hex_digit(b: u8) -> bool {
    b.is_ascii_hexdigit() || b == b'_'
}

fn is_keyword(word: &str) -> bool {
    matches!(
        word,
        "fn" | "var" | "let" | "const" | "struct" | "if" | "else" | "for" | "while"
            | "return" | "loop" | "break" | "continue" | "switch" | "case" | "default"
            | "enable" | "alias" | "override" | "discard" | "true" | "false"
            | "continuing" | "diagnostic" | "requires" | "const_assert"
    )
}

fn is_type_name(word: &str) -> bool {
    matches!(
        word,
        "f32" | "f16" | "u32" | "i32" | "bool"
            | "vec2" | "vec3" | "vec4"
            | "mat2x2" | "mat2x3" | "mat2x4"
            | "mat3x2" | "mat3x3" | "mat3x4"
            | "mat4x2" | "mat4x3" | "mat4x4"
            | "sampler" | "sampler_comparison"
            | "texture_1d" | "texture_2d" | "texture_2d_array"
            | "texture_3d" | "texture_cube" | "texture_cube_array"
            | "texture_multisampled_2d" | "texture_depth_2d"
            | "texture_depth_2d_array" | "texture_depth_cube"
            | "texture_depth_cube_array" | "texture_depth_multisampled_2d"
            | "texture_storage_1d" | "texture_storage_2d"
            | "texture_storage_2d_array" | "texture_storage_3d"
            | "array" | "ptr" | "atomic"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section_texts<'a>(line: &'a str, sections: &[LayoutSection]) -> Vec<&'a str> {
        sections.iter().map(|s| &line[s.byte_range.clone()]).collect()
    }

    fn test_theme() -> WgslTheme {
        WgslTheme::dark(egui::FontId::monospace(14.0))
    }

    #[test]
    fn highlights_comment() {
        let line = "let x = 1; // hello";
        let sections = highlight_wgsl_line(line, &test_theme());
        let texts = section_texts(line, &sections);
        assert!(texts.contains(&"// hello"));
        assert_eq!(sections.last().unwrap().format.color, test_theme().comment);
    }

    #[test]
    fn highlights_keywords_and_types() {
        let line = "fn main() -> vec4<f32> {";
        let theme = test_theme();
        let sections = highlight_wgsl_line(line, &theme);
        assert_eq!(sections[0].format.color, theme.keyword); // fn
        assert_eq!(&line[sections[0].byte_range.clone()], "fn");
        let vec4_section = sections.iter().find(|s| &line[s.byte_range.clone()] == "vec4").unwrap();
        assert_eq!(vec4_section.format.color, theme.type_name);
        let f32_section = sections.iter().find(|s| &line[s.byte_range.clone()] == "f32").unwrap();
        assert_eq!(f32_section.format.color, theme.type_name);
    }

    #[test]
    fn highlights_attribute() {
        let line = "@fragment fn fs() {}";
        let theme = test_theme();
        let sections = highlight_wgsl_line(line, &theme);
        assert_eq!(&line[sections[0].byte_range.clone()], "@fragment");
        assert_eq!(sections[0].format.color, theme.attribute);
    }

    #[test]
    fn highlights_numbers() {
        let line = "var x = 3.14f;";
        let theme = test_theme();
        let sections = highlight_wgsl_line(line, &theme);
        let num_section = sections.iter().find(|s| line[s.byte_range.clone()].starts_with("3.14")).unwrap();
        assert_eq!(num_section.format.color, theme.number);
    }

    #[test]
    fn full_line_coverage() {
        let line = "@vertex fn vs(@builtin(vertex_index) idx: u32) -> vec4<f32> {";
        let theme = test_theme();
        let sections = highlight_wgsl_line(line, &theme);
        let total_covered: usize = sections.iter().map(|s| s.byte_range.end - s.byte_range.start).sum();
        assert_eq!(total_covered, line.len());
        for (i, s) in sections.iter().enumerate() {
            if i > 0 {
                assert_eq!(s.byte_range.start, sections[i - 1].byte_range.end);
            }
        }
    }
}
