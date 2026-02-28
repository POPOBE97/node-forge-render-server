use std::collections::HashMap;

use anyhow::{Context, Result, anyhow, bail};
use rust_wgpu_fiber::eframe::wgpu::{self, BlendState};

use crate::dsl::parse_str;

fn normalize_blend_token(s: &str) -> String {
    s.trim().to_ascii_lowercase().replace('_', "-")
}

fn parse_blend_operation(op: &str) -> Result<wgpu::BlendOperation> {
    let op = normalize_blend_token(op);
    Ok(match op.as_str() {
        "add" => wgpu::BlendOperation::Add,
        "subtract" => wgpu::BlendOperation::Subtract,
        "reverse-subtract" | "rev-subtract" => wgpu::BlendOperation::ReverseSubtract,
        "min" => wgpu::BlendOperation::Min,
        "max" => wgpu::BlendOperation::Max,
        other => bail!("unsupported blendfunc/blend operation: {other}"),
    })
}

fn parse_blend_factor(f: &str) -> Result<wgpu::BlendFactor> {
    let f = normalize_blend_token(f);
    Ok(match f.as_str() {
        "zero" => wgpu::BlendFactor::Zero,
        "one" => wgpu::BlendFactor::One,

        "src" | "src-color" => wgpu::BlendFactor::Src,
        "one-minus-src" | "one-minus-src-color" => wgpu::BlendFactor::OneMinusSrc,

        "src-alpha" => wgpu::BlendFactor::SrcAlpha,
        "one-minus-src-alpha" => wgpu::BlendFactor::OneMinusSrcAlpha,

        "dst" | "dst-color" => wgpu::BlendFactor::Dst,
        "one-minus-dst" | "one-minus-dst-color" => wgpu::BlendFactor::OneMinusDst,

        "dst-alpha" => wgpu::BlendFactor::DstAlpha,
        "one-minus-dst-alpha" => wgpu::BlendFactor::OneMinusDstAlpha,

        "src-alpha-saturated" => wgpu::BlendFactor::SrcAlphaSaturated,
        "constant" | "blend-color" => wgpu::BlendFactor::Constant,
        "one-minus-constant" | "one-minus-blend-color" => wgpu::BlendFactor::OneMinusConstant,
        other => bail!("unsupported blend factor: {other}"),
    })
}

pub(crate) fn default_blend_state_for_preset(preset: &str) -> Result<BlendState> {
    let preset = normalize_blend_token(preset);
    Ok(match preset.as_str() {
        "premul-alpha" | "premul" | "premultiplied-alpha" => BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        },
        "alpha" => BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        },
        "add" | "additive" => BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            // Alpha is coverage [0,1] — use premul-style blend to prevent
            // unbounded accumulation on HDR (Rgba16Float) targets.
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        },
        "opaque" | "none" | "off" | "replace" => BlendState::REPLACE,
        // "custom" means: start from a neutral blend state and let explicit
        // blendfunc/src/dst overrides drive the final state.
        "custom" => BlendState::REPLACE,
        other => bail!("unsupported blend_preset: {other}"),
    })
}

fn require_custom_str<'a>(
    params: &'a HashMap<String, serde_json::Value>,
    key: &str,
) -> Result<&'a str> {
    let Some(raw) = params.get(key) else {
        bail!("blend_preset=custom requires '{key}'");
    };
    raw.as_str()
        .ok_or_else(|| anyhow!("blend_preset=custom requires '{key}' to be a string"))
}

fn parse_custom_blend_state(params: &HashMap<String, serde_json::Value>) -> Result<BlendState> {
    let blendfunc = require_custom_str(params, "blendfunc")?;
    let src_factor = require_custom_str(params, "src_factor")?;
    let dst_factor = require_custom_str(params, "dst_factor")?;
    let src_alpha_factor = require_custom_str(params, "src_alpha_factor")?;
    let dst_alpha_factor = require_custom_str(params, "dst_alpha_factor")?;

    Ok(BlendState {
        color: wgpu::BlendComponent {
            src_factor: parse_blend_factor(src_factor)
                .with_context(|| "invalid custom blend param 'src_factor'")?,
            dst_factor: parse_blend_factor(dst_factor)
                .with_context(|| "invalid custom blend param 'dst_factor'")?,
            operation: parse_blend_operation(blendfunc)
                .with_context(|| "invalid custom blend param 'blendfunc'")?,
        },
        alpha: wgpu::BlendComponent {
            src_factor: parse_blend_factor(src_alpha_factor)
                .with_context(|| "invalid custom blend param 'src_alpha_factor'")?,
            dst_factor: parse_blend_factor(dst_alpha_factor)
                .with_context(|| "invalid custom blend param 'dst_alpha_factor'")?,
            operation: parse_blend_operation(blendfunc)
                .with_context(|| "invalid custom blend param 'blendfunc'")?,
        },
    })
}

