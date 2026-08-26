# Android HDR Screencap → Clipboard → Canvas Ref Plan

## Goal

Make `⌘⌥3` and `⌘⌥4` capture the selected USB Android device into a clipboard payload that preserves HDR values for Node Forge while remaining a normal PNG for other applications.

- Node Forge Canvas pastes HDR captures as linear `Rgba16Float` references with values above `1.0` intact.
- Node Forge Canvas pastes SDR captures through the existing 8-bit reference path.
- Figma and other applications always receive a conventional 8-bit PNG. Figma is not expected to preserve or reconstruct values above `1.0`.
- `⌘⌥3` keeps the Android source-profile PNG behavior; `⌘⌥4` keeps the Figma-sRGB PNG behavior.
- Raw FP16 capture is preferred. A stock Android 16 gainmap PNG is the automatic fallback.

## Current State

- The shortcut routing and asynchronous status indicator live in `src/app/frame/ingest.rs` and `src/app/canvas/ops.rs`.
- `src/android_reference.rs` currently runs `adb exec-out screencap -p`. On macOS it places the original PNG bytes on `NSPasteboard`, while the Figma-sRGB branch decodes, converts, and re-encodes an 8-bit PNG.
- Canvas `⌘V` is consumed before scene input and loads a `RefImageSource::Clipboard`, but `arboard::get_image()` reduces clipboard content to RGBA8.
- The reference loader already stores linear `f32` pixels and uploads high-precision sources as `wgpu::TextureFormat::Rgba16Float`.
- The connected target is Android 16/API 36, `arm64-v8a`, build fingerprint `nubia/Nerida/Nerida:16/BQ2A.251016.001-BP2A.250605.031.A3/b1577:userdebug/test-keys`. It accepts root commands as `su 0 ...`. Its stock `screencap -p` has been verified to emit PNGs containing `gmAP` and `gdAT` while HDR content is visible, with base ICC description `Display P3 Gamut with sRGB Transfer`. `dumpsys SurfaceFlinger` reports `local_tonemap_screenshots: true` and `true_hdr_screenshots: true` on this exact ROM; the gainmap fallback guarantee is deliberately limited to that measured configuration.
- The exact ROM's `/linkerconfig/ld.config.txt` maps `/data/local/tmp` to the `unrestricted` section, whose default namespace is non-isolated and searches `/system/${LIB}`, `/system_ext/${LIB}`, `/odm/${LIB}`, `/vendor/${LIB}`, and `/product/${LIB}`. This measured exception is why the private-API helper may execute from `/data/local/tmp` on this fingerprint; it is not a general Android guarantee.

## Capture Pipeline

### Embedded rooted helper

Add a minimal AOSP-derived native helper plus a fixed prebuilt arm64 binary for the exact fingerprint above. Keep the source, `Android.bp`, license/provenance, build revision, SHA-256, supported ABI/API/fingerprint, and binary together under `tools/android-hdr-screencap/`. Embed the prebuilt bytes in the render-server binary.

The helper attempts two consecutive in-process SurfaceFlinger products during one rooted helper process and one `adb exec-out` session:

1. request `ui::PixelFormat::RGBA_FP16` + `ui::Dataspace::V0_SCRGB_LINEAR` and retain the raw result;
2. repeat the stock `screencap` capture/encoding path and, when that ROM-specific encode path is available, produce its profile-carrying 8-bit PNG including Android gainmap chunks.

The helper:

