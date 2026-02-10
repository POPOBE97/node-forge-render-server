
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

 @vertex
  fn vs_main(@location(0) position: vec3f, @location(1) uv: vec2f) -> VSOut {
      var out: VSOut;
 
      let _unused_geo_size = params.geo_size;
      let _unused_geo_translate = params.geo_translate;
     let _unused_geo_scale = params.geo_scale;
 
        // UV passed as vertex attribute.
        out.uv = uv;

        out.geo_size_px = params.geo_size;

         // Geometry-local pixel coordinate (GeoFragcoord).
         out.local_px = uv * out.geo_size_px;
 
       // Geometry vertices are in local pixel units centered at (0,0).
       // Convert to target pixel coordinates with bottom-left origin.
       let p_px = params.center + position.xy;



     // Convert pixels to clip space assuming bottom-left origin.
     // (0,0) => (-1,-1), (target_size) => (1,1)
     let ndc = (p_px / params.target_size) * 2.0 - vec2f(1.0, 1.0);
     out.position = vec4f(ndc, position.z, 1.0);

      // Pixel-centered like GLSL gl_FragCoord.xy.
      out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
      return out;
  }
  
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    
    let src_dims_u = textureDimensions(src_tex);
    let src_dims = vec2f(src_dims_u);
    let dst_dims = params.target_size;
    // Fragment position is pixel-centered, with top-left origin.
    let dst_xy = vec2f(in.position.xy);
    
    // Map destination pixel to source integer grid via ceil, matching Godot's
    // downsample shader: center_xy = ceil(UV * src_resolution).
    // With UV = dst_xy / dst_dims: center_xy = ceil(dst_xy * src_dims / dst_dims).
    let center_xy = dst_xy * src_dims / dst_dims;

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