pub(crate) fn parse_render_pass_blend_state(
    params: &HashMap<String, serde_json::Value>,
) -> Result<BlendState> {
    // Start with preset if present; otherwise default to REPLACE.
    // Note: RenderPass has scheme defaults for blendfunc/factors. If a user sets only
    // `blend_preset=replace` (common intent: disable blending), those default factor keys will
    // still exist in params after default-merging. We must treat replace/off/none/opaque as
    // authoritative and ignore factor overrides.
    if let Some(preset) = parse_str(params, "blend_preset") {
        let preset_norm = normalize_blend_token(preset);
        if matches!(preset_norm.as_str(), "opaque" | "none" | "off" | "replace") {
            return Ok(BlendState::REPLACE);
        }
        if preset_norm == "custom" {
            return parse_custom_blend_state(params);
        }
    }

    let mut state = if let Some(preset) = parse_str(params, "blend_preset") {
        default_blend_state_for_preset(preset)?
    } else {
        BlendState::REPLACE
    };

    // Override with explicit params if present.
    if let Some(op) = parse_str(params, "blendfunc") {
        let op = parse_blend_operation(op)?;
        state.color.operation = op;
        state.alpha.operation = op;
    }
    if let Some(src) = parse_str(params, "src_factor") {
        state.color.src_factor = parse_blend_factor(src)?;
    }
    if let Some(dst) = parse_str(params, "dst_factor") {
        state.color.dst_factor = parse_blend_factor(dst)?;
    }
    if let Some(src) = parse_str(params, "src_alpha_factor") {
        state.alpha.src_factor = parse_blend_factor(src)?;
    }
    if let Some(dst) = parse_str(params, "dst_alpha_factor") {
        state.alpha.dst_factor = parse_blend_factor(dst)?;
    }

    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn assert_blend_state_eq(actual: BlendState, expected: BlendState) {
        assert_eq!(format!("{actual:?}"), format!("{expected:?}"));
    }

    #[test]
    fn preset_alpha_matches_contract() {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("blend_preset".to_string(), json!("alpha"));
        let got = parse_render_pass_blend_state(&params).expect("parse alpha blend preset");
        let expected = BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        assert_blend_state_eq(got, expected);
    }

    #[test]
    fn preset_premul_alpha_matches_contract() {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("blend_preset".to_string(), json!("premul_alpha"));
        let got = parse_render_pass_blend_state(&params).expect("parse premul-alpha blend preset");
        let expected = BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
        };
        assert_blend_state_eq(got, expected);
    }

    #[test]
    fn preset_add_matches_contract() {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("blend_preset".to_string(), json!("add"));
        let got = parse_render_pass_blend_state(&params).expect("parse add blend preset");
        let expected = BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        assert_blend_state_eq(got, expected);
    }

    #[test]
    fn custom_preset_requires_all_explicit_fields() {
        let mut params: HashMap<String, serde_json::Value> = HashMap::new();
        params.insert("blend_preset".to_string(), json!("custom"));
        let err = parse_render_pass_blend_state(&params)
            .expect_err("custom preset without factors fails");
        assert!(format!("{err:#}").contains("blend_preset=custom requires 'blendfunc'"));
    }

    #[test]
    fn custom_preset_uses_verbatim_values() {
        let params: HashMap<String, serde_json::Value> = HashMap::from([
            ("blend_preset".to_string(), json!("custom")),
            ("blendfunc".to_string(), json!("reverse-subtract")),
            ("src_factor".to_string(), json!("one")),
            ("dst_factor".to_string(), json!("one")),
            ("src_alpha_factor".to_string(), json!("src-alpha")),
            ("dst_alpha_factor".to_string(), json!("one-minus-src-alpha")),
        ]);
        let got = parse_render_pass_blend_state(&params).expect("parse custom blend preset");
        let expected = BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::ReverseSubtract,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::ReverseSubtract,
            },
        };
        assert_blend_state_eq(got, expected);
    }
}
