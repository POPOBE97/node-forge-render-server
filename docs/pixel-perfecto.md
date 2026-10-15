# Pixel Perfecto

Pixel Perfecto aligns microscope photographs of Diamond RGB OLED subpixels to a shared lattice. It is independent of SceneDSL and does not modify `.nforge` archives.

## Open

From this repository:

```sh
cargo run --bin pixel-perfecto
cargo run --bin pixel-perfecto -- photo-a.png photo-b.jpg
cargo run --bin pixel-perfecto -- comparison.pixelperfecto
```

Alternatively, choose **Pixel Perfecto** at the top of the Render Server sidebar. This opens an independent native tool window using the existing GPU. `cargo run` still opens the Render Server. The standalone entry does not start the WebSocket server, animation runtime, or scene loader.

## Align photographs

1. Import PNG/JPEG files or drop them onto the window. Embedded sRGB and Display P3 profiles and EXIF orientation are handled during decoding. Unmarked files are assumed to be sRGB. PNG retains 8/16-bit source depth; unsupported profiles produce an error rather than an incorrect color conversion.
2. Import runs **automatic initial alignment** on a background worker. It looks for a repeating red lattice, then measures RGB centers and fits perspective/lens distortion. Unclear, very small, dark or nonperiodic patterns leave the initial transform unchanged and explain why. Each photo gets a provisional cell origin: periodic texture alone cannot identify the same absolute screen cell across photos.
3. Select a layer and enable **Calibrate**. There is **one full-size canvas**: the current corrected photograph, fixed RGB grid, manual anchors and residual arrows are shown together. Every successful correction updates this same view. Entering or leaving calibration retains the view center, zoom and accumulated correction. Pan with the middle button and zoom with the wheel.
4. **Image rotation (°)** is available directly in the selected-image panel. Drag the angle for continuous feedback or use **↶ 90° / ↷ 90°**. Positive angles are counterclockwise, around the current view center. Rotation changes only this image; lens parameters, original pixels and integer cell offset remain intact. Prefer setting coarse orientation before placing anchors: existing anchors retain their grid identities, so **Refine** will fit those targets again. A periodic RGB lattice cannot determine the intended quarter-turn orientation or absolute origin by itself.
5. Choose **R**, **B**, **G1 /**, or **G2 \** and **drag a fixed grid point onto its matching subpixel in the already-corrected photograph**. Its color and cell identity remain fixed. The gesture uses the current calibration to resolve that image position back to original source coordinates, then optionally snaps to a same-color center in the original. The magnifier shows the same corrected image/grid as the main canvas. Green crosshairs indicate a detected center; yellow means manual placement. Hold **Alt** to bypass snapping. **Esc** cancels the gesture.
6. Release to fit automatically. The existing correction stays visible while choosing the next correspondence and while the worker fits it; the result replaces the image on the same canvas as soon as fitting completes. All prior anchors participate, so corrections accumulate without re-warping already-resampled pixels. The model follows point geometry: one anchor adjusts position; two or more collinear anchors jointly refine position, rotation and uniform scale; three noncollinear anchors allow affine correction; and sufficiently spread correspondences allow perspective fitting. Four points on one line do not determine perspective. Unstable higher-order fits fall back to a valid simpler correction using all enabled manual anchors, preserving existing lens information. The inspector names the applied model and highlights collinear/narrow point layouts next to their RMS. Automatic completion searches close to predicted sites, requires color contrast, prohibits duplicate assignments, and rejects large-residual matches. If the automatic extension fails, the successful manual update still applies. User anchors receive more weight than automatic detections. Disable **Automatically add reliable points** to fit manual anchors only.
7. Start with two well-separated anchors, then choose points in other rows and columns, roughly around the corners of a rectangle. Two adjacent picks can amplify a small picking error far from those points; additional spread-out anchors reduce this sensitivity. If a row/column is already marked, add points well away from that line in two different areas. Lens fitting activates only with at least 12 measurements covering the center, at least three corners, and seven of nine image regions. The inspector suggests missing regions. Adding many points in one small area does not establish edge distortion. **Refine** regenerates measurements; **Auto initial alignment** is available before creating manual anchors. If initial detection fails, adjust rotation and **Fine alignment** to roughly position/scale the image, then drag anchors.
8. Residual arrows run from measured image centers to their fixed grid targets, directly on the corrected image. Cyan dots are automatic fitting measurements; purple dots are held out from the solver. Residuals over 2.5 original source pixels turn red. The inspector reports held-out RMS separately from fitted-point RMS. These are geometric checks, not a guarantee of absolute cell identity or accuracy everywhere. Correct, disable or delete bad manual anchors. Collinearity uses a simpler model; entirely coincident grid targets, collapsed measured centers or a failure to find any valid, non-folded correction preserve the last valid transform. Lens failure can fall back to a valid perspective fit with an explanation in the status bar. Diagnostics from earlier anchor sets are hidden while a new set is being fitted.
9. **Fine alignment** provides continuous translation/scale and **Reset alignment and anchors**; **Lens & perspective parameters** exposes the homography, `k1/k2/p1/p2`, and fixed distortion center. Editing a manual correspondence in the inspector also schedules a fit after release. The anchor gesture and its automatic fit undo together when no intervening edit has occurred. Undo also invalidates in-flight analysis. Rotation is undoable as one gesture.
10. Leave calibration mode to stack images. Left dragging moves a layer by complete RGB repeat cells; middle dragging moves only the viewport. Locked layers can be selected but cannot be moved, rotated or calibrated.

Manual anchors and the fitted transformation are saved in the project. Automatic measurements and held-out diagnostics are transient and regenerated by **Refine** after opening; they do not bloat the project or become permanent manual constraints.

The lattice uses `u=(1,1)` and `v=(-1,1)` in Y-up coordinates. Sites within each cell are `R=0`, `B=(u+v)/2`, `G1=u/2`, and `G2=v/2`. One world unit is the horizontal/vertical pitch between adjacent R/B centers. Snapped layer motion is an integer combination of `u` and `v`, preserving color and green orientation. Calibration points remain local to the layer, so snapped movement never modifies its calibration.

## Rulers and shared guides

Top and left rulers follow the shared canvas in both calibration and comparison modes. Tick spacing adapts to zoom; values use the same grid units described above, with X increasing right and Y increasing upward. These are image-alignment coordinates, not physical micrometers. The cursor position appears on both rulers.

- Drag from the **top ruler** to create a horizontal guide, or from the **left ruler** to create a vertical guide.
- Drag a guide to reposition it. Hold **Shift** for whole grid units, or **Alt/Option** while starting a drag to duplicate it.
- Click a guide to select it. Its position and distance to the nearest parallel guide appear on the canvas. Arrow keys move it by 1 unit; **Shift+Arrow** uses 10 units, **Alt/Option+Arrow** uses 0.1 unit. The **Rulers & guides** panel also accepts an exact coordinate.
- Press **Delete/Backspace**, choose **Remove guide** from its context menu, or drag it outside the canvas to remove it. **Esc** cancels a drag; **Cmd/Ctrl+Z** undoes it.
- Toggle rulers and guides with **Shift+R** or the toolbar's **Rulers** checkbox. Hide just guides or lock them from **Rulers & guides**. Right-click a ruler to clear its axis or lock guides. Locked guides remain visible and do not capture image drags.

The drag/duplicate/remove interactions follow [Figma's canvas guide workflow](https://help.figma.com/hc/en-us/articles/360040449713-Add-guides-to-the-canvas-or-frames). Guides belong to the project, so switching, moving, rotating or recalibrating an image leaves them in place. Use them to mark an element in one photo, then align the corresponding element in another photo against the same lines. Calibration handles and the comparison wipe handle have priority where they intersect guides. Normal image motion retains its integer lattice snap. Guide positions, visibility and locking are saved in `.pixelperfecto`; these editing aids are excluded from PNG exports.

## Measure

Choose **Measure** or press **M**, then click **A** and **B**, or drag one line across the subpixels. Drag either endpoint to adjust it; **Shift** constrains horizontal/vertical movement. Optional **Snap endpoints to grid centers** helps select exact endpoints; **Alt/Option** bypasses it. **Esc** cancels an unfinished gesture, **Delete** clears the line, and **Cmd/Ctrl+Z / Shift+Z** undo/redo.

The purpose is to count subpixels along the same segment and compare image alignment across many pixels:

- **Image pixels** counts luminous R/G/B regions crossed by the line in the selected original photograph. It scans the color signal at half-pixel-or-finer source spacing, follows the current perspective/lens mapping, and separates peaks by measured dark gaps. It does not use grid sites, expected spacing or expected counts to find image pixels. Green regions are counted as G without inferring their orientation from the grid.
- **Grid pixels** independently counts the ideal R/G1/G2/B footprints crossed by that line. Each footprint counts once, including subpixels containing A/B. This does not count intersections with diamond guide lines or the number of background cells. Grid visibility and zoom do not change the count.
- **Image − grid** shows the count difference. For example, a photograph with a finer pitch may have **7 image pixels / 5 grid pixels / +2**, useful evidence that the current scale/alignment needs adjustment. Counts alone do not establish the absolute cell identity or a complete calibration.

Image hits are marked **I1, I2…** and grid hits **G1, G2…** on the canvas; the inspector shows two aligned sequence tracks and R/G/B breakdowns. Each number represents one subpixel. Photo-file pixel distances are not the measurement result. Dark or merged regions can be missed and noise can create extra peaks: tune **Image threshold** and inspect the numbered marks. The implementation uses custom color-signal processing, with no OpenCV dependency.

Counting runs on a worker after endpoint placement and refreshes after switching images or changing alignment. The shared measurement line stays visible when returning to calibration/comparison, so you can align against it; editing its endpoints requires Measure mode. A partial line outside the image is explicitly identified; only its visible image portion is counted. Very long/invalid scans report their limit rather than silently downsampling. The endpoints and detection threshold are saved in `.pixelperfecto`; these annotations are excluded from PNG export.

## Compare

The top layer-list row is drawn last. Overlay uses visibility, ordering and opacity. Difference, A/B and Wipe use the explicitly selected A/B pair at full opacity, independently of overlay visibility/opacity. Difference computes absolute straight linear RGB differences only where both images have coverage; its gain is a display multiplier also included when exporting that comparison. Wipe has horizontal and vertical modes; drag the white handle. Tab switches the displayed A/B image.

Nearest-neighbor mode examines source samples; bilinear is the default. Filtering operates on linear premultiplied samples. Transparent pixels outside each image are never stretched to the canvas edge.

## Save and export

A `.pixelperfecto` file is a SQLite database containing a versioned document and content-addressed original image bytes. It can be moved independently of the original files. Decoded/GPU caches are not persisted. Saving builds a new transactional database next to the destination and replaces the destination only after success. Closing either the tool or its parent Render Server prompts for unsaved work.

Export writes the selected corrected image or the current comparison to an 8/16-bit sRGB PNG with transparent borders. The default crop is the current viewport; draw a crop in shared-grid coordinates or edit its bounds. The default density is the selected image's local sampling density at the view center, and default bit depth is the highest imported source depth. Export preserves microscope detail; it does not reconstruct logical screen pixels. Values outside sRGB SDR range are clipped when writing PNG. Grid export is optional and excludes editor labels and calibration markers.

Export is streamed one row at a time on a worker, so it does not allocate an output-size framebuffer or block the UI. Closing waits for active work. Inputs are decoded on a worker; hidden/unneeded GPU source textures are released. Standard 8-bit sRGB images use compact source textures, while high-depth/P3 sources retain linear FP32. This first version targets 2–10 photographs of roughly 12 MP each; an individual image must also fit the GPU texture dimension limit. It does not silently downsample images that exceed that limit.

## Implementation and verification

The shared core lives in `pixel_perfecto::{model,calibration,detection,measure,io}`. Window hosting and GPU resources are transient. The renderer maps each output position through `grid - snapped_offset -> homography -> lens distortion -> original image`; only the private image storage boundary converts Y-up to raster row order. Intermediate comparison textures are FP32; the displayed texture is FP16. Export uses the same mapping and linear sampling in CPU double precision geometry.

```sh
cargo test --lib pixel_perfecto
# Also run the explicit ten-image / 12 MP GPU load check:
cargo test --lib pixel_perfecto::gpu -- --include-ignored
cargo check --bins
cargo test --test image_texture_color_space --test image_texture_precision
```

The tests exercise color-preserving negative/positive lattice moves, cursor-anchored zoom, robust perspective/lens recovery, rejection cases, 16-bit preservation, embedded project roundtrips including guides and measurements, atomic failed saves, export, and actual GPU/CPU sample agreement. Ruler tests cover negative coordinates, zoom, Y-up mapping, high-DPI pointer gestures across ruler/canvas boundaries, duplication, deletion, cancellation, undo, and priority against calibration/wipe handles. Measurement tests cover independent image/grid counts before alignment (7 vs 5), pixel-footprint crossings rather than guide-line intersections, inclusive endpoints, dark gaps, rotation/perspective/lens mapping, high-DPI A/B gestures, cancellation/undo and stale worker results. Assisted detection tests include a rotated distorted RGB lattice with dark subpixels, fixed identities, progressive manual fitting, successive corrected-space picks under rotation/perspective/lens distortion and integer offsets, full-canvas calibration layout, pivot-preserving image rotation, color/distance gates, held-out validation and undercovered/degenerate input. RAW, TIFF and unconstrained mesh warping are outside this version.

Optional acceptance against a real microscope photograph:

```sh
PIXEL_PERFECTO_PHOTO=/absolute/path/to/photo.png cargo test --lib supplied_photo_automatic_acceptance -- --ignored --nocapture
```

For the `ocean-weather-icon.png` manual regression (independent of automatic initial alignment):

```sh
PIXEL_PERFECTO_PHOTO=/absolute/path/to/ocean-weather-icon.png cargo test --lib supplied_photo_collinear_anchors -- --ignored --nocapture
```

This fixture measures four visible centers in one column, using grid IDs `(-26,-3), (-27,-4), (-17,6), (-12,11)`. It checks progressive manual fitting and automatic completion without a successful automatic seed. The regular suite also covers picking noise in adjacent anchors, nearly collinear layouts, promotion from a line to affine/perspective, and application of the third/fourth anchor through the background worker.
