
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


struct GraphInputs {
    // Node: IslandRadiusPx
    node_IslandRadiusPx_1aff0e03: vec4f,
    // Node: IslandSizePx
    node_IslandSizePx_b573a1f9: vec4f,
    // Node: OuterContentScale
    node_OuterContentScale_1527e730: vec4f,
    // Node: Vector2Input_8e46d6bc_18
    node_Vector2Input_8e46d6bc_18_134a73ed: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_GuassianBlurPass_c0928d59_36: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_GuassianBlurPass_c0928d59_36: sampler;


// --- Extra WGSL declarations (generated) ---

struct ShaderMaterialInput {
    // Public material UV: bottom-left origin, Y increasing upward.
    uv: vec2f,
    // Public scene/target pixel coordinate: bottom-left origin, Y increasing upward.
    frag_coord: vec2f,
    // Public geometry-local pixel coordinate: bottom-left origin, Y increasing upward.
    local_position: vec3f,
    geometry_size: vec2f,
    target_size: vec2f,
    time: f32,
};

// Renderer-owned texture boundary. ShaderMaterial authors provide LocalUV and never
// convert to WebGPU's private raster-texture convention themselves.
fn sample_texture_local_uv(
    source: texture_2d<f32>,
    source_sampler: sampler,
    local_uv: vec2f,
) -> vec4f {
    return textureSample(source, source_sampler, vec2f(local_uv.x, 1.0 - local_uv.y));
}

fn shader_material_ShaderMaterial_faaaafde_35(
    in: ShaderMaterialInput,
    texture: texture_2d<f32>,
    texture_sampler: sampler,
    content_scale: f32,
) -> vec4f {
    let safe_geometry_size = max(in.geometry_size, vec2f(1.0));
    let source_size = max(vec2f(textureDimensions(texture)), vec2f(1.0));
    let safe_content_scale = max(content_scale, 0.001);
    let geometry_center = safe_geometry_size * 0.5;
    let source_center = source_size * 0.5;
    let source_px = source_center
        + (in.local_position.xy - geometry_center) / safe_content_scale;
    let source_uv = source_px / source_size;

    var content = vec4f(0.0);
    if all(source_uv >= vec2f(0.0)) && all(source_uv <= vec2f(1.0)) {
        content = sample_texture_local_uv(texture, texture_sampler, source_uv);
    }

    // Scale the complete outer-composite result around the island center. The
    // opaque black shell and rounded geometry remain at their authored size.
    return vec4f(content.rgb, 1.0);
}


// ---- 2D SDF helpers (generated) ----
// 2D SDF helper template.
//
// This file is the editable WGSL source for Sdf2D shape helper functions.
// The Rust compiler wires node inputs into calls to these helpers.

fn sdf2d_round_rect(p: vec2f, b: vec2f, rad4: vec4f) -> f32 {
    var r: f32 = rad4.x;
    if (p.x > 0.0 && p.y > 0.0) {
        r = rad4.y;
    } else if (p.x > 0.0 && p.y < 0.0) {
        r = rad4.z;
    } else if (p.x < 0.0 && p.y < 0.0) {
        r = rad4.w;
    }

    let q = abs(p) - b + vec2f(r, r);
    let outside = length(max(q, vec2f(0.0, 0.0)));
    let inside = min(max(q.x, q.y), 0.0);
    return outside + inside - r;
}

fn sdf2d_smooth_round_rect(point: vec2f, center: vec2f, radius: f32, axis_mix: vec2f) -> vec3f {
    let abs_radius = abs(radius);
    let scaled_radius = 1.5286649465560913 * abs_radius;
    let safe_scaled_radius = max(scaled_radius, 1e-6);
    let blended_radius = mix(scaled_radius, radius, max(axis_mix.x, axis_mix.y));

    let offset = point - center;
    let shifted_pos = vec2f(safe_scaled_radius, safe_scaled_radius) + offset;
    let normalized_pos = max(vec2f(0.0), shifted_pos / safe_scaled_radius);
    let abs_norm_pos = abs(normalized_pos);

    let axis_denom = max(abs_norm_pos.x, abs_norm_pos.y);
    let axis_ratio = select(
        clamp(min(abs_norm_pos.x, abs_norm_pos.y) / axis_denom, 0.0, 1.0),
        0.0,
        axis_denom == 0.0,
    );

    let poly_fit_0 = -0.7391197269 * axis_ratio + 2.4034927648;
    let poly_fit_1 = poly_fit_0 * axis_ratio - 2.4907319173;
    let poly_fit_2 = poly_fit_1 * axis_ratio + 0.4768708960;
    let poly_fit = poly_fit_2 * axis_ratio + 0.4747847594;
    let len_abs = length(abs_norm_pos);
    let denom = 1.0 - axis_ratio * axis_ratio * clamp(len_abs, 0.0, 1.0) * poly_fit;
    let safe_denom = select(denom, 1e-6, abs(denom) < 1e-6);
    let dist_base = (len_abs + 1.0) - 1.0 / safe_denom;
    let dist_alt_pos = max(
        vec2f(0.0),
        1.5286649465560913 * abs_norm_pos - vec2f(0.5286650061607361),
    );
    let dist_alt = 0.6541655659675598 * length(dist_alt_pos) + 0.3458344340324402;

    let dist_mix_x = mix(dist_base, dist_alt, axis_mix.x);
    let dist_mix_y = mix(dist_base, dist_alt, axis_mix.y);
    let axis_sign = select(-1.0, 1.0, abs_norm_pos.y > abs_norm_pos.x);
    let final_mix = mix(dist_mix_x, dist_mix_y, clamp(0.5 - axis_sign + axis_sign * axis_ratio, 0.0, 1.0));

    let radial_pos = vec2f(blended_radius, blended_radius) + offset;
    let dir_norm = normalize(max(vec2f(0.0), radial_pos));
    let fallback_axis = select(vec2f(0.0, 1.0), vec2f(1.0, 0.0), radial_pos.x > radial_pos.y);
    let fallback_dir = select(fallback_axis, dir_norm, dir_norm.x + dir_norm.y > 0.0);
    let final_height = min(max(radial_pos.x, radial_pos.y), 0.0) + safe_scaled_radius * (final_mix - 1.0);

    return vec3f(final_height, fallback_dir);
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
 out.uv = vec2f(uv.x, 1.0 - uv.y);

 let rect_size_px_base = (graph_inputs.node_IslandSizePx_b573a1f9).xy;
 let rect_center_px = (graph_inputs.node_Vector2Input_8e46d6bc_18_134a73ed).xy;
 let rect_dyn = vec4f(rect_center_px, rect_size_px_base);
 out.geo_size_px = rect_dyn.zw;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(uv * out.geo_size_px, 0.0);

 let p_rect_local_px = vec3f(position.xy * rect_dyn.zw, position.z);
 var p_local = p_rect_local_px;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = rect_dyn.xy + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Shader Material ShaderMaterial_faaaafde_35.material
    let shader_material_material = shader_material_ShaderMaterial_faaaafde_35(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        pass_tex_GuassianBlurPass_c0928d59_36,
        pass_samp_GuassianBlurPass_c0928d59_36,
        (graph_inputs.node_OuterContentScale_1527e730).x,
    );
    // Final composite
    let _frag_out = shader_material_material;
    let _frag_color = vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
    // Rect2DGeometry smooth corner coverage in transformed local pixel space.
    let _rect_half_size_px = in.geo_size_px * 0.5;
    let _rect_point_px = abs(in.local_px.xy - _rect_half_size_px);
    let _rect_radius_limit_px = max(min(_rect_half_size_px.x, _rect_half_size_px.y), 0.0);
    let _rect_radius_px = clamp((graph_inputs.node_IslandRadiusPx_1aff0e03).x, 0.0, _rect_radius_limit_px);
    let _rect_smooth = clamp(0.0, 0.0, 1.0);
    let _rect_distance = sdf2d_smooth_round_rect(
        _rect_point_px,
        _rect_half_size_px,
        _rect_radius_px,
        vec2f(_rect_smooth),
    ).x;
    let _rect_pixel_distance = max(fwidth(_rect_distance), 1e-4);
    let _rect_aa_width = 2.0 * _rect_pixel_distance;
    let _rect_mask_coverage = select(
        smoothstep(0.0, _rect_aa_width, -_rect_distance),
        1.0,
        _rect_radius_px <= 0.0,
    );
    let _frag_coverage = clamp(_rect_mask_coverage, 0.0, 1.0);
    if (_frag_coverage <= 0.0) {
        discard;
    }
    return _frag_color * _frag_coverage;
}
