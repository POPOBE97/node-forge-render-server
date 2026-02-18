# RenderPass MSAA Sample Count Adaptation

## Summary
This document describes the render-server changes required to support the inline
`RenderPass.msaaSampleCount` selector.

Accepted values:

- `1`: no multisampling (`1x`)
- `2`: MSAA 2x
- `4`: MSAA 4x
- `8`: MSAA 8x

## Contract

### Node schema

- `RenderPass.inputs` includes `msaaSampleCount: int` with default `1`
- `RenderPass.defaultParams.msaaSampleCount = 1`
- Canonical schema path is `assets/node-scheme.json`

### Validation

- `RenderPass.params.msaaSampleCount` must be one of `{1,2,4,8}`
- Any other value fails scene validation with a clear error including node id

### Runtime behavior

For each `RenderPass`:

1. Resolve `msaaSampleCount` from connected input first, then params/default.
2. Validate the requested value is in `{1,2,4,8}`.
3. If requested sample count is unsupported for the format/device, downgrade in
   descending order: `8 -> 4 -> 2 -> 1`.
4. Build supported sample counts from
   `TextureFormat::guaranteed_format_features(device.features())`.
5. If `device.features()` includes
   `TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` and adapter is available, refine
   support with
   `adapter.get_texture_format_features(format).flags.supported_sample_counts()`.
6. Log a warning with node id, texture format, requested count, supported
   counts, and the downgraded effective count.

`TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES` is requested at device creation when
the adapter reports support, so `2x` can be used where available.

### Per-pass isolate strategy

For `RenderPass` with effective sample count `>1`:

- Allocate a dedicated multisampled color attachment texture.
- Render to that MSAA texture and resolve into a single-sample texture.
- Keep downstream sampling and composition on single-sample textures.

This allows mixed MSAA settings per pass and avoids coupling all layers to one
global sample count.

## Implementation Checklist

1. Add schema fields in `assets/node-scheme.json`.
2. Add validation in `src/schema.rs`.
3. Add adapter injection in `src/renderer/shader_space/api.rs`.
4. Expose adapter from headless renderer in `3rd/rust-wgpu-fiber/src/headless.rs`.
5. Extend vendored fiber pass + texture APIs for sample count and resolve target:
   - `3rd/rust-wgpu-fiber/src/pass/mod.rs`
   - `3rd/rust-wgpu-fiber/src/pool/texture_pool.rs`
   - `3rd/rust-wgpu-fiber/src/shader_space.rs`
6. Add runtime selection + fallback + per-pass resolve handling in
   `src/renderer/shader_space/assembler.rs`.
7. Wire adapter through callsites:
   - `src/main.rs`
   - `src/app/scene_runtime.rs`
   - `src/renderer/shader_space/headless.rs`
   - `tests/instanced_math_closure.rs`
   - `tests/render_cases.rs`
8. Add tests for schema contract, value validation, and fallback behavior.

## Test Commands

```bash
cargo test --test file_render_target
cargo test --test scene_delta
cargo test --test instanced_math_closure
cargo test --test render_cases
cargo test
```

## Defaults and Assumptions

- Unsupported requested MSAA downgrades `8 -> 4 -> 2 -> 1` with a warning.
- No depth-attachment MSAA behavior is introduced in this change.
