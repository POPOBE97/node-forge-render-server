
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
    // Node: FloatInput_106
    node_FloatInput_106_79ef3817: vec4f,
    // Node: FloatInput_107
    node_FloatInput_107_c6ed3817: vec4f,
    // Node: FloatInput_108
    node_FloatInput_108_77003917: vec4f,
    // Node: FloatInput_109
    node_FloatInput_109_c4fe3817: vec4f,
    // Node: FloatInput_113
    node_FloatInput_113_a3d73517: vec4f,
    // Node: GroupInstance_111/Vector2Input_89
    node_GroupInstance_111_Vector2Input_89_d77a1c71: vec4f,
    // Node: GroupInstance_114/GroupInstance_102/Vector2Input_89
    node_GroupInstance_114_GroupInstance_102_Vector2Input_89_11126536: vec4f,
    // Node: GroupInstance_114/GroupInstance_103/Vector2Input_89
    node_GroupInstance_114_GroupInstance_103_Vector2Input_89_92ff9e15: vec4f,
    // Node: GroupInstance_114/GroupInstance_104/Vector2Input_89
    node_GroupInstance_114_GroupInstance_104_Vector2Input_89_dbee00ee: vec4f,
    // Node: GroupInstance_114/GroupInstance_97/Vector2Input_89
    node_GroupInstance_114_GroupInstance_97_Vector2Input_89_0c3e8ae7: vec4f,
    // Node: GroupInstance_99/GroupInstance_102/Vector2Input_89
    node_GroupInstance_99_GroupInstance_102_Vector2Input_89_21dd5194: vec4f,
    // Node: GroupInstance_99/GroupInstance_103/Vector2Input_89
    node_GroupInstance_99_GroupInstance_103_Vector2Input_89_2261b0d2: vec4f,
    // Node: GroupInstance_99/GroupInstance_104/Vector2Input_89
    node_GroupInstance_99_GroupInstance_104_Vector2Input_89_6b00a0e9: vec4f,
    // Node: GroupInstance_99/GroupInstance_97/Vector2Input_89
    node_GroupInstance_99_GroupInstance_97_Vector2Input_89_9c3bbd47: vec4f,
    // Node: Vector2Input_115
    node_Vector2Input_115_82666989: vec4f,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_sys_group_sampleFromMipmap_RenderPass_85: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_sys_group_sampleFromMipmap_RenderPass_85: sampler;

@group(1) @binding(2)
var pass_tex_sys_group_sampleFromMipmap_Downsample_mip1: texture_2d<f32>;

@group(1) @binding(3)
var pass_samp_sys_group_sampleFromMipmap_Downsample_mip1: sampler;

@group(1) @binding(4)
var pass_tex_sys_group_sampleFromMipmap_Downsample_mip2: texture_2d<f32>;

@group(1) @binding(5)
var pass_samp_sys_group_sampleFromMipmap_Downsample_mip2: sampler;

@group(1) @binding(6)
var pass_tex_sys_group_sampleFromMipmap_Downsample_mip3: texture_2d<f32>;

@group(1) @binding(7)
var pass_samp_sys_group_sampleFromMipmap_Downsample_mip3: sampler;

@group(1) @binding(8)
var pass_tex_sys_group_sampleFromMipmap_Downsample_mip4: texture_2d<f32>;

@group(1) @binding(9)
var pass_samp_sys_group_sampleFromMipmap_Downsample_mip4: sampler;

@group(1) @binding(10)
var pass_tex_sys_group_sampleFromMipmap_Downsample_mip5: texture_2d<f32>;

@group(1) @binding(11)
var pass_samp_sys_group_sampleFromMipmap_Downsample_mip5: sampler;

@group(1) @binding(12)
var pass_tex_sys_group_sampleFromMipmap_Downsample_mip6: texture_2d<f32>;

@group(1) @binding(13)
var pass_samp_sys_group_sampleFromMipmap_Downsample_mip6: sampler;


