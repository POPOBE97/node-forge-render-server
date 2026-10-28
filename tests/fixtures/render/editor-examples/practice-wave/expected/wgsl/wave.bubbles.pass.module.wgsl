
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
     @location(4) @interpolate(flat) instance_index: u32,
 };


struct GraphInputs {
    // Node: FloatInput_a2ef122f_67
    node_FloatInput_a2ef122f_67_2755b17d: vec4f,
    // Node: Vector2Input_a744c104_66
    node_Vector2Input_a744c104_66_52e4dbac: vec4f,
    // Node: wave.bubbles.extent
    node_wave_bubbles_extent_31975d26: vec4f,
    // Node: wave.bubbles.lifetime
    node_wave_bubbles_lifetime_462f5baa: vec4f,
    // Node: wave.bubbles.opacity
    node_wave_bubbles_opacity_be30ad18: vec4f,
    // Node: wave.bubbles.radius
    node_wave_bubbles_radius_5bf21d9b: vec4f,
    // Node: wave.bubbles.rise
    node_wave_bubbles_rise_68f2373f: vec4f,
    // Node: wave.speed
    node_wave_speed_f1e39e0f: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_ChromaticBlur_f4fd3448_13: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_ChromaticBlur_f4fd3448_13: sampler;


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

// Particle-local UV is bottom-left / Y-up. The explicit extent belongs to this layer.
fn shader_material_wave_bubbles_material(in: ShaderMaterialInput, particle: vec4f, extent: vec2f, opacity: f32, background: texture_2d<f32>, background_sampler: sampler) -> vec4f {
    let q = in.uv * 2.0 - vec2f(1.0);
    let r = length(q);
    let aa = max(fwidth(r), 0.015);
    let silhouette = 1.0 - smoothstep(0.96 - aa, 0.96 + aa, r);
    let rim = exp(-pow((r - 0.875) / 0.065, 2.0));
    let inner_rim = exp(-pow((r - 0.73) / 0.10, 2.0));
    let light = max(dot(q / max(r, 0.001), normalize(vec2f(-0.65, 0.76))), 0.0);
    let crescent = rim * pow(light, 3.0);
    let glint = exp(-dot((q - vec2f(-0.32, 0.53)) / vec2f(0.13, 0.075), (q - vec2f(-0.32, 0.53)) / vec2f(0.13, 0.075)));
    let base_uv = (particle.xy + q * particle.z * 0.5) / extent;
    let refract_uv = base_uv + q * (1.0 - min(r*r, 1.0)) * particle.z / extent * 0.17;
    let refracted = sample_texture_local_uv(background, background_sampler, refract_uv).rgb;
    let film = mix(vec3f(0.20, 0.52, 0.69), vec3f(0.75, 0.94, 1.0), light);
    let alpha = silhouette * particle.w * opacity * clamp(0.08 + rim * 0.46 + crescent * 0.32 + glint * 0.75, 0.0, 0.92);
    let color = refracted * (0.88 - 0.2 * inner_rim) + film * rim * 0.48 + vec3f(1.0) * (crescent * 0.65 + glint * 0.85);
    return vec4f(color * alpha, alpha);
}

fn nf_math_closure_fragment_closure(uv: vec2<f32>, index: i32, progress: f32, origin: vec2<f32>, extent: vec2<f32>, radius: f32, rise: f32, lifetime: f32, speed: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var index_1: i32;
    var progress_1: f32;
    var origin_1: vec2<f32>;
    var extent_1: vec2<f32>;
    var radius_1: f32;
    var rise_1: f32;
    var lifetime_1: f32;
    var speed_1: f32;
    var output: vec4<f32>;
    var seed: f32;
    var rx: f32;
    var ry: f32;
    var rz: f32;
    var birthPosition: vec2<f32>;
    var birth: f32;
    var age: f32;
    var live: f32;
    var travel: f32;
    var diameter: f32;
    var drift: vec2<f32>;
    var center: vec2<f32>;

    uv_1 = uv;
    index_1 = index;
    progress_1 = progress;
    origin_1 = origin;
    extent_1 = extent;
    radius_1 = radius;
    rise_1 = rise;
    lifetime_1 = lifetime;
    speed_1 = speed;
    let _e19: i32 = index_1;
    seed = (f32(_e19) + 1f);
    let _e24: f32 = seed;
    let _e29: f32 = seed;
    let _e37: f32 = seed;
    let _e42: f32 = seed;
    rx = fract((sin(((_e42 * 127.1f) + 3.7f)) * 43758.547f));
    let _e52: f32 = seed;
    let _e57: f32 = seed;
    let _e65: f32 = seed;
    let _e70: f32 = seed;
    ry = fract((sin(((_e70 * 269.5f) + 9.2f)) * 22578.146f));
    let _e80: f32 = seed;
    let _e85: f32 = seed;
    let _e93: f32 = seed;
    let _e98: f32 = seed;
    rz = fract((sin(((_e98 * 73.3f) + 1.8f)) * 19642.35f));
    let _e113: f32 = rx;
    let _e120: f32 = ry;
    let _e123: vec2<f32> = extent_1;
    birthPosition = (vec2<f32>(mix(0.07f, 0.93f, _e113), mix(0.025f, 0.86f, _e120)) * _e123);
    let _e126: vec2<f32> = birthPosition;
    let _e127: vec2<f32> = origin_1;
    let _e129: vec2<f32> = birthPosition;
    let _e130: vec2<f32> = origin_1;
    let _e133: vec2<f32> = extent_1;
    let _e135: f32 = speed_1;
    let _e138: vec2<f32> = extent_1;
    let _e140: f32 = speed_1;
    birth = ((length((_e129 - _e130)) / max((_e138.y * _e140), 1f)) + 0.014f);
    let _e148: f32 = progress_1;
    let _e149: f32 = birth;
    let _e153: f32 = lifetime_1;
    age = ((_e148 - _e149) / max(_e153, 0.01f));
    let _e163: f32 = age;
    let _e171: f32 = age;
    live = (smoothstep(0f, 0.1f, _e163) * (1f - smoothstep(0.62f, 1f, _e171)));
    let _e179: f32 = age;
    travel = clamp(_e179, 0f, 1f);
    let _e185: f32 = radius_1;
    let _e192: f32 = rz;
    let _e202: f32 = age;
    let _e211: f32 = age;
    diameter = (((2f * _e185) * mix(0.4f, 1.3f, _e192)) * mix(0.35f, 1f, smoothstep(0f, 0.18f, _e211)));
    let _e216: f32 = travel;
    let _e219: f32 = seed;
    let _e223: f32 = travel;
    let _e226: f32 = seed;
    let _e231: f32 = seed;
    let _e234: f32 = seed;
    let _e241: f32 = radius_1;
    drift = ((vec2<f32>((sin(((_e223 * 5f) + (_e226 * 1.7f))) - sin((_e234 * 1.7f))), 0f) * _e241) * 1.2f);
    let _e246: vec2<f32> = birthPosition;
    let _e247: vec2<f32> = drift;
    let _e250: f32 = rise_1;
    let _e251: f32 = travel;
    let _e258: f32 = rz;
    center = ((_e246 + _e247) + vec2<f32>(0f, ((_e250 * _e251) * mix(0.65f, 1.15f, _e258))));
    let _e264: vec2<f32> = center;
    let _e267: f32 = diameter;
    let _e270: f32 = live;
    output = vec4<f32>(_e264.x, _e264.y, max(_e267, 0.01f), _e270);
    let _e274: vec4<f32> = output;
    return _e274;
}


// --- Extra WGSL declarations (generated) ---
fn nf_math_closure_vertex_22ee4a90_closure(uv: vec2<f32>, particle: vec4<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var particle_1: vec4<f32>;
    var output: vec2<f32>;

    uv_1 = uv;
    particle_1 = particle;
    let _e5: vec4<f32> = particle_1;
    output = vec2(_e5.z);
    let _e8: vec2<f32> = output;
    return _e8;
}

fn nf_math_closure_vertex_bd8ac578_closure(uv: vec2<f32>, index: i32, progress: f32, origin: vec2<f32>, extent: vec2<f32>, radius: f32, rise: f32, lifetime: f32, speed: f32) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var index_1: i32;
    var progress_1: f32;
    var origin_1: vec2<f32>;
    var extent_1: vec2<f32>;
    var radius_1: f32;
    var rise_1: f32;
    var lifetime_1: f32;
    var speed_1: f32;
    var output: vec4<f32>;
    var seed: f32;
    var rx: f32;
    var ry: f32;
    var rz: f32;
    var birthPosition: vec2<f32>;
    var birth: f32;
    var age: f32;
    var live: f32;
    var travel: f32;
    var diameter: f32;
    var drift: vec2<f32>;
    var center: vec2<f32>;

