//! Geometry node compilation and vertex generation.
//!
//! This module handles geometry-related nodes like Rect2DGeometry,
//! providing vertex data generation for GPU buffers.

use anyhow::{Context, Result, bail};

/// Generate interleaved vertices for a 2D rectangle geometry.
///
/// Each vertex is `[x, y, z, u, v]` where `u,v` are in [0,1].
///
/// Creates 6 vertices (2 triangles) for a rectangle centered at origin.
/// The vertices are in counter-clockwise order for front-facing triangles.
///
/// UV convention: **top-left origin** — `(0,0)` at the top-left corner of the rect,
/// `(1,1)` at the bottom-right. This matches wgpu's texture coordinate convention
/// so that pass-to-pass texture sampling chains do not introduce Y-flips.
///
/// User-facing GLSL-like coordinates (`local_px`, `frag_coord_gl`) are derived
/// from the UV in the vertex shader with a Y-flip: `local_px = vec2(uv.x, 1-uv.y) * geo_size`.
///
/// # Arguments
/// * `width` - Width of the rectangle (clamped to minimum 1.0)
/// * `height` - Height of the rectangle (clamped to minimum 1.0)
///
/// # Returns
/// Array of 6 vertices as `[x, y, z, u, v]`.
pub fn rect2d_geometry_vertices(width: f32, height: f32) -> [[f32; 5]; 6] {
    let w = width.max(1.0);
    let h = height.max(1.0);
    let hw = w * 0.5;
    let hh = h * 0.5;
    [
        // Triangle 1: bottom-left, bottom-right, top-right
        // UV uses top-left origin: BL=(0,1), BR=(1,1), TR=(1,0)
        [-hw, -hh, 0.0, 0.0, 1.0],
        [hw, -hh, 0.0, 1.0, 1.0],
        [hw, hh, 0.0, 1.0, 0.0],
        // Triangle 2: bottom-left, top-right, top-left
        [-hw, -hh, 0.0, 0.0, 1.0],
        [hw, hh, 0.0, 1.0, 0.0],
        [-hw, hh, 0.0, 0.0, 0.0],
    ]
}

/// Generate interleaved vertices for a centered unit rectangle (unit quad).
///
/// Each vertex is `[x, y, z, u, v]` where `x,y` are in local unit space and `u,v` in [0,1].
///
/// The quad is centered at origin with corners at (-0.5,-0.5) .. (0.5,0.5).
///
/// UV convention: **top-left origin** — matches `rect2d_geometry_vertices`.
///
/// This is used when Rect2DGeometry size/position are dynamic and applied in the vertex shader.
pub fn rect2d_unit_geometry_vertices() -> [[f32; 5]; 6] {
    let hw = 0.5;
    let hh = 0.5;
    [
        // Triangle 1: bottom-left, bottom-right, top-right
        [-hw, -hh, 0.0, 0.0, 1.0],
        [hw, -hh, 0.0, 1.0, 1.0],
        [hw, hh, 0.0, 1.0, 0.0],
        // Triangle 2: bottom-left, top-right, top-left
        [-hw, -hh, 0.0, 0.0, 1.0],
        [hw, hh, 0.0, 1.0, 0.0],
        [-hw, hh, 0.0, 0.0, 0.0],
    ]
}

// ---------------------------------------------------------------------------
// glTF / GLB / OBJ mesh loading
// ---------------------------------------------------------------------------

