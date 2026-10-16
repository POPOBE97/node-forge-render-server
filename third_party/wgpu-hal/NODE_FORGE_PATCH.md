# Local wgpu-hal 30.0.1 patch

Source: the crates.io `wgpu-hal` 30.0.1 package. Original licenses are included.

`src/metal/surface.rs`: fix the dynamic lookup of CoreGraphics color-space constants.
The exported symbol is a `CFStringRef` variable. `libloading::Library::get` returns its
address, so the variable must be dereferenced to obtain the CFString object. Treating
the variable address as the object crashes in `CGColorSpaceCreateWithName` when a
window first switches to `ExtendedDisplayP3` (also affects the PQ/HLG constants).

The same file explicitly assigns `kCGColorSpaceSRGB` for ordinary sRGB surfaces.
`CAMetalLayer.colorspace = nil` disables Core Animation color matching; it does not
declare sRGB. On a P3 display this made sRGB output appear as display-native RGB.
Color-space creation failure is reported instead of silently disabling color matching.

Validation:

- `cargo test --manifest-path third_party/wgpu-hal/Cargo.toml --lib --features metal native_layer_color_space_switches_keep_color_matching_enabled`
  reads the actual layer color-space name and EDR flag while switching ordinary and
  extended sRGB/P3. This test fails with the upstream nil mapping.
- `cargo run --locked --example verify_window_color_space` configures a real native
  window and checks repeated format/color-space switches through the application path.

Remove these patches when upgrading to an upstream version containing both fixes.
