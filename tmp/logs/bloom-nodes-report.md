# bloom-nodes case report

- generated_at_utc: 2026-02-19T14:05:38Z
- scene: /Users/ruiyao/Desktop/Projects/node-forge-render-server/tests/cases/bloom-nodes/scene.json
- repo: /Users/ruiyao/Desktop/Projects/node-forge-render-server

## run status

1. command: 
   
   cargo run -q -- --headless --dsl-json ./tests/cases/bloom-nodes/scene.json --outputdir ./tmp/out
   
   result: failed (scene requires file render target)

2. command:
   
   cargo run -q -- --headless --dsl-json ./tests/cases/bloom-nodes/scene.json --render-to-file --output /Users/ruiyao/Desktop/Projects/node-forge-render-server/tmp/out/bloom-nodes.png
   
   result: failed in sandbox (headless adapter not found)

3. command (outside sandbox):
   
   cargo run -q -- --headless --dsl-json ./tests/cases/bloom-nodes/scene.json --render-to-file --output /Users/ruiyao/Desktop/Projects/node-forge-render-server/tmp/out/bloom-nodes.png
   
   result: success

## output artifacts

- render png: /Users/ruiyao/Desktop/Projects/node-forge-render-server/tmp/out/bloom-nodes.png
- dependency log: /Users/ruiyao/Desktop/Projects/node-forge-render-server/tmp/logs/bloom-nodes-deps.log
- run log: /Users/ruiyao/Desktop/Projects/node-forge-render-server/tmp/logs/bloom-nodes-run.log

## dependency analysis

- total nodes: 37
- total connections: 42
- output root: Composite_5
- reachable upstream from output root: 36 nodes
- unreached node from output root traversal: Screen_1 (expected sink in forward direction)
- source nodes: GLTFGeometry_2, RenderTexture_6, ColorInput_7, Kernel_11, Kernel_13, Kernel_15, Kernel_17, Kernel_19, Kernel_21
- sink node: Screen_1
- longest upstream dependency depth from root: 28
- type composition:
  - Downsample: 6, Kernel: 6, GuassianBlurPass: 6, Upsample: 6, MathClosure: 6
  - RenderPass/Composite/RenderTexture/GLTFGeometry/SetTransform/ColorInput/Screen: 1 each

pipeline shape: base scene render -> 6-level downsample chain -> blur+upsample+additive merge pyramid -> final add with base render -> composite to screen

## runtime observations

- warnings emitted during build/run: 13 (from existing code; no fatal warnings)
- prepare_to_pass buffer creation lines: 90
- final success line:
  - [headless] saved: /Users/ruiyao/Desktop/Projects/node-forge-render-server/tmp/out/bloom-nodes.png

## notes

- this case is configured for file render target in headless mode; it must be run with --render-to-file --output.
- execution in this environment required running outside sandbox to access a usable headless GPU adapter.
