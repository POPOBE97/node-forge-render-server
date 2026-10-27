
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
  };


struct GraphInputs {
    // Node: chart.patterns/dp.chart.pattern.iconInsetPx
    float_input_dp: vec4f,
    // Node: chart.patterns/dp.chart.pattern.iconRadiusPx
    float_input_dp_84690123: vec4f,
    // Node: chart.show_colors
    chart_show_colors: vec4i,
    // Node: chart.show_grid
    chart_show_grid: vec4i,
    // Node: chart.show_patterns
    chart_show_patterns: vec4i,
};

@group(0) @binding(2)
var<uniform> graph_inputs: GraphInputs;

@group(0) @binding(1)
var<storage, read> baked_data_parse: array<vec4f>;
@group(1) @binding(0)
var pass_tex_chart_patterns_chart_blur_vertical_sampler: texture_2d<f32>;

@group(1) @binding(1)
var pass_samp_chart_patterns_chart_blur_vertical_sampler: sampler;


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

// Public coordinates are LocalPx/LocalUV, bottom-left origin. These helpers
// never convert scene coordinates or texture-storage coordinates.
fn inside_chart_patterns_chart_pattern_material(p: vec2f, box: vec4f) -> bool {
  return all(p >= box.xy) && all(p < box.zw);
}

fn fill_coverage_chart_patterns_chart_pattern_material(distance: f32) -> f32 {
  return clamp(0.5 - distance, 0.0, 1.0);
}

fn stroke_coverage_chart_patterns_chart_pattern_material(distance: f32, width: f32) -> f32 {
  return fill_coverage_chart_patterns_chart_pattern_material(distance - width * 0.5) - fill_coverage_chart_patterns_chart_pattern_material(distance + width * 0.5);
}

fn circle_mask_chart_patterns_chart_pattern_material(p: vec2f) -> f32 {
  let r = length(p);
  // Exact source radii, in physical pixels. The rings remain the same size
  // when the chart layout changes resolution.
  return max(fill_coverage_chart_patterns_chart_pattern_material(r - 5.833333333), max(
    stroke_coverage_chart_patterns_chart_pattern_material(r - 14.583333333, 5.0), max(
    stroke_coverage_chart_patterns_chart_pattern_material(r - 26.25, 5.0), stroke_coverage_chart_patterns_chart_pattern_material(r - 37.916666667, 5.0))));
}

fn blur_sigma_chart_patterns_chart_pattern_material(index: i32) -> f32 {
  // True Gaussian sigma in pixels, calibrated against Figma's NORMAL layer
  // blur. The source radius labels are 0, 4, 8, 16, 32, 64; they are not sigma.
  var values = array<f32, 6>(0.0, 1.70, 3.52, 6.80, 13.44, 26.88);
  return values[clamp(index, 0, 5)];
}

fn segment_distance_chart_patterns_chart_pattern_material(p: vec2f, a: vec2f, b: vec2f) -> f32 {
  let ab = b - a;
  return length(p - a - ab * clamp(dot(p - a, ab) / max(dot(ab, ab), 1e-9), 0.0, 1.0));
}

// Equal-radius Figma continuous-corner SDF, adapted from figma-squircle 1.1.0
// (MIT, Copyright (c) 2021 Tien Pham). See LICENSE.figma-squircle.
// Cubic + circular arc, not a superellipse fit.
fn cubic_point_chart_patterns_chart_pattern_material(t: f32, p0: vec2f, p1: vec2f, p2: vec2f, p3: vec2f) -> vec2f {
  let s = 1.0 - t;
  return s*s*s*p0 + 3.0*s*s*t*p1 + 3.0*s*t*t*p2 + t*t*t*p3;
}

