# Android HDR Screencap Implementation Review

## Scope

Review the current implementation against `docs/plans/android-hdr-screencap-ref-plan.md`. This is a review record, not an implementation patch. The review covers the rooted Android helper, stock gainmap fallback, private/public macOS clipboard flavors, Canvas Ref loading, color/alpha handling, Shortwire artifacts, timeouts, and deployment.

Validation already run:

- `cargo test --lib`: 913 passed, 1 ignored.
- `cargo check --all-targets`: passed.
- The required fingerprint-tested helper prebuilt is absent from `tools/android-hdr-screencap/prebuilt/arm64-v8a/`.

## Current Findings

### 1. Blocker: the authoritative helper path is not shipped

Evidence:

- `build.rs` only sets `android_hdr_helper_prebuilt` when `tools/android-hdr-screencap/prebuilt/arm64-v8a/node-forge-hdr-screencap` exists.
- `src/android_screencap/helper.rs` returns `None` in production without that cfg.
- `src/android_screencap/capture.rs::try_helper` immediately returns `Ok(None)` when embedded bytes are absent.
- Tests substitute `TEST_HELPER_BYTES`, so they exercise host orchestration without compiling, deploying, or running the native helper.

Impact: every production capture currently skips `nf-screencap` and uses stock PNG fallback. The C++ helper and host FP16 decode path exist, but the authoritative path is not shipped or enabled in a production build.

Required correction:

- Build the helper against the recorded OEM/AOSP API, smoke-test it on the exact fingerprint, verify finite raw values above `1.0` for known HDR content, record its SHA-256/provenance, and add the prebuilt at the configured path.
- Add a build/release assertion or explicit capability diagnostic so an accidentally missing prebuilt cannot look like a fully enabled helper build.

### 2. Correctness: ordinary Display-P3 SDR can be misclassified as HDR/high precision

Evidence:

- `decode_png_bytes` marks any non-gainmap PNG high precision when its linear-sRGB conversion contains RGB outside `[0,1]`.
- Display-P3 primaries legitimately convert to out-of-range linear-sRGB values for saturated SDR colors; the implementation's own `linear_p3_red_is_outside_srgb_without_clamping` test proves this.
- The measured target stock PNG base ICC is Display P3 with an sRGB transfer curve.

Impact: an SDR screenshot with saturated P3 colors can select `Rgba16Float`, contradicting the confirmed `SDR -> Rgba8Unorm` behavior. Gamut excursions are not evidence of HDR luminance.

Required correction:

- Decide precision from the trusted source classification: valid helper `capturedHdrLayers`, or a valid stock `gmAP`/`gdAT` pair.
- For an ordinary non-gainmap PNG, convert to linear sRGB, clamp/quantize all RGBA channels to the 8-bit path, and upload `Rgba8Unorm` regardless of P3 gamut excursions or PNG channel depth.
- If manually loaded 16-bit SDR PNG preservation is desired later, model it as a separate source capability rather than conflating bit depth/gamut with HDR.

### 3. Correctness: premultiplied helper pixels are encoded as straight PNG pixels

Evidence:

- Raw helper envelopes require premultiplied alpha.
- Helper-classified SDR retains `Alpha::Premultiplied` after quantization.
- `encode_shortwire_artifact`, synthesized public PNG, and preview generation pass `source_linear_rgba` to code that writes the RGB values unchanged into an ordinary straight-alpha PNG/egui image.
- PNG restore decodes the data as `Alpha::Straight` and premultiplies again before upload.

Impact: translucent helper SDR pixels are multiplied by alpha twice after a Shortwire round trip. Synthesized public PNG and Overlay preview also show dark fringes/colors.

Required correction:

- Add alpha-aware straight-output conversion. When source semantics are premultiplied, unpremultiply RGB for nonzero alpha before sRGB OETF/8-bit encoding; define deterministic RGB for zero alpha.
- Use it for helper SDR Shortwire PNG, synthesized public PNG, and egui preview.
- Keep helper HDR EXR associated/premultiplied and bypass straight-alpha conversion.
- Add translucent-pixel round-trip tests covering helper SDR PNG, raw HDR EXR, preview, and synthesized public PNG.

