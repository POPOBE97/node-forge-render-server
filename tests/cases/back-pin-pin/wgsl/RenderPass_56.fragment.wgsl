
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
      @location(5) normal: vec3f,
     @location(6) world_pos: vec3f,
 };


struct GraphInputs {
    // Node: GroupInstance_59/FloatInput_53
    node_GroupInstance_59_FloatInput_53_22997734: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var img_tex_GroupInstance_62_Matcap_65: texture_2d<f32>;

@group(1) @binding(1)
var img_samp_GroupInstance_62_Matcap_65: sampler;


// --- Extra WGSL declarations (generated) ---

fn matcap_uv(n: vec3f, v: vec3f) -> vec2f {
    let N = normalize(n);
    let V = normalize(v);
    let x_axis = normalize(vec3f(V.z, 0.0, -V.x));
    let y_axis = normalize(cross(V, x_axis));
    let uv = vec2f(dot(N, x_axis), dot(N, y_axis)) * 0.5 + 0.5;
    return clamp(uv, vec2f(0.0), vec2f(1.0));
}

fn mc_GroupInstance_62_MathClosure_63_(uv: vec2<f32>, uv_1: vec2<f32>, front_color: vec4<f32>) -> vec4<f32> {
    var uv_2: vec2<f32>;
    var uv_3: vec2<f32>;
    var front_color_1: vec4<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_2 = uv;
    uv_3 = uv_1;
    front_color_1 = front_color;
    let _e9: vec2<f32> = uv_3;
    if (_e9.y < 0f) {
        {
            output = vec4<f32>(1f, 0f, 0f, 1f);
        }
    } else {
        let _e18: vec2<f32> = uv_3;
        if (_e18.y > 0.5f) {
            {
                output = vec4<f32>(0f, 1f, 0f, 1f);
            }
        } else {
            {
                let _e27: vec4<f32> = front_color_1;
                output = _e27;
            }
        }
    }
    let _e28: vec4<f32> = output;
    return _e28;
}


// --- Extra WGSL declarations (generated) ---
fn mc_GroupInstance_59_MathClosure_43_(uv: vec2<f32>, t: f32) -> vec3<f32> {
    var uv_1: vec2<f32>;
    var t_1: f32;
    var output: vec3<f32> = vec3(0f);

    uv_1 = uv;
    t_1 = t;
    let _e8: f32 = t_1;
    let _e11: f32 = t_1;
    output = vec3<f32>(90f, (fract((_e11 / 10f)) * 360f), 0f);
    let _e19: vec3<f32> = output;
    return _e19;
}


fn sys_apply_trs_xyz(p: vec3f, t: vec3f, r_deg: vec3f, s: vec3f) -> vec3f {
    let rad = r_deg * 0.017453292519943295;

    let cx = cos(rad.x);
    let sx = sin(rad.x);
    let cy = cos(rad.y);
    let sy = sin(rad.y);
    let cz = cos(rad.z);
    let sz = sin(rad.z);

    let p0 = p * s;
    let p1 = vec3f(p0.x, p0.y * cx - p0.z * sx, p0.y * sx + p0.z * cx);
    let p2 = vec3f(p1.x * cy + p1.z * sy, p1.y, -p1.x * sy + p1.z * cy);
    let p3 = vec3f(p2.x * cz - p2.y * sz, p2.x * sz + p2.y * cz, p2.z);
    return p3 + t;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_GroupInstance_62_MathClosure_63_out: vec4f;
    {
        let uv = vec2f(in.uv.x, 1.0 - in.uv.y);
        let front_color = textureSample(img_tex_GroupInstance_62_Matcap_65, img_samp_GroupInstance_62_Matcap_65, matcap_uv(in.normal, normalize(params.camera_position.xyz - in.world_pos)));
        var output: vec4f;
        output = mc_GroupInstance_62_MathClosure_63_(in.uv, uv, front_color);
        mc_GroupInstance_62_MathClosure_63_out = output;
    }
    let _frag_out = mc_GroupInstance_62_MathClosure_63_out;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
