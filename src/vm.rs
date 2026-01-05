#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Globals {
    pub scale: [f32; 2],
    pub time: f32,
    pub _pad0: f32,
    pub prog_len: u32,
    pub const_len: u32,
    pub _pad1: [u32; 2],
}

pub fn as_bytes<T>(v: &T) -> &[u8] {
    unsafe { core::slice::from_raw_parts((v as *const T) as *const u8, core::mem::size_of::<T>()) }
}

pub fn as_bytes_slice<T>(v: &[T]) -> &[u8] {
    unsafe {
        core::slice::from_raw_parts(v.as_ptr() as *const u8, core::mem::size_of::<T>() * v.len())
    }
}

pub const OP_LOAD_CONST: u32 = 1;
pub const OP_UV: u32 = 2;
pub const OP_MUL: u32 = 3;
pub const OP_ADD: u32 = 4;
pub const OP_SIN_TIME: u32 = 5;
pub const OP_OUTPUT: u32 = 255;

pub fn pack(op: u32, dst: u32, a: u32, b: u32) -> u32 {
    (op & 0xFF) | ((dst & 0xFF) << 8) | ((a & 0xFF) << 16) | ((b & 0xFF) << 24)
}

pub fn program_uv_debug() -> Vec<u32> {
    vec![pack(OP_UV, 0, 0, 0), 0, pack(OP_OUTPUT, 0, 0, 0), 0]
}

pub fn program_constant_animated() -> Vec<u32> {
    vec![
        pack(OP_LOAD_CONST, 0, 0, 0),
        0,
        pack(OP_SIN_TIME, 1, 0, 0),
        0,
        pack(OP_MUL, 2, 0, 1),
        0,
        pack(OP_OUTPUT, 0, 2, 0),
        0,
    ]
}
