//! Compiler for Remap node.
//!
//! The Remap node shapes a scalar signal `t` based on `mode`.
//!
//! Modes supported:
//! - smoothstep(edge0, edge1, t)
//! - linearMap: clamp((t - from)/(to-from), 0..1)
//! - iq_* variants from https://iquilezles.org/articles/functions/

use anyhow::{bail, Result};
use std::collections::HashMap;

use super::super::types::{MaterialCompileContext, TypedExpr, ValueType};
use super::super::utils::coerce_to_type;
use crate::dsl::{incoming_connection, parse_f32, parse_str, Node, SceneDSL};

fn wgsl_f32_literal(v: f32) -> String {
    // Keep literals stable and unambiguous.
    if v.is_finite() {
        if v.fract() == 0.0 {
            format!("{v:.1}")
        } else {
            v.to_string()
        }
    } else {
        "0.0".to_string()
    }
}

fn resolve_f32_input<F>(
    scene: &SceneDSL,
    node: &Node,
    port_id: &str,
    default: f32,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: &F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    if let Some(conn) = incoming_connection(scene, &node.id, port_id) {
        let expr = compile_fn(&conn.from.node_id, Some(&conn.from.port_id), ctx, cache)?;
        return coerce_to_type(expr, ValueType::F32);
    }

    let v = parse_f32(&node.params, port_id).unwrap_or(default);
    Ok(TypedExpr::new(wgsl_f32_literal(v), ValueType::F32))
}