/// Load mesh geometry from a glTF or GLB asset.
///
/// Iterates all meshes and primitives, extracting POSITION, TEXCOORD_0, and NORMAL
/// attributes. Primitives are triangulated. Multi-mesh assets are merged into a
/// single flat vertex list.
///
/// Returns `(position_uv_verts, optional_normals)` where each vertex is `[x,y,z,u,v]`
/// and normals (if present on any primitive) are `[nx,ny,nz]` per vertex.
fn load_gltf_geometry(bytes: &[u8]) -> Result<(Vec<[f32; 5]>, Option<Vec<[f32; 3]>>)> {
    let (document, buffers, _images) =
        gltf::import_slice(bytes).context("failed to parse glTF/GLB asset")?;

    let mut verts: Vec<[f32; 5]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut has_any_normals = false;

    // Walk the node tree to collect (mesh_index, world_transform) pairs.
    // This correctly applies node TRS / matrix transforms to vertex positions.
    let mut mesh_instances: Vec<(usize, [[f32; 4]; 4])> = Vec::new();
    for scene in document.scenes() {
        for node in scene.nodes() {
            collect_mesh_nodes(&node, &mat4_identity(), &mut mesh_instances);
        }
    }

    // If no nodes reference meshes (unusual), fall back to raw meshes with identity transform.
    if mesh_instances.is_empty() {
        for mesh in document.meshes() {
            mesh_instances.push((mesh.index(), mat4_identity()));
        }
    }

    let meshes: Vec<gltf::Mesh<'_>> = document.meshes().collect();

    for (mesh_idx, world_mat) in &mesh_instances {
        let Some(mesh) = meshes.get(*mesh_idx) else {
            continue;
        };

        // Decompose the 3x3 upper-left for normal transformation.
        // For normals we need the inverse-transpose of the upper 3x3.
        // For uniform/rigid transforms (no shear), the upper 3x3 itself works
        // after renormalizing.
        let normal_mat = mat3_from_mat4(world_mat);

        for primitive in mesh.primitives() {
            let reader =
                primitive.reader(|buffer| buffers.get(buffer.index()).map(|d| d.0.as_slice()));

            let positions: Vec<[f32; 3]> = reader
                .read_positions()
                .ok_or_else(|| anyhow::anyhow!("glTF primitive missing POSITION attribute"))?
                .collect();

            let tex_coords: Vec<[f32; 2]> = reader
                .read_tex_coords(0)
                .map(|tc| tc.into_f32().collect())
                .unwrap_or_else(|| vec![[0.0, 0.0]; positions.len()]);

            let prim_normals: Option<Vec<[f32; 3]>> = reader.read_normals().map(|n| n.collect());

            if prim_normals.is_some() {
                has_any_normals = true;
            }

            // Read indices (if indexed) or generate sequential indices.
            let indices: Vec<u32> = if let Some(idx_reader) = reader.read_indices() {
                idx_reader.into_u32().collect()
            } else {
                (0..positions.len() as u32).collect()
            };

            // Emit triangulated vertices with world transform applied.
            let base_vert = verts.len();
            for &idx in &indices {
                let i = idx as usize;
                let p = positions.get(i).copied().unwrap_or([0.0, 0.0, 0.0]);
                let wp = mat4_transform_point(world_mat, p);
                let t = tex_coords.get(i).copied().unwrap_or([0.0, 0.0]);
                verts.push([wp[0], wp[1], wp[2], t[0], t[1]]);
            }

            // Emit normals (or zero-fill if this primitive lacks them but others have them).
            if let Some(ref pn) = prim_normals {
                for &idx in &indices {
                    let i = idx as usize;
                    let n = pn.get(i).copied().unwrap_or([0.0, 0.0, 1.0]);
                    let wn = mat3_transform_vec(&normal_mat, n);
                    normals.push(vec3_normalize(wn));
                }
            } else {
                // Placeholder normals — will only be used if has_any_normals is true.
                let count = verts.len() - base_vert;
                normals.extend(std::iter::repeat([0.0, 0.0, 1.0]).take(count));
            }
        }
    }

    if verts.is_empty() {
        bail!("glTF asset contains no mesh geometry");
    }

    let normals_out = if has_any_normals { Some(normals) } else { None };

    Ok((verts, normals_out))
}

// ---------------------------------------------------------------------------
// glTF node-tree helpers (inline math to avoid pulling in a linear-algebra crate)
// ---------------------------------------------------------------------------

fn mat4_identity() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

/// Multiply two 4×4 matrices (row-major storage, column-vector convention).
fn mat4_mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut out = [[0.0f32; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            out[r][c] =
                a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c] + a[r][3] * b[3][c];
        }
    }
    out
}

