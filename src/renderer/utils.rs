//! Utility functions for the renderer module.

use anyhow::{anyhow, bail, Result};
use base64::{engine::general_purpose, Engine as _};
use image::DynamicImage;

use super::types::{TypedExpr, ValueType};

use crate::dsl::{self, Node, SceneDSL};

/// Resolve a numeric input used by the renderer on CPU.
///
/// Contract (important for future node additions):
/// - Any time the renderer needs a number (size, radius, count, etc.) **on CPU**,
///   it must go through these helpers so DSL `connections` can drive the value.
/// - Resolution precedence: incoming connection -> inline param (`node.params[key]`).
pub fn cpu_num_f64(
    scene: &SceneDSL,
    nodes_by_id: &std::collections::HashMap<String, Node>,
    node: &Node,
    key: &str,
) -> Result<Option<f64>> {
    if let Some(v) = dsl::resolve_input_f64(scene, nodes_by_id, &node.id, key)? {
        return Ok(Some(v));
    }

    Ok(dsl::parse_f32(&node.params, key)
        .map(|x| x as f64)
        .or_else(|| dsl::parse_u32(&node.params, key).map(|x| x as f64)))
}

pub fn cpu_num_f32(
    scene: &SceneDSL,
    nodes_by_id: &std::collections::HashMap<String, Node>,
    node: &Node,
    key: &str,
    default: f32,
) -> Result<f32> {
    Ok(cpu_num_f64(scene, nodes_by_id, node, key)?.unwrap_or(default as f64) as f32)
}

pub fn cpu_num_f32_min_0(
    scene: &SceneDSL,
    nodes_by_id: &std::collections::HashMap<String, Node>,
    node: &Node,
    key: &str,
    default: f32,
) -> Result<f32> {
    Ok(cpu_num_f32(scene, nodes_by_id, node, key, default)?.max(0.0))
}

pub fn cpu_num_u32_floor(
    scene: &SceneDSL,
    nodes_by_id: &std::collections::HashMap<String, Node>,
    node: &Node,
    key: &str,
    default: u32,
) -> Result<u32> {
    if let Some(v) = dsl::resolve_input_u32(scene, nodes_by_id, &node.id, key)? {
        return Ok(v);
    }

    // Fallback to inline params (accept both integer and float literals).
    if let Some(v) = dsl::parse_u32(&node.params, key) {
        return Ok(v);
    }
    if let Some(v) = dsl::parse_f32(&node.params, key) {
        if v.is_finite() {
            return Ok(v.max(0.0).floor() as u32);
        }
    }

    Ok(default)
}

pub fn cpu_num_u32_min_1(
    scene: &SceneDSL,
    nodes_by_id: &std::collections::HashMap<String, Node>,
    node: &Node,
    key: &str,
    default: u32,
) -> Result<u32> {
    Ok(cpu_num_u32_floor(scene, nodes_by_id, node, key, default)?.max(1))
}

/// Format a float for WGSL, removing trailing zeros.
pub fn fmt_f32(v: f32) -> String {
    if v.is_finite() {
        let s = format!("{v:.9}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        "0.0".to_string()
    }
}

/// Format an array of 8 floats as a WGSL array literal.
pub fn array8_f32_wgsl(values: [f32; 8]) -> String {
    let parts: Vec<String> = values.into_iter().map(fmt_f32).collect();
    format!("array<f32, 8>({})", parts.join(", "))
}

/// Sanitize a string to be a valid WGSL identifier.
pub fn sanitize_wgsl_ident(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        out.push('_');
    }
    out
}

/// Splat an f32 expression to a target vector type.
pub fn splat_f32(x: &TypedExpr, target: ValueType) -> Result<TypedExpr> {
    if x.ty != ValueType::F32 {
        bail!("expected f32 for splat, got {:?}", x.ty);
    }
    Ok(match target {
        ValueType::F32 => x.clone(),
        ValueType::Vec2 => TypedExpr::with_time(format!("vec2f({})", x.expr), ValueType::Vec2, x.uses_time),
        ValueType::Vec3 => TypedExpr::with_time(format!("vec3f({})", x.expr), ValueType::Vec3, x.uses_time),
        ValueType::Vec4 => TypedExpr::with_time(
            format!("vec4f({}, {}, {}, 1.0)", x.expr, x.expr, x.expr),
            ValueType::Vec4,
            x.uses_time,
        ),
    })
}