pub fn compile_remap<F>(
    scene: &SceneDSL,
    _nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    out_port: Option<&str>,
    ctx: &mut MaterialCompileContext,
    cache: &mut HashMap<(String, String), TypedExpr>,
    compile_fn: F,
) -> Result<TypedExpr>
where
    F: Fn(
        &str,
        Option<&str>,
        &mut MaterialCompileContext,
        &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr>,
{
    let port = out_port.unwrap_or("result");
    if port != "result" {
        bail!("Remap: unsupported output port '{port}'");
    }

    let mode = parse_str(&node.params, "mode")
        .unwrap_or("smoothstep")
        .trim();

    let t = resolve_f32_input(scene, node, "t", 0.5, ctx, cache, &compile_fn)?;

    let eps = "1e-6";
    let pi = "3.141592653589793";
    let clamp01 = |x: &str| format!("clamp(({x}), 0.0, 1.0)");
    let safe_div = |num: &str, denom: &str| format!("(({num}) / max(abs(({denom})), {eps}))");

    let expr = match mode {
        "smoothstep" => {
            let e0 = resolve_f32_input(scene, node, "edge0", 0.0, ctx, cache, &compile_fn)?;
            let e1 = resolve_f32_input(scene, node, "edge1", 1.0, ctx, cache, &compile_fn)?;
            TypedExpr::with_time(
                format!("smoothstep({}, {}, {})", e0.expr, e1.expr, t.expr),
                ValueType::F32,
                t.uses_time || e0.uses_time || e1.uses_time,
            )
        }

        "linearMap" => {
            let from = resolve_f32_input(scene, node, "from", 0.0, ctx, cache, &compile_fn)?;
            let to = resolve_f32_input(scene, node, "to", 1.0, ctx, cache, &compile_fn)?;
            let denom = format!("({} - {})", to.expr, from.expr);
            let mapped = safe_div(&format!("({} - {})", t.expr, from.expr), &denom);
            TypedExpr::with_time(
                format!("clamp({}, 0.0, 1.0)", mapped),
                ValueType::F32,
                t.uses_time || from.uses_time || to.uses_time,
            )
        }

        // https://iquilezles.org/articles/functions/
        "iq_almostIdentity_v1" => {
            let m = resolve_f32_input(scene, node, "m", 1.0, ctx, cache, &compile_fn)?;
            let n = resolve_f32_input(scene, node, "n", 0.0, ctx, cache, &compile_fn)?;
            let m_safe = format!("max(abs(({})), {eps})", m.expr);
            let tt = format!("(({}) / ({m_safe}))", t.expr);
            let a = format!("(2.0*({}) - ({m_safe}))", n.expr);
            let b = format!("(2.0*({m_safe}) - 3.0*({}))", n.expr);
            let y = format!("((({a})*({tt}) + ({b}))*({tt})*({tt}) + ({}))", n.expr);
            TypedExpr::with_time(
                format!("select(({y}), ({}), (({}) > ({})))", t.expr, t.expr, m.expr),
                ValueType::F32,
                t.uses_time || m.uses_time || n.uses_time,
            )
        }
        "iq_almostIdentity_v2" => {
            let nn = resolve_f32_input(scene, node, "nn", 0.1, ctx, cache, &compile_fn)?;
            TypedExpr::with_time(
                format!(
                    "sqrt(({})*({}) + ({})*({}))",
                    t.expr, t.expr, nn.expr, nn.expr
                ),
                ValueType::F32,
                t.uses_time || nn.uses_time,
            )
        }
        "iq_integralSmoothstep" => {
            let tt = resolve_f32_input(scene, node, "T", 1.0, ctx, cache, &compile_fn)?;
            let t_safe = format!("max(abs(({})), {eps})", tt.expr);
            let y0 = format!("({} - ({})/2.0)", t.expr, t_safe);
            let y1 = format!(
                "({x})*({x})*({x})*(1.0-({x})*0.5/({t})) / (({t})*({t}))",
                x = t.expr,
                t = t_safe
            );
            TypedExpr::with_time(
                format!("select(({y1}), ({y0}), (({}) > ({})))", t.expr, tt.expr),
                ValueType::F32,
                t.uses_time || tt.uses_time,
            )
        }
        "iq_expImpulse" => {
            let k = resolve_f32_input(scene, node, "k", 1.0, ctx, cache, &compile_fn)?;
            let h = format!("(({})*({}))", k.expr, t.expr);
            TypedExpr::with_time(
                format!("({h})*exp(1.0-({h}))"),
                ValueType::F32,
                t.uses_time || k.uses_time,
            )
        }
        "iq_quaImpulse" => {
            let k = resolve_f32_input(scene, node, "k", 1.0, ctx, cache, &compile_fn)?;
            let denom = format!("(1.0 + ({})*({})*({}))", k.expr, t.expr, t.expr);
            TypedExpr::with_time(
                format!("(2.0*sqrt(({k}))*({x})/({denom}))", k = k.expr, x = t.expr),
                ValueType::F32,
                t.uses_time || k.uses_time,
            )
        }
        "iq_polyImpulse" => {
            let k = resolve_f32_input(scene, node, "k", 1.0, ctx, cache, &compile_fn)?;
            let n = resolve_f32_input(scene, node, "n", 2.0, ctx, cache, &compile_fn)?;
            // (n/(n-1))*pow((n-1)*k,1/n)*x/(1+k*pow(x,n))
            let n_minus_1 = format!("(({}) - 1.0)", n.expr);
            let pre = format!("(({}) / ({n_minus_1}))", n.expr);
            let pow1 = format!("pow(({n_minus_1})*({}), 1.0/({}))", k.expr, n.expr);
            let denom = format!("(1.0 + ({})*pow(({}), ({})))", k.expr, t.expr, n.expr);
            TypedExpr::with_time(
                format!("({pre})*({pow1})*({})/({denom})", t.expr),
                ValueType::F32,
                t.uses_time || k.uses_time || n.uses_time,
            )
        }
        "iq_expSustainedImpulse" => {
            let f = resolve_f32_input(scene, node, "f", 0.5, ctx, cache, &compile_fn)?;
            let k = resolve_f32_input(scene, node, "k", 1.0, ctx, cache, &compile_fn)?;
            let f_safe = format!("max(abs(({})), {eps})", f.expr);
            let s = format!("max(({})-({}), 0.0)", t.expr, f.expr);
            let a = format!(
                "min((({x})*({x}))/(({f})*({f})), 1.0 + (2.0/({f}))*({s})*exp(-({k})*({s})))",
                x = t.expr,
                f = f_safe,
                s = s,
                k = k.expr,
            );
            TypedExpr::with_time(a, ValueType::F32, t.uses_time || f.uses_time || k.uses_time)
        }
        "iq_sincImpulse" => {
            let k = resolve_f32_input(scene, node, "k", 1.0, ctx, cache, &compile_fn)?;
            let a = format!("({pi})*(({})*({}) - 1.0)", k.expr, t.expr);
            let a_safe = format!("select(({a}), ({eps}), abs(({a})) < {eps})");
            TypedExpr::with_time(
                format!("sin(({a})) / ({a_safe})"),
                ValueType::F32,
                t.uses_time || k.uses_time,
            )
        }
        "iq_trunc_falloff" => {
            let m = resolve_f32_input(scene, node, "m", 1.0, ctx, cache, &compile_fn)?;
            let x = safe_div(&t.expr, &m.expr);
            TypedExpr::with_time(
                format!("(({x}) - 2.0)*({x}) + 1.0"),
                ValueType::F32,
                t.uses_time || m.uses_time,
            )
        }
        "iq_almostUnitIdentity" => {
            let x = clamp01(&t.expr);
            TypedExpr::with_time(
                format!("({x})*({x})*(2.0-({x}))"),
                ValueType::F32,
                t.uses_time,
            )
        }
        "iq_gain" => {
            let k = resolve_f32_input(scene, node, "k", 1.0, ctx, cache, &compile_fn)?;
            let x = clamp01(&t.expr);
            let y = format!("select(1.0-({x}), ({x}), ({x}) < 0.5)");
            let a = format!("0.5*pow(2.0*({y}), ({}))", k.expr);
            TypedExpr::with_time(
                format!("select(1.0-({a}), ({a}), ({x}) < 0.5)"),
                ValueType::F32,
                t.uses_time || k.uses_time,
            )
        }
        "iq_parabola" => {
            let k = resolve_f32_input(scene, node, "k", 1.0, ctx, cache, &compile_fn)?;
            let x = clamp01(&t.expr);
            TypedExpr::with_time(
                format!("pow(4.0*({x})*(1.0-({x})), ({}))", k.expr),
                ValueType::F32,
                t.uses_time || k.uses_time,
            )
        }
        "iq_pcurve" => {
            let a = resolve_f32_input(scene, node, "a", 1.0, ctx, cache, &compile_fn)?;
            let b = resolve_f32_input(scene, node, "b", 1.0, ctx, cache, &compile_fn)?;
            let x = clamp01(&t.expr);
            let ab = format!("(({}) + ({}))", a.expr, b.expr);
            let kk = format!(
                "pow(({ab}), ({ab})) / (pow(({}), ({})) * pow(({}), ({})))",
                a.expr, a.expr, b.expr, b.expr
            );
            TypedExpr::with_time(
                format!(
                    "({kk})*pow(({x}), ({}))*pow(1.0-({x}), ({}))",
                    a.expr, b.expr
                ),
                ValueType::F32,
                t.uses_time || a.uses_time || b.uses_time,
            )
        }
        "iq_tone" => {
            let k = resolve_f32_input(scene, node, "k", 1.0, ctx, cache, &compile_fn)?;
            let denom = format!("(1.0 + ({})*({}))", k.expr, t.expr);
            TypedExpr::with_time(
                format!("((({})+1.0)/({}))", k.expr, denom),
                ValueType::F32,
                t.uses_time || k.uses_time,
            )
        }
        "iq_cubicPulse" => {
            let c = resolve_f32_input(scene, node, "c", 0.0, ctx, cache, &compile_fn)?;
            let w = resolve_f32_input(scene, node, "w", 1.0, ctx, cache, &compile_fn)?;
            let w_safe = format!("max(abs(({})), {eps})", w.expr);
            let x = format!("abs(({}) - ({}))", t.expr, c.expr);
            let xx = format!("(({x}) / ({w_safe}))");
            let y = format!("1.0 - ({xx})*({xx})*(3.0 - 2.0*({xx}))");
            TypedExpr::with_time(
                format!("select(({y}), 0.0, ({x}) > ({}))", w.expr),
                ValueType::F32,
                t.uses_time || c.uses_time || w.uses_time,
            )
        }
        "iq_rationalBump" => {
            let k = resolve_f32_input(scene, node, "k", 1.0, ctx, cache, &compile_fn)?;
            TypedExpr::with_time(
                format!("1.0/(1.0 + ({})*({})*({}))", k.expr, t.expr, t.expr),
                ValueType::F32,
                t.uses_time || k.uses_time,
            )
        }
        "iq_expStep" => {
            let n = resolve_f32_input(scene, node, "n", 2.0, ctx, cache, &compile_fn)?;
            let x = format!("max(({}), 0.0)", t.expr);
            TypedExpr::with_time(
                format!("exp2(-exp2(({}))*pow(({x}), ({})))", n.expr, n.expr),
                ValueType::F32,
                t.uses_time || n.uses_time,
            )
        }

        _ => {
            // Fall back to smoothstep.
            let e0 = resolve_f32_input(scene, node, "edge0", 0.0, ctx, cache, &compile_fn)?;
            let e1 = resolve_f32_input(scene, node, "edge1", 1.0, ctx, cache, &compile_fn)?;
            TypedExpr::with_time(
                format!("smoothstep({}, {}, {})", e0.expr, e1.expr, t.expr),
                ValueType::F32,
                t.uses_time || e0.uses_time || e1.uses_time,
            )
        }
    };

    Ok(expr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{Connection, Endpoint, Metadata};
    use anyhow::bail;

    fn test_scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
        SceneDSL {
            version: "1.0".to_string(),
            metadata: Metadata {
                name: "test".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections,
            outputs: None,
            groups: Vec::new(),
        }
    }

    fn mock_compile_fn(
        node_id: &str,
        _port: Option<&str>,
        _ctx: &mut MaterialCompileContext,
        _cache: &mut HashMap<(String, String), TypedExpr>,
    ) -> Result<TypedExpr> {
        match node_id {
            "t_src" => Ok(TypedExpr::new("x", ValueType::F32)),
            _ => bail!("unknown node"),
        }
    }

    #[test]
    fn remap_smoothstep_params() {
        let remap = Node {
            id: "remap".to_string(),
            node_type: "Remap".to_string(),
            params: HashMap::from([
                ("mode".to_string(), serde_json::json!("smoothstep")),
                ("edge0".to_string(), serde_json::json!(0.25)),
                ("edge1".to_string(), serde_json::json!(0.75)),
                ("t".to_string(), serde_json::json!(0.5)),
            ]),
            inputs: vec![],
            input_bindings: vec![],
            outputs: vec![],
        };
        let scene = test_scene(vec![remap.clone()], vec![]);
        let nodes_by_id: HashMap<String, Node> = vec![(remap.id.clone(), remap.clone())]
            .into_iter()
            .collect();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let out = compile_remap(
            &scene,
            &nodes_by_id,
            &remap,
            None,
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();
        assert_eq!(out.ty, ValueType::F32);
        assert!(out.expr.starts_with("smoothstep("));
    }

    #[test]
    fn remap_linear_map_connected_t() {
        let remap = Node {
            id: "remap".to_string(),
            node_type: "Remap".to_string(),
            params: HashMap::from([
                ("mode".to_string(), serde_json::json!("linearMap")),
                ("from".to_string(), serde_json::json!(0.0)),
                ("to".to_string(), serde_json::json!(10.0)),
            ]),
            inputs: vec![],
            input_bindings: vec![],
            outputs: vec![],
        };
        let t_src = Node {
            id: "t_src".to_string(),
            node_type: "FloatInput".to_string(),
            params: HashMap::new(),
            inputs: vec![],
            input_bindings: vec![],
            outputs: vec![],
        };
        let conn = Connection {
            id: "c1".to_string(),
            from: Endpoint {
                node_id: "t_src".to_string(),
                port_id: "value".to_string(),
            },
            to: Endpoint {
                node_id: "remap".to_string(),
                port_id: "t".to_string(),
            },
        };

        let scene = test_scene(vec![remap.clone(), t_src], vec![conn]);
        let nodes_by_id: HashMap<String, Node> = scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect();
        let mut ctx = MaterialCompileContext::default();
        let mut cache = HashMap::new();

        let out = compile_remap(
            &scene,
            &nodes_by_id,
            &remap,
            None,
            &mut ctx,
            &mut cache,
            mock_compile_fn,
        )
        .unwrap();
        assert_eq!(out.ty, ValueType::F32);
        assert!(out.expr.contains("clamp("));
        assert!(out.expr.contains("x"));
    }
}