- calls the same SurfaceFlinger display-capture APIs as AOSP `screencap`, including `ProcessState::self()->setThreadPoolMaxThreadCount(0)` followed by `startThreadPool()`;
- requests `ui::PixelFormat::RGBA_FP16` and `ui::Dataspace::V0_SCRGB_LINEAR`;
- initializes the Binder thread pool, resolves the same physical display and orientation as stock `screencap`, waits for and validates the capture fence, locks with `USAGE_SW_READ_OFTEN`, copies stride-aware rows, and always unlocks the buffer;
- leaves secure/protected capture disabled;
- reports `capturedHdrLayers` from the raw capture's `ScreenCaptureResults` as the same-frame HDR/SDR classification signal;
- rejects a returned buffer unless it is finite, FP16, linear extended sRGB, and readable;
- tight-copies already-premultiplied row-strided pixels into packed RGBA half-float rows without multiplying RGB by alpha again;
- reproduces the target AOSP `screencap` PNG/gainmap wiring and records its shared-library and SELinux requirements for the optional second product;
- writes the raw FP16 image and optional companion source PNG in one normalized binary protocol to stdout and diagnostics only to stderr.

The fixed prebuilt must be produced and smoke-tested against the recorded nubia ROM and its `/system/lib64` private ABI, not merely against generic Android 16 headers. The smoke test verifies that the staged binary starts from `/data/local/tmp`, resolves its platform libraries, reaches SurfaceFlinger, returns the required format/dataspace, and, with known HDR content, demonstrates finite raw values above `1.0` when `capturedHdrLayers=true`; a prebuilt that cannot do so is not shipped as authoritative. Record the exact AOSP/OEM source revision and reproducible target lunch/Soong build command beside the source. Track the small arm64 prebuilt directly in Git, not Git LFS, and gate `include_bytes!` plus deployment code to native non-Wasm targets. Before first use, Render Server checks device ABI, SDK, complete fingerprint, and the measured unrestricted `/data/local/tmp` linker mapping. It must not execute the helper on any mismatch; alternate ROMs use stock PNG fallback rather than installing into or remounting `/system`. For a match it compares the remote and embedded SHA-256, creates `/data/local/tmp/node-forge/`, writes/replaces the helper, applies mode `0755`, and executes it through the detected root syntax. If the device lacks `sha256sum`, deploy unconditionally; file size alone is not an integrity check.

Introduce a process-owning ADB runner rather than wrapping `Command::output()` in a UI timer. Replace the current hard-coded 15-second indicator timeout with one absolute 45-second request deadline for every in-app and standalone CLI capture; the caller cannot know whether deployment is needed when it starts. Every getprop/probe/hash/mkdir/chmod command has a 3-second cap, push has 10 seconds, helper capture has 15 seconds, and stock fallback has 15 seconds, but every child also receives only the smaller remaining portion of the absolute deadline. Cache validated device identity/deployment state after success so warm operations normally avoid probe cost. Each local child is killed and reaped on expiry.

Probe root launch forms without restarting adbd: use direct `adb shell` when `id -u` is already `0`, otherwise try `su 0 sh -c ...`, then `su -c '...'`. Do not invoke `adb root`. Launch the helper through `adb exec-out` and a root wrapper which writes the remote shell/helper PID to `/data/local/tmp/node-forge/capture-<request-id>.pid` before `exec`. On cancellation or timeout, permanently close paste eligibility at the 45-second deadline, then allow one separate maximum 3-second root kill/reap/cleanup grace command so killing local `adb` cannot leave a remote capture running. Binary stdout must begin with the protocol magic at byte zero; root banners, CRLF, or any other stdout noise invalidate it and trigger fallback.

Helper failure and stock fallback run sequentially in the same worker, bounded by that same absolute deadline. On deadline expiry, cancel/kill/reap the local ADB child, spend at most the cleanup grace on the recorded remote PID, then mark the request failed. While a capture worker is active, subsequent `⌘⌥3/4` requests are ignored with the existing in-progress indicator instead of spawning detached concurrent captures. The worker only returns a decoded/validated result through a channel. The UI/main thread is the sole pasteboard writer and checks the request generation and deadline immediately before mutation, so an expired, cancelled, or superseded worker can never publish late data.