    uv_1 = uv;
    index_1 = index;
    progress_1 = progress;
    origin_1 = origin;
    extent_1 = extent;
    radius_1 = radius;
    rise_1 = rise;
    lifetime_1 = lifetime;
    speed_1 = speed;
    let _e19: i32 = index_1;
    seed = (f32(_e19) + 1f);
    let _e24: f32 = seed;
    let _e29: f32 = seed;
    let _e37: f32 = seed;
    let _e42: f32 = seed;
    rx = fract((sin(((_e42 * 127.1f) + 3.7f)) * 43758.547f));
    let _e52: f32 = seed;
    let _e57: f32 = seed;
    let _e65: f32 = seed;
    let _e70: f32 = seed;
    ry = fract((sin(((_e70 * 269.5f) + 9.2f)) * 22578.146f));
    let _e80: f32 = seed;
    let _e85: f32 = seed;
    let _e93: f32 = seed;
    let _e98: f32 = seed;
    rz = fract((sin(((_e98 * 73.3f) + 1.8f)) * 19642.35f));
    let _e113: f32 = rx;
    let _e120: f32 = ry;
    let _e123: vec2<f32> = extent_1;
    birthPosition = (vec2<f32>(mix(0.07f, 0.93f, _e113), mix(0.025f, 0.86f, _e120)) * _e123);
    let _e126: vec2<f32> = birthPosition;
    let _e127: vec2<f32> = origin_1;
    let _e129: vec2<f32> = birthPosition;
    let _e130: vec2<f32> = origin_1;
    let _e133: vec2<f32> = extent_1;
    let _e135: f32 = speed_1;
    let _e138: vec2<f32> = extent_1;
    let _e140: f32 = speed_1;
    birth = ((length((_e129 - _e130)) / max((_e138.y * _e140), 1f)) + 0.014f);
    let _e148: f32 = progress_1;
    let _e149: f32 = birth;
    let _e153: f32 = lifetime_1;
    age = ((_e148 - _e149) / max(_e153, 0.01f));
    let _e163: f32 = age;
    let _e171: f32 = age;
    live = (smoothstep(0f, 0.1f, _e163) * (1f - smoothstep(0.62f, 1f, _e171)));
    let _e179: f32 = age;
    travel = clamp(_e179, 0f, 1f);
    let _e185: f32 = radius_1;
    let _e192: f32 = rz;
    let _e202: f32 = age;
    let _e211: f32 = age;
    diameter = (((2f * _e185) * mix(0.4f, 1.3f, _e192)) * mix(0.35f, 1f, smoothstep(0f, 0.18f, _e211)));
    let _e216: f32 = travel;
    let _e219: f32 = seed;
    let _e223: f32 = travel;
    let _e226: f32 = seed;
    let _e231: f32 = seed;
    let _e234: f32 = seed;
    let _e241: f32 = radius_1;
    drift = ((vec2<f32>((sin(((_e223 * 5f) + (_e226 * 1.7f))) - sin((_e234 * 1.7f))), 0f) * _e241) * 1.2f);
    let _e246: vec2<f32> = birthPosition;
    let _e247: vec2<f32> = drift;
    let _e250: f32 = rise_1;
    let _e251: f32 = travel;
    let _e258: f32 = rz;
    center = ((_e246 + _e247) + vec2<f32>(0f, ((_e250 * _e251) * mix(0.65f, 1.15f, _e258))));
    let _e264: vec2<f32> = center;
    let _e267: f32 = diameter;
    let _e270: f32 = live;
    output = vec4<f32>(_e264.x, _e264.y, max(_e267, 0.01f), _e270);
    let _e274: vec4<f32> = output;
    return _e274;
}

