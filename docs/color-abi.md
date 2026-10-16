# Color ABI

SceneDSL 6.0 uses one canonical representation for every typed color value:

```text
HEX / CSS sRGB -> sRGB EOTF -> working-primary conversion -> unpremultiplied scene-linear RGBA
State / Mutation / Transition / Derivation -> render parameter packing
-> premultiply exactly once before blending -> Screen primary conversion -> canvas presentation
```

- `color`, `packed<color>`, State Params, Motion values, Render Graph uniforms, MeshGradient,
  clear colors, and IntelligentLight parameters use scene-linear RGB.
- RGB is not clamped and may exceed `1.0`. Alpha is coverage and never passes through an RGB
  transfer function.
- Numeric arrays are canonical linear values. HEX strings are an encoded sRGB boundary syntax and
  are decoded while parsing. Runtime code must not infer encoding from component magnitude.
- Image textures decode their declared `encoderSpace` then convert into the scene working primaries.
  `srgb` and `linear` sources use sRGB primaries; `display-p3` uses P3 primaries. Transfer decoding
  and primary conversion are independent.
- Render targets retain their authored formats. In particular, `rgba8unorm` stores linear values
  with 8-bit quantization; projects needing more precision or HDR should use `rgba16float`.
- Pixel Inspector reports shader-visible linear premultiplied RGBA. It decodes readback from
  `rgba8unorm-srgb`, while `rgba8unorm`, `rgba16float`, and `rgba32float` are read as stored.

## Persisted working space and Screen output

`SceneDSL.workingColorSpace` is required: `linear-srgb` or `linear-display-p3`. It is stored in the
archive's `workingColorSpace` document section. New documents default to `linear-srgb`.
`Screen.params.outputColorSpace` is required: `srgb` or `display-p3`, defaulting to `srgb` on new
Screen nodes. Invalid/missing spaces are rejected; existing example archives have been upgraded.

Shader output, numeric color inputs, render targets, State/Motion/Derivation colors, and FLOAT
color controls use the selected working primaries. Changing working space reinterprets existing
numeric values; it does not rewrite shader code or convert authored numeric tuples. Explicitly
tagged HEX and image inputs retain their source color meaning and convert into working space.

| Working space | Screen output | Final linear RGB operation |
| --- | --- | --- |
| linear sRGB | sRGB | Identity |
| linear Display P3 | sRGB | P3 → sRGB matrix |
| linear sRGB | Display P3 | sRGB → P3 matrix |
| linear Display P3 | Display P3 | Identity |

These operations apply no gamut clipping, compression, tone mapping, or RGB transfer function.
Alpha passes through unchanged. P3 → sRGB may produce negative or greater-than-one coordinates;
conversion preserves them in an RGBA16Float presentation target. This does not recover values
already lost in an authored UNORM target or an explicit clamp node.

On macOS, Screen selects the **actual native window surface declaration** together with the format of its
original scene render target. A float intermediate created for primary conversion does not
change whether that authored target requests SDR or extended output.

| Screen source format | Window sRGB | Window Display P3 |
| --- | --- | --- |
| UNORM (including `*-srgb`) | `Srgb`, UNORM surface, EDR off | `DisplayP3`, UNORM surface, EDR off |
| Float | `ExtendedSrgbLinear`, RGBA16Float, EDR on | `ExtendedDisplayP3`, RGBA16Float, EDR on |

The UI compositor stays RGBA16Float in the selected linear primaries. Normal sRGB/P3 and extended
P3 receive the sRGB transfer function in a final full-window pass, after blending. Normal output
uses a supported BGRA8Unorm (or RGBA8Unorm) surface, whose normalized storage bounds the final
encoded values to [0,1]. Extended sRGB goes directly to its linear float surface; extended P3
keeps negative and greater-than-one values through sign-preserving transfer encoding. An
`*-srgb` source texture is automatically decoded by sampling, so it is encoded only once at output.

The painter reconfigures the surface before acquiring the next drawable; switching does not
restart the renderer. Unsupported format/color-space combinations produce a visible error.
Matching canvas/output primaries use no further matrix. Other preview textures and ordinary sRGB
UI colors convert into the selected linear composition space, so UI colors retain their meaning.
Reference comparisons use linear sRGB. Actual display gamut/HDR capability is determined by the
OS and display. The local wgpu-hal Metal patch fixes a CoreGraphics constant lookup crash when
ExtendedDisplayP3 is first selected and explicitly assigns `kCGColorSpaceSRGB` for normal sRGB.
A nil `CAMetalLayer.colorspace` disables macOS color matching and must never stand in for sRGB;
see `third_party/wgpu-hal/NODE_FORGE_PATCH.md`.