Any failure before a valid raw envelope exists falls back to stock `adb exec-out screencap -p`. If raw capture validates but the optional companion encode fails, retain raw as the private authority and obtain only the public PNG with host stock `screencap -p`; if that also fails, synthesize a logged, clamped sRGB 8-bit public PNG from raw rather than discarding the private capture. A late worker can never modify the pasteboard after the operation has timed out or been superseded.

### Capture result selection

The validated helper raw result is always the Node Forge authority when the helper invocation succeeds. The companion PNG is from the second, immediately adjacent in-process capture and is used only as the public clipboard image; it never replaces, previews, or reclassifies the earlier raw result.

- Helper succeeds and raw `capturedHdrLayers=true`: private payload is Raw FP16 and Canvas uploads `Rgba16Float`.
- Helper succeeds and raw `capturedHdrLayers=false`: private payload still carries the same raw frame, but Canvas converts that frame from linear scRGB to linear sRGB, clips SDR gamut/range, quantizes it, and uploads `Rgba8Unorm`. This intentionally honors the confirmed SDR→8-bit behavior even though wide-gamut SDR scRGB can contain finite excursions outside `[0,1]`.
- Helper raw is unavailable or invalid: run host stock `adb exec-out screencap -p`. A valid `gmAP` + `gdAT` pair selects gainmap reconstruction to `Rgba16Float`; an ordinary PNG selects `Rgba8Unorm`.

The helper's second capture or host public-PNG fallback makes the public PNG an adjacent-frame representation on animated content. It never changes payload kind, texture format or HDR/SDR classification after raw validates. Gainmap reconstruction selects HDR only for host stock fallback or a clipboard PNG without a valid private flavor. For a raw HDR Shortwire capture, store the loaded raw frame itself as EXR rather than treating the public PNG as an exact artifact.

## Binary Interfaces

### Helper stdout and private clipboard envelope

Use the same fixed little-endian `NodeForgeReferenceImageV1` envelope for helper stdout and macOS private clipboard data. The 64-byte v1 header is:

| Offset | Field |
| ---: | --- |
| 0 | magic `[u8; 8] = b\"NFRCLIP\\0\"` |
| 8 | version `u16 = 1` |
| 10 | header length `u16 = 64` |
| 12 | flags `u32`: bit 0 `captured_hdr_layers`, bit 1 `has_source_png`; all other bits reject |
| 16 | payload kind `u16`: 1 `LinearRgba16F`, 2 `EncodedPng` |
| 18 | transfer `u16`: 1 `LinearScRgb`, 2 `EncodedProfiled` |
| 20 | alpha `u16`: 1 `Premultiplied`, 2 `Straight` |
| 22 | reserved `u16 = 0` |
| 24 | width `u32` |
| 28 | height `u32` |
| 32 | tight bytes-per-row `u32` |
| 36 | reserved `u32 = 0` |
| 40 | primary payload length `u64` |
| 48 | source PNG length `u64` |
| 56 | reserved `u64 = 0` |

Payload order is primary bytes followed by the optional stock-encoded source PNG. `LinearRgba16F` requires `LinearScRgb`, tight `width * 8` rows and premultiplied alpha. `EncodedPng` requires `EncodedProfiled`, straight alpha and zero bytes-per-row. The helper always emits Raw FP16 primary data and may append its companion PNG; stock fallback emits Encoded PNG with no companion.

The helper validates that the returned Android buffer is `RGBA_FP16` in `V0_SCRGB_LINEAR` before emitting normalized protocol enums rather than Android enum values. The host validates checked dimension/length arithmetic, a 16K per-axis limit, a 512 MiB aggregate payload limit, reserved zeros, flag/payload consistency, exact EOF, expected row length, finite half values, and alpha in `[0,1]`. Negative finite scRGB values are allowed. Unknown versions, enums, flags, or trailing data fail closed and trigger PNG fallback.

### macOS pasteboard: `com.node-forge.reference-image.v1`

Write the validated envelope as `com.node-forge.reference-image.v1` alongside `NSPasteboardTypePNG`. The public PNG behavior remains:

- `⌘⌥3`: stock-encoded companion or fallback PNG, including its ICC profile and gainmap chunks if present;
- `⌘⌥4`: existing Display-P3-to-sRGB conversion and 8-bit re-encode, intentionally flattening/removing gainmap metadata for Figma.

Figma only consumes the public PNG. HDR content therefore appears as the Android SDR base/tone-mapped preview, not as values above `1.0`.

The private `NodeForgeReferenceImageV1` bytes are identical for `⌘⌥3` and `⌘⌥4`; only the public `NSPasteboardTypePNG` representation differs. Never wrap the flattened `⌘⌥4` public PNG as the private payload.

Public encoding is deliberately separate from Canvas HDR decode. `⌘⌥3` passes through the stock-encoded PNG bytes. `⌘⌥4` decodes only that PNG's SDR base, never applies its gainmap, then performs the existing profile-aware Display-P3→sRGB 8-bit encode. If the companion profile is unsupported but validated raw exists, generate the public sRGB PNG from a clamped raw preview without dropping the private payload; when only stock fallback exists, preserve the existing explicit unsupported-profile failure rather than guessing a color space.

## Canvas Decode and Ref Loading

On macOS, clipboard paste precedence is:

1. validated Node Forge private envelope;
2. raw `NSPasteboardTypePNG` bytes;
3. existing `arboard::get_image()` fallback.

If the private type is absent, continue to raw PNG/arboard. If it is present but malformed or unsupported, fail the paste and retain the existing Ref rather than silently substituting the public SDR PNG. Do not replace the current Ref until the selected payload has decoded and uploaded successfully.

### Raw FP16

Convert half values to the existing linear `f32` reference representation without transfer conversion or RGB clamping. Add a direct linear-f32 Ref constructor so this path does not round-trip through `DynamicImage`. Preserve the payload's premultiplied source representation, initialize both the Ref and visible alpha-mode UI control from the envelope rather than stale state, and derive GPU/preview data exactly once from immutable source pixels. A later user alpha-mode change re-derives from that source; it must not double-premultiply. For helper-classified SDR, perform the specified linear-scRGB→linear-sRGB clip/quantize path and upload `Rgba8Unorm` instead.

### PNG color management and Android gainmap

All encoded PNG entry points—private `EncodedPng`, raw pasteboard PNG, stock SDR fallback, Shortwire restore, and manually chosen/dropped `.png` files—share one byte-oriented decoder. Non-PNG manual EXR/JPEG loading keeps its existing path. The PNG decoder parses sRGB/ICC metadata before pixel conversion. SDR Display P3 is decoded to linear P3 and converted to linear sRGB in floating point; it must not use the existing clamping `display_p3_rgba8_to_srgb` helper. This preserves the current AppKit/arboard color-managed behavior after raw PNG bytes take precedence.

Add a bounded PNG chunk reader that validates the PNG signature, chunk lengths, CRCs, ordering, and exact stream end. Behavior:

- neither `gmAP` nor `gdAT`: decode through the existing ordinary PNG path;
- exactly one of the outer pair, duplicates, unsupported minimum-reader version, malformed metadata, or malformed embedded PNG: return an explicit error and preserve the previous Ref; do not recursively reconstruct a nested `gdAT` found inside the embedded gainmap PNG—ignore it as Skia does;
- valid pair: require outer version 0, treat `gdAT` as the complete embedded gainmap PNG, and parse the nested `gmAP` ISO 21496-1 metadata.