fn smooth_rect_sdf_chart_patterns_chart_pattern_material(point: vec2f, half_size: vec2f, radius: f32, smoothing: f32) -> f32 {
  let r = max(0.001, min(radius, min(half_size.x, half_size.y)));
  let smoothing_factor = min(smoothing, min(half_size.x, half_size.y) / r - 1.0);
  let extent = (1.0 + smoothing_factor) * r;
  var p = abs(point);
  var b = half_size;
  if (p.x - b.x > p.y - b.y) { p = p.yx; b = b.yx; }
  if (p.x <= b.x - extent) { return p.y - b.y; }

  let beta = 0.7853981633974483 * smoothing_factor;
  let arc = sin(0.7853981633974483 * (1.0 - smoothing_factor)) * r * 1.4142135623730951;
  let c = r * tan(beta * 0.5) * cos(beta);
  let d = c * tan(beta);
  let handle_b = (extent - arc - c - d) / 3.0;
  let a = 2.0 * handle_b;
  let center = b - vec2f(r);
  let v = p - center;
  if (atan2(v.y, v.x) <= 1.5707963267948966 - beta) {
    return length(v) - r;
  }

  let p0 = vec2f(b.x - extent, b.y);
  let p1 = p0 + vec2f(a, 0.0);
  let p2 = p0 + vec2f(a + handle_b, 0.0);
  let p3 = p0 + vec2f(a + handle_b + c, -d);
  var t = clamp((p.x - p0.x) / max(p3.x - p0.x, 0.001), 0.0, 1.0);
  for (var iteration = 0; iteration < 6; iteration = iteration + 1) {
    let s = 1.0 - t;
    let q = cubic_point_chart_patterns_chart_pattern_material(t, p0, p1, p2, p3);
    let first = 3.0*s*s*(p1-p0) + 6.0*s*t*(p2-p1) + 3.0*t*t*(p3-p2);
    let second = 6.0*s*(p2-2.0*p1+p0) + 6.0*t*(p3-2.0*p2+p1);
    let denom = dot(first, first) + dot(q-p, second);
    if (abs(denom) > 0.000001) { t = clamp(t - dot(q-p, first) / denom, 0.0, 1.0); }
  }
  let q = cubic_point_chart_patterns_chart_pattern_material(t, p0, p1, p2, p3);
  let s = 1.0 - t;
  let tangent = 3.0*s*s*(p1-p0) + 6.0*s*t*(p2-p1) + 3.0*t*t*(p3-p2);
  let delta = p - q;
  return length(delta) * select(-1.0, 1.0, dot(delta, vec2f(-tangent.y, tangent.x)) >= 0.0);
}

fn smooth_rect_sdf_coverage_chart_patterns_chart_pattern_material(distance: f32) -> f32 {
  // A 1.5px antialias interval centered on the SDF zero level set.
  return clamp(0.5 - distance / 1.5, 0.0, 1.0);
}

fn grid_x_chart_patterns_chart_pattern_material(size: vec2f, column: i32) -> f32 { return round(size.x * f32(column) / 9.0); }
fn grid_y_chart_patterns_chart_pattern_material(size: vec2f, row: i32) -> f32 { return round(size.y * f32(row) / 20.0); }
fn cell_box_chart_patterns_chart_pattern_material(size: vec2f, x0: i32, y0: i32, x1: i32, y1: i32) -> vec4f {
  return vec4f(grid_x_chart_patterns_chart_pattern_material(size, x0), grid_y_chart_patterns_chart_pattern_material(size, y0), grid_x_chart_patterns_chart_pattern_material(size, x1), grid_y_chart_patterns_chart_pattern_material(size, y1));
}
fn inset_box_chart_patterns_chart_pattern_material(box: vec4f) -> vec4f { return box + vec4f(1.0, 1.0, -1.0, -1.0); }

fn frequency_mask_chart_patterns_chart_pattern_material(x: f32) -> f32 {
  // Positions and coverage of the Figma source's 1px / 0.5px lines.
  var columns = array<f32, 10>(14.0,24.0,34.0,43.0,52.0,60.0,68.0,75.0,82.0,88.0);
  for (var i = 0; i < 10; i = i + 1) {
    if (floor(x) == columns[i]) { return select(1.0, 0.5, i % 2 == 1); }
  }
  if (floor(x) == 96.0 || floor(x) == 101.0 || floor(x) == 105.0 ||
      floor(x) == 108.0 || floor(x) == 111.0 || floor(x) == 114.0 ||
      (x >= 116.0 && x < 134.0)) { return 0.5; }
  return 0.0;
}