### 4. Correctness: gainmap ICC failures silently become sRGB

Evidence:

- `decode_gainmap_png` calls `detect_png_rgb_space(bytes).unwrap_or(PngRgbSpace::Srgb)`.
- `detect_png_rgb_space` returns an error for an unsupported or malformed embedded ICC profile.
- Three-channel gainmaps use the gainmap image color space as the gainmap math space.

Impact: an unsupported/malformed three-channel gainmap ICC is silently reconstructed in sRGB, producing numerically incorrect HDR data and Diff output. This violates the plan's fail-closed color policy.

Required correction:

- Distinguish absent profile from invalid/unsupported profile.
- Propagate invalid/unsupported ICC errors.
- Apply the explicitly documented default only when the profile is genuinely absent and the relevant Skia/ISO behavior permits it.
- Add unknown-ICC and malformed-ICC three-channel gainmap tests.

### 5. Correctness: the shared PNG decoder destroys 16-bit samples before linear decode

Evidence:

- `decode_png_rgba` always returns RGBA8.
- Its RGBA16 branch divides every `u16` sample by 257 and casts to `u8`.
- The caller subsequently labels the result high precision when the input bit depth was 16, but the original precision has already been lost.

Impact: manual/clipboard 16-bit PNGs and any 16-bit gainmap base are numerically reduced to 8-bit while appearing to use a high-precision texture. The embedded gainmap image decoder is separate and retains 16-bit samples with `/ 65535.0`; this finding does not apply to that image. The measured Android target screenshots use an 8-bit base, so this is lower priority for the current device-acceptance path than findings 1–4.

Required correction:

- Decode 16-bit PNG channels directly to normalized `f32`/`u16` before transfer/color conversion.
- Keep a separate 8-bit SDR quantization stage where the product contract requires it.
- Cover values that differ only in low 8 bits so tests prove precision is retained where intended.

### 6. Process control: helper companion capture escapes the recorded PID model

Evidence:

- `tools/android-hdr-screencap/src/main.cpp::stock_png` uses `popen("/system/bin/screencap -p", "r")`.
- The remote PID file records only the helper process PID.
- Host cleanup sends `TERM` only to that PID and removes the PID file.

Impact: timeout/cancellation does not guarantee that the spawned shell/stock-screencap child is killed and reaped. This also diverges from the planned in-process second capture.

Required correction:

- Prefer the planned in-process stock-style capture/encode.
- If a subprocess is retained, create/manage a process group, record enough identity to avoid PID reuse, terminate the whole group, and reap it under the cleanup grace period.
- Add a process-tree timeout test or device smoke test.

### 7. Transactionality: alpha UI state changes before Ref commit succeeds

Evidence:

- `load_reference_image_from_pixels` assigns `app.canvas.reference.alpha_mode` before texture creation/upload and before `ref_image` replacement.

Impact: a later GPU/upload error preserves the previous Ref but leaves the global alpha control changed, violating the plan's failure atomicity.

Required correction:

- Build/decode/upload the candidate completely first.
- Commit `ref_image`, alpha UI state, desired source override, and Android USB stop only after every required ordinary-paste operation succeeds. When Shortwire is active, include these changes in the wider all-or-nothing transaction described in finding 9.
- Add a failure-injection test showing the old Ref and all surrounding state remain unchanged.

### 8. Deployment integrity: a newly pushed helper is not re-hashed

Evidence:

- `ensure_helper` compares the existing remote hash before deployment using `output.contains(&expected)` rather than parsing and exactly matching the digest token.
- After a mismatch/missing hash it pushes and chmods the helper, then returns the expected hash without reading the remote file again.

Impact: a truncated/corrupted push is not detected until execution, and the deployment cache records the expected hash even though it was never verified on-device.

Required correction:

- Re-run the remote SHA-256 after push/chmod and require an exact token match before caching deployment success.
- If `sha256sum` is unavailable, either upload to a temporary name and validate with another recorded digest mechanism before atomic rename, or do not mark the deployment cache as hash-verified.

### 9. Blocker: the Shortwire paste transaction commits Canvas before patch/diff acceptance

Evidence:

- `paste_reference_from_clipboard` prepares the optional artifact and then loads/replaces the Canvas Ref, stops Android USB, and selects Manual source.
- Only after that function returns does `ingest.rs` call `request_active_shortwire_diff_capture`.
- That later request can return `MissingPatch` or `Inactive`; in those cases the registry does not commit the artifact or pending diff, but the Canvas Ref and surrounding source state have already changed.

Impact: an active Shortwire paste can partially succeed. The user sees a new Ref even though the corresponding artifact/patch/diff transaction was rejected, violating the plan's all-or-nothing failure semantics.

Required correction:

- Prepare the decoded/uploadable candidate Ref, artifact bytes, image-only patch, and diff-capture request without mutating live application state.
- Validate that the active Shortwire transaction can accept the complete payload.
- Commit the Ref/source/Android USB changes and Shortwire artifact/patch/pending-diff state together, or commit none of them.
- Add failure-injection tests for `MissingPatch`, `Inactive`, artifact rejection, texture/upload failure, and successful atomic commit.

### 10. Process control: UI timeout permits a second capture before the first worker exits

Evidence:

- `ops.rs` treats the 45-second UI deadline as a terminal failure and drops the `Running` receive state.
- The capture worker thread may still be blocked in ADB/helper/fallback work after the UI state is no longer `Running`.
- A second screenshot shortcut can therefore start another worker while the first worker is still alive. Late pasteboard publication remains UI-thread gated, but worker serialization and cleanup do not.

Impact: timeouts can produce concurrent capture workers and overlapping remote helper/stock-screencap activity. This fails the intended single-flight and "kill local and remote work, then fail" contract.

Required correction:

- Give the running capture an explicit cancellation handle and retain a `Running`/`Cancelling` state until the worker acknowledges cleanup and exits.
- Do not accept another capture shortcut until that acknowledgement, even after the user-visible deadline has fired.
- Make all ADB/subprocess stages cancellation-aware and ensure local children plus the remote process tree are terminated and reaped before completion.
- Add a deterministic timeout test proving that a second capture cannot start before the first worker exits.

### 11. Helper contract gap: native output validation is deferred entirely to the host

Evidence:

- The helper serializes half-float pixels without rejecting non-finite values itself.
- The helper does not make the `waitForever` result part of the envelope success decision.
- The host parser rejects invalid/non-finite payloads and can fall back, so malformed data is not directly committed as a Ref.

Impact: this is not presently a host correctness blocker, but it weakens the helper's specified envelope contract and makes device failures less diagnosable.

Required correction:

- Reject non-finite half-float components in the helper before envelope publication.
- Check and report the capture wait/fence status rather than emitting a nominally successful envelope.
- Add native/device smoke coverage for both failure modes.

## Review Position

The host architecture is substantially implemented and its current unit suite passes, but the feature is not ready to call complete. Findings 1, 2, and 9 are immediate release blockers for the confirmed device behavior. Findings 3 and 4 are correctness blockers before claiming the complete alpha/color contract; their practical device impact depends on whether captured frames contain translucency and whether the ROM emits three-channel gainmaps. Findings 6 and 10 are required process-lifecycle fixes before device acceptance. Findings 7 and 8 are required atomicity/deployment fixes. Finding 5 is a real precision defect but is lower priority for the measured Android target because its stock base PNG is 8-bit; it still must be fixed before claiming general 16-bit PNG fidelity. Finding 11 is a helper-contract and diagnostics gap.

Useful additional verification after the required fixes:

- Use an EXR round-trip fixture at least two rows high so the current Y-orientation convention is actually exercised.
- Log an explicit runtime/build capability message when the helper prebuilt cfg is disabled.
- If the stock binary remains as the companion capture, verify on-device that it preserves the ROM's authoritative gainmap encoding and metadata.

## Grok Review Log

### Iteration 1 — not converged

Configuration: Grok 4.6, `xhigh` reasoning, 80 maximum turns, read-only Plan mode, with explicit permission to inspect the full repository, project instructions, implementation, tests, and relevant platform behavior.

Adopted:

- Corrected finding 1 to distinguish an implemented helper/host path from a helper binary that is not shipped.
- Scoped finding 5 to the shared/base PNG decoder; the embedded gainmap decoder retains its 16-bit samples, and the current device base is 8-bit.
- Expanded finding 8 to include the non-exact pre-deployment hash match.
- Added the missed Shortwire all-or-nothing transaction blocker (finding 9).
- Added the missed worker-timeout/single-flight process-lifecycle defect (finding 10).
- Added the native helper finite-value/wait-status contract gap (finding 11), correctly classified below the host blockers because the host already validates the envelope.
- Added the suggested EXR multi-row, capability diagnostic, and ROM companion-capture verification checks.

Not adopted as a mandatory design choice:

- The suggestion that retaining the stock `screencap` binary may be preferable to an in-process companion capture. Preserving the ROM's gainmap behavior is important, but either design is acceptable if it meets the process-control contract and device evidence proves metadata/pixel fidelity. The review therefore keeps both options open.

### Iteration 2 — converged

Configuration: Grok 4.6, `xhigh` reasoning, 80 maximum turns, read-only Plan mode, again with explicit permission to independently explore the complete repository and platform assumptions rather than trusting the review document.

Result:

- Re-verified all 11 findings against the implementation and found each factually supported.
- Confirmed that the Iteration 1 additions and the single non-adoption are consistent with the saved plan.
- Found no missing release blocker, false finding, or required correction that should be removed.
- Suggested only a severity wording refinement: findings 1, 2, and 9 directly block the confirmed device path, while findings 3 and 4 block completeness of the alpha/color contract but may not trigger on every device frame. This refinement was adopted in `Review Position`.
- Final verdict: `CONVERGED`.

## Implementation Follow-up — 2026-08-26

Resolved in code:

- Finding 2: ordinary PNGs, including Display-P3 and 16-bit SDR PNGs, now explicitly clamp/quantize to the `Rgba8Unorm` SDR path. The PNG decoder retains full 16-bit samples until this policy stage so gainmap reconstruction does not lose low bits.
- Finding 3: premultiplied helper pixels are unpremultiplied before straight-alpha PNG and preview encoding, with deterministic black RGB at zero alpha. EXR remains associated/premultiplied.
- Finding 4: unsupported or malformed embedded gainmap ICC profiles now fail closed.
- Findings 5, 7, and 9: full-precision PNG decode precedes source classification; Clipboard Ref creation is prepared without live state mutation; active Shortwire accepts the artifact/patch/diff request before the prepared Canvas Ref, alpha state, Manual override, and Android USB stop are committed.
- Findings 6 and 10: helper capture uses an isolated process group and PID/start-time identity for remote tree cleanup; UI timeout raises a shared cancellation signal, terminates the active local ADB process, and retains a non-reentrant `Cancelling` state until the worker exits.
- Finding 8: deployment requires an exact SHA-256 token and re-hashes after push/chmod before caching success.
- Finding 11: the helper checks fence wait status and rejects non-finite FP16 components before publishing an envelope.
- Missing-helper capability is now logged explicitly at runtime.

Remaining release gate:

- Finding 1 remains external to this checkout. The connected device matches the recorded fingerprint, SDK 36, arm64 ABI, and direct-root requirement, but neither the device nor the repository contains a helper binary and this Mac has no matching AOSP/Soong private-ABI build tree. A prebuilt must still be produced in that environment, copied to the configured path, and pass the recorded on-device HDR smoke test before it can be shipped. The stock PNG/gainmap fallback remains active until then.

Post-fix validation:

- `cargo check --all-targets`: passed.
- `cargo test --lib`: 922 passed, 1 ignored.
- Connected-device stock fallback: passed on serial `324660644123`; captured a 1260×2800 SDR frame, classified it as `sdr-png`/`Rgba8Unorm`, and wrote both the private envelope and public PNG to the macOS clipboard with the explicit `helper-unavailable` diagnostic.
- Added regression coverage for Display-P3 SDR classification, 16-bit PNG low-bit retention, premultiplied PNG/preview output, invalid gainmap ICC, exact/post-push helper hashes, PID/process-group cleanup, local ADB cancellation, timeout single-flight draining, and two-row EXR orientation.