Port the narrowly required ISO 21496-1 v0 behavior and reconstruction math rather than adding a general Ultra HDR/JPEG dependency. Pin the normative implementation to Skia revision `d1d26bf84dc788a4459ae491c1acdad02663fa9c` or a later recorded revision containing PNG chunk layout, ISO parsing and gainmap-color-space support, plus fixtures captured from the target ROM. Parse the outer base PNG `gmAP` with `ParseVersion` only and require minimum reader version 0; parse full ISO metadata from the nested gainmap PNG's `gmAP`. Port Skia's exact `W=1` log-space mix, including epsilons, gamma and min/max gain ratios. Decode A8, grayscale, grayscale-alpha, RGB and RGBA gainmap PNGs with Skia-compatible channel selection. Bilinearly sample the gainmap if dimensions differ. For a three-channel gainmap, honor the gainmap PNG's ICC primaries as Skia does; otherwise apply the map in the metadata-selected/base primaries. Convert the result to linear sRGB when needed. Preserve base alpha as straight alpha, reject non-finite output, do not clamp RGB, mark the result high precision, and upload as `Rgba16Float`.

Only sRGB and Display P3 Android base profiles are supported initially. An HDR PNG with an unknown profile fails explicitly instead of silently producing numerically incorrect Diff input.

### Lifecycle and Shortwire

- Both paths continue to produce `RefImageSource::Clipboard`, Manual source override, default Overlay mode, 50% opacity, zero offset, and stop an active Android USB Ref.
- Ordinary paste never creates a PNG artifact and is not removed by Shortwire cleanup.
- Generalize `ShortwirePastedReferenceImage` from PNG-only bytes to encoded image bytes plus MIME type while keeping the existing `ShortwireReferenceImage` and `DebugArtifactItem` field sets unchanged. When Shortwire is active, prepare the artifact before replacing `ref_image` or queuing diff capture: helper HDR becomes half-float linear-scRGB EXR with associated/premultiplied alpha, `.exr`, and `image/x-exr`; helper SDR becomes sRGB-OETF 8-bit preview pixels with an sRGB marker, `.png`, and `image/png`; stock gainmap fallback retains the exact gainmap PNG. EXR restore bypasses the sRGB EOTF and enters the linear-f32 constructor with Premultiplied alpha; SDR PNG restore applies the ordinary sRGB EOTF; gainmap PNG restore uses reconstruction. `⌘⌥4` Shortwire artifacts come from this loaded Ref path, never from the flattened public Figma-sRGB flavor. Ordinary paste still performs no artifact encoding.
- Clipboard read/decode/GPU upload failure or Shortwire artifact encoding failure leaves the previous Ref, patch, artifact and pending diff state untouched and logs one actionable error. Commit the prepared Ref and Shortwire state atomically only after every required step succeeds.

## Testing and Acceptance

### Automated tests

- Envelope/helper protocol: valid raw-only and raw-plus-PNG HDR/SDR frames; every byte offset and enum; row packing; endian/version/flag/reserved rejection; truncation/trailing data; aggregate/dimension overflow; wrong format/transfer/alpha combinations; NaN/Inf; negative finite RGB; invalid alpha.
- Deployment/process control through an injected fake ADB runner: exact fingerprint success; mismatch never executes; hash hit skips deployment; missing `sha256sum` deploys; hash mismatch creates the directory, deploys, and chmods; direct-root, `su 0 sh -c`, and `su -c` argv forms; stdout banner rejection; absolute 45-second deadline and numeric per-child caps; local kill/reap; remote PID kill/cleanup; generation/deadline checked immediately before pasteboard mutation; companion failure preserves raw while separately obtaining/synthesizing public PNG; every raw-helper failure class reaches stock PNG fallback; a second shortcut cannot replace an in-flight worker.
- Pasteboard through an injected macOS pasteboard seam: private flavor wins; `⌘⌥3` exposes stock-encoded source PNG publicly; `⌘⌥4` decodes only the SDR base and exposes flattened sRGB publicly; public encode failure cannot discard valid private raw; ordinary external PNG and arboard-only images remain compatible.
- Gainmap/color: synthetic tiny base/gainmap PNGs with ISO metadata; values above `1.0`; outer-version versus inner-metadata parsing; A8/gray/gray-alpha/RGB/RGBA maps including gainmap ICC; SDR and HDR P3 conversion; scaled gainmap sampling; malformed/duplicate/missing chunks; CRC/version/profile failures; ordinary SDR PNG unchanged; manual `.png` byte path matches clipboard decode.
- Ref lifecycle: helper HDR selects `Rgba16Float`; helper SDR quantizes the same raw frame to `Rgba8Unorm`; fallback gainmap selects `Rgba16Float`; source alpha initializes Ref and UI correctly and mode toggles never double-premultiply; metrics observe `max > 1`; old Ref survives failures; Android USB stop/manual override; ordinary versus Shortwire cleanup; EXR round-trip preserves the same half-float linear pixels and `max > 1`; SDR PNG round-trip matches the 8-bit Ref; artifact encode failure is atomic.
- Retain and run existing clipboard gesture routing, Shortwire image-only patch, artifact round-trip, pending diff capture, full library tests, and `cargo check --all-targets`.

