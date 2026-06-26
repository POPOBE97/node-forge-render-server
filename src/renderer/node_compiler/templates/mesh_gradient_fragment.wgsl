// MeshGradient fragment template.

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    return in.color;
}