fn triangle_wave_chart_patterns_chart_pattern_material(value: f32) -> f32 { return 1.0 - abs(fract(value) * 2.0 - 1.0); }
fn triangle_pixel_chart_patterns_chart_pattern_material(value: f32) -> f32 {
  // Integrate across the pixel footprint, including the triangular peaks.
  return (triangle_wave_chart_patterns_chart_pattern_material((value-0.375)/9.0)+triangle_wave_chart_patterns_chart_pattern_material((value-0.125)/9.0)+
          triangle_wave_chart_patterns_chart_pattern_material((value+0.125)/9.0)+triangle_wave_chart_patterns_chart_pattern_material((value+0.375)/9.0))*0.25;
}

fn diagonal_coverage_chart_patterns_chart_pattern_material(p: vec2f, a: vec2f, b: vec2f) -> f32 {
  // Figma's diagonal coverage matches a 4x4 sample grid. Keep the actual
  // 1px stroke and butt caps, rather than softening the complete chart.
  let tangent = normalize(b-a);
  let normal = vec2f(-tangent.y,tangent.x);
  let line_length = length(b-a);
  var coverage = 0.0;
  for (var y = 0; y < 4; y = y + 1) {
    for (var x = 0; x < 4; x = x + 1) {
      let q = p + (vec2f(f32(x),f32(y))+0.5)/4.0-0.5-a;
      if (abs(dot(q,normal)) <= 0.5 && dot(q,tangent) >= 0.0 && dot(q,tangent) <= line_length) {
        coverage += 1.0/16.0;
      }
    }
  }
  return coverage;
}

fn axis_line_coverage_chart_patterns_chart_pattern_material(value: f32, center: f32, along: f32, start: f32, end: f32) -> f32 {
  return stroke_coverage_chart_patterns_chart_pattern_material(value-center,1.0)*
    clamp(min(along-start+0.5,end-along+0.5),0.0,1.0);
}

