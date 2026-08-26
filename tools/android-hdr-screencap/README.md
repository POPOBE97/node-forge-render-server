# Android HDR screencap helper

Minimal AOSP-derived native helper used by Node Forge Render Server for
`⌘⌥3` / `⌘⌥4`. One rooted process performs:

1. `RGBA_FP16` + `V0_SCRGB_LINEAR` SurfaceFlinger capture (private authority)
2. Adjacent stock `screencap -p` encode (public PNG, including gainmap chunks
   when the ROM encoder produces them)

See `PROVENANCE.md` for the fingerprint lock, shared-library/SELinux notes,
AOSP revision, and the required smoke test. The arm64 prebuilt lives at
`prebuilt/arm64-v8a/node-forge-hdr-screencap` once that smoke test passes.

Until a fingerprint-tested prebuilt is present, Render Server skips helper
deploy and uses stock `adb exec-out screencap -p` (Android 16 gainmap PNG
fallback on this ROM).
