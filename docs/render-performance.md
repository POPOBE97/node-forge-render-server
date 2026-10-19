# Live render performance

Click the compact CPU/GPU summary in the canvas's upper-left corner to open
**Render Performance**, a separate native window. Closing it leaves the summary
available. The Statistics visibility command controls the canvas summary.

The window supports a 30/60/90/120/144/240 Hz comparison budget, a frozen view,
resetting collected samples, a GPU history plot, and a complete pass ranking.
It refreshes at 4 Hz and keeps at most 240 samples per series. Freeze affects only
the detailed view, not rendering or collection. Samples reset when the shader
space is replaced so different scenes are not combined.

## Reading the numbers

| Metric | Source and scope | Interpretation |
| --- | --- | --- |
| CPU encode + submit | Host clock around scene command encoding and submission | CPU cost of submitting the scene. This does not include animation, analysis or display presentation. |
| GPU scene span | wgpu pass timestamp queries; earliest start to latest end | Time envelope of scene render/compute passes. Passes may overlap on tile GPUs, so summing pass times gives a different quantity. |
| P50 / P95 / P99 / max | Nearest-rank percentiles of the last ≤240 valid samples | Typical cost and slow-tail behavior. Small sample sets cannot provide stable tail estimates. |
| Over budget | Samples strictly above the selected frame budget | Cost or cadence threshold crossings, **not** dropped display frames. |
| Continuous redraw interval | Difference between consecutive scene-submitting UI updates during playback/forced redraw | Submission pacing. Idle UI updates end a sequence; static scenes do not accrue artificial slow frames. This is not compositor FPS or input-to-display latency. |
| CPU update phases | Host timers around ingest, animation/derivation, render/analysis, canvas UI and finalization | Locate costs outside GPU command encoding. Total app update excludes eframe painting and swapchain present. Phase percentiles do not add up. |
| Pass GPU / CPU | Both values preserved from the same completed submission | Rank GPU work; fall back to CPU order when timestamps are unavailable. |
| Draws / dispatches / workgroups | Commands encoded by ShaderSpace | Submission workload; excludes egui, analysis scopes, matrix cells, and extra draw-capture work. |
| Vertex/index elements × instances | Submitted element counts, multiplied by instance counts | Not unique vertices, shader invocations, primitives or pixels. |
| Scene resource bytes | Prepared buffers; texture format blocks, dimensions, layers, mips and MSAA | Texture estimate excludes alias views and driver padding/compression. Unknown format sizes are counted separately. Not resident memory or actual bandwidth. |
| Metal device allocations | `MTLDevice.currentAllocatedSize` on the actual wgpu Metal device | Device resource allocation including UI/analysis, not only the scene pool. Not system-wide GPU memory or process RSS. |
| Recommended working set | `MTLDevice.recommendedMaxWorkingSetSize` | A performance-oriented recommendation, not free VRAM or a hard allocation limit. |

The GPU budget bar is GPU span divided by the selected budget. It is **not GPU
utilization**, occupancy, nor a definitive CPU/GPU bottleneck classifier. CPU and
GPU execute concurrently. Vsync, presentation and workload outside the scene can
limit displayed FPS even when both scene timings are low. A stale sample is marked
`(last)` in the summary; the detailed window shows the completed-sample age.

## Capture mechanics and overhead