fn nf_math_closure_vertex_closure(uv: vec2<f32>, particle: vec4<f32>) -> vec2<f32> {
    var uv_1: vec2<f32>;
    var particle_1: vec4<f32>;
    var output: vec2<f32>;

    uv_1 = uv;
    particle_1 = particle;
    let _e5: vec4<f32> = particle_1;
    output = _e5.xy;
    let _e7: vec2<f32> = output;
    return _e7;
}


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
     @location(2) i0: vec4f,
     @location(3) i1: vec4f,
     @location(4) i2: vec4f,
     @location(5) i3: vec4f,
     @builtin(instance_index) instance_index: u32,
 ) -> VSOut {
 var out: VSOut;

    let math_closure_out = nf_math_closure_vertex_bd8ac578_closure(vec2f(uv.x, 1.0 - uv.y), i32(instance_index), (graph_inputs.node_FloatInput_a2ef122f_67_2755b17d).x, (graph_inputs.node_Vector2Input_a744c104_66_52e4dbac).xy, (graph_inputs.node_wave_bubbles_extent_31975d26).xy, (graph_inputs.node_wave_bubbles_radius_5bf21d9b).x, (graph_inputs.node_wave_bubbles_rise_68f2373f).x, (graph_inputs.node_wave_bubbles_lifetime_462f5baa).x, (graph_inputs.node_wave_speed_f1e39e0f).x);
    let math_closure_out_81dcdbeb = nf_math_closure_vertex_closure(vec2f(uv.x, 1.0 - uv.y), math_closure_out);
    let math_closure_out_2f799989 = nf_math_closure_vertex_22ee4a90_closure(vec2f(uv.x, 1.0 - uv.y), math_closure_out);
 out.instance_index = instance_index;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = vec2f(uv.x, 1.0 - uv.y);

 let inst_m = mat4x4f(i0, i1, i2, i3);
 let geo_sx = length(inst_m[0].xy);
 let geo_sy = length(inst_m[1].xy);
 let rect_size_px_base = math_closure_out_2f799989;
 let rect_center_px = math_closure_out_81dcdbeb;
 let rect_dyn = vec4f(rect_center_px, rect_size_px_base);
 let geo_size_px = rect_dyn.zw * vec2f(geo_sx, geo_sy);
 out.geo_size_px = geo_size_px;
 out.local_px = vec3f(uv * geo_size_px, 0.0);

 let p_rect_local_px = vec3f(position.xy * rect_dyn.zw, position.z);
 var p_local = (inst_m * vec4f(p_rect_local_px, 1.0)).xyz;

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
    let math_closure_out = nf_math_closure_fragment_closure(vec2f(in.uv.x, 1.0 - in.uv.y), i32(in.instance_index), (graph_inputs.node_FloatInput_a2ef122f_67_2755b17d).x, (graph_inputs.node_Vector2Input_a744c104_66_52e4dbac).xy, (graph_inputs.node_wave_bubbles_extent_31975d26).xy, (graph_inputs.node_wave_bubbles_radius_5bf21d9b).x, (graph_inputs.node_wave_bubbles_rise_68f2373f).x, (graph_inputs.node_wave_bubbles_lifetime_462f5baa).x, (graph_inputs.node_wave_speed_f1e39e0f).x);
    // Shader Material wave.bubbles.material.material
    let shader_material_material = shader_material_wave_bubbles_material(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        math_closure_out,
        (graph_inputs.node_wave_bubbles_extent_31975d26).xy,
        (graph_inputs.node_wave_bubbles_opacity_be30ad18).x,
        pass_tex_ChromaticBlur_f4fd3448_13,
        pass_samp_ChromaticBlur_f4fd3448_13,
    );
    // Final composite
    let _frag_out = shader_material_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
