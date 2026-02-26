use std::collections::HashMap;

use anyhow::{Result, anyhow, bail};

use crate::{
    dsl::{Node, SceneDSL, find_node, incoming_connection},
    renderer::utils::{cpu_num_f32, mat4_is_identity, parse_strict_mat4_param_column_major},
};

const EPSILON: f32 = 1e-6;
const CAMERA_MATRIX_COMPARE_EPSILON: f32 = 1e-5;

pub fn legacy_projection_camera_matrix(target_size: [f32; 2]) -> [f32; 16] {
    let w = if target_size[0].is_finite() && target_size[0] > 0.0 {
        target_size[0]
    } else {
        1.0
    };
    let h = if target_size[1].is_finite() && target_size[1] > 0.0 {
        target_size[1]
    } else {
        1.0
    };

    [
        2.0 / w,
        0.0,
        0.0,
        0.0,
        0.0,
        2.0 / h,
        0.0,
        0.0,
        0.0,
        0.0,
        1.0 / w,
        0.0,
        -1.0,
        -1.0,
        0.0,
        1.0,
    ]
}

pub fn resolve_effective_camera_for_pass_node(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    target_size: [f32; 2],
) -> Result<[f32; 16]> {
    if let Some(conn) = incoming_connection(scene, &node.id, "camera") {
        return resolve_mat4_output_column_major(
            scene,
            nodes_by_id,
            &conn.from.node_id,
            &conn.from.port_id,
        )
        .map_err(|e| {
            anyhow!(
                "{}.camera failed to resolve connected mat4 from {}.{}: {e:#}",
                node.id,
                conn.from.node_id,
                conn.from.port_id
            )
        });
    }

    let context = format!("{}.camera", node.id);
    if let Some(camera) = parse_strict_mat4_param_column_major(&node.params, "camera", &context)? {
        if mat4_is_identity(&camera) {
            return Ok(legacy_projection_camera_matrix(target_size));
        }
        return Ok(camera);
    }

    Ok(legacy_projection_camera_matrix(target_size))
}

pub fn camera_matrices_approximately_equal(lhs: &[f32; 16], rhs: &[f32; 16], epsilon: f32) -> bool {
    let epsilon = if epsilon.is_finite() && epsilon >= 0.0 {
        epsilon
    } else {
        0.0
    };
    lhs.iter()
        .zip(rhs.iter())
        .all(|(l, r)| (*l - *r).abs() <= epsilon)
}

pub fn pass_node_uses_custom_camera(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    target_size: [f32; 2],
) -> Result<bool> {
    let resolved = resolve_effective_camera_for_pass_node(scene, nodes_by_id, node, target_size)?;
    let fallback = legacy_projection_camera_matrix(target_size);
    Ok(!camera_matrices_approximately_equal(
        &resolved,
        &fallback,
        CAMERA_MATRIX_COMPARE_EPSILON,
    ))
}

pub fn resolve_mat4_output_column_major(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node_id: &str,
    port_id: &str,
) -> Result<[f32; 16]> {
    let node = find_node(nodes_by_id, node_id)?;
    match node.node_type.as_str() {
        "PerspectiveCamera" => {
            if port_id != "camera" {
                bail!(
                    "PerspectiveCamera '{}' has no output port '{}' (expected 'camera')",
                    node_id,
                    port_id
                );
            }
            resolve_perspective_camera_matrix(scene, nodes_by_id, node)
        }
        "OrthographicCamera" => {
            if port_id != "camera" {
                bail!(
                    "OrthographicCamera '{}' has no output port '{}' (expected 'camera')",
                    node_id,
                    port_id
                );
            }
            resolve_orthographic_camera_matrix(scene, nodes_by_id, node)
        }
        _ => {
            let context = format!("{}.{}", node.id, port_id);
            if let Some(matrix) =
                parse_strict_mat4_param_column_major(&node.params, port_id, &context)?
            {
                return Ok(matrix);
            }

            bail!(
                "node '{}' ({}) output '{}': expected mat4-producing node/output",
                node.id,
                node.node_type,
                port_id
            );
        }
    }
}

