
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
    @location(2) local_px: vec2f,
    // Geometry size in pixels after applying geometry/instance transforms.
    @location(3) geo_size_px: vec2f,
};


@group(1) @binding(0)

var src_tex: texture_2d<f32>;
@group(1) @binding(1)
var src_samp: sampler;

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    
    let src_dims_u = textureDimensions(src_tex);
    let src_dims = vec2f(src_dims_u);
    let dst_dims = params.target_size;
    // Use in.uv (top-left convention) to map directly to source pixel space.
    let center_xy = in.uv * src_dims;

  let kw: i32 = 3;
  let kh: i32 = 3;
  let half_w: i32 = kw / 2;
  let half_h: i32 = kh / 2;
  let k = array<f32, 9>(0, 0.25, 0, 0.25, 0, 0.25, 0, 0.25, 0);

    var sum = vec4f(0.0);
    for (var y: i32 = 0; y < kh; y = y + 1) {
        for (var x: i32 = 0; x < kw; x = x + 1) {
            let ix = x - half_w;
            let iy = y - half_h;
            // Offset from integer center.
            let sample_xy = center_xy + vec2f(f32(ix), f32(iy));
            // Sample at integer-coord / src_dims (texel boundary).
            // With a linear sampler this gives a proper 2x2 bilinear average,
            // matching Godot's manual bilinear() at integer coordinates.
            let uv = sample_xy / src_dims;

            let idx: i32 = y * kw + x;
            sum = sum + textureSampleLevel(src_tex, src_samp, uv, 0.0) * k[u32(idx)];
        }
    }
    return sum;
  
}
