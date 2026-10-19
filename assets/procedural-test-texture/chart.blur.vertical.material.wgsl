// Public coordinates are LocalPx/LocalUV, bottom-left origin. These helpers
// never convert scene coordinates or texture-storage coordinates.
fn inside(p: vec2f, box: vec4f) -> bool {
  return all(p >= box.xy) && all(p < box.zw);
}

fn fill_coverage(distance: f32) -> f32 {
  return clamp(0.5 - distance, 0.0, 1.0);
}

fn stroke_coverage(distance: f32, width: f32) -> f32 {
  return fill_coverage(distance - width * 0.5) - fill_coverage(distance + width * 0.5);
}

fn circle_mask(p: vec2f) -> f32 {
  let r = length(p);
  // Exact source radii, in physical pixels. The rings remain the same size
  // when the chart layout changes resolution.
  return max(fill_coverage(r - 5.833333333), max(
    stroke_coverage(r - 14.583333333, 5.0), max(
    stroke_coverage(r - 26.25, 5.0), stroke_coverage(r - 37.916666667, 5.0))));
}

fn blur_sigma(index: i32) -> f32 {
  // True Gaussian sigma in pixels, calibrated against Figma's NORMAL layer
  // blur. The source radius labels are 0, 4, 8, 16, 32, 64; they are not sigma.
  var values = array<f32, 6>(0.0, 1.70, 3.52, 6.80, 13.44, 26.88);
  return values[clamp(index, 0, 5)];
}

fn segment_distance(p: vec2f, a: vec2f, b: vec2f) -> f32 {
  let ab = b - a;
  return length(p - a - ab * clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-9), 0.0, 1.0));
}

fn shader_material(in: ShaderMaterialInput, horizontal: texture_2d<f32>, horizontal_sampler: sampler) -> vec4f {
  let tile = i32(floor(in.local_position.x / 320.0));
  let sigma = blur_sigma(tile);
  if (sigma < 0.01) {
    return sample_texture_local_uv(horizontal, horizontal_sampler, in.uv);
  }
  let radius = i32(ceil(4.0 * sigma));
  var value = 0.0;
  var total = 0.0;
  for (var i = -radius; i <= radius; i = i + 1) {
    let weight = exp(-0.5 * f32(i * i) / (sigma * sigma));
    let uv = in.uv + vec2f(0.0, f32(i) / 320.0);
    let sample = sample_texture_local_uv(horizontal, horizontal_sampler, uv).r;
    value += select(0.0, sample, uv.y >= 0.0 && uv.y <= 1.0) * weight;
    total += weight;
  }
  return vec4f(value / total, 0.0, 0.0, 1.0);
}
