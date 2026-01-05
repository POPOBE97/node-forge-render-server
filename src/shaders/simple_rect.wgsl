struct VSOut {
  @builtin(position) position: vec4f,
};

@vertex
fn vs_main(@location(0) position: vec3f) -> VSOut {
  var out: VSOut;
  out.position = vec4f(position, 1.0);
  return out;
}

@fragment
fn fs_main() -> @location(0) vec4f {
  // Minimal visible output (solid color)
  return vec4f(0.9, 0.2, 0.2, 1.0);
}
