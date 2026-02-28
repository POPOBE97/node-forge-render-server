
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
    // Node: ColorInput_48
    node_ColorInput_48_cd78eb69: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_ImageTexture_46: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_ImageTexture_46: sampler;

@group(1) @binding(2)
var img_tex_ImageTexture_50: texture_2d<f32>;

@group(1) @binding(3)
var img_samp_ImageTexture_50: sampler;

@group(1) @binding(4)
var pass_tex_RenderPass_4: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp_RenderPass_4: sampler;

@group(1) @binding(6)
var pass_tex_Upsample_41: texture_2d<f32>;

@group(1) @binding(7)
var pass_samp_Upsample_41: sampler;


// --- Extra WGSL declarations (generated) ---

// ---- ColorMix (Blend Color) helpers (generated) ----

fn blendColorBurnComponent(src: vec2f, dst: vec2f) -> f32 {
    let t = select(0.0, dst.y, dst.y == dst.x);
    let d = select(
        t,
        dst.y - min(dst.y, (dst.y - dst.x) * src.y / (src.x + 0.001)),
        abs(src.x) > 0.0,
    );
    return (d * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn blendColorDodgeComponent(src: vec2f, dst: vec2f) -> f32 {
    let dxScale = select(1.0, 0.0, dst.x == 0.0);
    let delta = dxScale * min(
        dst.y,
        select(dst.y, (dst.x * src.y) / ((src.y - src.x) + 0.001), abs(src.y - src.x) > 0.0),
    );
    return (delta * src.y + src.x * (1.0 - dst.y)) + dst.x * (1.0 - src.y);
}

fn blendOverlayComponent(src: vec2f, dst: vec2f) -> f32 {
    return select(
        src.y * dst.y - (2.0 * (dst.y - dst.x)) * (src.y - src.x),
        (2.0 * src.x) * dst.x,
        2.0 * dst.x <= dst.y,
    );
}

fn blendSoftLightComponent(src: vec2f, dst: vec2f) -> f32 {
    let EPSILON = 0.0;

    if (2.0 * src.x <= src.y) {
        return (((dst.x * dst.x) * (src.y - 2.0 * src.x)) / (dst.y + EPSILON) +
            (1.0 - dst.y) * src.x) +
            dst.x * ((-src.y + 2.0 * src.x) + 1.0);
    } else if (4.0 * dst.x <= dst.y) {
        let dSqd = dst.x * dst.x;
        let dCub = dSqd * dst.x;
        let daSqd = dst.y * dst.y;
        let daCub = daSqd * dst.y;

        return (((daSqd * (src.x - dst.x * ((3.0 * src.y - 6.0 * src.x) - 1.0)) +
            ((12.0 * dst.y) * dSqd) * (src.y - 2.0 * src.x)) -
            (16.0 * dCub) * (src.y - 2.0 * src.x)) -
            daCub * src.x) / (daSqd + EPSILON);
    } else {
        return ((dst.x * ((src.y - 2.0 * src.x) + 1.0) + src.x) -
            sqrt(dst.y * dst.x) * (src.y - 2.0 * src.x)) -
            dst.y * src.x;
    }
}

fn blendColorSaturation(color: vec3f) -> f32 {
    return max(max(color.x, color.y), color.z) - min(min(color.x, color.y), color.z);
}

fn blendHSLColor(flipSat: vec2f, src: vec4f, dst: vec4f) -> vec4f {
    let EPSILON = 0.0;
    let MIN_NORMAL_HALF = 6.10351562e-05;

    let alpha = dst.a * src.a;
    let sda = src.rgb * dst.a;
    let dsa = dst.rgb * src.a;

    let flip_x = flipSat.x != 0.0;
    let flip_y = flipSat.y != 0.0;

    var l = select(sda, dsa, flip_x);
    var r = select(dsa, sda, flip_x);

    if (flip_y) {
        let mn = min(min(l.x, l.y), l.z);
        let mx = max(max(l.x, l.y), l.z);
        l = select(vec3f(0.0), ((l - mn) * blendColorSaturation(r)) / (mx - mn), mx > mn);
        r = dsa;
    }

    let lum = dot(vec3f(0.3, 0.59, 0.11), r);
    var result = (lum - dot(vec3f(0.3, 0.59, 0.11), l)) + l;

    let minComp = min(min(result.x, result.y), result.z);
    let maxComp = max(max(result.x, result.y), result.z);

    if (minComp < 0.0 && lum != minComp) {
        result = lum + (result - lum) * (lum / ((lum - minComp + MIN_NORMAL_HALF) + EPSILON));
    }
    if (maxComp > alpha && maxComp != lum) {
        result = lum + ((result - lum) * (alpha - lum)) / ((maxComp - lum + MIN_NORMAL_HALF) + EPSILON);
    }

    return vec4f(
        ((result + dst.rgb) - dsa + src.rgb) - sda,
        src.a + dst.a - alpha,
    );
}

fn blendNormal(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb * (1.0 - src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendDarken(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - max(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendMultiply(src: vec4f, dst: vec4f) -> vec4f {
    return src * (1.0 - dst.a) + dst * (1.0 - src.a) + src * dst;
}

fn blendPlusDarker(src: vec4f, dst: vec4f) -> vec4f {
    let a = src.a + (1.0 - src.a) * dst.a;
    let color = max(vec3f(0.0), a - (dst.a - dst.rgb) - (src.a - src.rgb));
    return vec4f(color, a);
}

fn blendColorBurn(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        blendColorBurnComponent(src.ra, dst.ra),
        blendColorBurnComponent(src.ga, dst.ga),
        blendColorBurnComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendLighten(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendScreen(src: vec4f, dst: vec4f) -> vec4f {
    return vec4f(1.0 - (1.0 - src.rgb) * (1.0 - dst.rgb), src.a + dst.a * (1.0 - src.a));
}

fn blendPlusLighter(src: vec4f, dst: vec4f) -> vec4f {
    let color = min(src.rgb + dst.rgb, vec3f(1.0));
    let alpha = src.a + (1.0 - src.a) * dst.a;
    return vec4f(color, alpha);
}

fn blendColorDodge(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        blendColorDodgeComponent(src.ra, dst.ra),
        blendColorDodgeComponent(src.ga, dst.ga),
        blendColorDodgeComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendOverlay(src: vec4f, dst: vec4f) -> vec4f {
    var c = vec3f(
        blendOverlayComponent(src.ra, dst.ra),
        blendOverlayComponent(src.ga, dst.ga),
        blendOverlayComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    c += dst.rgb * (1.0 - src.a) + src.rgb * (1.0 - dst.a);
    return vec4f(c, a);
}

fn blendSoftLight(src: vec4f, dst: vec4f) -> vec4f {
    let c = vec3f(
        blendSoftLightComponent(src.ra, dst.ra),
        blendSoftLightComponent(src.ga, dst.ga),
        blendSoftLightComponent(src.ba, dst.ba),
    );
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendHardLight(src: vec4f, dst: vec4f) -> vec4f {
    return blendOverlay(dst, src);
}

fn blendDifference(src: vec4f, dst: vec4f) -> vec4f {
    let c = src.rgb + dst.rgb - 2.0 * min(src.rgb * dst.a, dst.rgb * src.a);
    let a = src.a + dst.a * (1.0 - src.a);
    return vec4f(c, a);
}

fn blendExclusion(src: vec4f, dst: vec4f) -> vec4f {
    let c = (dst.rgb + src.rgb) - (2.0 * dst.rgb * src.rgb);
    let a = src.a + (1.0 - src.a) * dst.a;
    return vec4f(c, a);
}

fn blendHue(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(0.0, 1.0), src, dst);
}

fn blendSaturation(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(1.0), src, dst);
}

fn blendColor(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(0.0), src, dst);
}

fn blendLuminance(src: vec4f, dst: vec4f) -> vec4f {
    return blendHSLColor(vec2f(1.0, 0.0), src, dst);
}

fn mc_MathClosure_42_(uv: vec2<f32>, intelli: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var intelli_1: vec4<f32>;
    var output: vec4<f32> = vec4(0f);
    var c_l0_: vec4<f32>;
    var c_l1_: vec4<f32>;
    var bloom: vec4<f32>;

    uv_1 = uv;
    intelli_1 = intelli;
    let _e8: vec2<f32> = uv_1;
    let _e9: vec4<f32> = sample_pass_RenderPass_4_(_e8);
    c_l0_ = _e9;
    let _e12: vec2<f32> = uv_1;
    let _e13: vec4<f32> = sample_pass_Upsample_41_(_e12);
    c_l1_ = _e13;
    let _e15: vec4<f32> = c_l0_;
    let _e16: vec4<f32> = c_l1_;
    bloom = (_e15 + _e16);
    let _e19: vec4<f32> = bloom;
    let _e20: vec4<f32> = intelli_1;
    output = ((_e19 * _e20) * 1.5f);
    let _e24: vec4<f32> = output;
    return _e24;
}

fn mc_MathClosure_51_(uv: vec2<f32>, x: vec4<f32>, y: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var x_1: vec4<f32>;
    var y_1: vec4<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    x_1 = x;
    y_1 = y;
    let _e9: vec4<f32> = x_1;
    let _e10: vec4<f32> = y_1;
    output = (_e9 + _e10);
    let _e12: vec4<f32> = output;
    return _e12;
}

fn sample_pass_RenderPass_4_(uv_in: vec2f) -> vec4f {
    return textureSample(pass_tex_RenderPass_4, pass_samp_RenderPass_4, uv_in);
}

fn sample_pass_Upsample_41_(uv_in: vec2f) -> vec4f {
    return textureSample(pass_tex_Upsample_41, pass_samp_Upsample_41, uv_in);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_42_out: vec4f;
    {
        let intelli = textureSample(img_tex_ImageTexture_50, img_samp_ImageTexture_50, (in.uv));
        var output: vec4f;
        output = mc_MathClosure_42_(in.uv, intelli);
        mc_MathClosure_42_out = output;
    }
    var mc_MathClosure_51_out: vec4f;
    {
        let x = blendNormal((vec4f((graph_inputs.node_ColorInput_48_cd78eb69).rgb * (graph_inputs.node_ColorInput_48_cd78eb69).a, (graph_inputs.node_ColorInput_48_cd78eb69).a)), (textureSample(img_tex_ImageTexture_46, img_samp_ImageTexture_46, (in.uv))));
        let y = mc_MathClosure_42_out;
        var output: vec4f;
        output = mc_MathClosure_51_(in.uv, x, y);
        mc_MathClosure_51_out = output;
    }
    let _frag_out = mc_MathClosure_51_out;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
