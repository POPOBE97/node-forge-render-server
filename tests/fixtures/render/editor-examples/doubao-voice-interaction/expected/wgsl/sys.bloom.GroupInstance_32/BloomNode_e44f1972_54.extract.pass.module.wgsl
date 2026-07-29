
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
var pass_tex_GroupInstance_32_RenderPass_e44f1972_41: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_GroupInstance_32_RenderPass_e44f1972_41: sampler;

@group(1) @binding(2)
var pass_tex_GroupInstance_32_PassTexture_IntelligentLight: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_GroupInstance_32_PassTexture_IntelligentLight: sampler;


@vertex
fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
    var out: VSOut;
    out.uv = uv;
    out.geo_size_px = params.geo_size;
    out.local_px = vec3f(vec2f(uv.x, 1.0 - uv.y) * out.geo_size_px, position.z);

    let p_px = params.center + position.xy;
    out.position = params.camera * vec4f(p_px, position.z, 1.0);
    out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Pass Texture GroupInstance_32/PassTexture_IntelligentLight.color
    let pass_texture = textureSample(
        pass_tex_GroupInstance_32_PassTexture_IntelligentLight,
        pass_samp_GroupInstance_32_PassTexture_IntelligentLight,
        in.uv,
    );

let src = textureSample(pass_tex_GroupInstance_32_RenderPass_e44f1972_41, pass_samp_GroupInstance_32_RenderPass_e44f1972_41, in.uv);
let tint = clamp(pass_texture, vec4f(0.0), vec4f(1.0));
let lum = dot(src.rgb, vec3f(0.2126, 0.7152, 0.0722));
let mask = smoothstep(0.42156863, 0.57843137, lum);
let extracted = src.rgb * mask * 1.00000000;
let gray = dot(extracted, vec3f(0.2126, 0.7152, 0.0722));
let sat_rgb = mix(vec3f(gray), extracted, 1.00000000);
let tinted = sat_rgb * tint.rgb;
let alpha = clamp(src.a * mask * 1.00000000 * tint.a, 0.0, 1.0);
return vec4f(tinted, alpha);

}
