//! Utility functions for the renderer module.

use anyhow::{Result, anyhow, bail};
use base64::{Engine as _, engine::general_purpose};
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
    if matches!(out.chars().next(), Some('0'..='9')) {
        out.insert(0, '_');
    }
    out
}

/// Convert a scalar expression into an f32 expression.
pub(crate) fn coerce_scalar_to_f32(x: &TypedExpr) -> Option<TypedExpr> {
    match x.ty {
        ValueType::F32 => Some(x.clone()),
        ValueType::I32 => Some(TypedExpr::with_time(
            format!("f32({})", x.expr),
            ValueType::F32,
            x.uses_time,
        )),
        ValueType::Bool => Some(TypedExpr::with_time(
            format!("select(0.0, 1.0, {})", x.expr),
            ValueType::F32,
            x.uses_time,
        )),
        _ => None,
    }
}

/// Convert a scalar expression into an i32 expression.
pub(crate) fn coerce_scalar_to_i32(x: &TypedExpr) -> Option<TypedExpr> {
    match x.ty {
        ValueType::I32 => Some(x.clone()),
        ValueType::F32 => Some(TypedExpr::with_time(
            format!("i32({})", x.expr),
            ValueType::I32,
            x.uses_time,
        )),
        ValueType::Bool => Some(TypedExpr::with_time(
            format!("select(0, 1, {})", x.expr),
            ValueType::I32,
            x.uses_time,
        )),
        _ => None,
    }
}

/// Splat a scalar expression to a target vector type.
pub(crate) fn splat_scalar(x: &TypedExpr, target: ValueType) -> Result<TypedExpr> {
    let xf = coerce_scalar_to_f32(x)
        .or_else(|| match x.ty {
            ValueType::U32 => Some(TypedExpr::with_time(
                format!("f32({})", x.expr),
                ValueType::F32,
                x.uses_time,
            )),
            _ => None,
        })
        .ok_or_else(|| anyhow!("expected scalar for splat, got {:?}", x.ty))?;

    Ok(match target {
        ValueType::Vec2 => {
            TypedExpr::with_time(format!("vec2f({})", xf.expr), ValueType::Vec2, xf.uses_time)
        }
        ValueType::Vec3 => {
            TypedExpr::with_time(format!("vec3f({})", xf.expr), ValueType::Vec3, xf.uses_time)
        }
        ValueType::Vec4 => {
            TypedExpr::with_time(format!("vec4f({})", xf.expr), ValueType::Vec4, xf.uses_time)
        }
        other => bail!("unsupported scalar splat target: {:?}", other),
    })
}

/// Coerce a typed expression to a target ValueType.
///
/// Supported implicit conversions (editor contract):
/// - Scalar numeric: f32 <-> i32, bool -> f32/i32
/// - Scalar -> vector splat: f32|i32|bool -> vec2/vec3/vec4
/// - Vector dimension changes:
///   - vec2 -> vec3: vec3(vec2, 0.0)
///   - vec2 -> vec4: vec4(vec2, 0.0, 0.0)
///   - vec3 -> vec4: vec4(vec3, 0.0)
///   - vec4 -> vec3: vec3(vec4.xyz)
///   - vec4 -> vec2: vec2(vec4.xy)
///   - vec3 -> vec2: vec2(vec3.xy)
pub fn coerce_to_type(x: TypedExpr, target: ValueType) -> Result<TypedExpr> {
    if x.ty == target {
        return Ok(x);
    }

    // Textures are opaque resources, not value expressions.
    if x.ty == ValueType::Texture2D || target == ValueType::Texture2D {
        bail!("cannot coerce between {:?} and {:?}", x.ty, target);
    }

    // Scalar -> scalar numeric
    match target {
        ValueType::F32 => {
            if let Some(v) = coerce_scalar_to_f32(&x) {
                return Ok(v);
            }
        }
        ValueType::I32 => {
            if let Some(v) = coerce_scalar_to_i32(&x) {
                return Ok(v);
            }
        }
        ValueType::Bool => {
            // No implicit numeric->bool in the contract.
        }
        _ => {}
    }

    // Scalar -> vector splat
    if matches!(target, ValueType::Vec2 | ValueType::Vec3 | ValueType::Vec4) {
        if matches!(x.ty, ValueType::F32 | ValueType::I32 | ValueType::Bool) {
            return splat_scalar(&x, target);
        }
    }

    // Vector dimension conversions
    let wrap = |e: &str| format!("({e})");
    let swizzle = |expr: &str, suffix: &str| format!("{}.{}", wrap(expr), suffix);

    match (x.ty, target) {
        (ValueType::Vec4, ValueType::Vec3) => Ok(TypedExpr::with_time(
            format!("vec3f({})", swizzle(&x.expr, "xyz")),
            ValueType::Vec3,
            x.uses_time,
        )),
        (ValueType::Vec4, ValueType::Vec2) => Ok(TypedExpr::with_time(
            format!("vec2f({})", swizzle(&x.expr, "xy")),
            ValueType::Vec2,
            x.uses_time,
        )),
        (ValueType::Vec3, ValueType::Vec2) => Ok(TypedExpr::with_time(
            format!("vec2f({})", swizzle(&x.expr, "xy")),
            ValueType::Vec2,
            x.uses_time,
        )),

        (ValueType::Vec2, ValueType::Vec3) => Ok(TypedExpr::with_time(
            format!("vec3f({}, 0.0)", x.expr),
            ValueType::Vec3,
            x.uses_time,
        )),
        (ValueType::Vec2, ValueType::Vec4) => Ok(TypedExpr::with_time(
            format!("vec4f({}, 0.0, 1.0)", x.expr),
            ValueType::Vec4,
            x.uses_time,
        )),
        (ValueType::Vec3, ValueType::Vec4) => Ok(TypedExpr::with_time(
            format!("vec4f({}, 0.0)", x.expr),
            ValueType::Vec4,
            x.uses_time,
        )),

        (ValueType::U32, ValueType::I32) => Ok(TypedExpr::with_time(
            format!("i32({})", x.expr),
            ValueType::I32,
            x.uses_time,
        )),
        (ValueType::I32, ValueType::U32) => Ok(TypedExpr::with_time(
            format!("u32({})", x.expr),
            ValueType::U32,
            x.uses_time,
        )),

        _ => bail!("unsupported type coercion: {:?} -> {:?}", x.ty, target),
    }
}