### Device acceptance

- With HDR content visible, pressing either shortcut and then pasting into Canvas yields a Ref whose sampled/metric maximum exceeds `1.0`; logs identify source, `raw-fp16`/`gainmap-png` payload kind, fallback reason, dimensions, alpha semantics and selected texture format.
- With SDR-only content, first verify that stock `screencap -p` on this ROM contains neither `gmAP` nor `gdAT`; pressing either shortcut and then pasting yields an 8-bit Canvas Ref and unchanged visual output.
- Making the helper unavailable or invalid produces a logged `gainmap-png` fallback and still reconstructs HDR above `1.0` from the verified Android 16 PNG.
- Pasting into Figma always works as PNG: `⌘⌥3` retains the source profile appearance, while `⌘⌥4` is sRGB. Neither is claimed to retain HDR numeric range.
- Consecutive pastes replace the prior Ref; Overlay, Diff, offset, opacity, and metrics continue working.

## Scope Boundaries

- This captures the SurfaceFlinger-composited display, not an application's pre-composition material texture.
- The fixed helper is deliberately tied to the recorded ROM fingerprint. Other devices and ROM upgrades use PNG fallback until a separately reviewed prebuilt is added.
- The high-precision texture, Diff, pixel inspection, and metrics preserve values above `1.0`. The existing non-USB Overlay explicitly remains an sRGB 8-bit preview derived from the same authoritative raw or reconstructed linear pixels with a deterministic linear-sRGB→sRGB-OETF clamp; it never uses the adjacent companion frame. Making Overlay itself EDR/HDR is a separate change.
- The private clipboard flavor is macOS-only. Non-macOS keeps the existing RGBA8 clipboard behavior.
- The standalone `--android-screencap-clipboard` command is upgraded to the same dual-flavor capture behavior as the in-app shortcuts; its public PNG remains mode-compatible.
- Do not change WebSocket, DSL, `.nforge`, project persistence, or public protocol schemas.

## Review Log

### Grok review 1 — adopted

- Replace the indicator-only timeout with killable, serialized ADB process control and same-worker fallback.
- Route Shortwire restore through the same HDR-aware byte decoder.
- Color-manage every raw PNG path, including SDR Display P3, instead of depending on AppKit's TIFF translation.
- The original suggestion to reclassify helper output from a companion gainmap was later superseded: a validated helper raw frame now retains its same-frame `capturedHdrLayers` classification, and gainmap classification applies only to stock fallback/private-flavor absence.
- Specify Binder/fence/lock/stride/unlock/display behavior, fingerprint-tested OEM ABI, deployment directory creation, normalized helper enums, three-channel gainmap ICC, a direct linear-f32 Ref constructor, CLI behavior, and malformed-private-envelope policy.
- Explicitly scope the existing Overlay to its tone-mapped preview.

### Grok review 1 — not adopted

- Combining Raw FP16 and stock gainmap PNG into one SurfaceFlinger capture was not adopted: stock gainmap capture does not expose its internal FP16 HDR rendition, while requesting an FP16 output is a different capture. The documented adjacent-frame skew remains.
- Automatically treating an unrecognized `⌘⌥4` ICC profile as sRGB was not adopted because silent mis-coloring is worse than the existing explicit failure.

