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
  `rgba8unorm-srgb`, while `rgba8unorm` and `rgba16float` are read as stored.

The editor treats HEX, INT, CSS, HSV/HSL, Lab, and OKLCH as SDR/sRGB controls. FLOAT/LINEAR is the
authoritative HDR editor. An EDR-capable WebGPU canvas may preview values above SDR white; otherwise
the editor explicitly labels the clipped proxy as `HDR · SDR Preview`.
