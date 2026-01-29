use rust_wgpu_fiber::eframe::egui::Color32;

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn linear_to_srgb_channel(x: f32) -> f32 {
    // https://en.wikipedia.org/wiki/SRGB
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

fn xyz_to_linear_srgb([x, y, z]: [f32; 3]) -> [f32; 3] {
    // D65 standard illuminant
    // https://en.wikipedia.org/wiki/SRGB#From_CIE_XYZ_to_sRGB
    let r = 3.2406 * x + -1.5372 * y + -0.4986 * z;
    let g = -0.9689 * x + 1.8758 * y + 0.0415 * z;
    let b = 0.0557 * x + -0.2040 * y + 1.0570 * z;
    [r, g, b]
}

fn lab_to_xyz_d65([l, a, b]: [f32; 3]) -> [f32; 3] {
    // CIE L*a*b* -> XYZ, with D65 white point.
    // https://en.wikipedia.org/wiki/CIELAB_color_space
    // https://en.wikipedia.org/wiki/Standard_illuminant#White_points_of_standard_illuminants
    let fy = (l + 16.0) / 116.0;
    let fx = fy + a / 500.0;
    let fz = fy - b / 200.0;

    let epsilon = 216.0 / 24_389.0; // (6/29)^3
    let kappa = 24_389.0 / 27.0; // (29/3)^3

    let fx3 = fx * fx * fx;
    let fz3 = fz * fz * fz;

    let xr = if fx3 > epsilon {
        fx3
    } else {
        (116.0 * fx - 16.0) / kappa
    };
    let yr = if l > kappa * epsilon {
        fy * fy * fy
    } else {
        l / kappa
    };
    let zr = if fz3 > epsilon {
        fz3
    } else {
        (116.0 * fz - 16.0) / kappa
    };

    // D65 reference white
    let xn = 0.950_47;
    let yn = 1.0;
    let zn = 1.088_83;

    [xr * xn, yr * yn, zr * zn]
}

/// Create an egui Color32 from CIE L*a*b* (D65) coordinates.
///
/// Example:
/// ```
/// use node_forge_render_server::color::lab;
/// let c = lab(7.78201, -0.0000149, 0.0);
/// ```
pub fn lab(l: f32, a: f32, b: f32) -> Color32 {
    let xyz = lab_to_xyz_d65([l, a, b]);
    let rgb_linear = xyz_to_linear_srgb(xyz);
    let r = clamp01(linear_to_srgb_channel(rgb_linear[0]));
    let g = clamp01(linear_to_srgb_channel(rgb_linear[1]));
    let b = clamp01(linear_to_srgb_channel(rgb_linear[2]));

    Color32::from_rgb(
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lab_black_is_blackish() {
        let c = lab(0.0, 0.0, 0.0);
        // For Lab(0,0,0) we expect near black.
        let [r, g, b, _a] = c.to_array();
        assert!(r <= 1 && g <= 1 && b <= 1, "got rgb=({r},{g},{b})");
    }

    #[test]
    fn lab_white_is_whiteish() {
        let c = lab(100.0, 0.0, 0.0);
        let [r, g, b, _a] = c.to_array();
        assert!(r >= 250 && g >= 250 && b >= 250, "got rgb=({r},{g},{b})");
    }
}