fn resolve_perspective_camera_matrix(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
) -> Result<[f32; 16]> {
    let position =
        resolve_camera_vec3_input(scene, nodes_by_id, node, "position", [0.0, 0.0, 1000.0])?;
    let target = resolve_camera_vec3_input(scene, nodes_by_id, node, "target", [0.0, 0.0, 0.0])?;
    let up = resolve_camera_vec3_input(scene, nodes_by_id, node, "up", [0.0, 1.0, 0.0])?;

    let fovy_deg = resolve_camera_scalar_input(scene, nodes_by_id, node, "fovY", 60.0)?;
    if !(fovy_deg > 0.0 && fovy_deg < 180.0) {
        bail!(
            "{}.fovY must be > 0 and < 180 degrees, got {}",
            node.id,
            fovy_deg
        );
    }

    let aspect = resolve_camera_scalar_input(scene, nodes_by_id, node, "aspect", 1.0)?;
    if !(aspect > 0.0) {
        bail!("{}.aspect must be > 0, got {}", node.id, aspect);
    }

    let near = resolve_camera_scalar_input(scene, nodes_by_id, node, "near", 0.1)?;
    let far = resolve_camera_scalar_input(scene, nodes_by_id, node, "far", 10000.0)?;
    validate_near_far(node.id.as_str(), near, far)?;

    let view = look_at_view_matrix(position, target, up, node.id.as_str())?;
    let projection = perspective_rh_zo_matrix(fovy_deg.to_radians(), aspect, near, far);
    Ok(mat4_mul_col_major(projection, view))
}

fn resolve_orthographic_camera_matrix(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
) -> Result<[f32; 16]> {
    let position =
        resolve_camera_vec3_input(scene, nodes_by_id, node, "position", [0.0, 0.0, 1.0])?;
    let target = resolve_camera_vec3_input(scene, nodes_by_id, node, "target", [0.0, 0.0, 0.0])?;
    let up = resolve_camera_vec3_input(scene, nodes_by_id, node, "up", [0.0, 1.0, 0.0])?;

    let left = resolve_camera_scalar_input(scene, nodes_by_id, node, "left", -1.0)?;
    let right = resolve_camera_scalar_input(scene, nodes_by_id, node, "right", 1.0)?;
    let bottom = resolve_camera_scalar_input(scene, nodes_by_id, node, "bottom", -1.0)?;
    let top = resolve_camera_scalar_input(scene, nodes_by_id, node, "top", 1.0)?;
    let near = resolve_camera_scalar_input(scene, nodes_by_id, node, "near", 0.1)?;
    let far = resolve_camera_scalar_input(scene, nodes_by_id, node, "far", 10000.0)?;

    if (right - left).abs() <= EPSILON {
        bail!(
            "{}.right and {}.left must not be equal ({})",
            node.id,
            node.id,
            right
        );
    }
    if (top - bottom).abs() <= EPSILON {
        bail!(
            "{}.top and {}.bottom must not be equal ({})",
            node.id,
            node.id,
            top
        );
    }
    validate_near_far(node.id.as_str(), near, far)?;

    let view = look_at_view_matrix(position, target, up, node.id.as_str())?;
    let projection = orthographic_rh_zo_matrix(left, right, bottom, top, near, far);
    Ok(mat4_mul_col_major(projection, view))
}

fn validate_near_far(node_id: &str, near: f32, far: f32) -> Result<()> {
    if !(near > 0.0) {
        bail!("{node_id}.near must be > 0, got {near}");
    }
    if !(far > near) {
        bail!("{node_id}.far must be > near (near={near}, far={far})");
    }
    Ok(())
}

fn resolve_camera_scalar_input(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    key: &str,
    default: f32,
) -> Result<f32> {
    let value = cpu_num_f32(scene, nodes_by_id, node, key, default)?;
    if !value.is_finite() {
        bail!("{}.{} must be finite, got {}", node.id, key, value);
    }
    Ok(value)
}

fn resolve_camera_vec3_input(
    scene: &SceneDSL,
    nodes_by_id: &HashMap<String, Node>,
    node: &Node,
    key: &str,
    default: [f32; 3],
) -> Result<[f32; 3]> {
    if let Some(conn) = incoming_connection(scene, &node.id, key) {
        let from_node = find_node(nodes_by_id, &conn.from.node_id)?;
        if from_node.node_type != "Vector3Input" || conn.from.port_id != "vector" {
            bail!(
                "{}.{} expects Vector3Input.vector connection, got {}.{} ({})",
                node.id,
                key,
                conn.from.node_id,
                conn.from.port_id,
                from_node.node_type
            );
        }

        let x = cpu_num_f32(scene, nodes_by_id, from_node, "x", 0.0)?;
        let y = cpu_num_f32(scene, nodes_by_id, from_node, "y", 0.0)?;
        let z = cpu_num_f32(scene, nodes_by_id, from_node, "z", 0.0)?;
        let value = [x, y, z];
        ensure_finite_vec3(node.id.as_str(), key, value)?;
        return Ok(value);
    }

    let value = parse_inline_vec3_value(node, key, default)?;
    ensure_finite_vec3(node.id.as_str(), key, value)?;
    Ok(value)
}

