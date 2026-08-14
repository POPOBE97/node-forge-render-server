# Rect2DGeometry corner radius plan

## Decision

Keep `Rect2DGeometry` as a rectangular quad and apply analytic rounded-corner coverage only in the
fragment shader of the source `RenderPass`. Post-processing passes operate on the already masked
premultiplied texture; mask metadata does not propagate through Blur, Bloom, Downsample, or
Composite.

The shape implementation is shared with `Sdf2D(shape=smooth_round_rect)`. Both paths load the
canonical `sdf2d.wgsl` library and call `sdf2d_smooth_round_rect`; the library key deduplicates the
declaration when a shader uses both paths.

## Interface contract

- Input precedence is connection > inline param > port default.
- `radius` is measured in transformed local pixels and clamped to
  `0..min(in.geo_size_px) / 2`.
- `smooth` is clamped to `0..1` and becomes `axisMix = vec2f(smooth)`.
- Static `radius <= 0` does not inject the mask or SDF library.
- A connected radius keeps the mask compiled even when its current value is non-positive, allowing
  GraphInputs/uniform-only updates without pipeline reconstruction. Runtime radius values at or
  below zero select coverage `1`.
- No DSL, schema, or `.nforge` archive changes are required.

That statement applies to the Rect radius feature itself. RenderPass color attachment load control
is a separate DSL addition described below and does require persisted defaults.

## Outer PassTexture placement and attachment load

When an authored outer draw connects `PassTexture` as material and a rounded `Rect2DGeometry` as
geometry, the mask is already injected into that outer RenderPass. Radius/smooth expressions stay
on the Rect and are compiled through the normal geometry-consumer path; they are not copied into an
internal compose draw.

The authored RenderPass additionally exposes `loadOp` and `clearColor`:

- `clear` maps to `wgpu::LoadOp::Clear(clearColor)` and is the new-node default.
- `load` maps to `wgpu::LoadOp::Load`.
- `dont-care` maps to `wgpu::LoadOp::DontCare`.
- `none` retains planner-controlled first-writer clear / later-writer load behavior.

An outer pass can therefore clear to opaque black and draw the PassTexture island without a
separate background draw call. Existing examples are migrated to explicit `none` so adding the new
default does not change their established multi-pass composition order.

## Shape and antialiasing

Distance uses the existing local coordinate ABI:

```wgsl
let half_size = in.geo_size_px * 0.5;
let point = abs(in.local_px.xy - half_size);
let distance = sdf2d_smooth_round_rect(
    point,
    half_size,
    radius,
    vec2f(smooth),
).x;
```

The antialiasing interval extends from the zero-distance contour two screen pixels inward:

```wgsl
let pixel_distance = max(fwidth(distance), 1e-4);
let aa_width = 2.0 * pixel_distance;
let coverage = smoothstep(0.0, aa_width, -distance);
```

`fwidth` is always evaluated before dynamically selecting rounded coverage or full coverage. This
avoids derivative-uniformity violations. The `1e-4` floor keeps degenerate gradients finite.

## Fragment output order

1. Compile and evaluate the material.
2. Preserve HDR RGB and clamp material alpha to `0..1`.
3. Evaluate Rect coverage.
4. Discard when coverage is zero so rounded-off corners do not write depth.
5. Multiply the complete premultiplied RGBA value by coverage to prevent transparent RGB leakage
   into downstream filters.

## Verification

- Validate generated WGSL for static and dynamically connected corner inputs.
- Assert Rect and Sdf2D emit one shared `sdf2d_smooth_round_rect` declaration.
- Assert static zero/negative radius uses the unmasked fast path.
- Assert `fwidth`, the two-pixel inward width, full-coverage dynamic fallback, discard, and full RGBA
  multiplication appear in the generated fragment shader.
- Assert radius/smooth GraphInputs changes remain uniform-only scene deltas.
- Exercise geometry wrapper resolution (TransformGeometry and instancing chains) and retain existing
  render/golden coverage for transformed and post-processed scenes.

## Status

Implemented in the render-pass WGSL builder with targeted shader validation and scene-delta tests.
Authored RenderPass load operations and clear colors are also implemented in the planner, schema,
editor controls, and canonical example archives.
