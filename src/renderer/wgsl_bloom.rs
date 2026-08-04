use crate::renderer::{types::WgslShaderBundle, wgsl::build_fullscreen_textured_bundle};

pub const BLOOM_MAX_MIPS: u32 = 6;

pub(crate) fn bloom_extract_fragment_body(
    threshold: f32,
    smooth_width_px: f32,
    strength: f32,
    saturation: f32,
    tint_expr: &str,
    source_texture: &str,
    source_sampler: &str,
) -> String {
    let threshold = threshold.clamp(0.0, 1.0);
    let smooth_width_px = smooth_width_px.max(0.0);
    let strength = strength.clamp(0.0, 1.0);
    let saturation = saturation.clamp(0.0, 1.0);

    let smooth_width_luma = (smooth_width_px / 255.0).max(1e-6);
    let edge0 = (threshold - smooth_width_luma).clamp(0.0, 1.0);
    let edge1 = (threshold + smooth_width_luma).clamp(0.0, 1.0);

    format!(
        r#"
let src = textureSample({source_texture}, {source_sampler}, in.uv);
let tint = clamp({tint_expr}, vec4f(0.0), vec4f(1.0));
let lum = dot(src.rgb, vec3f(0.2126, 0.7152, 0.0722));
let mask = smoothstep({edge0:.8}, {edge1:.8}, lum);
let extracted = src.rgb * mask * {strength:.8};
let gray = dot(extracted, vec3f(0.2126, 0.7152, 0.0722));
let sat_rgb = mix(vec3f(gray), extracted, {saturation:.8});
let tinted = sat_rgb * tint.rgb;
let alpha = clamp(src.a * mask * {strength:.8} * tint.a, 0.0, 1.0);
return vec4f(tinted, alpha);
"#,
    )
}

pub fn build_bloom_extract_bundle(
    threshold: f32,
    smooth_width_px: f32,
    strength: f32,
    saturation: f32,
    tint: [f32; 4],
) -> WgslShaderBundle {
    let tint = [
        tint[0].clamp(0.0, 1.0),
        tint[1].clamp(0.0, 1.0),
        tint[2].clamp(0.0, 1.0),
        tint[3].clamp(0.0, 1.0),
    ];

    let tint_expr = format!(
        "vec4f({:.8}, {:.8}, {:.8}, {:.8})",
        tint[0], tint[1], tint[2], tint[3]
    );
    let body = bloom_extract_fragment_body(
        threshold,
        smooth_width_px,
        strength,
        saturation,
        &tint_expr,
        "src_tex",
        "src_samp",
    );

    build_fullscreen_textured_bundle(body)
}

pub fn build_bloom_additive_combine_bundle() -> WgslShaderBundle {
    let common = r#"
struct Params {
    target_size: vec2f,
    geo_size: vec2f,
    center: vec2f,

    geo_translate: vec2f,
    geo_scale: vec2f,

    time: f32,
    _pad0: f32,

    color: vec4f,
    camera: mat4x4f,
    camera_position: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) frag_coord_gl: vec2f,
    @location(2) local_px: vec3f,
    @location(3) geo_size_px: vec2f,
};

@group(1) @binding(0)
var base_tex: texture_2d<f32>;
@group(1) @binding(1)
var base_samp: sampler;
@group(1) @binding(2)
var add_tex: texture_2d<f32>;
@group(1) @binding(3)
var add_samp: sampler;
"#
    .to_string();

    let vertex = r#"
@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
    var out: VSOut;
    out.uv = vec2f(uv.x, 1.0 - uv.y);
    out.geo_size_px = params.geo_size;
    out.local_px = vec3f(uv * out.geo_size_px, position.z);

    let p_px = params.center + position.xy;
    out.position = params.camera * vec4f(p_px, position.z, 1.0);
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}
"#
    .to_string();

    let fragment = r#"
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    let base = textureSample(base_tex, base_samp, in.uv);
    let add = textureSample(add_tex, add_samp, in.uv);
    let base_strength = params.color.x;
    let add_strength = params.color.y;
    // RGB is additive (HDR glow), alpha is coverage clamped to [0,1].
    return vec4f(
        base.rgb * base_strength + add.rgb * add_strength,
        clamp(base.a * base_strength + add.a * add_strength, 0.0, 1.0),
    );
}
"#
    .to_string();

    let vertex_src = format!("{common}{vertex}");
    let fragment_src = format!("{common}{fragment}");
    let module = format!("{common}{vertex}{fragment}");

    WgslShaderBundle {
        common,
        vertex: vertex_src,
        fragment: fragment_src,
        compute: None,
        module,
        image_textures: Vec::new(),
        pass_textures: Vec::new(),
        graph_schema: None,
        graph_binding_kind: None,
        shader_parameter_schema: None,
    }
}

pub fn build_bloom_output_bundle() -> WgslShaderBundle {
    build_fullscreen_textured_bundle(
        r#"
let src = textureSample(src_tex, src_samp, in.uv);
let strength = params.color.x;
return vec4f(src.rgb * strength, clamp(src.a * strength, 0.0, 1.0));
"#
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::{build_bloom_additive_combine_bundle, build_bloom_output_bundle};

    #[test]
    fn additive_combine_uses_per_input_strengths() {
        let bundle = build_bloom_additive_combine_bundle();
        assert!(
            bundle
                .fragment
                .contains("let base_strength = params.color.x;")
        );
        assert!(
            bundle
                .fragment
                .contains("let add_strength = params.color.y;")
        );
        assert!(
            bundle
                .fragment
                .contains("base.rgb * base_strength + add.rgb * add_strength")
        );
    }

    #[test]
    fn output_copy_applies_the_remaining_level_strength() {
        let bundle = build_bloom_output_bundle();
        assert!(bundle.fragment.contains("let strength = params.color.x;"));
        assert!(bundle.fragment.contains("src.rgb * strength"));
    }
}
