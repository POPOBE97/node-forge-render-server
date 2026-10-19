# Camera Studio

Camera Studio is a native camera acquisition workspace for the Canon EOS Rebel T4i / 650D on
Apple Silicon macOS. Open **Camera Studio** from the Editor's native **Window** menu, alongside
Pixel Perfecto and Android Mirror, or run it independently:

```sh
brew install libgphoto2 gphoto2
cargo run --bin camera-studio
cargo run --bin camera-studio -- --review /absolute/path/to/photo.jpg
# Diagnose connection/release without preview, capture, recording or downloading:
cargo run --bin camera-studio -- --check-connection --release-occupied
```

The standalone entry does not start the WebSocket server, scene loader or animation runtime.
Opening either window does not connect to a camera until **Connect camera** is clicked.
The main Render Server command and its existing workflows remain the same.

## Shooting

1. Connect one powered-on T4i / 650D by USB. Phones may remain connected: PTP discovery also lists
   iPhones, so Camera Studio filters for the supported Canon model and binds its exact USB port.
   It refuses multiple matching Canon cameras and never falls back to opening a phone.
2. Click **Connect camera**. A successful connection starts Live View once. If preview fails,
   the camera stays connected and **Retry preview** retries only the viewfinder.
3. Adjust the available ISO, shutter, aperture, exposure compensation, white balance and image
   settings. Options are read from the camera and checked again at write time. Camera modes can
   restrict these choices; use the physical mode dial where required.
4. For manual focus, keep the lens switch in **AF** with Live View active. Select Fine, Medium or
   Coarse, then click Near/Far (one click sends one step). The worker reads the current settings,
   turns off Continuous AF / Movie Servo AF when enabled, and verifies the readback before driving
   the lens. These settings stay off so autofocus cannot undo the adjustment; re-enable them in
   Advanced when wanted. Focus controls return photo review to Live. A command acknowledgement is
   displayed without claiming that physical lens movement was measured. If Fine is hard to see,
   try Coarse. Steps are relative, not calibrated distance coordinates.
5. Click **Take photo**. Photos are saved to the camera card and downloaded without deleting the
   camera originals. RAW+JPEG is useful for later measurement: CR2 is retained, and the JPEG can
   be selected for color-managed review. RAW-only captures are saved but not developed in this v1.
6. For video, insert an SD card and set the T4i's physical power switch to movie mode. Click
   **Record video**, then **Stop recording**. This records a real camera MOV to the card, not a
   recording of the low-resolution preview. The camera's own stop is also monitored.
   The record button requires the camera to offer the writable `Card` recording choice and is
   disabled when the physical movie switch reports still-photo mode.
7. New photographs appear in the bottom filmstrip without replacing the live picture or an older
   photograph being reviewed. Select a card to review its JPEG, and choose **← Live** to return.
   **100%** accounts for display scaling: one image pixel occupies one physical display pixel.
   Right-click a card for its files, capture settings, folder and download recovery.

## Workspace controls

The 340-point left sidebar keeps the device and a compact shooting row visible. Photo/Video
and the capture button share that row, using the same 28-point controls and 13-point text as the
rest of Node Forge. Exposure, Focus, Output and Advanced occupy a separately scrolling area. Exposure uses
paired controls; less frequent image/AF settings and test-pattern metadata live under Advanced.
The output-folder menu opens or changes the destination. Distinct camera format properties are
kept separate. USB-release preferences and technical logs are under **••• → Diagnostics**.

The canvas defaults to centered Fit. Scroll to pan, pinch to zoom around the pointer, or drag with
the middle mouse button. Fit, 100% and Rotate are always available. Live and review keep separate
transforms; rotation changes presentation only. Pausing keeps the last frame with a **Preview
paused** label. Disconnection and stale frames are distinguished from an active live feed.

The Photo/Video selector changes the controls without sending camera commands. The physical
camera switch and available settings still determine which operations are supported. Only
**Record video** starts a movie. Recording replaces that button with **Stop recording** and a
timer; stopping remains visible until the camera confirms the transition.

