struct Globals {
  scale: vec2f,
  time: f32,
  _pad0: f32,
  prog_len: u32,
  const_len: u32,
  _pad1: vec2u,
};

@group(0) @binding(0)
var<storage, read> globals: Globals;

@group(0) @binding(1)
var<storage, read> program: array<u32>;

@group(0) @binding(2)
var<storage, read> consts: array<vec4f>;

struct VSOut {
  @builtin(position) position: vec4f,
  @location(0) uv: vec2f,
};

@vertex
fn vs_main(@location(0) position: vec3f) -> VSOut {
  var out: VSOut;
  out.uv = position.xy * 0.5 + vec2f(0.5, 0.5);
  let scaled = vec2f(position.x * globals.scale.x, position.y * globals.scale.y);
  out.position = vec4f(scaled, position.z, 1.0);
  return out;
}

const OP_LOAD_CONST: u32 = 1u;
const OP_UV: u32 = 2u;
const OP_MUL: u32 = 3u;
const OP_ADD: u32 = 4u;
const OP_SIN_TIME: u32 = 5u;
const OP_OUTPUT: u32 = 255u;

fn unpack_op(word: u32) -> u32 { return word & 0xFFu; }
fn unpack_dst(word: u32) -> u32 { return (word >> 8u) & 0xFFu; }
fn unpack_a(word: u32) -> u32 { return (word >> 16u) & 0xFFu; }
fn unpack_b(word: u32) -> u32 { return (word >> 24u) & 0xFFu; }

fn safe_const(idx: u32) -> vec4f {
  if (idx >= globals.const_len) {
    return vec4f(1.0, 0.0, 1.0, 1.0);
  }
  return consts[idx];
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4f {
  var regs: array<vec4f, 16>;

  // Default output if program is empty.
  var out_color: vec4f = vec4f(0.0, 0.0, 0.0, 1.0);

  var ip: u32 = 0u;
  loop {
    if (ip + 1u >= globals.prog_len) { break; }

    let w0 = program[ip];
    let imm = program[ip + 1u];
    ip = ip + 2u;

    let op = unpack_op(w0);
    let dst = unpack_dst(w0) & 15u;
    let a = unpack_a(w0) & 15u;
    let b = unpack_b(w0) & 15u;

    if (op == OP_LOAD_CONST) {
      regs[dst] = safe_const(imm);
      continue;
    }

    if (op == OP_UV) {
      regs[dst] = vec4f(in.uv, 0.0, 1.0);
      continue;
    }

    if (op == OP_SIN_TIME) {
      let k = 0.6 + 0.4 * sin(globals.time);
      regs[dst] = vec4f(k, k, k, 1.0);
      continue;
    }

    if (op == OP_MUL) {
      regs[dst] = regs[a] * regs[b];
      continue;
    }

    if (op == OP_ADD) {
      regs[dst] = regs[a] + regs[b];
      continue;
    }

    if (op == OP_OUTPUT) {
      out_color = regs[a];
      break;
    }

    // Unknown opcode: fail loudly.
    out_color = vec4f(1.0, 0.0, 1.0, 1.0);
    break;
  }

  return out_color;
}
