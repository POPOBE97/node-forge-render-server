pub mod color;
pub mod decode;
pub mod envelope;
pub mod exr;
pub mod gainmap;
pub mod iso21496;
pub mod png_chunks;

pub use color::{
    PngRgbSpace, detect_png_rgb_space, display_p3_rgba8_to_srgb, encode_srgb_png,
    encode_srgb_png_from_linear, linear_p3_to_linear_srgb, preview_rgba8_from_linear,
};
pub use decode::{
    DecodedReferencePixels, ReferencePayloadKind, apply_alpha_mode, decode_linear_f32,
    decode_png_bytes, decode_png_sdr_base_rgba8, decode_private_envelope, envelope_from_encoded_png,
    envelope_from_raw_fp16, figma_srgb_png_from_source, is_exr, is_png, payload_kind_label,
    quantize_helper_sdr,
};
pub use envelope::{
    Alpha, MAGIC, PayloadKind, ReferenceImageEnvelope, Transfer, parse_envelope,
};
pub use exr::{EXR_MIME, PNG_MIME, decode_premultiplied_linear_exr, encode_premultiplied_linear_exr};
