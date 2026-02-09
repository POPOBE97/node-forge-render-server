//! Geometry node compilation and vertex generation.
//!
//! This module handles geometry-related nodes like Rect2DGeometry,
//! providing vertex data generation for GPU buffers.

/// Generate interleaved vertices for a 2D rectangle geometry.
///
/// Each vertex is `[x, y, z, u, v]` where `u,v` are in [0,1].
///
/// Creates 6 vertices (2 triangles) for a rectangle centered at origin.
/// The vertices are in counter-clockwise order for front-facing triangles.
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
        [-hw, -hh, 0.0, 0.0, 0.0],
        [hw, -hh, 0.0, 1.0, 0.0],
        [hw, hh, 0.0, 1.0, 1.0],
        // Triangle 2: bottom-left, top-right, top-left
        [-hw, -hh, 0.0, 0.0, 0.0],
        [hw, hh, 0.0, 1.0, 1.0],
        [-hw, hh, 0.0, 0.0, 1.0],
    ]
}

/// Generate interleaved vertices for a centered unit rectangle (unit quad).
///
/// Each vertex is `[x, y, z, u, v]` where `x,y` are in local unit space and `u,v` in [0,1].
///
/// The quad is centered at origin with corners at (-0.5,-0.5) .. (0.5,0.5).
///
/// This is used when Rect2DGeometry size/position are dynamic and applied in the vertex shader.
pub fn rect2d_unit_geometry_vertices() -> [[f32; 5]; 6] {
    let hw = 0.5;
    let hh = 0.5;
    [
        // Triangle 1: bottom-left, bottom-right, top-right
        [-hw, -hh, 0.0, 0.0, 0.0],
        [hw, -hh, 0.0, 1.0, 0.0],
        [hw, hh, 0.0, 1.0, 1.0],
        // Triangle 2: bottom-left, top-right, top-left
        [-hw, -hh, 0.0, 0.0, 0.0],
        [hw, hh, 0.0, 1.0, 1.0],
        [-hw, hh, 0.0, 0.0, 1.0],
    ]
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
        assert_eq!(verts[0], [-hw, -hh, 0.0, 0.0, 0.0]);
        assert_eq!(verts[1], [hw, -hh, 0.0, 1.0, 0.0]);
        assert_eq!(verts[2], [hw, hh, 0.0, 1.0, 1.0]);

        // Second triangle: bottom-left, top-right, top-left
        assert_eq!(verts[3], [-hw, -hh, 0.0, 0.0, 0.0]);
        assert_eq!(verts[4], [hw, hh, 0.0, 1.0, 1.0]);
        assert_eq!(verts[5], [-hw, hh, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_rect2d_geometry_vertices_minimum_size() {
        // Values less than 1.0 should be clamped to 1.0
        let verts = rect2d_geometry_vertices(0.5, 0.1);

        // Should be clamped to 1.0 x 1.0
        let hw = 0.5;
        let hh = 0.5;

        assert_eq!(verts[0], [-hw, -hh, 0.0, 0.0, 0.0]);
        assert_eq!(verts[1], [hw, -hh, 0.0, 1.0, 0.0]);
    }

    #[test]
    fn test_rect2d_geometry_vertices_square() {
        let verts = rect2d_geometry_vertices(200.0, 200.0);

        let hw = 100.0;
        let hh = 100.0;

        // All corners should be equidistant from center
        assert_eq!(verts[0], [-hw, -hh, 0.0, 0.0, 0.0]);
        assert_eq!(verts[2], [hw, hh, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn test_rect2d_unit_geometry_vertices() {
        let verts = rect2d_unit_geometry_vertices();
        assert_eq!(verts.len(), 6);
        assert_eq!(verts[0], [-0.5, -0.5, 0.0, 0.0, 0.0]);
        assert_eq!(verts[2], [0.5, 0.5, 0.0, 1.0, 1.0]);
        assert_eq!(verts[5], [-0.5, 0.5, 0.0, 0.0, 1.0]);
    }
}