fn parse_inline_vec3_value(node: &Node, key: &str, default: [f32; 3]) -> Result<[f32; 3]> {
    let Some(value) = node.params.get(key) else {
        return Ok(default);
    };

    if let Some(obj) = value.as_object() {
        let mut out = default;
        out[0] = obj.get("x").and_then(json_number_f32).unwrap_or(default[0]);
        out[1] = obj.get("y").and_then(json_number_f32).unwrap_or(default[1]);
        out[2] = obj.get("z").and_then(json_number_f32).unwrap_or(default[2]);
        return Ok(out);
    }

    if let Some(arr) = value.as_array() {
        if arr.len() < 3 {
            bail!(
                "{}.{} must be vec3 object {{x,y,z}} or array [x,y,z]",
                node.id,
                key
            );
        }
        let x = arr
            .first()
            .and_then(json_number_f32)
            .ok_or_else(|| anyhow!("{}.{}[0] must be numeric", node.id, key))?;
        let y = arr
            .get(1)
            .and_then(json_number_f32)
            .ok_or_else(|| anyhow!("{}.{}[1] must be numeric", node.id, key))?;
        let z = arr
            .get(2)
            .and_then(json_number_f32)
            .ok_or_else(|| anyhow!("{}.{}[2] must be numeric", node.id, key))?;
        return Ok([x, y, z]);
    }

    bail!(
        "{}.{} must be vec3 object {{x,y,z}} or array [x,y,z]",
        node.id,
        key
    )
}

fn json_number_f32(value: &serde_json::Value) -> Option<f32> {
    value
        .as_f64()
        .map(|x| x as f32)
        .or_else(|| value.as_i64().map(|x| x as f32))
        .or_else(|| value.as_u64().map(|x| x as f32))
}

fn ensure_finite_vec3(node_id: &str, key: &str, value: [f32; 3]) -> Result<()> {
    if value[0].is_finite() && value[1].is_finite() && value[2].is_finite() {
        Ok(())
    } else {
        bail!(
            "{}.{} must contain finite numbers, got [{}, {}, {}]",
            node_id,
            key,
            value[0],
            value[1],
            value[2]
        )
    }
}

fn mat4_mul_col_major(a: [f32; 16], b: [f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            out[col * 4 + row] = a[row] * b[col * 4]
                + a[4 + row] * b[col * 4 + 1]
                + a[8 + row] * b[col * 4 + 2]
                + a[12 + row] * b[col * 4 + 3];
        }
    }
    out
}

fn perspective_rh_zo_matrix(fovy_radians: f32, aspect: f32, near: f32, far: f32) -> [f32; 16] {
    let f = 1.0 / (0.5 * fovy_radians).tan();
    let z_scale = far / (near - far);
    let z_translate = (far * near) / (near - far);

    [
        f / aspect,
        0.0,
        0.0,
        0.0,
        0.0,
        f,
        0.0,
        0.0,
        0.0,
        0.0,
        z_scale,
        -1.0,
        0.0,
        0.0,
        z_translate,
        0.0,
    ]
}

fn orthographic_rh_zo_matrix(
    left: f32,
    right: f32,
    bottom: f32,
    top: f32,
    near: f32,
    far: f32,
) -> [f32; 16] {
    let sx = 2.0 / (right - left);
    let sy = 2.0 / (top - bottom);
    let sz = 1.0 / (near - far);

    let tx = -(right + left) / (right - left);
    let ty = -(top + bottom) / (top - bottom);
    let tz = near / (near - far);

    [
        sx, 0.0, 0.0, 0.0, 0.0, sy, 0.0, 0.0, 0.0, 0.0, sz, 0.0, tx, ty, tz, 1.0,
    ]
}

fn vec3_sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn vec3_dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn vec3_cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn vec3_length(v: [f32; 3]) -> f32 {
    vec3_dot(v, v).sqrt()
}

fn vec3_normalize(v: [f32; 3], what: &str, node_id: &str) -> Result<[f32; 3]> {
    let len = vec3_length(v);
    if !(len > EPSILON) {
        bail!("{}: degenerate {} vector", node_id, what);
    }
    Ok([v[0] / len, v[1] / len, v[2] / len])
}

