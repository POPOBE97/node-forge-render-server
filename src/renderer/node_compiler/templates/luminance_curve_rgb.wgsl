// LuminanceCurve (RGB) — helper template
//
// Embedded at compile time via template_loader. No markers — this file is the
// raw WGSL helper body that gets registered in `extra_wgsl_decls`.
//
// Algorithm:
//   - Un-premultiply input color by alpha to get linear RGB.
//   - Compute Rec.709 luminance from RGB.
//   - Apply a cubic Bézier curve (defined by `factors.xyzw` at x = 0, 1/3, 2/3, 1)
//     to luminance only, then re-add the original chroma residual scaled by
//     the luminance change.
//   - Mix between remapped and original RGB by `mix_factor`.
//   - Re-premultiply by alpha and return.

fn lc_luminance_curve_rgb(color: vec4f, factors: vec4f, mix_factor: f32) -> vec4f {
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
    let luminance = clamp(dot(rgb, vec3f(0.2125, 0.7153, 0.0721)), 0.0, 1.0);
    var target_luminance = luminance * factor_adjust.x + factor_adjust.y;
    target_luminance = target_luminance * luminance + factor_adjust.z;
    target_luminance = target_luminance * luminance + factor_adjust.w;

    let chroma = rgb - vec3f(luminance);
    let chroma_scale = clamp(target_luminance / max(luminance, 1e-6), 0.0, 1.0);
    let remapped_rgb = vec3f(target_luminance) + chroma * chroma_scale;
    let mixed = max(vec3f(0.0), mix(rgb, remapped_rgb, mix_factor));

    return vec4f(mixed * color.a, color.a);
}