// --- Extra WGSL declarations (generated) ---
fn mc_GroupInstance_111_MathClosure_95_(uv: vec2<f32>, xy: vec2<f32>, level: f32, mip0_size: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var level_1: f32;
    var mip0_size_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    xy_1 = xy;
    level_1 = level;
    mip0_size_1 = mip0_size;
    let _e11: f32 = level_1;
    if (_e11 == 0f) {
        {
            let _e15: vec2<f32> = mip0_size_1;
            let _e19: f32 = level_1;
            let _e23: vec2<f32> = xy_1;
            let _e24: vec2<f32> = mip0_size_1;
            let _e28: f32 = level_1;
            let _e32: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(_e23, (_e24 / vec2(pow(2f, _e28))));
            output = _e32;
        }
    } else {
        let _e33: f32 = level_1;
        if (_e33 == 1f) {
            {
                let _e37: vec2<f32> = mip0_size_1;
                let _e41: f32 = level_1;
                let _e45: vec2<f32> = xy_1;
                let _e46: vec2<f32> = mip0_size_1;
                let _e50: f32 = level_1;
                let _e54: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(_e45, (_e46 / vec2(pow(2f, _e50))));
                output = _e54;
            }
        } else {
            let _e55: f32 = level_1;
            if (_e55 == 2f) {
                {
                    let _e59: vec2<f32> = mip0_size_1;
                    let _e63: f32 = level_1;
                    let _e67: vec2<f32> = xy_1;
                    let _e68: vec2<f32> = mip0_size_1;
                    let _e72: f32 = level_1;
                    let _e76: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(_e67, (_e68 / vec2(pow(2f, _e72))));
                    output = _e76;
                }
            } else {
                let _e77: f32 = level_1;
                if (_e77 == 3f) {
                    {
                        let _e81: vec2<f32> = mip0_size_1;
                        let _e85: f32 = level_1;
                        let _e89: vec2<f32> = xy_1;
                        let _e90: vec2<f32> = mip0_size_1;
                        let _e94: f32 = level_1;
                        let _e98: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(_e89, (_e90 / vec2(pow(2f, _e94))));
                        output = _e98;
                    }
                } else {
                    let _e99: f32 = level_1;
                    if (_e99 == 4f) {
                        {
                            let _e103: vec2<f32> = mip0_size_1;
                            let _e107: f32 = level_1;
                            let _e111: vec2<f32> = xy_1;
                            let _e112: vec2<f32> = mip0_size_1;
                            let _e116: f32 = level_1;
                            let _e120: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(_e111, (_e112 / vec2(pow(2f, _e116))));
                            output = _e120;
                        }
                    } else {
                        let _e121: f32 = level_1;
                        if (_e121 == 5f) {
                            {
                                let _e125: vec2<f32> = mip0_size_1;
                                let _e129: f32 = level_1;
                                let _e133: vec2<f32> = xy_1;
                                let _e134: vec2<f32> = mip0_size_1;
                                let _e138: f32 = level_1;
                                let _e142: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(_e133, (_e134 / vec2(pow(2f, _e138))));
                                output = _e142;
                            }
                        } else {
                            let _e143: f32 = level_1;
                            if (_e143 == 6f) {
                                {
                                    let _e147: vec2<f32> = mip0_size_1;
                                    let _e151: f32 = level_1;
                                    let _e155: vec2<f32> = xy_1;
                                    let _e156: vec2<f32> = mip0_size_1;
                                    let _e160: f32 = level_1;
                                    let _e164: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(_e155, (_e156 / vec2(pow(2f, _e160))));
                                    output = _e164;
                                }
                            } else {
                                {
                                    return vec4(0f);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e167: vec4<f32> = output;
    return _e167;
}

fn mc_GroupInstance_114_GroupInstance_102_MathClosure_95_(uv: vec2<f32>, xy: vec2<f32>, level: f32, mip0_size: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var level_1: f32;
    var mip0_size_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    xy_1 = xy;
    level_1 = level;
    mip0_size_1 = mip0_size;
    let _e11: f32 = level_1;
    if (_e11 == 0f) {
        {
            let _e15: vec2<f32> = mip0_size_1;
            let _e19: f32 = level_1;
            let _e23: vec2<f32> = xy_1;
            let _e24: vec2<f32> = mip0_size_1;
            let _e28: f32 = level_1;
            let _e32: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(_e23, (_e24 / vec2(pow(2f, _e28))));
            output = _e32;
        }
    } else {
        let _e33: f32 = level_1;
        if (_e33 == 1f) {
            {
                let _e37: vec2<f32> = mip0_size_1;
                let _e41: f32 = level_1;
                let _e45: vec2<f32> = xy_1;
                let _e46: vec2<f32> = mip0_size_1;
                let _e50: f32 = level_1;
                let _e54: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(_e45, (_e46 / vec2(pow(2f, _e50))));
                output = _e54;
            }
        } else {
            let _e55: f32 = level_1;
            if (_e55 == 2f) {
                {
                    let _e59: vec2<f32> = mip0_size_1;
                    let _e63: f32 = level_1;
                    let _e67: vec2<f32> = xy_1;
                    let _e68: vec2<f32> = mip0_size_1;
                    let _e72: f32 = level_1;
                    let _e76: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(_e67, (_e68 / vec2(pow(2f, _e72))));
                    output = _e76;
                }
            } else {
                let _e77: f32 = level_1;
                if (_e77 == 3f) {
                    {
                        let _e81: vec2<f32> = mip0_size_1;
                        let _e85: f32 = level_1;
                        let _e89: vec2<f32> = xy_1;
                        let _e90: vec2<f32> = mip0_size_1;
                        let _e94: f32 = level_1;
                        let _e98: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(_e89, (_e90 / vec2(pow(2f, _e94))));
                        output = _e98;
                    }
                } else {
                    let _e99: f32 = level_1;
                    if (_e99 == 4f) {
                        {
                            let _e103: vec2<f32> = mip0_size_1;
                            let _e107: f32 = level_1;
                            let _e111: vec2<f32> = xy_1;
                            let _e112: vec2<f32> = mip0_size_1;
                            let _e116: f32 = level_1;
                            let _e120: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(_e111, (_e112 / vec2(pow(2f, _e116))));
                            output = _e120;
                        }
                    } else {
                        let _e121: f32 = level_1;
                        if (_e121 == 5f) {
                            {
                                let _e125: vec2<f32> = mip0_size_1;
                                let _e129: f32 = level_1;
                                let _e133: vec2<f32> = xy_1;
                                let _e134: vec2<f32> = mip0_size_1;
                                let _e138: f32 = level_1;
                                let _e142: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(_e133, (_e134 / vec2(pow(2f, _e138))));
                                output = _e142;
                            }
                        } else {
                            let _e143: f32 = level_1;
                            if (_e143 == 6f) {
                                {
                                    let _e147: vec2<f32> = mip0_size_1;
                                    let _e151: f32 = level_1;
                                    let _e155: vec2<f32> = xy_1;
                                    let _e156: vec2<f32> = mip0_size_1;
                                    let _e160: f32 = level_1;
                                    let _e164: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(_e155, (_e156 / vec2(pow(2f, _e160))));
                                    output = _e164;
                                }
                            } else {
                                {
                                    return vec4(0f);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e167: vec4<f32> = output;
    return _e167;
}

fn mc_GroupInstance_114_GroupInstance_103_MathClosure_95_(uv: vec2<f32>, xy: vec2<f32>, level: f32, mip0_size: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var level_1: f32;
    var mip0_size_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    xy_1 = xy;
    level_1 = level;
    mip0_size_1 = mip0_size;
    let _e11: f32 = level_1;
    if (_e11 == 0f) {
        {
            let _e15: vec2<f32> = mip0_size_1;
            let _e19: f32 = level_1;
            let _e23: vec2<f32> = xy_1;
            let _e24: vec2<f32> = mip0_size_1;
            let _e28: f32 = level_1;
            let _e32: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(_e23, (_e24 / vec2(pow(2f, _e28))));
            output = _e32;
        }
    } else {
        let _e33: f32 = level_1;
        if (_e33 == 1f) {
            {
                let _e37: vec2<f32> = mip0_size_1;
                let _e41: f32 = level_1;
                let _e45: vec2<f32> = xy_1;
                let _e46: vec2<f32> = mip0_size_1;
                let _e50: f32 = level_1;
                let _e54: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(_e45, (_e46 / vec2(pow(2f, _e50))));
                output = _e54;
            }
        } else {
            let _e55: f32 = level_1;
            if (_e55 == 2f) {
                {
                    let _e59: vec2<f32> = mip0_size_1;
                    let _e63: f32 = level_1;
                    let _e67: vec2<f32> = xy_1;
                    let _e68: vec2<f32> = mip0_size_1;
                    let _e72: f32 = level_1;
                    let _e76: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(_e67, (_e68 / vec2(pow(2f, _e72))));
                    output = _e76;
                }
            } else {
                let _e77: f32 = level_1;
                if (_e77 == 3f) {
                    {
                        let _e81: vec2<f32> = mip0_size_1;
                        let _e85: f32 = level_1;
                        let _e89: vec2<f32> = xy_1;
                        let _e90: vec2<f32> = mip0_size_1;
                        let _e94: f32 = level_1;
                        let _e98: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(_e89, (_e90 / vec2(pow(2f, _e94))));
                        output = _e98;
                    }
                } else {
                    let _e99: f32 = level_1;
                    if (_e99 == 4f) {
                        {
                            let _e103: vec2<f32> = mip0_size_1;
                            let _e107: f32 = level_1;
                            let _e111: vec2<f32> = xy_1;
                            let _e112: vec2<f32> = mip0_size_1;
                            let _e116: f32 = level_1;
                            let _e120: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(_e111, (_e112 / vec2(pow(2f, _e116))));
                            output = _e120;
                        }
                    } else {
                        let _e121: f32 = level_1;
                        if (_e121 == 5f) {
                            {
                                let _e125: vec2<f32> = mip0_size_1;
                                let _e129: f32 = level_1;
                                let _e133: vec2<f32> = xy_1;
                                let _e134: vec2<f32> = mip0_size_1;
                                let _e138: f32 = level_1;
                                let _e142: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(_e133, (_e134 / vec2(pow(2f, _e138))));
                                output = _e142;
                            }
                        } else {
                            let _e143: f32 = level_1;
                            if (_e143 == 6f) {
                                {
                                    let _e147: vec2<f32> = mip0_size_1;
                                    let _e151: f32 = level_1;
                                    let _e155: vec2<f32> = xy_1;
                                    let _e156: vec2<f32> = mip0_size_1;
                                    let _e160: f32 = level_1;
                                    let _e164: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(_e155, (_e156 / vec2(pow(2f, _e160))));
                                    output = _e164;
                                }
                            } else {
                                {
                                    return vec4(0f);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e167: vec4<f32> = output;
    return _e167;
}

fn mc_GroupInstance_114_GroupInstance_104_MathClosure_95_(uv: vec2<f32>, xy: vec2<f32>, level: f32, mip0_size: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var level_1: f32;
    var mip0_size_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    xy_1 = xy;
    level_1 = level;
    mip0_size_1 = mip0_size;
    let _e11: f32 = level_1;
    if (_e11 == 0f) {
        {
            let _e15: vec2<f32> = mip0_size_1;
            let _e19: f32 = level_1;
            let _e23: vec2<f32> = xy_1;
            let _e24: vec2<f32> = mip0_size_1;
            let _e28: f32 = level_1;
            let _e32: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(_e23, (_e24 / vec2(pow(2f, _e28))));
            output = _e32;
        }
    } else {
        let _e33: f32 = level_1;
        if (_e33 == 1f) {
            {
                let _e37: vec2<f32> = mip0_size_1;
                let _e41: f32 = level_1;
                let _e45: vec2<f32> = xy_1;
                let _e46: vec2<f32> = mip0_size_1;
                let _e50: f32 = level_1;
                let _e54: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(_e45, (_e46 / vec2(pow(2f, _e50))));
                output = _e54;
            }
        } else {
            let _e55: f32 = level_1;
            if (_e55 == 2f) {
                {
                    let _e59: vec2<f32> = mip0_size_1;
                    let _e63: f32 = level_1;
                    let _e67: vec2<f32> = xy_1;
                    let _e68: vec2<f32> = mip0_size_1;
                    let _e72: f32 = level_1;
                    let _e76: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(_e67, (_e68 / vec2(pow(2f, _e72))));
                    output = _e76;
                }
            } else {
                let _e77: f32 = level_1;
                if (_e77 == 3f) {
                    {
                        let _e81: vec2<f32> = mip0_size_1;
                        let _e85: f32 = level_1;
                        let _e89: vec2<f32> = xy_1;
                        let _e90: vec2<f32> = mip0_size_1;
                        let _e94: f32 = level_1;
                        let _e98: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(_e89, (_e90 / vec2(pow(2f, _e94))));
                        output = _e98;
                    }
                } else {
                    let _e99: f32 = level_1;
                    if (_e99 == 4f) {
                        {
                            let _e103: vec2<f32> = mip0_size_1;
                            let _e107: f32 = level_1;
                            let _e111: vec2<f32> = xy_1;
                            let _e112: vec2<f32> = mip0_size_1;
                            let _e116: f32 = level_1;
                            let _e120: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(_e111, (_e112 / vec2(pow(2f, _e116))));
                            output = _e120;
                        }
                    } else {
                        let _e121: f32 = level_1;
                        if (_e121 == 5f) {
                            {
                                let _e125: vec2<f32> = mip0_size_1;
                                let _e129: f32 = level_1;
                                let _e133: vec2<f32> = xy_1;
                                let _e134: vec2<f32> = mip0_size_1;
                                let _e138: f32 = level_1;
                                let _e142: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(_e133, (_e134 / vec2(pow(2f, _e138))));
                                output = _e142;
                            }
                        } else {
                            let _e143: f32 = level_1;
                            if (_e143 == 6f) {
                                {
                                    let _e147: vec2<f32> = mip0_size_1;
                                    let _e151: f32 = level_1;
                                    let _e155: vec2<f32> = xy_1;
                                    let _e156: vec2<f32> = mip0_size_1;
                                    let _e160: f32 = level_1;
                                    let _e164: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(_e155, (_e156 / vec2(pow(2f, _e160))));
                                    output = _e164;
                                }
                            } else {
                                {
                                    return vec4(0f);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e167: vec4<f32> = output;
    return _e167;
}

fn mc_GroupInstance_114_GroupInstance_97_MathClosure_95_(uv: vec2<f32>, xy: vec2<f32>, level: f32, mip0_size: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var level_1: f32;
    var mip0_size_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    xy_1 = xy;
    level_1 = level;
    mip0_size_1 = mip0_size;
    let _e11: f32 = level_1;
    if (_e11 == 0f) {
        {
            let _e15: vec2<f32> = mip0_size_1;
            let _e19: f32 = level_1;
            let _e23: vec2<f32> = xy_1;
            let _e24: vec2<f32> = mip0_size_1;
            let _e28: f32 = level_1;
            let _e32: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(_e23, (_e24 / vec2(pow(2f, _e28))));
            output = _e32;
        }
    } else {
        let _e33: f32 = level_1;
        if (_e33 == 1f) {
            {
                let _e37: vec2<f32> = mip0_size_1;
                let _e41: f32 = level_1;
                let _e45: vec2<f32> = xy_1;
                let _e46: vec2<f32> = mip0_size_1;
                let _e50: f32 = level_1;
                let _e54: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(_e45, (_e46 / vec2(pow(2f, _e50))));
                output = _e54;
            }
        } else {
            let _e55: f32 = level_1;
            if (_e55 == 2f) {
                {
                    let _e59: vec2<f32> = mip0_size_1;
                    let _e63: f32 = level_1;
                    let _e67: vec2<f32> = xy_1;
                    let _e68: vec2<f32> = mip0_size_1;
                    let _e72: f32 = level_1;
                    let _e76: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(_e67, (_e68 / vec2(pow(2f, _e72))));
                    output = _e76;
                }
            } else {
                let _e77: f32 = level_1;
                if (_e77 == 3f) {
                    {
                        let _e81: vec2<f32> = mip0_size_1;
                        let _e85: f32 = level_1;
                        let _e89: vec2<f32> = xy_1;
                        let _e90: vec2<f32> = mip0_size_1;
                        let _e94: f32 = level_1;
                        let _e98: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(_e89, (_e90 / vec2(pow(2f, _e94))));
                        output = _e98;
                    }
                } else {
                    let _e99: f32 = level_1;
                    if (_e99 == 4f) {
                        {
                            let _e103: vec2<f32> = mip0_size_1;
                            let _e107: f32 = level_1;
                            let _e111: vec2<f32> = xy_1;
                            let _e112: vec2<f32> = mip0_size_1;
                            let _e116: f32 = level_1;
                            let _e120: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(_e111, (_e112 / vec2(pow(2f, _e116))));
                            output = _e120;
                        }
                    } else {
                        let _e121: f32 = level_1;
                        if (_e121 == 5f) {
                            {
                                let _e125: vec2<f32> = mip0_size_1;
                                let _e129: f32 = level_1;
                                let _e133: vec2<f32> = xy_1;
                                let _e134: vec2<f32> = mip0_size_1;
                                let _e138: f32 = level_1;
                                let _e142: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(_e133, (_e134 / vec2(pow(2f, _e138))));
                                output = _e142;
                            }
                        } else {
                            let _e143: f32 = level_1;
                            if (_e143 == 6f) {
                                {
                                    let _e147: vec2<f32> = mip0_size_1;
                                    let _e151: f32 = level_1;
                                    let _e155: vec2<f32> = xy_1;
                                    let _e156: vec2<f32> = mip0_size_1;
                                    let _e160: f32 = level_1;
                                    let _e164: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(_e155, (_e156 / vec2(pow(2f, _e160))));
                                    output = _e164;
                                }
                            } else {
                                {
                                    return vec4(0f);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e167: vec4<f32> = output;
    return _e167;
}

fn mc_GroupInstance_114_MathClosure_105(uv_in: vec2f, w_in: array<vec2f, 4>) -> vec2f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var output: vec2f;
    output = vec2f(w[2].x, w[2].y);
    return output;
}

fn mc_GroupInstance_114_MathClosure_106(uv_in: vec2f, w_in: array<vec2f, 4>) -> vec2f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var output: vec2f;
    output = vec2f(w[3].x, w[2].y);
    return output;
}

fn mc_GroupInstance_114_MathClosure_107(uv_in: vec2f, w_in: array<vec2f, 4>) -> vec2f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var output: vec2f;
    output = vec2f(w[2].x, w[3].y);
    return output;
}

fn mc_GroupInstance_114_MathClosure_108(uv_in: vec2f, w_in: array<vec2f, 4>) -> vec2f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var output: vec2f;
    output = vec2f(w[3].x, w[3].y);
    return output;
}

fn mc_GroupInstance_114_MathClosure_109(uv_in: vec2f, dc_in: vec2f, scale_in: f32) -> array<vec2f, 4> {
    var uv: vec2f = uv_in;
    var dc: vec2f = dc_in;
    var scale: f32 = scale_in;
    var output: array<vec2f, 4>;
    var d: vec2f = dc * scale - 0.5;
    var c: vec2f = floor(d);
    var x: vec2f = c - d + 1.0;
    var X: vec2f = d - c;
    var x3: vec2f = x * x * x;
    var coeff: vec2f = 0.5 * x * x + 0.5 * x + 0.166667;
    var w1: vec2f = -0.333333 * x3 + coeff;
    var w2: vec2f = 1.0 - w1;
    var o1: vec2f = (-0.5 * x3 + coeff) / w1 + c - 0.5;
    var o2: vec2f = (X * X * X / 6.0) / w2 + c + 1.5;
    output = array<vec2f, 4>(w1, w2, o1, o2);
    return output;
}

fn mc_GroupInstance_114_MathClosure_111_(uv: vec2<f32>, level: f32) -> f32 {
    var uv_1: vec2<f32>;
    var level_1: f32;
    var output: f32 = 0f;

    uv_1 = uv;
    level_1 = level;
    let _e11: f32 = level_1;
    output = (1f / pow(2f, _e11));
    let _e14: f32 = output;
    return _e14;
}

fn mc_GroupInstance_114_MathClosure_99(uv_in: vec2f, w_in: array<vec2f, 4>, c0_in: vec4f, c1_in: vec4f, c2_in: vec4f, c3_in: vec4f) -> vec4f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var c0: vec4f = c0_in;
    var c1: vec4f = c1_in;
    var c2: vec4f = c2_in;
    var c3: vec4f = c3_in;
    var output: vec4f;
    var o: vec4f = vec4f(0.0);
    o += w[0].x * w[0].y * c0;
    o += w[1].x * w[0].y * c1;
    o += w[0].x * w[1].y * c2;
    o += w[1].x * w[1].y * c3;
    output = o;
    return output;
}

fn mc_GroupInstance_99_GroupInstance_102_MathClosure_95_(uv: vec2<f32>, xy: vec2<f32>, level: f32, mip0_size: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var level_1: f32;
    var mip0_size_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    xy_1 = xy;
    level_1 = level;
    mip0_size_1 = mip0_size;
    let _e11: f32 = level_1;
    if (_e11 == 0f) {
        {
            let _e15: vec2<f32> = mip0_size_1;
            let _e19: f32 = level_1;
            let _e23: vec2<f32> = xy_1;
            let _e24: vec2<f32> = mip0_size_1;
            let _e28: f32 = level_1;
            let _e32: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(_e23, (_e24 / vec2(pow(2f, _e28))));
            output = _e32;
        }
    } else {
        let _e33: f32 = level_1;
        if (_e33 == 1f) {
            {
                let _e37: vec2<f32> = mip0_size_1;
                let _e41: f32 = level_1;
                let _e45: vec2<f32> = xy_1;
                let _e46: vec2<f32> = mip0_size_1;
                let _e50: f32 = level_1;
                let _e54: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(_e45, (_e46 / vec2(pow(2f, _e50))));
                output = _e54;
            }
        } else {
            let _e55: f32 = level_1;
            if (_e55 == 2f) {
                {
                    let _e59: vec2<f32> = mip0_size_1;
                    let _e63: f32 = level_1;
                    let _e67: vec2<f32> = xy_1;
                    let _e68: vec2<f32> = mip0_size_1;
                    let _e72: f32 = level_1;
                    let _e76: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(_e67, (_e68 / vec2(pow(2f, _e72))));
                    output = _e76;
                }
            } else {
                let _e77: f32 = level_1;
                if (_e77 == 3f) {
                    {
                        let _e81: vec2<f32> = mip0_size_1;
                        let _e85: f32 = level_1;
                        let _e89: vec2<f32> = xy_1;
                        let _e90: vec2<f32> = mip0_size_1;
                        let _e94: f32 = level_1;
                        let _e98: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(_e89, (_e90 / vec2(pow(2f, _e94))));
                        output = _e98;
                    }
                } else {
                    let _e99: f32 = level_1;
                    if (_e99 == 4f) {
                        {
                            let _e103: vec2<f32> = mip0_size_1;
                            let _e107: f32 = level_1;
                            let _e111: vec2<f32> = xy_1;
                            let _e112: vec2<f32> = mip0_size_1;
                            let _e116: f32 = level_1;
                            let _e120: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(_e111, (_e112 / vec2(pow(2f, _e116))));
                            output = _e120;
                        }
                    } else {
                        let _e121: f32 = level_1;
                        if (_e121 == 5f) {
                            {
                                let _e125: vec2<f32> = mip0_size_1;
                                let _e129: f32 = level_1;
                                let _e133: vec2<f32> = xy_1;
                                let _e134: vec2<f32> = mip0_size_1;
                                let _e138: f32 = level_1;
                                let _e142: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(_e133, (_e134 / vec2(pow(2f, _e138))));
                                output = _e142;
                            }
                        } else {
                            let _e143: f32 = level_1;
                            if (_e143 == 6f) {
                                {
                                    let _e147: vec2<f32> = mip0_size_1;
                                    let _e151: f32 = level_1;
                                    let _e155: vec2<f32> = xy_1;
                                    let _e156: vec2<f32> = mip0_size_1;
                                    let _e160: f32 = level_1;
                                    let _e164: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(_e155, (_e156 / vec2(pow(2f, _e160))));
                                    output = _e164;
                                }
                            } else {
                                {
                                    return vec4(0f);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e167: vec4<f32> = output;
    return _e167;
}

fn mc_GroupInstance_99_GroupInstance_103_MathClosure_95_(uv: vec2<f32>, xy: vec2<f32>, level: f32, mip0_size: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var level_1: f32;
    var mip0_size_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    xy_1 = xy;
    level_1 = level;
    mip0_size_1 = mip0_size;
    let _e11: f32 = level_1;
    if (_e11 == 0f) {
        {
            let _e15: vec2<f32> = mip0_size_1;
            let _e19: f32 = level_1;
            let _e23: vec2<f32> = xy_1;
            let _e24: vec2<f32> = mip0_size_1;
            let _e28: f32 = level_1;
            let _e32: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(_e23, (_e24 / vec2(pow(2f, _e28))));
            output = _e32;
        }
    } else {
        let _e33: f32 = level_1;
        if (_e33 == 1f) {
            {
                let _e37: vec2<f32> = mip0_size_1;
                let _e41: f32 = level_1;
                let _e45: vec2<f32> = xy_1;
                let _e46: vec2<f32> = mip0_size_1;
                let _e50: f32 = level_1;
                let _e54: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(_e45, (_e46 / vec2(pow(2f, _e50))));
                output = _e54;
            }
        } else {
            let _e55: f32 = level_1;
            if (_e55 == 2f) {
                {
                    let _e59: vec2<f32> = mip0_size_1;
                    let _e63: f32 = level_1;
                    let _e67: vec2<f32> = xy_1;
                    let _e68: vec2<f32> = mip0_size_1;
                    let _e72: f32 = level_1;
                    let _e76: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(_e67, (_e68 / vec2(pow(2f, _e72))));
                    output = _e76;
                }
            } else {
                let _e77: f32 = level_1;
                if (_e77 == 3f) {
                    {
                        let _e81: vec2<f32> = mip0_size_1;
                        let _e85: f32 = level_1;
                        let _e89: vec2<f32> = xy_1;
                        let _e90: vec2<f32> = mip0_size_1;
                        let _e94: f32 = level_1;
                        let _e98: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(_e89, (_e90 / vec2(pow(2f, _e94))));
                        output = _e98;
                    }
                } else {
                    let _e99: f32 = level_1;
                    if (_e99 == 4f) {
                        {
                            let _e103: vec2<f32> = mip0_size_1;
                            let _e107: f32 = level_1;
                            let _e111: vec2<f32> = xy_1;
                            let _e112: vec2<f32> = mip0_size_1;
                            let _e116: f32 = level_1;
                            let _e120: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(_e111, (_e112 / vec2(pow(2f, _e116))));
                            output = _e120;
                        }
                    } else {
                        let _e121: f32 = level_1;
                        if (_e121 == 5f) {
                            {
                                let _e125: vec2<f32> = mip0_size_1;
                                let _e129: f32 = level_1;
                                let _e133: vec2<f32> = xy_1;
                                let _e134: vec2<f32> = mip0_size_1;
                                let _e138: f32 = level_1;
                                let _e142: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(_e133, (_e134 / vec2(pow(2f, _e138))));
                                output = _e142;
                            }
                        } else {
                            let _e143: f32 = level_1;
                            if (_e143 == 6f) {
                                {
                                    let _e147: vec2<f32> = mip0_size_1;
                                    let _e151: f32 = level_1;
                                    let _e155: vec2<f32> = xy_1;
                                    let _e156: vec2<f32> = mip0_size_1;
                                    let _e160: f32 = level_1;
                                    let _e164: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(_e155, (_e156 / vec2(pow(2f, _e160))));
                                    output = _e164;
                                }
                            } else {
                                {
                                    return vec4(0f);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e167: vec4<f32> = output;
    return _e167;
}

fn mc_GroupInstance_99_GroupInstance_104_MathClosure_95_(uv: vec2<f32>, xy: vec2<f32>, level: f32, mip0_size: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var level_1: f32;
    var mip0_size_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    xy_1 = xy;
    level_1 = level;
    mip0_size_1 = mip0_size;
    let _e11: f32 = level_1;
    if (_e11 == 0f) {
        {
            let _e15: vec2<f32> = mip0_size_1;
            let _e19: f32 = level_1;
            let _e23: vec2<f32> = xy_1;
            let _e24: vec2<f32> = mip0_size_1;
            let _e28: f32 = level_1;
            let _e32: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(_e23, (_e24 / vec2(pow(2f, _e28))));
            output = _e32;
        }
    } else {
        let _e33: f32 = level_1;
        if (_e33 == 1f) {
            {
                let _e37: vec2<f32> = mip0_size_1;
                let _e41: f32 = level_1;
                let _e45: vec2<f32> = xy_1;
                let _e46: vec2<f32> = mip0_size_1;
                let _e50: f32 = level_1;
                let _e54: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(_e45, (_e46 / vec2(pow(2f, _e50))));
                output = _e54;
            }
        } else {
            let _e55: f32 = level_1;
            if (_e55 == 2f) {
                {
                    let _e59: vec2<f32> = mip0_size_1;
                    let _e63: f32 = level_1;
                    let _e67: vec2<f32> = xy_1;
                    let _e68: vec2<f32> = mip0_size_1;
                    let _e72: f32 = level_1;
                    let _e76: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(_e67, (_e68 / vec2(pow(2f, _e72))));
                    output = _e76;
                }
            } else {
                let _e77: f32 = level_1;
                if (_e77 == 3f) {
                    {
                        let _e81: vec2<f32> = mip0_size_1;
                        let _e85: f32 = level_1;
                        let _e89: vec2<f32> = xy_1;
                        let _e90: vec2<f32> = mip0_size_1;
                        let _e94: f32 = level_1;
                        let _e98: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(_e89, (_e90 / vec2(pow(2f, _e94))));
                        output = _e98;
                    }
                } else {
                    let _e99: f32 = level_1;
                    if (_e99 == 4f) {
                        {
                            let _e103: vec2<f32> = mip0_size_1;
                            let _e107: f32 = level_1;
                            let _e111: vec2<f32> = xy_1;
                            let _e112: vec2<f32> = mip0_size_1;
                            let _e116: f32 = level_1;
                            let _e120: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(_e111, (_e112 / vec2(pow(2f, _e116))));
                            output = _e120;
                        }
                    } else {
                        let _e121: f32 = level_1;
                        if (_e121 == 5f) {
                            {
                                let _e125: vec2<f32> = mip0_size_1;
                                let _e129: f32 = level_1;
                                let _e133: vec2<f32> = xy_1;
                                let _e134: vec2<f32> = mip0_size_1;
                                let _e138: f32 = level_1;
                                let _e142: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(_e133, (_e134 / vec2(pow(2f, _e138))));
                                output = _e142;
                            }
                        } else {
                            let _e143: f32 = level_1;
                            if (_e143 == 6f) {
                                {
                                    let _e147: vec2<f32> = mip0_size_1;
                                    let _e151: f32 = level_1;
                                    let _e155: vec2<f32> = xy_1;
                                    let _e156: vec2<f32> = mip0_size_1;
                                    let _e160: f32 = level_1;
                                    let _e164: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(_e155, (_e156 / vec2(pow(2f, _e160))));
                                    output = _e164;
                                }
                            } else {
                                {
                                    return vec4(0f);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e167: vec4<f32> = output;
    return _e167;
}

fn mc_GroupInstance_99_GroupInstance_97_MathClosure_95_(uv: vec2<f32>, xy: vec2<f32>, level: f32, mip0_size: vec2<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var level_1: f32;
    var mip0_size_1: vec2<f32>;
    var output: vec4<f32> = vec4(0f);

    uv_1 = uv;
    xy_1 = xy;
    level_1 = level;
    mip0_size_1 = mip0_size;
    let _e11: f32 = level_1;
    if (_e11 == 0f) {
        {
            let _e15: vec2<f32> = mip0_size_1;
            let _e19: f32 = level_1;
            let _e23: vec2<f32> = xy_1;
            let _e24: vec2<f32> = mip0_size_1;
            let _e28: f32 = level_1;
            let _e32: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(_e23, (_e24 / vec2(pow(2f, _e28))));
            output = _e32;
        }
    } else {
        let _e33: f32 = level_1;
        if (_e33 == 1f) {
            {
                let _e37: vec2<f32> = mip0_size_1;
                let _e41: f32 = level_1;
                let _e45: vec2<f32> = xy_1;
                let _e46: vec2<f32> = mip0_size_1;
                let _e50: f32 = level_1;
                let _e54: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(_e45, (_e46 / vec2(pow(2f, _e50))));
                output = _e54;
            }
        } else {
            let _e55: f32 = level_1;
            if (_e55 == 2f) {
                {
                    let _e59: vec2<f32> = mip0_size_1;
                    let _e63: f32 = level_1;
                    let _e67: vec2<f32> = xy_1;
                    let _e68: vec2<f32> = mip0_size_1;
                    let _e72: f32 = level_1;
                    let _e76: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(_e67, (_e68 / vec2(pow(2f, _e72))));
                    output = _e76;
                }
            } else {
                let _e77: f32 = level_1;
                if (_e77 == 3f) {
                    {
                        let _e81: vec2<f32> = mip0_size_1;
                        let _e85: f32 = level_1;
                        let _e89: vec2<f32> = xy_1;
                        let _e90: vec2<f32> = mip0_size_1;
                        let _e94: f32 = level_1;
                        let _e98: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(_e89, (_e90 / vec2(pow(2f, _e94))));
                        output = _e98;
                    }
                } else {
                    let _e99: f32 = level_1;
                    if (_e99 == 4f) {
                        {
                            let _e103: vec2<f32> = mip0_size_1;
                            let _e107: f32 = level_1;
                            let _e111: vec2<f32> = xy_1;
                            let _e112: vec2<f32> = mip0_size_1;
                            let _e116: f32 = level_1;
                            let _e120: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(_e111, (_e112 / vec2(pow(2f, _e116))));
                            output = _e120;
                        }
                    } else {
                        let _e121: f32 = level_1;
                        if (_e121 == 5f) {
                            {
                                let _e125: vec2<f32> = mip0_size_1;
                                let _e129: f32 = level_1;
                                let _e133: vec2<f32> = xy_1;
                                let _e134: vec2<f32> = mip0_size_1;
                                let _e138: f32 = level_1;
                                let _e142: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(_e133, (_e134 / vec2(pow(2f, _e138))));
                                output = _e142;
                            }
                        } else {
                            let _e143: f32 = level_1;
                            if (_e143 == 6f) {
                                {
                                    let _e147: vec2<f32> = mip0_size_1;
                                    let _e151: f32 = level_1;
                                    let _e155: vec2<f32> = xy_1;
                                    let _e156: vec2<f32> = mip0_size_1;
                                    let _e160: f32 = level_1;
                                    let _e164: vec4<f32> = sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(_e155, (_e156 / vec2(pow(2f, _e160))));
                                    output = _e164;
                                }
                            } else {
                                {
                                    return vec4(0f);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    let _e167: vec4<f32> = output;
    return _e167;
}

fn mc_GroupInstance_99_MathClosure_105(uv_in: vec2f, w_in: array<vec2f, 4>) -> vec2f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var output: vec2f;
    output = vec2f(w[2].x, w[2].y);
    return output;
}

fn mc_GroupInstance_99_MathClosure_106(uv_in: vec2f, w_in: array<vec2f, 4>) -> vec2f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var output: vec2f;
    output = vec2f(w[3].x, w[2].y);
    return output;
}

fn mc_GroupInstance_99_MathClosure_107(uv_in: vec2f, w_in: array<vec2f, 4>) -> vec2f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var output: vec2f;
    output = vec2f(w[2].x, w[3].y);
    return output;
}

fn mc_GroupInstance_99_MathClosure_108(uv_in: vec2f, w_in: array<vec2f, 4>) -> vec2f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var output: vec2f;
    output = vec2f(w[3].x, w[3].y);
    return output;
}

fn mc_GroupInstance_99_MathClosure_109(uv_in: vec2f, dc_in: vec2f, scale_in: f32) -> array<vec2f, 4> {
    var uv: vec2f = uv_in;
    var dc: vec2f = dc_in;
    var scale: f32 = scale_in;
    var output: array<vec2f, 4>;
    var d: vec2f = dc * scale - 0.5;
    var c: vec2f = floor(d);
    var x: vec2f = c - d + 1.0;
    var X: vec2f = d - c;
    var x3: vec2f = x * x * x;
    var coeff: vec2f = 0.5 * x * x + 0.5 * x + 0.166667;
    var w1: vec2f = -0.333333 * x3 + coeff;
    var w2: vec2f = 1.0 - w1;
    var o1: vec2f = (-0.5 * x3 + coeff) / w1 + c - 0.5;
    var o2: vec2f = (X * X * X / 6.0) / w2 + c + 1.5;
    output = array<vec2f, 4>(w1, w2, o1, o2);
    return output;
}

fn mc_GroupInstance_99_MathClosure_111_(uv: vec2<f32>, level: f32) -> f32 {
    var uv_1: vec2<f32>;
    var level_1: f32;
    var output: f32 = 0f;

    uv_1 = uv;
    level_1 = level;
    let _e11: f32 = level_1;
    output = (1f / pow(2f, _e11));
    let _e14: f32 = output;
    return _e14;
}

fn mc_GroupInstance_99_MathClosure_99(uv_in: vec2f, w_in: array<vec2f, 4>, c0_in: vec4f, c1_in: vec4f, c2_in: vec4f, c3_in: vec4f) -> vec4f {
    var uv: vec2f = uv_in;
    var w: array<vec2f, 4> = w_in;
    var c0: vec4f = c0_in;
    var c1: vec4f = c1_in;
    var c2: vec4f = c2_in;
    var c3: vec4f = c3_in;
    var output: vec4f;
    var o: vec4f = vec4f(0.0);
    o += w[0].x * w[0].y * c0;
    o += w[1].x * w[0].y * c1;
    o += w[0].x * w[1].y * c2;
    o += w[1].x * w[1].y * c3;
    output = o;
    return output;
}

fn mc_MathClosure_100_(uv: vec2<f32>, m: f32) -> f32 {
    var uv_1: vec2<f32>;
    var m_1: f32;
    var output: f32 = 0f;

    uv_1 = uv;
    m_1 = m;
    let _e8: f32 = m_1;
    output = floor(_e8);
    let _e10: f32 = output;
    return _e10;
}

fn mc_MathClosure_101_(uv: vec2<f32>, mLo: f32) -> f32 {
    var uv_1: vec2<f32>;
    var mLo_1: f32;
    var output: f32 = 0f;

    uv_1 = uv;
    mLo_1 = mLo;
    let _e7: f32 = mLo_1;
    output = (_e7 + 1f);
    let _e10: f32 = output;
    return _e10;
}

fn mc_MathClosure_102_(uv: vec2<f32>, mLo: f32, mHi: f32, m: f32, mip0_: vec4<f32>, cLo: vec4<f32>, cHi: vec4<f32>) -> vec4<f32> {
    var uv_1: vec2<f32>;
    var mLo_1: f32;
    var mHi_1: f32;
    var m_1: f32;
    var mip0_1: vec4<f32>;
    var cLo_1: vec4<f32>;
    var cHi_1: vec4<f32>;
    var output: vec4<f32> = vec4(0f);
    var scale: f32;

    uv_1 = uv;
    mLo_1 = mLo;
    mHi_1 = mHi;
    m_1 = m;
    mip0_1 = mip0_;
    cLo_1 = cLo;
    cHi_1 = cHi;
    let _e18: f32 = mLo_1;
    if (_e18 < 0.1f) {
        {
            let _e21: vec4<f32> = mip0_1;
            cLo_1 = _e21;
        }
    }
    let _e24: f32 = m_1;
    let _e25: f32 = mLo_1;
    let _e27: vec4<f32> = cLo_1;
    let _e28: vec4<f32> = cHi_1;
    let _e29: f32 = m_1;
    let _e30: f32 = mLo_1;
    output = mix(_e27, _e28, vec4((_e29 - _e30)));
    let _e34: vec4<f32> = output;
    return _e34;
}

fn mc_MathClosure_calculateMaskLinear(uv: vec2<f32>, xy: vec2<f32>, start_y: f32, end_y: f32, start_sigma: f32, end_sigma: f32) -> f32 {
    var uv_1: vec2<f32>;
    var xy_1: vec2<f32>;
    var start_y_1: f32;
    var end_y_1: f32;
    var start_sigma_1: f32;
    var end_sigma_1: f32;
    var output: f32 = 0f;
    var pStart: vec3<f32>;
    var pEnd: vec3<f32>;
    var qBase: vec2<f32>;
    var md: f32;
    var q: vec2<f32>;
    var p: f32;
    var m: f32;

    uv_1 = uv;
    xy_1 = xy;
    start_y_1 = start_y;
    end_y_1 = end_y;
    start_sigma_1 = start_sigma;
    end_sigma_1 = end_sigma;
    let _e16: f32 = start_y_1;
    let _e17: f32 = start_sigma_1;
    pStart = vec3<f32>(0f, _e16, _e17);
    let _e21: f32 = end_y_1;
    let _e22: f32 = end_sigma_1;
    pEnd = vec3<f32>(0f, _e21, _e22);
    let _e25: vec3<f32> = pEnd;
    let _e27: vec3<f32> = pStart;
    qBase = (_e25.xy - _e27.xy);
    let _e33: vec2<f32> = qBase;
    let _e34: vec2<f32> = qBase;
    md = dot(_e33, _e34);
    let _e37: vec2<f32> = xy_1;
    let _e38: vec3<f32> = pStart;
    q = (_e37 - _e38.xy);
    let _e44: vec2<f32> = q;
    let _e45: vec2<f32> = qBase;
    p = dot(_e44, _e45);
    let _e51: f32 = md;
    let _e53: f32 = p;
    m = smoothstep(_e51, 0f, _e53);
    let _e56: vec3<f32> = pEnd;
    let _e58: vec3<f32> = pStart;
    let _e60: vec3<f32> = pEnd;
    let _e63: f32 = m;
    m = (_e56.z + ((_e58.z - _e60.z) * _e63));
    let _e66: f32 = m;
    let _e69: f32 = m;
    m = log2((_e69 * 1.333333f));
    let _e76: f32 = m;
    m = clamp(_e76, 0f, 6f);
    let _e80: f32 = m;
    output = _e80;
    let _e81: f32 = output;
    return _e81;
}

fn sample_pass_sys_group_sampleFromMipmap_Downsample_mip1_(xy_in: vec2f, res_in: vec2f) -> vec4f {
    let uv = xy_in / res_in;
    return textureSample(pass_tex_sys_group_sampleFromMipmap_Downsample_mip1, pass_samp_sys_group_sampleFromMipmap_Downsample_mip1, uv);
}

fn sample_pass_sys_group_sampleFromMipmap_Downsample_mip2_(xy_in: vec2f, res_in: vec2f) -> vec4f {
    let uv = xy_in / res_in;
    return textureSample(pass_tex_sys_group_sampleFromMipmap_Downsample_mip2, pass_samp_sys_group_sampleFromMipmap_Downsample_mip2, uv);
}

fn sample_pass_sys_group_sampleFromMipmap_Downsample_mip3_(xy_in: vec2f, res_in: vec2f) -> vec4f {
    let uv = xy_in / res_in;
    return textureSample(pass_tex_sys_group_sampleFromMipmap_Downsample_mip3, pass_samp_sys_group_sampleFromMipmap_Downsample_mip3, uv);
}

fn sample_pass_sys_group_sampleFromMipmap_Downsample_mip4_(xy_in: vec2f, res_in: vec2f) -> vec4f {
    let uv = xy_in / res_in;
    return textureSample(pass_tex_sys_group_sampleFromMipmap_Downsample_mip4, pass_samp_sys_group_sampleFromMipmap_Downsample_mip4, uv);
}

fn sample_pass_sys_group_sampleFromMipmap_Downsample_mip5_(xy_in: vec2f, res_in: vec2f) -> vec4f {
    let uv = xy_in / res_in;
    return textureSample(pass_tex_sys_group_sampleFromMipmap_Downsample_mip5, pass_samp_sys_group_sampleFromMipmap_Downsample_mip5, uv);
}

fn sample_pass_sys_group_sampleFromMipmap_Downsample_mip6_(xy_in: vec2f, res_in: vec2f) -> vec4f {
    let uv = xy_in / res_in;
    return textureSample(pass_tex_sys_group_sampleFromMipmap_Downsample_mip6, pass_samp_sys_group_sampleFromMipmap_Downsample_mip6, uv);
}

fn sample_pass_sys_group_sampleFromMipmap_RenderPass_85_(xy_in: vec2f, res_in: vec2f) -> vec4f {
    let uv = xy_in / res_in;
    return textureSample(pass_tex_sys_group_sampleFromMipmap_RenderPass_85, pass_samp_sys_group_sampleFromMipmap_RenderPass_85, uv);
}


@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
        var mc_MathClosure_calculateMaskLinear_out: f32;
    {
        let xy = in.local_px.xy;
        let start_y = (graph_inputs.node_FloatInput_106_79ef3817).x;
        let end_y = (graph_inputs.node_FloatInput_107_c6ed3817).x;
        let start_sigma = (graph_inputs.node_FloatInput_108_77003917).x;
        let end_sigma = (graph_inputs.node_FloatInput_109_c4fe3817).x;
        var output: f32;
        output = mc_MathClosure_calculateMaskLinear(in.uv, xy, start_y, end_y, start_sigma, end_sigma);
        mc_MathClosure_calculateMaskLinear_out = output;
    }
    var mc_MathClosure_100_out: f32;
    {
        let m = mc_MathClosure_calculateMaskLinear_out;
        var output: f32;
        output = mc_MathClosure_100_(in.uv, m);
        mc_MathClosure_100_out = output;
    }
    var mc_MathClosure_101_out: f32;
    {
        let mLo = mc_MathClosure_100_out;
        var output: f32;
        output = mc_MathClosure_101_(in.uv, mLo);
        mc_MathClosure_101_out = output;
    }
    var mc_GroupInstance_111_MathClosure_95_out: vec4f;
    {
        let xy = in.local_px.xy;
        let level = (graph_inputs.node_FloatInput_113_a3d73517).x;
        let mip0_size = (graph_inputs.node_GroupInstance_111_Vector2Input_89_d77a1c71).xy;
        var output: vec4f;
        output = mc_GroupInstance_111_MathClosure_95_(in.uv, xy, level, mip0_size);
        mc_GroupInstance_111_MathClosure_95_out = output;
    }
    var mc_GroupInstance_99_MathClosure_111_out: f32;
    {
        let level = mc_MathClosure_100_out;
        var output: f32;
        output = mc_GroupInstance_99_MathClosure_111_(in.uv, level);
        mc_GroupInstance_99_MathClosure_111_out = output;
    }
    var mc_GroupInstance_99_MathClosure_109_out: array<vec2f, 4>;
    {
        let dc = in.local_px.xy;
        let scale = mc_GroupInstance_99_MathClosure_111_out;
        var output: array<vec2f, 4>;
        output = mc_GroupInstance_99_MathClosure_109(in.uv, dc, scale);
        mc_GroupInstance_99_MathClosure_109_out = output;
    }
    var mc_GroupInstance_99_MathClosure_105_out: vec2f;
    {
        let w = mc_GroupInstance_99_MathClosure_109_out;
        var output: vec2f;
        output = mc_GroupInstance_99_MathClosure_105(in.uv, w);
        mc_GroupInstance_99_MathClosure_105_out = output;
    }
    var mc_GroupInstance_99_GroupInstance_97_MathClosure_95_out: vec4f;
    {
        let xy = mc_GroupInstance_99_MathClosure_105_out;
        let level = mc_MathClosure_100_out;
        let mip0_size = (graph_inputs.node_GroupInstance_99_GroupInstance_97_Vector2Input_89_9c3bbd47).xy;
        var output: vec4f;
        output = mc_GroupInstance_99_GroupInstance_97_MathClosure_95_(in.uv, xy, level, mip0_size);
        mc_GroupInstance_99_GroupInstance_97_MathClosure_95_out = output;
    }
    var mc_GroupInstance_99_MathClosure_106_out: vec2f;
    {
        let w = mc_GroupInstance_99_MathClosure_109_out;
        var output: vec2f;
        output = mc_GroupInstance_99_MathClosure_106(in.uv, w);
        mc_GroupInstance_99_MathClosure_106_out = output;
    }
    var mc_GroupInstance_99_GroupInstance_102_MathClosure_95_out: vec4f;
    {
        let xy = mc_GroupInstance_99_MathClosure_106_out;
        let level = mc_MathClosure_100_out;
        let mip0_size = (graph_inputs.node_GroupInstance_99_GroupInstance_102_Vector2Input_89_21dd5194).xy;
        var output: vec4f;
        output = mc_GroupInstance_99_GroupInstance_102_MathClosure_95_(in.uv, xy, level, mip0_size);
        mc_GroupInstance_99_GroupInstance_102_MathClosure_95_out = output;
    }
    var mc_GroupInstance_99_MathClosure_107_out: vec2f;
    {
        let w = mc_GroupInstance_99_MathClosure_109_out;
        var output: vec2f;
        output = mc_GroupInstance_99_MathClosure_107(in.uv, w);
        mc_GroupInstance_99_MathClosure_107_out = output;
    }
    var mc_GroupInstance_99_GroupInstance_103_MathClosure_95_out: vec4f;
    {
        let xy = mc_GroupInstance_99_MathClosure_107_out;
        let level = mc_MathClosure_100_out;
        let mip0_size = (graph_inputs.node_GroupInstance_99_GroupInstance_103_Vector2Input_89_2261b0d2).xy;
        var output: vec4f;
        output = mc_GroupInstance_99_GroupInstance_103_MathClosure_95_(in.uv, xy, level, mip0_size);
        mc_GroupInstance_99_GroupInstance_103_MathClosure_95_out = output;
    }
    var mc_GroupInstance_99_MathClosure_108_out: vec2f;
    {
        let w = mc_GroupInstance_99_MathClosure_109_out;
        var output: vec2f;
        output = mc_GroupInstance_99_MathClosure_108(in.uv, w);
        mc_GroupInstance_99_MathClosure_108_out = output;
    }
    var mc_GroupInstance_99_GroupInstance_104_MathClosure_95_out: vec4f;
    {
        let xy = mc_GroupInstance_99_MathClosure_108_out;
        let level = mc_MathClosure_100_out;
        let mip0_size = (graph_inputs.node_GroupInstance_99_GroupInstance_104_Vector2Input_89_6b00a0e9).xy;
        var output: vec4f;
        output = mc_GroupInstance_99_GroupInstance_104_MathClosure_95_(in.uv, xy, level, mip0_size);
        mc_GroupInstance_99_GroupInstance_104_MathClosure_95_out = output;
    }
    var mc_GroupInstance_99_MathClosure_99_out: vec4f;
    {
        let w = mc_GroupInstance_99_MathClosure_109_out;
        let c0 = mc_GroupInstance_99_GroupInstance_97_MathClosure_95_out;
        let c1 = mc_GroupInstance_99_GroupInstance_102_MathClosure_95_out;
        let c2 = mc_GroupInstance_99_GroupInstance_103_MathClosure_95_out;
        let c3 = mc_GroupInstance_99_GroupInstance_104_MathClosure_95_out;
        var output: vec4f;
        output = mc_GroupInstance_99_MathClosure_99(in.uv, w, c0, c1, c2, c3);
        mc_GroupInstance_99_MathClosure_99_out = output;
    }
    var mc_GroupInstance_114_MathClosure_111_out: f32;
    {
        let level = mc_MathClosure_101_out;
        var output: f32;
        output = mc_GroupInstance_114_MathClosure_111_(in.uv, level);
        mc_GroupInstance_114_MathClosure_111_out = output;
    }
    var mc_GroupInstance_114_MathClosure_109_out: array<vec2f, 4>;
    {
        let dc = in.local_px.xy;
        let scale = mc_GroupInstance_114_MathClosure_111_out;
        var output: array<vec2f, 4>;
        output = mc_GroupInstance_114_MathClosure_109(in.uv, dc, scale);
        mc_GroupInstance_114_MathClosure_109_out = output;
    }
    var mc_GroupInstance_114_MathClosure_105_out: vec2f;
    {
        let w = mc_GroupInstance_114_MathClosure_109_out;
        var output: vec2f;
        output = mc_GroupInstance_114_MathClosure_105(in.uv, w);
        mc_GroupInstance_114_MathClosure_105_out = output;
    }
    var mc_GroupInstance_114_GroupInstance_97_MathClosure_95_out: vec4f;
    {
        let xy = mc_GroupInstance_114_MathClosure_105_out;
        let level = mc_MathClosure_101_out;
        let mip0_size = (graph_inputs.node_GroupInstance_114_GroupInstance_97_Vector2Input_89_0c3e8ae7).xy;
        var output: vec4f;
        output = mc_GroupInstance_114_GroupInstance_97_MathClosure_95_(in.uv, xy, level, mip0_size);
        mc_GroupInstance_114_GroupInstance_97_MathClosure_95_out = output;
    }
    var mc_GroupInstance_114_MathClosure_106_out: vec2f;
    {
        let w = mc_GroupInstance_114_MathClosure_109_out;
        var output: vec2f;
        output = mc_GroupInstance_114_MathClosure_106(in.uv, w);
        mc_GroupInstance_114_MathClosure_106_out = output;
    }
    var mc_GroupInstance_114_GroupInstance_102_MathClosure_95_out: vec4f;
    {
        let xy = mc_GroupInstance_114_MathClosure_106_out;
        let level = mc_MathClosure_101_out;
        let mip0_size = (graph_inputs.node_GroupInstance_114_GroupInstance_102_Vector2Input_89_11126536).xy;
        var output: vec4f;
        output = mc_GroupInstance_114_GroupInstance_102_MathClosure_95_(in.uv, xy, level, mip0_size);
        mc_GroupInstance_114_GroupInstance_102_MathClosure_95_out = output;
    }
    var mc_GroupInstance_114_MathClosure_107_out: vec2f;
    {
        let w = mc_GroupInstance_114_MathClosure_109_out;
        var output: vec2f;
        output = mc_GroupInstance_114_MathClosure_107(in.uv, w);
        mc_GroupInstance_114_MathClosure_107_out = output;
    }
    var mc_GroupInstance_114_GroupInstance_103_MathClosure_95_out: vec4f;
    {
        let xy = mc_GroupInstance_114_MathClosure_107_out;
        let level = mc_MathClosure_101_out;
        let mip0_size = (graph_inputs.node_GroupInstance_114_GroupInstance_103_Vector2Input_89_92ff9e15).xy;
        var output: vec4f;
        output = mc_GroupInstance_114_GroupInstance_103_MathClosure_95_(in.uv, xy, level, mip0_size);
        mc_GroupInstance_114_GroupInstance_103_MathClosure_95_out = output;
    }
    var mc_GroupInstance_114_MathClosure_108_out: vec2f;
    {
        let w = mc_GroupInstance_114_MathClosure_109_out;
        var output: vec2f;
        output = mc_GroupInstance_114_MathClosure_108(in.uv, w);
        mc_GroupInstance_114_MathClosure_108_out = output;
    }
    var mc_GroupInstance_114_GroupInstance_104_MathClosure_95_out: vec4f;
    {
        let xy = mc_GroupInstance_114_MathClosure_108_out;
        let level = mc_MathClosure_101_out;
        let mip0_size = (graph_inputs.node_GroupInstance_114_GroupInstance_104_Vector2Input_89_dbee00ee).xy;
        var output: vec4f;
        output = mc_GroupInstance_114_GroupInstance_104_MathClosure_95_(in.uv, xy, level, mip0_size);
        mc_GroupInstance_114_GroupInstance_104_MathClosure_95_out = output;
    }
    var mc_GroupInstance_114_MathClosure_99_out: vec4f;
    {
        let w = mc_GroupInstance_114_MathClosure_109_out;
        let c0 = mc_GroupInstance_114_GroupInstance_97_MathClosure_95_out;
        let c1 = mc_GroupInstance_114_GroupInstance_102_MathClosure_95_out;
        let c2 = mc_GroupInstance_114_GroupInstance_103_MathClosure_95_out;
        let c3 = mc_GroupInstance_114_GroupInstance_104_MathClosure_95_out;
        var output: vec4f;
        output = mc_GroupInstance_114_MathClosure_99(in.uv, w, c0, c1, c2, c3);
        mc_GroupInstance_114_MathClosure_99_out = output;
    }
    var mc_MathClosure_102_out: vec4f;
    {
        let mLo = mc_MathClosure_100_out;
        let mHi = mc_MathClosure_101_out;
        let m = mc_MathClosure_calculateMaskLinear_out;
        let mip0 = mc_GroupInstance_111_MathClosure_95_out;
        let cLo = mc_GroupInstance_99_MathClosure_99_out;
        let cHi = mc_GroupInstance_114_MathClosure_99_out;
        var output: vec4f;
        output = mc_MathClosure_102_(in.uv, mLo, mHi, m, mip0, cLo, cHi);
        mc_MathClosure_102_out = output;
    }
    return mc_MathClosure_102_out;
}