The interactive path uses pass-boundary timestamp writes, a submission-completion
callback, then a separate resolve/copy submission and asynchronous map callback.
Resolve is not submitted until sampling has completed: local Metal testing
reproduced incomplete end timestamps with same-submission resolution, matching
[wgpu issue #9414](https://github.com/gfx-rs/wgpu/issues/9414). This completion gate
also applies to the existing synchronous timestamp profiler. Three reusable query/readback slots bound memory and
outstanding work. If all slots are pending, the scene still renders and timestamp
sampling is skipped. Pending and skipped counts are visible. Polling never waits
for GPU completion; idle UI ticks drain the final pending frame.

Each completed sample retains the original pass identities and CPU timing. Results
are consumed once in submission order. Invalid/missing timestamps produce an
explicit error rather than a zero-cost frame. Unsupported devices still show CPU
and workload data. The existing synchronous headless profiler remains available.

Timestamp instrumentation itself has a cost and can alter GPU scheduling. Native
window composition and analysis tools also use the GPU. Use consistent settings
and the same scene when comparing runs; use a release build for shipping-cost
comparisons. The normal development profile remains optimized with debug symbols.

## Metal research and industry references

- [Apple: Explore Live GPU Profiling with Metal Counters](https://developer.apple.com/videos/play/tech-talks/10001/)
  describes capability discovery, pass/stage-boundary timing, collecting results
  after completion, validating counters and keeping HUD refresh readable. The
  implementation uses wgpu's supported pass-boundary timestamp API rather than
  assuming draw-boundary support on Apple tile GPUs.
- [wgpu 30 Features](https://docs.rs/wgpu/30.0.1/wgpu/struct.Features.html)
  lists timestamp-query support and restricts pipeline-statistics queries to
  Vulkan/DX12. The current synchronous profiler can obtain invocation counts on
  those supported backends. Metal does not gain those metrics merely by enabling
  `PIPELINE_STATISTICS_QUERY`.
- [Apple: Optimize Metal apps and games with GPU counters](https://developer.apple.com/videos/play/wwdc2020/10603/)
  covers performance limiters, memory bandwidth, occupancy and hidden-surface
  removal. These require actual hardware counter data. This HUD does not derive
  occupancy, ALU/texture limiter, cache misses, overdraw or bandwidth from frame
  time or texture sizes. Use Xcode GPU capture / Instruments Metal System Trace
  when those deeper metrics are needed.
- [Apple: currentAllocatedSize](https://developer.apple.com/documentation/metal/mtldevice/currentallocatedsize)
  and [recommendedMaxWorkingSetSize](https://developer.apple.com/documentation/metal/mtldevice/recommendedmaxworkingsetsize)
  are the native device-memory APIs used here. The HAL device guard is retained
  while reading; no native queues, resources or synchronization are modified.
- [Unity: Rendering Profiler module](https://docs.unity3d.com/Manual/ProfilerRendering.html)
  motivates exposing draw/dispatch workload and resource counts alongside time.
  Workload counters explain what changed; measured durations determine whether
  that change matters on the current device.

## Verification

From the render-server directory (uses its patched wgpu/eframe dependencies):

```sh
cargo test --lib app::performance::tests
cargo test -p rust-wgpu-fiber --lib shader_space::live_profile::tests
cargo test --test live_performance -- --nocapture
```

The native GPU integration test validates bounded readback, skipping under
backpressure, slot reuse, preservation of CPU/pass identity, single consumption,
the synchronous path, and a static scene's final sample using non-blocking polls
without additional scene submissions. It excludes the noop backend; adapter-less environments
explicitly report a skip and cannot validate real GPU timings.

### Verification without an unlocked desktop

The performance window can be tested through the real egui input and paint paths
without launching a native application or accessing the window server:

```sh
cargo test --lib app::performance::ui_tests
cargo test --lib app::performance::ui_tests::performance_offscreen_metal_snapshots -- --ignored --nocapture
```

The first command sends pointer/wheel events to the actual controls and exercises
the deferred viewport callback, close/reopen, budget selection, freeze/reset,
pass sorting, minimum window size and idle repaint scheduling. The second command
requires a real native GPU but no display: it rasterizes the same egui output into
textures and writes `tmp/performance-ui/{overview,resources,compact,compact-resources}.png`. These
images use deterministic test values for visual inspection, not measured scene
performance. This verifies application layout and event handling; OS window-manager
chrome and physical display presentation remain outside this headless check.
