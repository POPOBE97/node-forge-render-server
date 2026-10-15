# Color ABI

SceneDSL 6.0 uses one canonical representation for every typed color value:

```text
HEX / CSS sRGB -> sRGB EOTF -> unpremultiplied scene-linear RGBA
State / Mutation / Transition / Derivation -> render parameter packing
-> premultiply exactly once before blending -> final present OETF -> display
```

- `color`, `packed<color>`, State Params, Motion values, Render Graph uniforms, MeshGradient,
  clear colors, and IntelligentLight parameters use scene-linear RGB.
- RGB is not clamped and may exceed `1.0`. Alpha is coverage and never passes through an RGB
  transfer function.
- Numeric arrays are canonical linear values. HEX strings are an encoded sRGB boundary syntax and
  are decoded while parsing. Runtime code must not infer encoding from component magnitude.
- Image textures follow their declared `encoderSpace` or sRGB texture format independently; image
  decoding is not a typed-color conversion.
- Render targets retain their authored formats. In particular, `rgba8unorm` stores linear values
  with 8-bit quantization; projects needing more precision or HDR should use `rgba16float`.
- Pixel Inspector reports shader-visible linear premultiplied RGBA. It decodes readback from
  `rgba8unorm-srgb`, while `rgba8unorm`, `rgba16float`, and `rgba32float` are read as stored.

## Image source precision

Choose the upload path from decoded component depth, independently of the authored `encoderSpace`:

| Decoded source | Upload and preparation |
| --- | --- |
| 8-bit grayscale/RGB/RGBA | Expand channels at 8-bit depth; use UNORM8 or hardware sRGB decoding. When primary conversion or alpha premultiplication is needed, store the prepared result in RGBA32Float. |
| 16-bit grayscale/RGB/RGBA | Expand channels without truncating the integers, normalize to f32, decode the declared transfer function, convert primaries, and premultiply before uploading RGBA32Float. |
| 32-bit floating RGB/RGBA | Preserve f32 components and range. Apply only the declared transfer/primary/alpha operations, then upload RGBA32Float. |

The high precision path decodes before filtering. There is no 16/32-bit sRGB texture format;
adding an sRGB suffix to an UNORM16/Float32 format cannot perform that decoding. Do not replace
UNORM16 with float16: half precision cannot distinguish every 16-bit source code.

Native and headless devices request `FLOAT32_FILTERABLE` when available. Unsupported device
capabilities produce a validation error rather than silently lowering texture precision.
Nearest/linear sampling still follows the authored sampler parameters.

FP32 preparation prevents intermediate 8-bit/half quantization; it is not a promise of exact
real-number color arithmetic. Authored render-target formats, image resizing/filtering, and the
final display/export format retain their own precision limits. An 8-bit PNG preview is not
evidence of the precision of the source texture or HDR render target.

Pixel readback and EXR export support both floating formats without an 8-bit conversion or alpha
clamp. Named texture/pass exports use `.rgba16f` for exact half bits and `.rgba32f` for exact f32
bits. Both raw containers have a 20-byte little-endian header: `NF16`/`NF32`, version `1`, width,
height, and channel count `4`, followed by row-major RGBA components. Requesting `.rgba16f` for a
FP32 texture is an error.

The editor treats HEX, INT, CSS, HSV/HSL, Lab, and OKLCH as SDR/sRGB controls. FLOAT/LINEAR is the
authoritative HDR editor. An EDR-capable WebGPU canvas may preview values above SDR white; otherwise
the editor explicitly labels the clipped proxy as `HDR · SDR Preview`.