fn look_at_view_matrix(
    position: [f32; 3],
    target: [f32; 3],
    up: [f32; 3],
    node_id: &str,
) -> Result<[f32; 16]> {
    let forward = vec3_normalize(vec3_sub(target, position), "direction", node_id)?;
    let up_norm = vec3_normalize(up, "up", node_id)?;

    let right_raw = vec3_cross(forward, up_norm);
    let right = vec3_normalize(right_raw, "right", node_id).map_err(|_| {
        anyhow!(
            "{}: degenerate direction/up vectors (cannot build orthonormal basis)",
            node_id
        )
    })?;
    let true_up = vec3_cross(right, forward);

    Ok([
        right[0],
        true_up[0],
        -forward[0],
        0.0,
        right[1],
        true_up[1],
        -forward[1],
        0.0,
        right[2],
        true_up[2],
        -forward[2],
        0.0,
        -vec3_dot(right, position),
        -vec3_dot(true_up, position),
        vec3_dot(forward, position),
        1.0,
    ])
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::{
        dsl::{Connection, Endpoint, Metadata, Node, SceneDSL},
        renderer::utils::{IDENTITY_MAT4, mat4_row_major_to_column_major},
    };
    use serde_json::json;

    fn node(id: &str, node_type: &str, params: serde_json::Value) -> Node {
        let params = params
            .as_object()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .collect();
        Node {
            id: id.to_string(),
            node_type: node_type.to_string(),
            params,
            inputs: vec![],
            outputs: vec![],
            input_bindings: vec![],
        }
    }

    fn conn(from_node: &str, from_port: &str, to_node: &str, to_port: &str) -> Connection {
        Connection {
            id: format!("{from_node}.{from_port}->{to_node}.{to_port}"),
            from: Endpoint {
                node_id: from_node.to_string(),
                port_id: from_port.to_string(),
            },
            to: Endpoint {
                node_id: to_node.to_string(),
                port_id: to_port.to_string(),
            },
        }
    }

    fn scene(nodes: Vec<Node>, connections: Vec<Connection>) -> SceneDSL {
        SceneDSL {
            version: "2.0".to_string(),
            metadata: Metadata {
                name: "camera-tests".to_string(),
                created: None,
                modified: None,
            },
            nodes,
            connections,
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
        }
    }

    fn nodes_by_id(scene: &SceneDSL) -> HashMap<String, Node> {
        scene
            .nodes
            .iter()
            .cloned()
            .map(|n| (n.id.clone(), n))
            .collect()
    }

    fn mat4_mul_vec4_col_major(m: [f32; 16], v: [f32; 4]) -> [f32; 4] {
        [
            m[0] * v[0] + m[4] * v[1] + m[8] * v[2] + m[12] * v[3],
            m[1] * v[0] + m[5] * v[1] + m[9] * v[2] + m[13] * v[3],
            m[2] * v[0] + m[6] * v[1] + m[10] * v[2] + m[14] * v[3],
            m[3] * v[0] + m[7] * v[1] + m[11] * v[2] + m[15] * v[3],
        ]
    }

    #[test]
    fn legacy_projection_matches_previous_mapping() {
        let m = legacy_projection_camera_matrix([400.0, 200.0]);
        assert_eq!(m[0], 2.0 / 400.0);
        assert_eq!(m[5], 2.0 / 200.0);
        assert_eq!(m[10], 1.0 / 400.0);
        assert_eq!(m[12], -1.0);
        assert_eq!(m[13], -1.0);
    }

    #[test]
    fn identity_constant_is_still_identity() {
        assert!(mat4_is_identity(&IDENTITY_MAT4));
    }

    #[test]
    fn look_at_canonical_camera_is_identity() {
        let view = look_at_view_matrix([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], [0.0, 1.0, 0.0], "cam")
            .expect("look_at");
        for (i, (a, b)) in view.iter().zip(IDENTITY_MAT4.iter()).enumerate() {
            assert!(
                (a - b).abs() < 1e-6,
                "matrix mismatch at index {i}: got {a}, expected {b}"
            );
        }
    }

    #[test]
    fn look_at_applies_camera_translation() {
        let view = look_at_view_matrix([0.0, 0.0, 1.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], "cam")
            .expect("look_at");
        let p_view = mat4_mul_vec4_col_major(view, [0.0, 0.0, 0.0, 1.0]);
        assert!((p_view[0] - 0.0).abs() < 1e-6);
        assert!((p_view[1] - 0.0).abs() < 1e-6);
        assert!((p_view[2] + 1.0).abs() < 1e-6);
        assert!((p_view[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn connected_camera_overrides_inline_param() {
        let inline_row_major = [
            1.0, 0.0, 0.0, 5.0, 0.0, 1.0, 0.0, 6.0, 0.0, 0.0, 1.0, 7.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let scene = scene(
            vec![
                node(
                    "rp",
                    "RenderPass",
                    json!({
                        "camera": inline_row_major
                    }),
                ),
                node(
                    "cam",
                    "OrthographicCamera",
                    json!({
                        "position": {"x": 0.0, "y": 0.0, "z": 1.0},
                        "target": {"x": 0.0, "y": 0.0, "z": 0.0},
                        "up": {"x": 0.0, "y": 1.0, "z": 0.0},
                        "left": 0.0,
                        "right": 320.0,
                        "bottom": 0.0,
                        "top": 240.0,
                        "near": 0.1,
                        "far": 1000.0
                    }),
                ),
            ],
            vec![conn("cam", "camera", "rp", "camera")],
        );
        let nodes_by_id = nodes_by_id(&scene);
        let pass = nodes_by_id.get("rp").expect("render pass node");
        let resolved =
            resolve_effective_camera_for_pass_node(&scene, &nodes_by_id, pass, [320.0, 240.0])
                .expect("effective camera");
        let connected = resolve_mat4_output_column_major(&scene, &nodes_by_id, "cam", "camera")
            .expect("connected camera");
        assert_eq!(resolved, connected);
        assert_ne!(
            resolved,
            mat4_row_major_to_column_major(inline_row_major),
            "connected camera should take precedence over inline params.camera",
        );
    }

    #[test]
    fn inline_camera_overrides_legacy_fallback() {
        let inline_row_major = [
            1.0, 0.0, 0.0, 5.0, 0.0, 1.0, 0.0, 6.0, 0.0, 0.0, 1.0, 7.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let scene = scene(
            vec![node(
                "rp",
                "RenderPass",
                json!({
                    "camera": inline_row_major
                }),
            )],
            vec![],
        );
        let nodes_by_id = nodes_by_id(&scene);
        let pass = nodes_by_id.get("rp").expect("render pass node");
        let resolved =
            resolve_effective_camera_for_pass_node(&scene, &nodes_by_id, pass, [320.0, 240.0])
                .expect("effective camera");
        let expected_inline = mat4_row_major_to_column_major(inline_row_major);
        assert_eq!(resolved, expected_inline);
        assert_ne!(resolved, legacy_projection_camera_matrix([320.0, 240.0]));
    }

    #[test]
    fn identity_inline_camera_uses_legacy_fallback_override() {
        let scene = scene(
            vec![node(
                "rp",
                "RenderPass",
                json!({
                    "camera": IDENTITY_MAT4
                }),
            )],
            vec![],
        );
        let nodes_by_id = nodes_by_id(&scene);
        let pass = nodes_by_id.get("rp").expect("render pass node");
        let resolved =
            resolve_effective_camera_for_pass_node(&scene, &nodes_by_id, pass, [320.0, 240.0])
                .expect("effective camera");
        assert_eq!(resolved, legacy_projection_camera_matrix([320.0, 240.0]));
    }

    #[test]
    fn pass_node_custom_camera_detection_defaults_to_fallback() {
        let scene = scene(vec![node("rp", "RenderPass", json!({}))], vec![]);
        let nodes_by_id = nodes_by_id(&scene);
        let pass = nodes_by_id.get("rp").expect("render pass node");
        let uses_custom = pass_node_uses_custom_camera(&scene, &nodes_by_id, pass, [320.0, 240.0])
            .expect("custom camera detection");
        assert!(!uses_custom);
    }

    #[test]
    fn pass_node_custom_camera_detection_accepts_custom_inline_matrix() {
        let inline_row_major = [
            1.0, 0.0, 0.0, 10.0, 0.0, 1.0, 0.0, 20.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
        ];
        let scene = scene(
            vec![node(
                "rp",
                "RenderPass",
                json!({
                    "camera": inline_row_major
                }),
            )],
            vec![],
        );
        let nodes_by_id = nodes_by_id(&scene);
        let pass = nodes_by_id.get("rp").expect("render pass node");
        let uses_custom = pass_node_uses_custom_camera(&scene, &nodes_by_id, pass, [320.0, 240.0])
            .expect("custom camera detection");
        assert!(uses_custom);
    }

    #[test]
    fn camera_matrix_approximate_equality_uses_epsilon() {
        let mut b = legacy_projection_camera_matrix([320.0, 240.0]);
        b[0] += 5e-6;
        assert!(camera_matrices_approximately_equal(
            &legacy_projection_camera_matrix([320.0, 240.0]),
            &b,
            1e-5
        ));
        assert!(!camera_matrices_approximately_equal(
            &legacy_projection_camera_matrix([320.0, 240.0]),
            &b,
            1e-7
        ));
    }
}
