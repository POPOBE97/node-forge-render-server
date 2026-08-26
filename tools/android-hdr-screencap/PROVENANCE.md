# node-forge-hdr-screencap provenance

## Target device (fingerprint-locked)

- ABI: `arm64-v8a`
- SDK: `36` (Android 16)
- Fingerprint: `nubia/Nerida/Nerida:16/BQ2A.251016.001-BP2A.250605.031.A3/b1577:userdebug/test-keys`
- Root: `su 0 ...` (do not `adb root`)
- Linker: this ROM maps `/data/local/tmp` to the `unrestricted` namespace
  (`namespace.default.isolated = false`, search paths include `/system/${LIB}`,
  `/system_ext/${LIB}`, `/odm/${LIB}`, `/vendor/${LIB}`, `/product/${LIB}`).

## Sources

- AOSP `frameworks/base/cmds/screencap/screencap.cpp` (Android 16 / API 36 ScreenCapture path)
- AOSP `libs/gui` `ScreenshotClient::captureDisplay` / `DisplayCaptureArgs`
- Shared libraries required at runtime: `libgui`, `libbinder`, `libui`, `libutils`,
  `libcutils`, `liblog`, `libsync` from `/system/lib64`
- SELinux: userdebug root execution from `/data/local/tmp` on the measured ROM

## Skia pin (host gainmap reconstruction)

- `d1d26bf84dc788a4459ae491c1acdad02663fa9c` (PNG gmAP/gdAT, ISO 21496-1, gainmap color space)

## Reproducible AOSP build

The prebuilt must be produced against this OEM `/system/lib64` ABI, not generic
headers alone, then smoke-tested on the fingerprint above.

```
source build/envsetup.sh
lunch <matching-arm64-userdebug-target>
m node-forge-hdr-screencap
```

Copy the resulting arm64 binary to:

```
tools/android-hdr-screencap/prebuilt/arm64-v8a/node-forge-hdr-screencap
```

Record its SHA-256 next to the binary. Track the binary in Git, not Git LFS.

## Smoke test (required before shipping a prebuilt)

On the recorded ROM, with HDR content visible:

1. `adb push` the binary to `/data/local/tmp/node-forge/node-forge-hdr-screencap`
2. `chmod 0755`
3. Launch via `su 0` / already-root `adb exec-out`
4. Confirm the process starts, resolves platform libraries, reaches SurfaceFlinger,
   returns `RGBA_FP16` + `V0_SCRGB_LINEAR`, and with `capturedHdrLayers=true`
   produces finite raw values above `1.0`
5. Confirm stdout begins with `NFRCLIP\0` and has no root-banner prefix

A prebuilt that cannot do this is not authoritative and must not be checked in.

Host Render Server embeds the prebuilt with `include_bytes!` only on native
non-Wasm targets, and refuses to execute it unless ABI, SDK, complete
fingerprint, and the unrestricted `/data/local/tmp` linker mapping all match.