The filmstrip is 112 points high and can be hidden. RAW/JPEG companions share a card. RAW-only
captures use their embedded JPEG for review; movie captures have type placeholders and open in
the system player. Failed transfers
have a per-record retry action that never repeats the shutter. Historical empty records show
**No files received**, rather than pretending that a camera task is still active.

Keyboard shortcuts apply only to the active window outside text fields, menus and dialogs:
**⌘Enter** takes a photo, **F** fits, **1** selects 100%, and **Esc** returns to Live.
There is no single-key movie-record shortcut.

T4i stops a recording at 29:59 and splits files at 4 GB. Completed segments from one recording
are associated with one capture record. Movie downloads wait for recording to stop and for the
camera to finish publishing its files. Actual preview performance and simultaneous movie preview
depend on the camera mode and remain subject to hardware validation.

Live View and movie recording use separate commands. Canon's `movierecordtarget=SDRAM` is also
reported during still-photo Live View; only `Card` confirms recording to the card. `SDRAM` must
not start a REC timer, create a movie record or block **Stop preview**. An already idle/SDRAM
camera receives no movie-stop write. After a real stop command, the REC state remains until the
camera confirms it has stopped. The `eosmoviemode` action widget is not a recording-status flag.

## Files and recovery

The default directory is `~/Pictures/Node Forge Captures`. Override it with
`NODE_FORGE_CAPTURE_DIR`, or select a folder in the window. Every capture has its own directory:

```text
capture-<time>-<sequence>/
  capture.json
  files/<camera-folder-hash>/IMG_0001.CR2
  files/<camera-folder-hash>/IMG_0001.JPG
```

`capture.json` records the camera's settings and identity, host capture time, original camera paths,
pending downloads, local paths, byte sizes, SHA-256 hashes and optional test-pattern ID. It is
updated atomically. Files are streamed through a temporary file, synced, hashed and published
without overwriting an existing capture. The original JPEG/CR2/MOV bytes and EXIF are preserved.
If publication succeeded but saving the manifest failed, a retry compares the existing file's
SHA-256 against a fresh download before completing the record; it never replaces different bytes.
RAW/JPEG companions are grouped by their camera folder and filename stem. Duplicate camera events
do not download the same object twice.

**Recover downloads** retries pending transfers and reconciles newly appearing card files.
It never repeats a shutter command. Reconnect the same camera after a disconnection. Pending file
paths are persisted so a subsequent connection to the same capture directory can recover them.
Existing files already on the card at the first connection are not automatically imported.
Capture history is loaded from the selected destination folder, including when no camera is attached.

When closing during capture or recording, choose to finish the work first. If transfer recovery is
blocked, **Close; keep pending on card** closes after the active operation finishes; failed transfer
entries remain in the manifest and camera originals remain on the card. Abrupt process termination
or power loss can still interrupt a camera operation. A leftover `.partial` file is never silently
overwritten or deleted by a later retry; inspect or move it before retrying that transfer.

## Implementation boundaries

- `camera_studio::model` defines typed commands/events, parameter descriptors, capture records and
  the `CameraBackend` interface. `CameraService` owns the backend on one dedicated worker thread.
- `camera_studio::gphoto` runtime-loads libgphoto2's C API. It does not spawn CLI processes for
  controls or frames and does not depend on the old Intel EDSDK from EOS Utility 2.
- `NODE_FORGE_GPHOTO2_LIBRARY` can select a specific libgphoto2 library. Homebrew macOS paths and
  standard Unix library names are tried otherwise. No link-time camera dependency is imposed on
  machines building the renderer. This v1 uses a locally installed library; bundling an application
  for other machines needs the library and its driver modules/dependencies plus license notices.
- All camera I/O and preview decoding happen on the worker. Preview uses a one-frame mailbox;
  acquisition and transfer events use a separate reliable channel. Original-image decoding uses
  a separate background job. The window uses egui textures on the existing renderer device.
- Captures are independent of SceneDSL, `.nforge`, MotionEngine, and render graph schemas. Original
  camera raster data stays at the file/presentation boundary; no new public coordinate ports are
  introduced. Later calibration must use the workspace's bottom-left/Y-up coordinate ABI.
