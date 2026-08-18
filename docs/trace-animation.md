# Animation Trace CLI (`--trace-animation`)

CPU-only, deterministic MotionEngine / state-machine diagnostics for
`node-forge-render-server`. Uses the same `AnimationSession` path as the live
app and the `animation_values` goldens.

## Why

Motion-driven rendering decomposes each animated property as:

```text
S  = state override / inherit
Q  = mutation target (setTo / to spring)
E  = transition residual
P  = Q − E   (physical value read by Derivation)
```

Visual jumps can come from:

1. **Handoff discontinuity** — `P` changes at state/transition edges.
2. **Mutation retarget** — `Q` / springs recreated or double-stepped.
3. **Derivation-only** — `P` smooth but overrides (e.g. blob `positions`) jump.

This tool dumps per-frame `S/Q/E/P` plus optional overrides and a jump summary
so you can classify the failure without temporary `eprintln!` probes.

## Quick start

```bash
cd node-forge-render-server

# Free-run 2s @ 60fps
cargo run -q -- \
  --nforge ./tests/fixtures/render/editor-examples/back-pin-pin/scene.nforge \
  --trace-animation \
  --trace-seconds 2 \
  --trace-format summary,table \
  --trace-output ./out/trace.json

# Scripted diagnosis (listening → thinking)
cargo run -q -- \
  --nforge ./tests/fixtures/render/editor-examples/doubao-voice-interaction/scene.nforge \
  --trace-animation \
  --trace-scenario ./scenarios/doubao-listening-to-thinking.json \
  --trace-format json,summary,table \
  --trace-output ./out/thinking-trace.json
```

No GPU, UI, or WebSocket. Exit early after writing the report.

## Flags

| Flag | Default | Meaning |
|---|---|---|
| `--trace-animation` | off | Enable mode |
| `--nforge` / `--dsl-json` | required | Scene source |
| `--trace-scenario <path>` | — | Scenario script (preferred) |
| `--trace-seconds <f64>` | `2` if no scenario | Free-run duration from t=0 |
| `--trace-fps <u32>` | `60` | Fixed step rate |
| `--trace-include-end` / `--trace-no-include-end` | include end | Free-run schedule end sample |
| `--trace-initial-state <id>` | — | `force_state` pin before run (routing forced) |
| `--trace-events <path>` | — | Legacy `events.json` for free-run only |
| `--trace-channels <csv\|\*>` | `*` | Motion channel keys |
| `--trace-overrides <csv\|\*\|none>` | `*` | Override keys `nodeId:param` |
| `--trace-values all\|none\|filter` | `all` | Store dense per-frame values |
| `--trace-output <path\|->` | `-` | JSON destination |
| `--trace-format` | `json,summary` | `json` / `summary` / `table` |
| `--trace-jump-threshold-channel` | `0.05` | Channel jump threshold |
| `--trace-jump-threshold-override` | `5.0` | Override jump threshold |
| `--trace-check-identity` / `--trace-no-check-identity` | on | `P ≟ Q − E` |
| `--trace-fail-on-jump` | off | Exit 2 when channel/override jumps exist |
| `--trace-pretty` / `--trace-compact` | pretty for files | JSON formatting |

Cannot combine with `--headless`, `--profile`, `--render-to-file`,
`--dump-wgsl-dir`, or `--continuous-redraw`.

## Scenario schema (v1)

```json
{
  "schemaVersion": 1,
  "name": "listening-to-thinking",
  "fps": 60,
  "track": {
    "channels": ["sp_…"],
    "overrides": ["Vector2ArrayInput_IntelligentLightPositions:value"]
  },
  "analyze": {
    "jumpThresholdChannel": 0.05,
    "jumpThresholdOverride": 5.0,
    "checkPhysicIdentity": true,
    "failOnJump": false
  },
  "actions": [
    { "type": "step", "frames": 0 },
    { "type": "settle", "maxFrames": 360 },
    { "type": "event", "eventType": "keydown", "key": " " },
    { "type": "step", "seconds": 0.21 },
    { "type": "assertState", "stateId": "st_listening" },
    { "type": "event", "eventType": "keyup", "key": " " },
    { "type": "assertTransition", "id": "tr_listening_to_thinking" },
    { "type": "step", "seconds": 2.0 }
  ]
}
```

### Actions

