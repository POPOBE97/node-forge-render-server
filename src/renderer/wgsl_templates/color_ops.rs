// This module previously contained `build_image_premultiply_wgsl` for a dedicated GPU
// premultiply prepass. That pass has been eliminated; premultiply is now inlined at the
// sampling site via `nf_premultiply()` in compile_image_texture.