/// Transform a 3D point by a 4×4 matrix (assumes w=1).
fn mat4_transform_point(m: &[[f32; 4]; 4], p: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * p[0] + m[0][1] * p[1] + m[0][2] * p[2] + m[0][3],
        m[1][0] * p[0] + m[1][1] * p[1] + m[1][2] * p[2] + m[1][3],
        m[2][0] * p[0] + m[2][1] * p[1] + m[2][2] * p[2] + m[2][3],
    ]
}

/// Extract the upper-left 3×3 from a 4×4 matrix.
fn mat3_from_mat4(m: &[[f32; 4]; 4]) -> [[f32; 3]; 3] {
    [
        [m[0][0], m[0][1], m[0][2]],
        [m[1][0], m[1][1], m[1][2]],
        [m[2][0], m[2][1], m[2][2]],
    ]
}

/// Transform a 3D vector by a 3×3 matrix.
fn mat3_transform_vec(m: &[[f32; 3]; 3], v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

fn vec3_normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len < 1e-12 {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

/// Convert glTF `Transform` (column-major as returned by the crate) to our row-major 4×4.
fn gltf_transform_to_mat4(transform: gltf::scene::Transform) -> [[f32; 4]; 4] {
    let cols = transform.matrix();
    // `cols` is [[f32; 4]; 4] in column-major order: cols[col][row].
    // Transpose to row-major.
    [
        [cols[0][0], cols[1][0], cols[2][0], cols[3][0]],
        [cols[0][1], cols[1][1], cols[2][1], cols[3][1]],
        [cols[0][2], cols[1][2], cols[2][2], cols[3][2]],
        [cols[0][3], cols[1][3], cols[2][3], cols[3][3]],
    ]
}

/// Recursively walk the node tree, accumulating world transforms.
fn collect_mesh_nodes(
    node: &gltf::Node<'_>,
    parent_world: &[[f32; 4]; 4],
    out: &mut Vec<(usize, [[f32; 4]; 4])>,
) {
    let local = gltf_transform_to_mat4(node.transform());
    let world = mat4_mul(parent_world, &local);

    if let Some(mesh) = node.mesh() {
        out.push((mesh.index(), world));
    }

    for child in node.children() {
        collect_mesh_nodes(&child, &world, out);
    }
}

/// Load mesh geometry from an OBJ asset.
///
/// Uses `tobj` to parse the OBJ format. All meshes are merged into a single vertex
/// list. Indices are expanded into a flat triangle list.
///
/// Returns `(position_uv_verts, optional_normals)`.
fn load_obj_geometry(bytes: &[u8]) -> Result<(Vec<[f32; 5]>, Option<Vec<[f32; 3]>>)> {
    let mut cursor = std::io::Cursor::new(bytes);
    let (models, _materials) = tobj::load_obj_buf(
        &mut cursor,
        &tobj::LoadOptions {
            triangulate: true,
            single_index: true,
            ..Default::default()
        },
        |_mat_path| {
            // We don't support external MTL files; return empty materials.
            Ok((Vec::new(), Default::default()))
        },
    )
    .context("failed to parse OBJ asset")?;

    let mut verts: Vec<[f32; 5]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut has_any_normals = false;

    for model in &models {
        let mesh = &model.mesh;
        let has_normals = !mesh.normals.is_empty();
        let has_texcoords = !mesh.texcoords.is_empty();

        if has_normals {
            has_any_normals = true;
        }

        for &idx in &mesh.indices {
            let i = idx as usize;

            let px = mesh.positions.get(i * 3).copied().unwrap_or(0.0);
            let py = mesh.positions.get(i * 3 + 1).copied().unwrap_or(0.0);
            let pz = mesh.positions.get(i * 3 + 2).copied().unwrap_or(0.0);

            let u = if has_texcoords {
                mesh.texcoords.get(i * 2).copied().unwrap_or(0.0)
            } else {
                0.0
            };
            let v = if has_texcoords {
                mesh.texcoords.get(i * 2 + 1).copied().unwrap_or(0.0)
            } else {
                0.0
            };

            verts.push([px, py, pz, u, v]);

            if has_normals {
                let nx = mesh.normals.get(i * 3).copied().unwrap_or(0.0);
                let ny = mesh.normals.get(i * 3 + 1).copied().unwrap_or(0.0);
                let nz = mesh.normals.get(i * 3 + 2).copied().unwrap_or(1.0);
                normals.push([nx, ny, nz]);
            } else {
                normals.push([0.0, 0.0, 1.0]);
            }
        }
    }

    if verts.is_empty() {
        bail!("OBJ asset contains no mesh geometry");
    }

    let normals_out = if has_any_normals { Some(normals) } else { None };

    Ok((verts, normals_out))
}

/// Load mesh geometry from asset bytes, dispatching by file extension.
///
/// Supported extensions (case-insensitive): `.gltf`, `.glb`, `.obj`.
///
/// Returns `(position_uv_verts, optional_normals)` where:
/// - `position_uv_verts` is a flat triangle list of `[x, y, z, u, v]` per vertex.
/// - `optional_normals` is `Some(normals)` when any primitive in the asset had normals,
///   where each entry is `[nx, ny, nz]` matching the vertex at the same index.
pub fn load_geometry_from_asset(
    bytes: &[u8],
    path: &str,
) -> Result<(Vec<[f32; 5]>, Option<Vec<[f32; 3]>>)> {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".gltf") || lower.ends_with(".glb") {
        load_gltf_geometry(bytes)
    } else if lower.ends_with(".obj") {
        load_obj_geometry(bytes)
    } else {
        bail!(
            "GLTFGeometry only supports .gltf/.glb/.obj assets, got: {}",
            path
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rect2d_geometry_vertices_basic() {
        let verts = rect2d_geometry_vertices(100.0, 50.0);

        // Check that we get 6 vertices (2 triangles)
        assert_eq!(verts.len(), 6);

        // Check half-width and half-height
        let hw = 50.0;
        let hh = 25.0;

        // First triangle: bottom-left, bottom-right, top-right
        // UV top-left origin: BL=(0,1), BR=(1,1), TR=(1,0)
        assert_eq!(verts[0], [-hw, -hh, 0.0, 0.0, 1.0]);
        assert_eq!(verts[1], [hw, -hh, 0.0, 1.0, 1.0]);
        assert_eq!(verts[2], [hw, hh, 0.0, 1.0, 0.0]);

        // Second triangle: bottom-left, top-right, top-left
        assert_eq!(verts[3], [-hw, -hh, 0.0, 0.0, 1.0]);
        assert_eq!(verts[4], [hw, hh, 0.0, 1.0, 0.0]);
        assert_eq!(verts[5], [-hw, hh, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn test_rect2d_geometry_vertices_minimum_size() {
        // Values less than 1.0 should be clamped to 1.0
        let verts = rect2d_geometry_vertices(0.5, 0.1);

        // Should be clamped to 1.0 x 1.0
        let hw = 0.5;
        let hh = 0.5;

        assert_eq!(verts[0], [-hw, -hh, 0.0, 0.0, 1.0]);
        assert_eq!(verts[1], [hw, -hh, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn test_rect2d_geometry_vertices_square() {
        let verts = rect2d_geometry_vertices(200.0, 200.0);

        let hw = 100.0;
        let hh = 100.0;

        // All corners should be equidistant from center
        assert_eq!(verts[0], [-hw, -hh, 0.0, 0.0, 1.0]);
        assert_eq!(verts[2], [hw, hh, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn test_rect2d_unit_geometry_vertices() {
        let verts = rect2d_unit_geometry_vertices();
        assert_eq!(verts.len(), 6);
        assert_eq!(verts[0], [-0.5, -0.5, 0.0, 0.0, 1.0]);
        assert_eq!(verts[2], [0.5, 0.5, 0.0, 1.0, 0.0]);
        assert_eq!(verts[5], [-0.5, 0.5, 0.0, 0.0, 0.0]);
    }
}