fn shader_material_chart_patterns_chart_pattern_material(in: ShaderMaterialInput, blur: texture_2d<f32>, blur_sampler: sampler,
  show_grid: bool, show_patterns: bool, show_colors: bool, iconInsetPx: f32, iconRadiusPx: f32) -> vec4f {
  let p = in.local_position.xy;
  let size = in.target_size;
  // Figma encoded gray 0.39073893427848816, decoded once when authoring.
  let background = vec3f(0.12646998463008155);
  var rgb = background;

  // All layout coordinates are Y-up. Pixel edges are rounded once and shared.
  if (show_grid) {
    var grid = false;
    for (var x = 0; x <= 9; x = x + 1) { grid = grid || abs(p.x - grid_x_chart_patterns_chart_pattern_material(size, x)) < 1.0; }
    for (var y = 0; y <= 20; y = y + 1) { grid = grid || abs(p.y - grid_y_chart_patterns_chart_pattern_material(size, y)) < 1.0; }
    if (grid) { rgb = vec3f(1.0); }
    let radius = min(size.x, size.y) * 0.5;
    let center = vec2f(size.x * 0.5, size.y - radius);
    rgb = mix(rgb, vec3f(1.0), stroke_coverage_chart_patterns_chart_pattern_material(length(p-center) - radius, 2.0));
  }

  // Source content cards mask the underlying grid; text is added later as
  // independent TextPass layers, without altering the pattern material.
  var cards = array<vec4i, 6>(vec4i(2,18,7,19),vec4i(3,17,6,18),vec4i(4,16,5,17),
    vec4i(4,13,8,16),vec4i(0,8,9,10),vec4i(3,0,6,1));
  for (var i = 0; i < 6; i = i + 1) {
    let c = cards[i];
    if (inside_chart_patterns_chart_pattern_material(p, inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,c.x,c.y,c.z,c.w)))) { rgb = background; }
  }

  if (show_patterns) {
    // Pixel line and triangular-wave patches. Their pitch is fixed in pixels.
    var patches = array<vec2i,5>(vec2i(1,17),vec2i(2,17),vec2i(2,16),vec2i(6,17),vec2i(6,16));
    for (var i = 0; i < 5; i = i + 1) {
      let c = patches[i];
      let box = inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,c.x,c.y,c.x+1,c.y+1));
      if (inside_chart_patterns_chart_pattern_material(p, box)) {
        rgb = vec3f(0.0);
        let q = p - box.xy;
        let bounds = vec4f(4.0,4.0,box.z-box.x-4.0,box.w-box.y-4.0);
        if (inside_chart_patterns_chart_pattern_material(q,bounds)) {
          if (i == 0) { rgb = vec3f(1.0-frequency_mask_chart_patterns_chart_pattern_material(q.x)); }
          if (i == 1) { rgb = vec3f(select(0.0,1.0,i32(floor(q.x-4.0)) % 2 == 0)); }
          if (i == 2) { rgb = vec3f(select(0.0,1.0,i32(floor(box.w-p.y-4.0)) % 2 == 0)); }
        }
        if (i == 3 && inside_chart_patterns_chart_pattern_material(q,vec4f(6.0,4.0,box.z-box.x-6.0,box.w-box.y-4.0))) {
          rgb = vec3f(triangle_pixel_chart_patterns_chart_pattern_material(q.x-6.0));
        }
        if (i == 4 && inside_chart_patterns_chart_pattern_material(q,vec4f(4.0,6.0,box.z-box.x-4.0,box.w-box.y-6.0))) {
          rgb = vec3f(triangle_pixel_chart_patterns_chart_pattern_material(box.w-p.y-6.0));
        }
      }
    }

    // Smooth, filled placeholder generated from the continuous-corner SDF.
    let icon = inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,4,16,5,17));
    if (inside_chart_patterns_chart_pattern_material(p,icon)) {
      let half_size = vec2f(min(icon.z-icon.x,icon.w-icon.y)*0.5-iconInsetPx);
      let sdf = smooth_rect_sdf_chart_patterns_chart_pattern_material(p-(icon.xy+icon.zw)*0.5,half_size,min(iconRadiusPx,min(half_size.x,half_size.y)),0.6);
      rgb = mix(rgb,vec3f(1.0),smooth_rect_sdf_coverage_chart_patterns_chart_pattern_material(sdf));
    }

    // Source frame/cross use integer stroke centers: a 1px stroke deliberately
    // covers two pixels by 50%. This is distinct from the crisp stripe patches.
    let align_box = inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,7,17,8,18));
    if (inside_chart_patterns_chart_pattern_material(p,align_box)) {
      rgb = background;
      let a = align_box.xy + vec2f(6.0,6.0);
      let b = align_box.zw - vec2f(6.0,6.0);
      let center = floor((a+b)*0.5);
      var coverage = array<f32,8>(
        axis_line_coverage_chart_patterns_chart_pattern_material(p.y,b.y,p.x,a.x,b.x),
        axis_line_coverage_chart_patterns_chart_pattern_material(p.x,b.x,p.y,a.y,b.y),
        axis_line_coverage_chart_patterns_chart_pattern_material(p.y,a.y,p.x,a.x,b.x),
        axis_line_coverage_chart_patterns_chart_pattern_material(p.x,a.x,p.y,a.y,b.y),
        axis_line_coverage_chart_patterns_chart_pattern_material(p.x,center.x,p.y,a.y,b.y),
        axis_line_coverage_chart_patterns_chart_pattern_material(p.y,center.y,p.x,a.x,b.x),
        diagonal_coverage_chart_patterns_chart_pattern_material(p,a,b),
        diagonal_coverage_chart_patterns_chart_pattern_material(p,vec2f(a.x,b.y),vec2f(b.x,a.y)));
      for (var i = 0; i < 8; i = i + 1) { rgb = mix(rgb,vec3f(1.0),coverage[i]); }
    }

    // Native-resolution Gaussian samples: no scaling of the generated atlas.
    for (var column = 1; column <= 2; column = column + 1) {
      for (var row = 13; row <= 15; row = row + 1) {
        let box = inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,column,row,column+1,row+1));
        if (inside_chart_patterns_chart_pattern_material(p,box)) {
          let index = select(15-row,row-10,column==2);
          let center = floor((box.xy+box.zw)*0.5);
          let offset = p-center;
          let uv = (offset+vec2f(f32(index)*320.0+160.0,160.0))/vec2f(1920.0,320.0);
          let value = sample_texture_local_uv(blur,blur_sampler,uv).r;
          // Large output cells must not sample a neighboring atlas tile.
          rgb = vec3f(select(0.0,value,all(abs(offset)<vec2f(160.0))));
        }
      }
    }

    // Seven OUTSIDE strokes use the same smooth-corner SDF and 1.5px antialias
    // interval. Geometry scales to its allocation; stroke widths do not.
    let corner_box = inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,4,13,8,16));
    if (inside_chart_patterns_chart_pattern_material(p,corner_box) && p.x >= grid_x_chart_patterns_chart_pattern_material(size,5)) {
      let center = vec2f(grid_x_chart_patterns_chart_pattern_material(size,5),grid_y_chart_patterns_chart_pattern_material(size,13));
      let scale = min((grid_x_chart_patterns_chart_pattern_material(size,8)-center.x)/420.0,(grid_y_chart_patterns_chart_pattern_material(size,16)-center.y)/420.0);
      var radii = array<f32,7>(198.333333333,175.0,151.666666667,128.333333333,105.0,81.666666667,58.333333333);
      var widths = array<f32,7>(0.25,0.5,1.0,2.0,4.0,8.0,16.0);
      var coverage = 0.0;
      for (var i = 0; i < 7; i = i + 1) {
        let r = radii[i]*scale;
        let sdf = smooth_rect_sdf_chart_patterns_chart_pattern_material(p-center,vec2f(2.0*r),r,0.6);
        coverage = max(coverage,smooth_rect_sdf_coverage_chart_patterns_chart_pattern_material(sdf-widths[i])-smooth_rect_sdf_coverage_chart_patterns_chart_pattern_material(sdf));
      }
      rgb = mix(rgb,vec3f(1.0),coverage);
    }
  }

  if (show_colors) {
    // These reference patches are authored in linear Display P3. The document
    // uses that working space; changing it reinterprets all numeric colors.
    // Top pair: identical RGB triplets in sRGB (upper) and Display P3 (lower).
    // This matrix converts only the tagged sRGB reference, never the fragment.
    var gamut_colors = array<vec3f,9>(vec3f(1.0),vec3f(1.0,0.0,0.0),vec3f(1.0,1.0,0.0),
      vec3f(0.0,1.0,0.0),vec3f(0.0,1.0,1.0),vec3f(0.0,0.0,1.0),
      vec3f(1.0,0.0,1.0),vec3f(1.0,0.25,0.0),vec3f(0.25,1.0,0.0));
    let srgb_to_p3 = mat3x3f(vec3f(0.82246196,0.0331942,0.01708263),
      vec3f(0.17753804,0.9668058,0.07239744),vec3f(0.0,0.0,0.9105199));
    // Keep the three color rows directly above the gray steps. Every cell is
    // filled edge-to-edge inside its grid border, with no seam between halves.
    var hdr_levels = array<f32,9>(0.0,0.25,0.5,0.75,1.0,1.1,1.25,1.4,1.5);
    for (var i = 0; i < 9; i = i + 1) {
      let gamut = inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,i,6,i+1,7));
      if (inside_chart_patterns_chart_pattern_material(p,gamut)) {
        let middle = (gamut.y+gamut.w)*0.5;
        rgb = select(gamut_colors[i],srgb_to_p3*gamut_colors[i],p.y >= middle);
      }
      let white = inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,i,5,i+1,6));
      if (inside_chart_patterns_chart_pattern_material(p,white)) {
        rgb = vec3f(hdr_levels[i]);
      }
      let hdr = inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,i,4,i+1,5));
      if (inside_chart_patterns_chart_pattern_material(p,hdr)) {
        let middle = (hdr.y+hdr.w)*0.5;
        rgb = gamut_colors[i]*select(1.5,1.25,p.y >= middle);
      }
    }
    // Authored encoded levels [0,32,64,96,128,160,192,224,255]/255,
    // decoded offline. Coverage and procedural interpolation remain linear.
    var grays = array<f32,9>(0.0,0.014443843596092545,0.051269458374043238,
      0.11697066775851084,0.21586050011389926,0.35153259950043936,
      0.52711512570581309,0.74540420954038744,1.0);
    for (var i = 0; i < 9; i = i + 1) {
      if (inside_chart_patterns_chart_pattern_material(p,inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,i,3,i+1,4)))) { rgb = vec3f(grays[i]); }
    }
    for (var row = 1; row <= 2; row = row + 1) {
      let box = inset_box_chart_patterns_chart_pattern_material(cell_box_chart_patterns_chart_pattern_material(size,0,row,9,row+1));
      if (inside_chart_patterns_chart_pattern_material(p,box)) {
        let ramp = 1.0 - (p.x-box.x)/(box.z-box.x);
        let upper = p.y >= (box.y+box.w)*0.5;
        if (row == 2) { rgb = select(vec3f(0.0,ramp,0.0),vec3f(ramp,0.0,0.0),upper); }
        else { rgb = select(vec3f(ramp),vec3f(0.0,0.0,ramp),upper); }
      }
    }
  }
  // Procedural RGB is linear in the selected working color space.
  return vec4f(rgb,1.0);
}


 @vertex
 fn vs_main(
     @location(0) position: vec3f,
     @location(1) uv: vec2f,
 ) -> VSOut {
 var out: VSOut;

 let _unused_geo_size = params.geo_size;
 let _unused_geo_translate = params.geo_translate;
 let _unused_geo_scale = params.geo_scale;

 // UV passed as vertex attribute.
 out.uv = vec2f(uv.x, 1.0 - uv.y);

 out.geo_size_px = params.geo_size;
 // Geometry-local pixel coordinate (GeoFragcoord).
 out.local_px = vec3f(uv * out.geo_size_px, 0.0);

 var p_local = position;

 // Geometry vertices are in local pixel units centered at (0,0).
 // Convert to target pixel coordinates with bottom-left origin.
 out.local_px = vec3f(out.local_px.xy, p_local.z);
 let p_px = params.center + p_local.xy;

 out.position = params.camera * vec4f(p_px, p_local.z, 1.0);

 // Pixel-centered like GLSL gl_FragCoord.xy.
 out.frag_coord_gl = p_px + vec2f(0.5, 0.5);
 return out;
 }
@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
    // Shader Material chart.patterns/chart.pattern.material.material
    let show_patterns_material = shader_material_chart_patterns_chart_pattern_material(
        ShaderMaterialInput(vec2f(in.uv.x, 1.0 - in.uv.y), in.frag_coord_gl, in.local_px, in.geo_size_px, params.target_size, params.time),
        pass_tex_chart_patterns_chart_blur_vertical_sampler,
        pass_samp_chart_patterns_chart_blur_vertical_sampler,
        ((graph_inputs.chart_show_grid).x != 0),
        ((graph_inputs.chart_show_patterns).x != 0),
        ((graph_inputs.chart_show_colors).x != 0),
        (graph_inputs.float_input_dp).x,
        (graph_inputs.float_input_dp_84690123).x,
    );
    // Final composite
    let _frag_out = show_patterns_material;
    return vec4f(_frag_out.rgb, clamp(_frag_out.a, 0.0, 1.0));
}
