
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
    // Node: GroupInstance_62/ColorInput_68
    group_instance_62_color_input_68: vec4f,
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

fn nf_color_fragment_closure(uv: vec2<f32>, sampleUV: vec2<f32>, front_color: vec4<f32>, input3_: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var sampleUV_1: vec2<f32>;
    var front_color_1: vec4<f32>;
    var input3_1: vec4<f32>;
    var output: vec4<f32>;

    uv_1 = uv;
    sampleUV_1 = sampleUV;
    front_color_1 = front_color;
    input3_1 = input3_;
    let _e9: vec2<f32> = sampleUV_1;
    if (_e9.y < 0f) {
        {
            output = vec4<f32>(1f, 0f, 0f, 1f);
        }
    } else {
        let _e18: vec2<f32> = sampleUV_1;
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


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Matcap GroupInstance_62/Matcap_65.color
    let matcap = textureSample(
        img_tex_GroupInstance_62_Matcap_65,
        img_samp_GroupInstance_62_Matcap_65,
        matcap_uv(in.normal, normalize(params.camera_position.xyz - in.world_pos)),
    );
    let color_out = nf_color_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), vec2f(in.uv.x, 1.0 - in.uv.y), matcap, vec4f((graph_inputs.group_instance_62_color_input_68).rgb * (graph_inputs.group_instance_62_color_input_68).a, (graph_inputs.group_instance_62_color_input_68).a));
    // Final composite
    let _frag_out = color_out;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