/// Coerce two typed expressions for binary operations (promoting scalars to vectors as needed).
pub fn coerce_for_binary(a: TypedExpr, b: TypedExpr) -> Result<(TypedExpr, TypedExpr, ValueType)> {
    if a.ty == b.ty {
        let ty = a.ty;
        return Ok((a, b, ty));
    }
    // Promote scalar to vector if needed.
    if a.ty == ValueType::F32 && b.ty != ValueType::F32 {
        let target_ty = b.ty;
        let aa = splat_f32(&a, b.ty)?;
        return Ok((aa, b, target_ty));
    }
    if b.ty == ValueType::F32 && a.ty != ValueType::F32 {
        let target_ty = a.ty;
        let bb = splat_f32(&b, a.ty)?;
        return Ok((a, bb, target_ty));
    }
    bail!("incompatible types for binary op: {:?} and {:?}", a.ty, b.ty);
}

/// Convert a typed expression to vec4 color format.
pub fn to_vec4_color(x: TypedExpr) -> TypedExpr {
    match x.ty {
        ValueType::F32 => TypedExpr::with_time(
            format!("vec4f({}, {}, {}, 1.0)", x.expr, x.expr, x.expr),
            ValueType::Vec4,
            x.uses_time,
        ),
        ValueType::Vec2 => TypedExpr::with_time(
            format!("vec4f({}, 0.0, 1.0)", x.expr),
            ValueType::Vec4,
            x.uses_time,
        ),
        ValueType::Vec3 => TypedExpr::with_time(
            format!("vec4f({}, 1.0)", x.expr),
            ValueType::Vec4,
            x.uses_time,
        ),
        ValueType::Vec4 => x,
    }
}

/// Convert bytes slice to raw bytes for GPU upload.
pub fn as_bytes<T>(v: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts((v as *const T) as *const u8, core::mem::size_of::<T>()) }
}

/// Convert slice of items to raw bytes for GPU upload.
pub fn as_bytes_slice<T>(v: &[T]) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(v.as_ptr() as *const u8, core::mem::size_of::<T>() * v.len())
    }
}

/// Decode percent-encoded bytes in a data URL.
fn percent_decode_to_bytes(s: &str) -> Result<Vec<u8>> {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                if i + 2 >= bytes.len() {
                    bail!("invalid percent-encoding: truncated");
                }
                let hi = bytes[i + 1];
                let lo = bytes[i + 2];
                let hex = |b: u8| -> Option<u8> {
                    match b {
                        b'0'..=b'9' => Some(b - b'0'),
                        b'a'..=b'f' => Some(b - b'a' + 10),
                        b'A'..=b'F' => Some(b - b'A' + 10),
                        _ => None,
                    }
                };
                let Some(hi) = hex(hi) else { bail!("invalid percent-encoding"); };
                let Some(lo) = hex(lo) else { bail!("invalid percent-encoding"); };
                out.push((hi << 4) | lo);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    Ok(out)
}

/// Decode a data URL to raw bytes.
pub fn decode_data_url(data_url: &str) -> Result<Vec<u8>> {
    let s = data_url.trim();
    if !s.starts_with("data:") {
        bail!("not a data URL");
    }

    let (_, rest) = s.split_at("data:".len());
    let (meta, data) = rest
        .split_once(',')
        .ok_or_else(|| anyhow!("invalid data URL: missing comma"))?;

    let is_base64 = meta
        .split(';')
        .any(|t| t.trim().eq_ignore_ascii_case("base64"));

    if is_base64 {
        general_purpose::STANDARD
            .decode(data.trim())
            .or_else(|_| general_purpose::URL_SAFE.decode(data.trim()))
            .map_err(|e| anyhow!("invalid base64 in data URL: {e}"))
    } else {
        percent_decode_to_bytes(data)
    }
}

/// Load an image from a data URL.
pub fn load_image_from_data_url(data_url: &str) -> Result<DynamicImage> {
    let bytes = decode_data_url(data_url)?;
    image::load_from_memory(&bytes).map_err(|e| anyhow!("failed to decode image bytes: {e}"))
}
