# Screen Test

Camera Studio has three independent workflows. Geometry lives on the canvas; color workflows
remain under **Screen test**:

* **Geometry calibration** fits the current camera/phone arrangement. The active profile belongs
  to the Render Server process, survives window reopening and device changes, and is replaced only
  by a successful explicit calibration. Exiting the process clears the active profile. Geometry
  captures, charts and previews use a temporary session directory, cleaned up with the controller.
  They are not imported into the color database or added to capture history. The canvas has a
  **Calibrate** button in its upper-right toolbar, with inline progress/completion feedback.
* **Color calibration** saves iPhone CR2 captures, at multiple brightness settings, batches and
  conditions. Select compatible captures and publish an immutable baseline version. No Android
  device is needed.
* **Color inspection** captures Android and compares it with a selected stored iPhone baseline.
  The iPhone need not be connected. Reports retain their exact reference version and source IDs.
  Captures can also be saved without a baseline; select one later and use `Compare selected with
  chosen baseline`, which creates a new analysis instead of modifying the original capture.

Geometry is a one-button flow: connect the phone physically over USB, frame the full screen, and
press **Calibrate** on the canvas. The host reuses the connected display or detects the single USB phone,
installs the built Android APK or signed iPhone app if missing, launches it, reads its native drawable size,
selects an SDR display mode and 70% brightness, presents the chart and waits for readiness. If the
Canon is not connected, the same request connects it before capturing. Analysis waits for the
original photo format to be restored. On Android the host stops only the calibration player and
restores the previously resumed Activity, first by revealing/focusing its existing task (preserving
private activities and page state), then by starting that exact component only if its task was
reclaimed. It verifies the resulting foreground Activity. This uses Android's
[task focus command](https://android.googlesource.com/platform/frameworks/base/+/refs/heads/main/services/core/java/com/android/server/am/ActivityManagerShellCommand.java).
iPhone brightness ownership is released after calibration. Failure paths also restore the display.
Completion keeps the canvas live and the capture controls in place; no automatic photo review or
sidebar workflow switch is required. Multiple unselected phones produce an explicit ambiguity error.
iOS device trust, Developer Mode and a valid signature remain OS requirements. The default bundle
is `tools/screen-test-player/ios/build/Build/Products/Debug-iphoneos/ScreenTest.app`;
`NODE_FORGE_SCREEN_TEST_IOS_APP` can point to a signed `.app` or `.ipa` (temporarily extracted for
CoreDevice installation). No unsigned app or build/signing prompt is presented during acquisition.

For color reference/inspection, select manual exposure and fixed ISO in Camera settings; display
mode and brightness remain intentional saved measurement conditions. Geometry does not depend on
ISO or white balance: its camera identity tracks the camera, lens and focal length, and each later
capture must still pass optical marker validation against the frozen mapping.
Screen Test captures temporarily select RAW and verify the camera's read-back before releasing
the shutter. After file collection/download, the original photo format is restored and verified.
An existing RAW/RAW+JPEG setting is kept as-is. Only `imageformat` is changed; the independent SD
format setting is untouched. Unsupported/ignored writes never trigger a shutter. Failed captures
and downloads still restore the format; an ambiguous shutter timeout is reconciled without
recapture. Failed restoration blocks the next capture and retries, including after reconnecting
the same camera. Ordinary Camera Studio photos keep the user's format. Connect the display over USB.
`Show pattern` never presses the shutter. The workflow's
capture button presents a new identified chart, waits for confirmation and settling, then submits
one capture through the existing camera service. No second PTP session is opened.

Every color capture requires session geometry before the shutter can run. Before any color
measurement, the RAW's optical chart identity and marker positions must match that profile, and
geometric rectification must produce a normalized EXR and mask. Only then can the corrected ROI
mapping sample the original CFA. A typed rectified input prevents bypassing this sequence.
Each analyzed capture stores evidence linking its RAW checksum, chart generation, geometry profile,
maximum residual and normalized EXR checksum. Publishing references, producing reports and loading
reference members verify this evidence. Historical records without it cannot be used as baselines;
reanalyze them explicitly with valid matching geometry. Failed RAWs remain archived with the error,
without measurements or publishable evidence. Geometry is never silently refitted by an individual
color capture. Moving a phone or changing camera conditions requires explicit recalibration.
Keep phone brightness and display settings fixed. Disable True Tone, Night Shift and automatic
brightness on the reference iPhone and record relevant display settings in the condition notes.
Equal brightness percentages on different phones do not imply equal light output.

### iPhone brightness during acquisition

`Brightness` on the desktop is applied when showing/capturing a chart. The iOS player holds that
setting for the displayed chart, disables the idle timer (preventing idle dimming and Auto-Lock),
and monitors brightness notifications plus a 250 ms watchdog. If the setting drifts, it reapplies
the target and requires a fresh one-second settling period. Leaving the app or disconnecting the
display restores the previous brightness and idle-timer setting; returning foreground resumes the
chart's target and starts a new settling period. The CLI `show ios` keeps the session open until
Enter is pressed so the target remains held while viewing the chart.

The player reports the requested/read-back brightness, hold/settling state, thermal state, Low Power
Mode, and a monotonic interruption revision. Before the shutter, the host requires a settled hold;
after download it verifies the same revision. A temporary dip, background transition, thermal or
power-state change invalidates the measurement even if brightness has recovered. RAW is still
saved, but that capture cannot become a valid reference or report input. Install the updated player
before new iPhone captures; stored historical captures remain readable.

App brightness is a requested system setting, not a luminance measurement. Public iOS APIs cannot
disable Auto-Brightness/True Tone/Night Shift or bypass thermal dimming. Disable those display
adjustments manually; the host blocks capture in serious/critical/unknown thermal states and Low
Power Mode. Brightness read-back and thermal telemetry cannot certify constant physical light
output. See [UIScreen brightness](https://developer.apple.com/documentation/uikit/uiscreen/brightness),
[idle timer](https://developer.apple.com/documentation/uikit/uiapplication/isidletimerdisabled), and
[Apple temperature behavior](https://support.apple.com/en-us/118431).

Run the hardware-independent lifecycle tests with
`sh tools/screen-test-player/ios/test-brightness.sh`; host acceptance tests are included in
`cargo test --lib screen_test::device::tests`.

The updated player was installed on the connected iPhone 16 and verified over USB at 35%, 65% and
50%. At 50%, 68 status samples over 35 seconds retained the target and interruption revision with
the idle timer disabled. Explicit release restored the initial 100% setting and re-enabled the idle
timer (UIScreen read-back completes asynchronously). This verifies control/telemetry, not optical
luminance or actual thermal dimming. Lifecycle/thermal/drift paths are covered with fake hardware.

## Build and USB players

```sh
brew install pkg-config libraw opencv libomp libusbmuxd xcodegen
tools/screen-test-native/build.sh
cargo build --bin camera-studio --bin screen-test
tools/screen-test-player/android/build.sh
```

The superproject's `scripts/dev.zsh` builds the native helper and Android Screen Test APK before
starting the editor. A standalone Cargo build does not produce these components. After cleaning
`target/`, rebuild the native helper before calibrating: chart generation uses its ArUco dictionary,
so a missing helper aborts calibration and closes the temporary phone player even if the APK
launched successfully.

The Android build uses the local Android SDK/JBR, Gradle wrapper and Kotlin/OpenGL ES 3. It requires
Android 15/API 35 or later. Install the generated
`tools/screen-test-player/android/app/build/outputs/apk/debug/app-debug.apk` through the Screen Test
panel, or `adb install -r`. USB debugging must be authorized. Connecting starts the player and uses
an ADB port forward; Wi-Fi devices are not selected automatically.

The iPhone player requires Xcode, iOS 18 or later, a development signing team, Developer Mode and
device trust. Unlock the phone while installing:

```sh
DEVELOPMENT_TEAM=YOUR_TEAM tools/screen-test-player/ios/build.sh device
xcrun devicectl list devices
xcrun devicectl device install app --device YOUR_DEVICE \
  tools/screen-test-player/ios/build/Build/Products/Debug-iphoneos/ScreenTest.app
```

Connecting resolves CoreDevice identifiers to USB UDIDs, starts the signed app and creates a local
`iproxy` tunnel restricted to USB. Blank device identifiers select a single connected device.
Both players must remain foreground. They bind their command listener to localhost, not Wi-Fi.
Closing the controller removes its ADB forward or terminates its `iproxy` child.

The helper path defaults to `target/screen-test/screen-test-native` in this repository;
`NODE_FORGE_SCREEN_TEST_NATIVE` can override it. Native helper work has a 60-second deadline.
If building Cargo into a different target directory, still run the helper build script.

## Chart and display contract

`screen-chart-2` is regenerated for the actual drawable pixels and safe insets. It keeps the source
example's circle, line, text, gray-ramp and color-target ideas; it does not stretch the original
1080×2400 bitmap or use its blurred scene output. The `.nforge` archive is unchanged.

The chart has 60 semantic patches (including spatially repeated neutral patches), 36 perimeter
ArUco markers and 3 interior markers. Six perimeter IDs encode the current generation's identity.
Native-pixel line targets and integer pixel-font targets are generated afresh at each resolution.
`manifest.json` stores Y-up pixel rectangles, native dimensions, insets, semantic colors, display
mode, generation and SHA-256 of the FP16 payload. Raster storage is top-left only at the boundary.

USB RPC uses little-endian `u32 JSON-length`, UTF-8 JSON, then `payload_bytes` bytes. Commands are
`status` and `present`; the latter includes manifest and brightness. iOS also accepts `release` to
end brightness ownership. Responses are framed JSON, including an iOS `brightness_guard` snapshot.
`pattern.rgba16f` is little-endian RGBA16F, nearest sampled without app tone mapping. Presentations
are SHA-256 checked. iOS confirms with the Metal drawable presentation callback; Android completes
GPU rendering and waits two display callbacks after buffer submission. The desktop also waits one
second and checks size, generation, brightness and HDR headroom before allowing a shutter command.
The RAW must independently contain the correct optical identity markers.

sRGB uses linear sRGB primaries, P3 uses linear Display P3, and HDR uses extended linear sRGB,
relative to SDR white. These are not PQ/HLG video outputs. The players explicitly bind their output
color spaces and request FP16 surfaces. A missing mode or insufficient/changing headroom prevents
HDR measurement. Capability reports and screenshots alone cannot establish optical HDR accuracy.
Current mobile players lock portrait orientation; the host chart/geometry code also tests landscape
dimensions, but mobile rotation is not offered in this version.

## RAW and geometry

LibRaw only opens/unpacks the original active Bayer area. It never calls processed-image APIs.
Measurements subtract the sensor black level and gather R/G1/G2/B statistics directly inside eroded
geometrically corrected patch ROIs without demosaicing or color normalization. The normalized
EXR is generated first as a geometric artifact; patch statistics use the inverse mapping into
original CFA samples to avoid resampling bias. There is no white balance, gamma, ICC transform,
denoising, sharpening, auto gain or JPEG measurement. Negative black-subtracted values survive.

OpenCV detects ArUco corners in a separate green-plane detection image. Its 1st–99.5th percentile
range is stretched before 8-bit conversion, so dark screen captures retain marker contrast and
isolated hot pixels do not control detection gain. This never modifies the original CFA values
used for color measurement. Missing/unreadable identity markers are reported separately from a
decoded generation mismatch; all six identity markers are still required.
The shared Pixel Perfecto solver
fits homography plus Brown–Conrady distortion. Independent holdout markers must be within 1 RAW
pixel RMS. Subsequent color captures check the frozen profile, never refit it; a marker residual
above 3 pixels rejects that profile for the capture without clearing the session profile.
This is a calibration of the current planar arrangement, not a general camera intrinsic model.
The marker IDs and canonical corner order determine screen orientation, so photos may be rotated
by arbitrary angles as well as tilted in perspective. RAW is kept in sensor coordinates; EXIF
orientation and canvas rotation never change the fit. See the [OpenCV marker corner contract](https://docs.opencv.org/3.4.18/d5/dae/tutorial_aruco_detection.html).
Screen-specific lens coverage is checked along the detected plane's principal axes, so rotating
a narrow phone does not change a valid fit into an X/Y coverage error. Full chart coverage,
interior landmarks, minimum image footprint and independent validation are still required.

Quality checks include minimum CFA sample count, sensor clipping, weak shadow signal, coherent row
variation after removal of a linear shading gradient, chart identity, geometry and display state.
Row variation is a conservative acquisition heuristic, not a calibrated PWM or uniformity metric.
Display-state failures remain attached to reanalyses and cannot be cleared by fitting geometry.

Clipped/dark patches can be covered by extra manually selected shutter exposures. Keep ISO,
aperture, focus, phone placement, brightness and condition labels fixed. Reference publication and
`Merge exposures and inspect` aggregate only valid per-patch responses in DN/second. Originals and
exposure membership remain immutable. The interactive workflow never brackets automatically. The batch sweep below also keeps exposure fixed and never brackets automatically.
Publication and exposure inspection also produce a normalized bracket EXR: each output sample
uses the longest unclipped shutter exposure, in fixed DN/second units. Repeated frames all
contribute patch statistics; the normalized image uses one representative per shutter time,
chosen by valid-patch count. `members.json` records both sets. One export accepts up to 16 distinct
shutter times. This is exposure fusion, not a tone-mapped HDR image.

Normalized files are floating-point EXR, plus an SDR PNG proxy and valid mask (0 outside coverage,
128 sensor clipped, 255 covered/unclipped). The EXR retains negative and above-white values. It is
camera-linear DN/second until a validated reference-derived 3×3 transform is available; only then
is it labelled extended-linear-sRGB in `encoding.json`. Preview PNGs never enter measurement.
`Saved images` opens the unrectified sensor proxy, normalized preview or geometry overlay in the
central canvas. The overlay marks detected corners in cyan and eroded measurement ROIs in amber.
The sensor proxy uses a fixed sensor-range display scale and is not a camera-rendered JPEG.

## Reference library and reports

Default storage is `~/Library/Application Support/Node Forge/Screen Tests`; override with
`NODE_FORGE_SCREEN_TEST_DIR`. SQLite indexes immutable captures, reference versions/members and
reports. Original CR2s are content addressed, atomically saved and checksum verified before
publishing records. Large files stay outside SQLite. Back up the whole directory, not only the DB.

Publish selected reference captures only when camera/lens/ISO/aperture/focal length, phone identity,
display mode, chart semantics, brightness, headroom and batch/notes are compatible. Different
brightness conditions are separate baselines. Reanalysis creates a new capture analysis record;
it does not overwrite source RAW, an existing baseline or a report.

The primary baseline is the actual iPhone CFA response, not ideal chart RGB. A derived 3×3 transform
may be fitted with held-out colors; high error leaves that optional transform unavailable. Android
is never refitted or white-balanced against its own chart. The RGB and white-response differences
therefore preserve actual brightness/color bias. They are relative camera measurements, not CIE
Delta E or absolute nits. A camera is not a substitute for a traceable colorimeter/spectroradiometer.

Reports contain exact baseline members, source exposures, conditions, geometry, patch differences,
locations and configured thresholds. The report window includes a patch map. Red is outside an
explicit threshold, amber has no valid measurement, neutral is measured. With no thresholds the
conclusion says **no pass/fail thresholds configured**, never an implied certification.
The central-canvas difference map uses a fixed 0–10% of reference-white scale. It visualizes
semantic patch differences, not direct pixel subtraction between differently sized phone charts.

## Full iPhone reference sweep

```sh
# Close the Camera Studio camera session first; keep iPhone unlocked and the full screen framed.
# Canon: mode M, fixed ISO/aperture/focus; USB and SD card ready.
target/debug/screen-test calibrate-sweep [IPHONE_USB_UDID]
```

This is one batch with **33 conditions**: sRGB, Display P3, HDR-linear, each at requested brightness
0.0, 0.1, …, 1.0. Zero is sent as zero, never substituted with 0.01. HDR-linear uses extended linear
sRGB with peak 2× SDR white. Insufficient actual EDR headroom fails that condition without a shutter;
no SDR fallback or relabelled peak is allowed.

The runner exclusively uses CameraService's existing serial worker. It deploys the current signed
iPhone player (including zero-brightness support), validates native presentation and brightness,
and explicitly calibrates geometry once at 100% phone brightness for the current placement in temporary session storage.
Every color RAW must then pass fresh optical identity, geometry and display validation and produce
its rectified artifacts before measurement. Failed per-image geometry is recorded without color
measurement; changed display conditions or camera settings abort further captures. The standalone CLI session has its own geometry; it does not pretend to share an active
profile with another process. Geometry-only captures are not added to the reference database.

The batch reads and locks the existing manual shutter, aperture and ISO for **every condition**,
including the geometry capture. It never changes exposure or adds brackets in response to phone
brightness. Camera settings are checked before each shutter and against RAW metadata. The run
stores the exact fixed settings. A condition has one RAW; clipped/noisy/undetectable captures are
retained with quality failures and cannot become verified baselines. Only the phone's display mode
and brightness vary. There is no automatic repeat after a capture/download error. The original
photo format is restored after every RAW transaction, and phone brightness is restored on exit.

SQLite `calibration_runs` and `runs/<batch-id>.json` record all planned conditions, captures, exact
reference IDs and failures incrementally. Each condition publishes a separate immutable baseline;
different brightness/mode conditions never merge. A partial run remains partial and the CLI exits
unsuccessfully unless all 33 references were saved and cleanup succeeded. Re-running starts a new
batch; it does not silently overwrite an earlier baseline. Back up the entire storage directory,
including RAW and normalized artifacts. The batch code and synthetic tests do not establish actual
camera/phone calibration accuracy; use the run's recorded physical results.

### Recalibrate a saved reference photograph

```sh
target/debug/screen-test recalibrate-reference CAPTURE_ID
```

This offline command fits geometry from that photograph's own markers and checks independent
holdout markers before rectifying and measuring its original CFA pixels. It never connects to a
phone/camera or changes the shared session geometry. Display-state failures cannot be cleared by
this operation. The source capture, its RAW, and the original sweep remain unchanged; a new
analysis and, when all patches pass, a new reference version are appended to the same library.

`results/<recalibration-id>/result.json` links the source, new analysis, reference, geometric
holdout, valid-patch count, color transform and errors. A color transform can be tested on the
valid subset even if some patches fail, but this does not publish an incomplete baseline.
`color/normalized.exr` and `color/preview.png` show the fitted result when the color-transform
holdout passes. This chart-relative matrix demonstrates the processing pipeline, not independent
absolute colorimetry; preview images never feed back into RAW measurements.

## Diagnostics and verification

```sh
cargo test --lib screen_test::
cargo test --lib native_chart_detection_roundtrip -- --ignored
cargo test --lib camera_studio::
cargo test --lib pixel_perfecto::calibration::
cargo run --bin screen-test -- raw original.CR2
cargo run --bin screen-test -- detect original.CR2 /tmp/raw-marker-diagnostics
cargo run --bin screen-test -- generate 1260 2800 srgb /tmp/screen-chart
cargo run --bin screen-test -- show ios USB_UDID srgb 0.5 1 /tmp/iphone-chart
cargo run --bin screen-test -- show android USB_SERIAL hdr 0.5 2 /tmp/android-chart
```

Only the explicit `calibrate-sweep` command opens a camera. The other diagnostic commands, including
`geometry` and `measure`, never open a camera; see `screen-test --help`. Camera Studio UI fixture states `screen-geometry`,
`screen-reference` and `screen-inspection` render the real native UI with isolated simulated data
and save screenshots, without constructing any camera/phone worker.

Verified on this Mac: native LibRaw/OpenCV helper build; original EOS 650D CR2 unpack (5208×3476
active CFA, with per-channel black levels and exposure); Android APK build/install; signed iPhone
device build/install; iPhone 16 USB presentation in sRGB, P3 and linear HDR at native 1179×2556.
At brightness 50%, the tested HDR presentation reported 8× headroom and accepted peak 2× SDR white.
This is software/display-path verification, not an optical color/geometry calibration result.

Pending physical acceptance: Canon photographs of both displayed charts, repeated-capture
stability, geometric holdout on those photographs, reference-transform residuals, real clipping/
banding behavior and measured phone differences. UI fixtures and synthetic tests do not establish
these results. Canon remote focus verification remains independent of this feature.