- Photo review reuses Pixel Perfecto's ICC-aware decoder. Its displayed SDR proxy is not a raw
  measurement. Absolute display colorimetry still needs a validated camera calibration and a
  reference instrument. Phone pattern sequencing, automatic focus, geometric fitting and display
  correction/LUT application are outside v1.

## Validation

```sh
cargo test --lib camera_studio
cargo test --lib camera_studio::gphoto::tests::runtime_library_loads_without_connecting -- --ignored
cargo test --lib pixel_perfecto
cargo check --bin camera-studio --bin node-forge-render-server
```

Tests exercise paired RAW/JPEG events, duplicate suppression, ambiguous shutter responses, failed
streaming writes and retry, persisted recovery, multipart movie recording/natural stop, safe file
publication, existing-card inventory and overlapping command rejection.
They also cover USB-release selection/retry limits, temporary USB disappearance after release,
recovery timeout, protection of an existing Camera Studio
session, late RAW companions, camera-initiated consecutive recordings, metadata recovery after
file publication, offline window reuse and close guards. Regression coverage uses the T4i's
reported SDRAM-only Live View choices to verify no false recording, no invalid stop write,
working preview stop/disconnect, rejected unavailable movie starts, and camera-confirmed stops
before downloading.

UI regression tests cover connection-only automatic preview (including failure and retry),
record-scoped transfer recovery and errors, selection stability across capture events, stale
original-image decode rejection, independent transforms and physical-pixel/zoom anchoring.
Thumbnail decoding runs off the UI thread with a 96-entry cache; one original decode runs at a
time, with newer selections replacing pending requests.

For repeatable native-window visual acceptance without opening a USB session:

```sh
cargo run --bin camera-studio -- --ui-preview /absolute/path/to/photo.jpg --state live
```

The visibly marked **UI PREVIEW · simulated camera** menu switches between live, review,
recording, stopping, downloading, transfer error, disconnected and paused states. It also sets
1000×680 / 1400×900 window sizes. **Save all acceptance screenshots** captures those eight
states at 1400×900 and live at 1000×680 to `tmp/camera-studio-ui/`, using the native framebuffer
and current display pixel scale. These fixtures validate UI behavior only, never device protocol.

The standalone native window was checked with `IMG_0077.JPG`: EXIF orientation, Fit/1:1 review
and idle close work. Both executable targets build, and the main Render Server initializes its
GPU/rendering successfully. Its sidebar click-through could not be completed after the Mac locked;
the window host is additionally exercised by an offline egui test.

Hardware status for this implementation session:

- Confirmed USB ID `04a9:323b`, successful libgphoto2 session, firmware reported as `3-1.0.1`, lens
  EF-S18-135mm f/3.5-5.6 IS STM and the camera's live parameter/choice list.
- Confirmed a real macOS USB-claim conflict while EOS Utility / ptpcamerad held the device, followed
  by successful enumeration and settings reads after releasing those clients.
- Connection recovery was subsequently reproduced and verified on the T4i after a reported USB
  ownership error: EOSWebcamService and ptpcamerad were released, the first reconnect encountered
  transient USB unavailability, and the second connected as Canon EOS 650D with 82 parameters.
  The diagnostic then closed the session successfully. This validates connection recovery, not capture.
- A user-reported false REC state was traced to a saved camera snapshot with
  `movierecordtarget=SDRAM` and `eosmovieswitch=0`. The state interpretation and command guards
  are fixed and covered by mock regression tests. Actual recording on the camera during that
  report was not confirmed; no active camera session was interrupted to test the fix.
- Further shooting tests were deferred at the user's request after the camera disconnected. No
  preview frame, parameter write, focus drive, still capture or movie recording has been claimed
  as validated in this session.
- Pending: 10-minute live preview, 20 consecutive RAW+JPEG captures, focus response, parameter
  readback, short MOV and large multi-segment movie download, camera-initiated recording stop,
  unplug/reconnect, no/full card and camera-busy behavior. No simulated test establishes physical
  camera throughput, image quality or the correctness of a real recorded MOV.

## Releasing USB occupancy on macOS

