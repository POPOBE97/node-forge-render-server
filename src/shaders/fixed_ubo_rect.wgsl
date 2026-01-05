struct Params {
  // Scale in NDC units (applied to geometry position.xy)
  scale: vec2f,
  time: f32,
  material_kind: u32,
  color: vec4f,
};

@group(0) @binding(0)
var<uniform> params: Params;

struct VSOut {
  @builtin(position) position: vec4f,
  @location(0) uv: vec2f,
};

@vertex
fn vs_main(@location(0) position: vec3f) -> VSOut {
  var out: VSOut;

  // Synthesize UV from the *unscaled* input position.
  out.uv = position.xy * 0.5 + vec2f(0.5, 0.5);

  let scaled = vec2f(position.x * params.scale.x, position.y * params.scale.y);
  out.position = vec4f(scaled, position.z, 1.0);
  return out;
}

fn animated_color(c: vec4f, t: f32) -> vec4f {
  // Minimal animation hook: modulate brightness by time.
  let k = 0.6 + 0.4 * sin(t);
  return vec4f(c.rgb * k, c.a);
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
  // material_kind:
  // 0 = constant color (params.color)
  // 1 = UV debug (vec4(uv, 0, 1))
  if (params.material_kind == 1u) {
    return vec4f(in.uv.x, in.uv.y, 0.0, 1.0);
  }

  return animated_color(params.color, params.time);
}