| type | Fields | Behavior |
|---|---|---|
| `step` | `frames` **or** `seconds` | Advance; `frames:0` = `dt=0` sample. Aligned `seconds` expand to N×(1/fps) frames; non-aligned `seconds` is one physical step with that `dt` (e.g. `0.21`) |
| `settle` | `maxFrames` | Step until no active transition or max |
| `event` | `eventType`, `key?`, `button?`, `repeat?`, `modifiers?` | Queue event + `step(0)` |
| `mouse` | `x`, `y` | ScenePx mouse (bottom-left ABI) |
| `forceState` | `stateId` | Debug pin (`routingMode: forced`) |
| `assertState` | `stateId` | Abort exit 2 on mismatch |
| `assertTransition` | `id` \| null | Abort exit 2 on mismatch |
| `label` | `name` | Tag subsequent frames for analysis |

Every recorded tick becomes a frame (including settle and event-only ticks).

Shipped examples live under `scenarios/`:

- `doubao-listening-to-thinking.json`
- `doubao-thinking-free-run.json`
- `back-pin-pin-basic.json`

## Output schema (v2)

```json
{
  "schemaVersion": 2,
  "tool": "trace-animation",
  "scene": { "source": "…", "stateMachineId": "…" },
  "config": {
    "fps": 60,
    "scenarioName": "…",
    "routingMode": "natural" | "forced",
    "filters": { "channels": ["*"], "overrides": ["*"], "includeValues": true },
    "analyze": { "…": "…" }
  },
  "summary": {
    "frameCount": 180,
    "finalStateId": "st_thinking",
    "transitionsSeen": ["tr_listening_to_thinking"],
    "identityViolations": 0,
    "jumps": [/* channel | override | state | transition */],
    "maxAbsChannelDelta": { "sp_…": 0.04 },
    "maxAbsOverrideDelta": { "node:param": 3.1 },
    "handoffs": [
      {
        "frameIndex": 30,
        "fromState": "st_listening",
        "toState": "st_thinking",
        "transitionId": "tr_listening_to_thinking",
        "channelContinuity": [
          { "key": "sp_…", "outgoingP": [0.3], "incomingP": [0.3], "ok": true }
        ]
      }
    ]
  },
  "frames": [
    {
      "frameIndex": 0,
      "timeSecs": 0.0,
      "dtSecs": 0.0,
      "label": "enter-thinking",
      "currentStateId": "st_thinking",
      "activeTransitionId": "tr_listening_to_thinking",
      "sceneTimeSecs": 1.2,
      "stateLocalTimes": { "st_thinking": 0.0 },
      "motionChannels": [
        {
          "key": "sp_…",
          "stateValue": [0.45],
          "targetValue": [0.45],
          "transitionError": [0.15],
          "value": [0.30],
          "velocity": [0.0],
          "mutationDriver": "spring",
          "transitionDriver": "spring",
          "currentTimingNodeId": "spring_to_waypoint",
          "pendingTimingNodeIds": ["spring_to_target"],
          "canceledTimingNodeIds": []
        }
      ],
      "values": { "…": "…" },
      "analysis": { "identityOk": true, "identityViolations": [], "jumps": [] }
    }
  ]
}
```

Field names on `motionChannels` use camelCase (`stateValue` = S, `targetValue` = Q,
`transitionError` = E, `value` = P). Plan-tree traces additionally expose the active Timing node,
delay-pending Timing nodes in activation order, and Timing nodes canceled by parallel takeover.
The table format includes the same data in its `timing`, `pending`, and `canceled` columns.

## Interpreting results

| Observation | Likely cause |
|---|---|
| Handoff `channelContinuity.ok == false` | Activation / residual bug (`activate_transition`, `P=Q−E`) |
| Handoff ok, channel jumps mid-state | Mutation `to(spring)` retarget / `localElapsedTime` |
| Channels smooth, override jumps | Derivation (`sceneElapsedTime` step quantization, layout inputs) |
| CLI clean, UI only jumps | Variable wall-clock `dt`, interaction bridge, scene delta |

## Exit codes

| Code | Meaning |
|---|---|
| `0` | Success (jumps may still be listed) |
| `2` | Scenario assert failed or `--trace-fail-on-jump` |
| `1` | Usage / load / compile / I/O error |

## Relation to goldens

`tests/animation_values.rs` calls
`animation::generate_animation_trace_log`, the same session schedule driver
used by free-run CLI. Keep product diagnostics and goldens on one path.

Legacy `events.json` next to fixtures still works with free-run
`--trace-events`. Prefer scenario files for multi-step diagnoses.

## Library API

```rust
use node_forge_render_server::animation::{
    TraceScenario, run_trace, run_schedule_trace, generate_animation_trace_log,
};
```