**Release other camera apps if busy** is enabled by default on macOS. A connection attempt first
tries to open the camera normally. If libgphoto2 reports USB occupancy, Camera Studio terminates
known camera clients belonging to the current user, then attempts reconnection. The status
line identifies the released processes and PIDs. It sends SIGTERM, waits up to 800 ms, and uses
SIGKILL for matching processes that remain. Their unfinished camera work may be interrupted.

Terminating those clients can temporarily make the device disappear from libgphoto2's USB list.
Reconnection therefore waits and re-enumerates, up to seven attempts with 7.5 seconds of total
backoff (USB calls can add time). Only missing/disconnected-device errors are retried; multiple
matching Canon cameras, an active Camera Studio session and other errors stop recovery. Process termination
runs only once per Connect action. A service that reclaims the camera is reported distinctly
from a camera that did not reappear. Capture/shutter commands are never retried by this mechanism.

The allowlist covers Canon EOS Utility / launcher under `/Applications/Canon Utilities`, Apple's
Image Capture / PTP camera processes at their system paths, and Canon's installed EOSWebcamService.
Both executable path and user identity are checked before signaling. An active Camera Studio
session is protected, and unrelated processes or services belonging to another user are excluded.
Disable the checkbox to manage camera ownership manually. If another application holds the USB
interface or a service immediately reclaims it, recovery reports the remaining error.

The release selection and retry policy are covered by mock tests. The connection-only diagnostic
has verified real process release and USB reconnection on this Mac; shooting validation remains pending.

Screen measurement workflows are documented separately in [Screen Test](screen-test.md): independent
session geometry, a persistent iPhone RAW baseline library, and Android comparison reports.

After a successful canvas **Calibrate**, Live View, subsequent photo review and capture thumbnails
automatically show the rectified phone screen at its native aspect ratio. The same session mapping
includes perspective and lens distortion; the display adapter accounts for the 650D RAW/JPEG sensor
crop and EXIF rotation. **Corrected / Original** switches the display without changing originals.
Each new capture keeps its session geometry association, so recalibrating does not change the
appearance of earlier captures. Imported files and captures from a previous App session are not
silently assigned the current placement. Geometry and its prior revisions remain in memory only.

Each calibrated capture also saves corrected output in its capture folder. A capture containing
CR2 produces `results/rectified.dng`: a 16-bit Bayer CFA RAW with the original camera color
matrix, white balance, black/white levels and capture EXIF. Geometric resampling reconstructs each
output value only from photosites of the same color; it never substitutes a demosaiced PNG for RAW.
The original CR2 remains byte-for-byte unchanged. `results/rectified.png` is also written as the
review/thumbnail preview, and `results/rectified.json` records hashes and provenance for both
outputs. JPEG-only captures produce only the PNG. Processing happens asynchronously after
download; the card shows **Correcting…** until the files are saved. Capture details offers **Open
corrected RAW**, **Open preview** and **Open original** separately. Saved results survive App
restart without restoring a calibration session. Failed writes expose **Retry correction**, which
only reprocesses downloaded images and never triggers a shutter. Closing waits for pending result
writes. Older captures with only an expired session association explicitly show that no corrected
result was saved; a new placement is never applied to them automatically.

RAW-only captures can be reviewed using the CR2's embedded JPEG. This is a display proxy only:
original JPEG/CR2 bytes remain intact, and color measurements continue to use unprocessed CFA data.
Moving the phone or camera, changing focal length, or changing the camera's viewfinder crop requires
another calibration. Display rectification cannot recover detail absent from the Live View feed.

Offline UI acceptance supports `--state corrected-live`, `corrected-review` and
`corrected-live-small` with `NODE_FORGE_CAMERA_FIXTURE_GEOMETRY` pointing to explicit fixture JSON
(`profile`, `screen_size`, `sensor_crop`). These fixtures do not connect to a camera or load geometry
into a production session.

References: [gPhoto supported cameras](https://gphoto.sourceforge.io/proj/libgphoto2/support.php),
[remote capture documentation](https://gphoto.sourceforge.io/doc/remote/),
[Canon T4i file and recording limits](https://sg.canon/en/support/8201509800).
