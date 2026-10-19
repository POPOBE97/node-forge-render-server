# Procedural Test Texture

Add **Texture → Procedural Test Texture** and connect its `texture` output where an
`ImageTexture.texture` was used, for example to `ImagePass.image`, a filter, or a shader texture input.
The node includes the complete procedural chart, Gaussian blur samples, resolution label, and
native-density text. It needs no image upload, external path, font connection, or document resource.

| Interface | Meaning |
| --- | --- |
| `width` | Texture width in physical pixels; integer 1–8192, default 1260 |
| `height` | Texture height in physical pixels; integer 1–8192, default 2800 |
| `texture` | Sampleable `rgba16float` texture in the scene's linear working color space |

Each node owns its source resolution independently of downstream render targets. Changing dimensions
recomputes layout and the resolution label; line widths, font sizes, and blur radii retain their
physical-pixel meaning. Small or unusually shaped textures can clip fixed-size samples. Scaling the
texture downstream has the same sampling implications as scaling an image.

Like the original chart graph, the node emits numeric RGB in the scene's linear working color space.
Changing **Working Color Space** on Screen reinterprets those values: the primary-color patches become
sRGB primaries in a linear sRGB scene and P3 primaries in a linear Display P3 scene. The node does not
convert from a fixed P3 source or preserve P3 appearance across working-space changes. HDR values above
1 remain intact; use floating-point downstream targets to retain them. Screen's **Output Color Space**
controls the final output conversion separately.

## Implementation and maintenance

`ProceduralTestTexture` lowers to the existing RenderPass, Composite, and TextPass graph during scene
preparation. The public node persists only its size, while shaders and fonts ship with the renderer.
Ordinary documents do not import or embed the full research graph. No bitmap of the chart is stored.

`node-forge-editor/examples/procedural-rendering-test.nforge` remains the authoring source. From the
superproject root, run `node scripts/sync-procedural-test-texture.mjs` after editing that archive through
the CLI/editor. This explicitly exports its graph, three materials, and fonts to renderer resources;
the existing identical MiSans font is shared. `--check` detects stale resources and is included in
`scripts/check-workspace.zsh`. Submodule builds never regenerate sibling-repository files.

The minimal reusable-node example is `node-forge-editor/examples/procedural-test-texture.nforge`.
Rebuild it with `node scripts/procedural-rendering-test/build-texture-example.mjs --force` from the
editor repository. The original example remains the full authoring graph.

Validation: `cargo test --test procedural_test_texture` checks source-size isolation, connected size
inputs, invalid dimensions, and GPU pixel parity with the canonical chart at regular and non-divisible
resolutions, in both working color spaces. Native Metal/GPU access is required for the pixel test.
