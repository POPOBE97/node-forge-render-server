// LuminanceCurve (LAB) — helper template
//
// Embedded at compile time via template_loader. No markers — this file is the
// raw WGSL helper body that gets registered in `extra_wgsl_decls`.
//
// Algorithm:
//   - Convert premultiplied RGB to linear RGB (divide by alpha).
//   - Convert linear RGB to OKLab via the Björn Ottosson matrices.
//   - Apply a cubic Bézier curve (defined by `factors.xyzw` at x = 0, 1/3, 2/3, 1)
//     to the L channel only, then mix between original and curved L by `mix_factor`.
//   - Convert back to linear RGB and re-premultiply by alpha.

fn lc_luminance_curve_lab(color: vec4f, factors: vec4f, mix_factor: f32) -> vec4f {
    let factor_adjust = vec4f(
        -1.0 * factors.x + 3.0 * factors.y - 3.0 * factors.z + 1.0 * factors.w,
        3.0 * factors.x - 6.0 * factors.y + 3.0 * factors.z,
        -3.0 * factors.x + 3.0 * factors.y,
        factors.x,
    );

    if (color.a <= 0.0001) {
        return color;
    }

    let rgb = color.rgb / color.a;
    let lms = vec3f(
        0.4122214708 * rgb.r + 0.5363325363 * rgb.g + 0.0514459929 * rgb.b,
        0.2119034982 * rgb.r + 0.6806995451 * rgb.g + 0.1073969566 * rgb.b,
        0.0883024619 * rgb.r + 0.2817188376 * rgb.g + 0.6299787005 * rgb.b,
    );
    let lms_cbrt = sign(lms) * pow(abs(lms), vec3f(1.0 / 3.0));
    let lab = vec3f(
        0.2104542553 * lms_cbrt.x + 0.7936177850 * lms_cbrt.y - 0.0040720468 * lms_cbrt.z,
        1.9779984951 * lms_cbrt.x - 2.4285922050 * lms_cbrt.y + 0.4505937099 * lms_cbrt.z,
        0.0259040371 * lms_cbrt.x + 0.7827717662 * lms_cbrt.y - 0.8086757660 * lms_cbrt.z,
    );

    let curve_input = clamp(lab.x, 0.0, 1.0);
    var target_l = curve_input * factor_adjust.x + factor_adjust.y;
    target_l = target_l * curve_input + factor_adjust.z;
    target_l = target_l * curve_input + factor_adjust.w;

    let mapped_l = mix(lab.x, target_l, mix_factor);
    let mapped_lms_cbrt = vec3f(
        mapped_l + 0.3963377774 * lab.y + 0.2158037573 * lab.z,
        mapped_l - 0.1055613458 * lab.y - 0.0638541728 * lab.z,
        mapped_l - 0.0894841775 * lab.y - 1.2914855480 * lab.z,
    );
    let mapped_lms = mapped_lms_cbrt * mapped_lms_cbrt * mapped_lms_cbrt;
    var mapped_rgb = vec3f(
        4.0767416621 * mapped_lms.x - 3.3077115913 * mapped_lms.y + 0.2309699292 * mapped_lms.z,
        -1.2684380046 * mapped_lms.x + 2.6097574011 * mapped_lms.y - 0.3413193965 * mapped_lms.z,
        -0.0041960863 * mapped_lms.x - 0.7034186147 * mapped_lms.y + 1.7076147010 * mapped_lms.z,
    );
    mapped_rgb = max(vec3f(0.0), mapped_rgb);

    return vec4f(mapped_rgb * color.a, color.a);
}