### Grok review 2 — adopted

- Make validated helper raw data authoritative and prevent an adjacent PNG from overriding or reclassifying it.
- Use one rooted helper process and one `adb exec-out` session for the raw and stock-style capture pair; retain host stock `screencap` only as helper fallback.
- Specify the private/helper envelope byte-for-byte, including normalized enums, payload ordering, alpha semantics, reserved fields and exact EOF.
- Add numeric deadlines, request-generation gating at pasteboard mutation, local child kill/reap, remote PID cleanup, binary-stdout validation and non-restarting root-mode probes.
- Preserve source alpha semantics, cover all Android gainmap channel layouts, pin the relevant Skia revisions, record the measured SurfaceFlinger HDR flags, and route manual PNG loads through the shared decoder.
- Specify missing packaging/deployment choices, deterministic previews, payload-kind logging and `⌘⌥4` Shortwire source selection.
- Preserve raw HDR Shortwire round-trips with EXR and generalize the internal pasted-artifact byte/MIME carrier.

### Grok review 2 — not adopted

- Always uploading helper output as `Rgba16Float` was not adopted because the confirmed product behavior requires helper-classified SDR captures to use the 8-bit path. The plan now quantizes the authoritative same-frame raw capture instead of consulting an adjacent PNG.
- Using the companion PNG as the raw HDR Shortwire artifact was not adopted because it is an adjacent frame and cannot exactly preserve the raw values; EXR preserves the loaded Ref.
- Automatically running `adb root` was not adopted; the helper uses already-root or `su` launch forms and leaves adbd state untouched.

### Grok review 3 — adopted

- Replace the incompatible 15/30/45-second timeout split with one absolute 45-second UI/CLI request deadline and remaining-time-bounded children; expiry kills local and remote work before failure is published.
- Decouple valid raw capture from optional companion/public encoding so a fragile ROM gainmap encoder or unsupported public ICC cannot discard private HDR data.
- Make Shortwire artifact preparation transactional and exact: linear associated-alpha EXR for helper HDR, sRGB PNG for helper SDR, and original gainmap PNG for fallback, using existing artifact schema fields.
- Parse the outer `gmAP` as version-only, parse nested ISO metadata fully, and pin a Skia revision that includes gainmap color-space handling and exact reconstruction behavior.
- Explicitly separate gainmap-aware Canvas decoding from the SDR-base-only public Figma encoder.
- Mark the review-1 companion reclassification suggestion as superseded, require sRGB Overlay preview bytes, specify exact Binder startup, synchronize alpha UI state, and add an injected pasteboard test seam.

### Grok review 3 — not adopted

- No correctness finding was rejected. The suggestion to silently color-convert an unsupported-profile stock-only fallback remains intentionally out of scope: without validated raw data or a known profile, the shortcut fails explicitly instead of guessing.

### Grok review 4 — adopted

- Treat helper startup/link resolution as part of the fingerprint smoke test and fall back if the recorded linker conditions change.
- State that both shortcuts publish identical private bytes, tight-copy already-premultiplied SurfaceFlinger pixels, derive Overlay from the authoritative frame, record the measured base ICC, verify the SDR stock-PNG no-gainmap case, match Skia's nested-`gdAT` behavior, and phrase device acceptance as shortcut-then-paste.

### Grok review 4 — not adopted

- The sole reported blocker claimed `/data/local/tmp` necessarily uses an isolated/unbundled namespace and therefore cannot resolve `libgui`/`libbinder`/`libui`. That general Android concern is valid but its premise is false for the fingerprint-locked target: the device's live `/linkerconfig/ld.config.txt` explicitly maps `/data/local/tmp` to `unrestricted`, with `namespace.default.isolated = false` and system library search paths. The plan now verifies this exact mapping and helper startup instead of performing an invasive `/system` remount/install.
