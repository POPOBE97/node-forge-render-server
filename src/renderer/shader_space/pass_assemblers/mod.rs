//! Per-pass-type assembler functions.
//!
//! Each sub-module exposes an `assemble_*` function that receives
//! `&SceneContext` + `&mut BuilderState` + the layer information, and pushes
//! textures, geometry buffers, and `RenderPassSpec`s into the builder state.

pub(crate) mod args;
pub(crate) mod bloom;
pub(crate) mod composite;
pub(crate) mod downsample;
pub(crate) mod gaussian_blur;
pub(crate) mod gradient_blur;
pub(crate) mod render_pass;
pub(crate) mod upsample;