/// Coerce two typed expressions for binary operations.
///
/// Rules:
/// - If types match, keep.
/// - If either side is a vector, splat the other scalar side.
/// - If both are scalars, coerce to a common numeric type (prefer f32).
pub fn coerce_for_binary(a: TypedExpr, b: TypedExpr) -> Result<(TypedExpr, TypedExpr, ValueType)> {
    if a.ty == b.ty {
        let ty = a.ty;
        return Ok((a, b, ty));
    }

    let is_vector = |t: ValueType| matches!(t, ValueType::Vec2 | ValueType::Vec3 | ValueType::Vec4);
    let is_scalar = |t: ValueType| {
        matches!(
            t,
            ValueType::F32 | ValueType::I32 | ValueType::U32 | ValueType::Bool
        )
    };

    // Vector/scalar: splat scalar to vector.
    if is_vector(a.ty) && is_scalar(b.ty) {
        let target_ty = a.ty;
        let bb = splat_scalar(&b, target_ty)?;
        return Ok((a, bb, target_ty));
    }
    if is_vector(b.ty) && is_scalar(a.ty) {
        let target_ty = b.ty;
        let aa = splat_scalar(&a, target_ty)?;
        return Ok((aa, b, target_ty));
    }

    // Scalar/scalar: prefer f32.
    if is_scalar(a.ty) && is_scalar(b.ty) {
        let aa = coerce_to_type(a, ValueType::F32)?;
        let bb = coerce_to_type(b, ValueType::F32)?;
        return Ok((aa, bb, ValueType::F32));
    }

    bail!(
        "incompatible types for binary op: {:?} and {:?}",
        a.ty,
        b.ty
    );
}

/// Convert a typed expression to vec4 color format.
pub fn to_vec4_color(x: TypedExpr) -> TypedExpr {
    // Color is treated as vec4f, but participates in scalar/vector coercions.
    // This helper is used at final material output plumbing.
    match x.ty {
        ValueType::Vec4 => x,
        _ => coerce_to_type(x, ValueType::Vec4)
            .unwrap_or_else(|_| TypedExpr::new("vec4f(0.0, 0.0, 0.0, 1.0)", ValueType::Vec4)),
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

#[cfg(test)]
mod type_coercion_tests {
    use super::*;

    #[test]
    fn bool_to_numeric_scalar() {
        let t = TypedExpr::new("true", ValueType::Bool);
        let f = TypedExpr::new("false", ValueType::Bool);

        let tf = coerce_to_type(t.clone(), ValueType::F32).unwrap();
        assert_eq!(tf.ty, ValueType::F32);
        assert!(tf.expr.contains("select(0.0, 1.0"));

        let ti = coerce_to_type(t, ValueType::I32).unwrap();
        assert_eq!(ti.ty, ValueType::I32);
        assert!(ti.expr.contains("select(0, 1"));

        let fi = coerce_to_type(f, ValueType::I32).unwrap();
        assert_eq!(fi.ty, ValueType::I32);
    }

    #[test]
    fn scalar_to_vector_splat() {
        let x = TypedExpr::new("1", ValueType::I32);
        let v2 = coerce_to_type(x.clone(), ValueType::Vec2).unwrap();
        assert_eq!(v2.ty, ValueType::Vec2);
        assert!(v2.expr.starts_with("vec2f("));

        let v4 = coerce_to_type(x, ValueType::Vec4).unwrap();
        assert_eq!(v4.ty, ValueType::Vec4);
        assert!(v4.expr.starts_with("vec4f("));
    }

    #[test]
    fn vector_dimension_changes_match_contract() {
        let v2 = TypedExpr::new("v2", ValueType::Vec2);
        let v3 = coerce_to_type(v2.clone(), ValueType::Vec3).unwrap();
        assert_eq!(v3.expr, "vec3f(v2, 0.0)");

        let v4 = coerce_to_type(v2, ValueType::Vec4).unwrap();
        assert_eq!(v4.expr, "vec4f(v2, 0.0, 1.0)");

        let v4in = TypedExpr::new("v4", ValueType::Vec4);
        let down2 = coerce_to_type(v4in.clone(), ValueType::Vec2).unwrap();
        assert_eq!(down2.expr, "vec2f((v4).xy)");

        let down3 = coerce_to_type(v4in, ValueType::Vec3).unwrap();
        assert_eq!(down3.expr, "vec3f((v4).xyz)");
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
                let Some(hi) = hex(hi) else {
                    bail!("invalid percent-encoding");
                };
                let Some(lo) = hex(lo) else {
                    bail!("invalid percent-encoding");
                };
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
