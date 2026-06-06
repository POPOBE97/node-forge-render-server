
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

    geo_translate: vec2f,
    geo_scale: vec2f,

    // Pack to 16-byte boundary.
    time: f32,
    _pad0: f32,

    // 16-byte aligned.
    color: vec4f,
    camera: mat4x4f,
    camera_position: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

 struct VSOut {
     @builtin(position) position: vec4f,
     @location(0) uv: vec2f,
     // GLSL-like gl_FragCoord.xy: bottom-left origin, pixel-centered.
     @location(1) frag_coord_gl: vec2f,
     // Geometry-local pixel coordinate (GeoFragcoord): origin at bottom-left.
     @location(2) local_px: vec3f,
     // Geometry size in pixels after applying geometry/instance transforms.
     @location(3) geo_size_px: vec2f,
  };


@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_GroupInstance_26_ImageTexture_2: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_GroupInstance_26_ImageTexture_2: sampler;


// --- Extra WGSL declarations (generated) ---

fn aspect_correct_uv_fit(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    // r = image_aspect / geo_aspect; r > 1 means image is relatively wider than geometry.
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(max(1.0 / r, 1.0), max(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}
fn aspect_correct_uv_fill(uv: vec2f, img_dim: vec2f, geo_dim: vec2f) -> vec2f {
    let r = (img_dim.x * geo_dim.y) / (img_dim.y * geo_dim.x);
    let s = vec2f(min(1.0 / r, 1.0), min(r, 1.0));
    return (uv - vec2f(0.5)) * s + vec2f(0.5);
}


// ---- LuminanceCurve RGB helper (generated) ----
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
    let chroma_scale = max(target_luminance / max(luminance, 1e-6), 0.0);
    let remapped_rgb = vec3f(target_luminance) + chroma * chroma_scale;
    let mixed = max(vec3f(0.0), mix(rgb, remapped_rgb, mix_factor));

    return vec4f(mixed * color.a, color.a);
}

fn mc_math_closure(uv: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    output = vec4<f32>(0f, 0.33333334f, 0.6666667f, 1f);
    let _e14: vec4<f32> = output;
    return _e14;
}


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
 ) -> VSOut {
 var out: VSOut;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = uv;

 out.geo_size_px = params.geo_size;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, 0.0);

 var p_local = position;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = params.center + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // ImageTexture GroupInstance_26/ImageTexture_2 aspect-correct uv
    let image_texture_uv = aspect_correct_uv_fill(
        (in.uv),
        vec2f(textureDimensions(img_tex_GroupInstance_26_ImageTexture_2)),
        in.geo_size_px,
    );
    // ImageTexture GroupInstance_26/ImageTexture_2.color
    let image_texture_sample = textureSample(
        img_tex_GroupInstance_26_ImageTexture_2,
        img_samp_GroupInstance_26_ImageTexture_2,
        image_texture_uv,
    );
    var math_closure_out: vec4f;
    {
        var output: vec4f;
        output = mc_math_closure(in.uv);
        math_closure_out = output;
    }
    // Final composite
    let _frag_out = lc_luminance_curve_rgb(image_texture_sample, math_closure_out, 1.0);
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
