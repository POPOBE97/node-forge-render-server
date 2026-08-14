// MeshGradient fragment template.

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return vec4f(in.color.rgb * in.color.a, in.color.a);
}