With float targets, unbounded Screen conversion preserves the represented color across output
space choices; extended sRGB is not an sRGB-gamut clipping preview. With UNORM targets, the normal
window output is bounded in its selected primaries. Pixel values at Screen output change when
its primaries change. Intermediate/scene output remains in working space. PNG/clipboard SDR output
converts working RGB to sRGB before its existing SDR encode; raw/EXR texture export keeps the
texture's numeric primaries.

The editor's Screen node exposes both Working Color Space and Window Color Space. Working Color
Space edits the document-wide setting and follows document persistence/undo/tab switching; Window
Color Space selects that Screen's final native window output space.

### Android Player

Android uses the same unbounded `UiHdrNative` Screen matrix pass. It selects the native output
from the **original scene Render Target format**, before any float conversion intermediate:

| Scene target | Android transport | Native declaration |
| --- | --- | --- |
| UNORM (including `*-srgb`) | RGBA/BGRA8, sRGB OETF exactly once | Ordinary sRGB or Display P3, matching Screen; headroom 1 |
| Float (`rgba16float`, `rgba32float`) | A2B10G10R10 UNORM, normalized HDR | Display P3 / sRGB transfer / RANGE_EXTENDED (`0x188a0000`); buffer/SDR ratio 4, desired headroom 4 |

This is an Android-specific mapping. Float targets always use the native P3 HDR transport;
Screen's selected primaries still determine the shared matrix. When Screen requests sRGB,
its unbounded linear sRGB result is converted into linear P3 at the transport boundary. This
preserves P3 colors represented by negative sRGB coordinates until final display encoding.
The Android native declaration is therefore P3 for either float Screen choice. macOS retains
its direct extended sRGB / extended P3 float surfaces.

The final encoding is `sRGB_OETF(linearP3 / H)` with `H = 4`. UNORM10 storage bounds the final
result to [0,1], equivalent to clipping **only the transport** to linear P3 [0,H]. There is no
clamp or tone mapping in the working-space/Screen conversion or scene Render Target. The
headroom is a transport range, not a scene gain: SDR white 1 is encoded as `OETF(1/4)` and the
system is told that the buffer white corresponds to 4 SDR whites. Values outside the transport
range cannot be displayed by this path. Scaling, native-pixel mode and HDR probes share the
same final encoding. Source `*-srgb` decoding remains hardware-managed and occurs once.

The native output uses a child `ASurfaceControl` with three imported AHardwareBuffers. Buffer,
dataspace and `ASurfaceTransaction_setExtendedRangeBrightness(H,H)` are submitted together.
Vulkan imports use dedicated allocation, FOREIGN queue ownership and compositor release fences.
wgpu renders into a 10-bit texture, transitions it to COPY_SRC and drains its queue; the native
copy finishes before the compositor receives its buffer. This first implementation waits on
the GPU each HDR frame. It favors correct synchronization; it is not a zero-stall presentation
path. Resize/destruction detaches the child and retains buffers until release callbacks finish.
No device-specific DSPP LUT, hidden Android API, root setting or forced GPU composition is used.

The parent SurfaceView requests headroom 1; the native child owns HDR metadata. Android's window
mode follows the output (default / wide-color / HDR). Unsupported native import/range APIs cause
an explicit scene-application error and retain the previous runtime/output. Initialization is
SDR-ready; the Editor tests `status.ready`, not `status.hdr`, before delivering the first scene.
Debug Editor builds use the freshly staged APK before any copied bundle resource.

Player status separates actual `colorSpace`/`surfaceFormat`/`bufferSdrRatio` from
`screenColorSpace` and `workingColorSpace`. Ordinary `player_capture` returns scene-linear
pixels before the Screen matrix. `player_capture` with `{"stage":"presentation"}` returns the
packed little-endian A2B10G10R10 HDR transport codes, drawable dimensions, sRGB transfer and buffer
ratio; decode RGB with `H * sRGB_EOTF(code/1023)`. It does not capture panel light or DSPP output.
Actual display headroom is reported separately by Android and may differ from the request.

Regression: `cargo test --test android_presentation --test screen_color_space --test scene_delta`
checks format mapping, all four primary combinations, transfer decoding, HDR reconstruction,
native-pixel/probe paths and unbounded Screen conversion. Build/stage the APK from the superproject
with `./scripts/build-android-player.zsh`. The transport follows the public native protocol in
`p3-vulkan-desktop/native/direct_output.h`; its device-specific measurement compensation is not used.

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
