# Animation Value Trace Testing

This repository includes deterministic animation value-trace testing for scenes with
`stateMachine` definitions.

Goldens share the **app `AnimationSession` path** with the product CLI
(`--trace-animation` / `animation::generate_animation_trace_log`). See
[`trace-animation.md`](trace-animation.md) for interactive diagnosis.

## What It Verifies

- A fixed timeline: `0.0s .. 10.0s` at `60fps` (inclusive end), i.e. `601` frames.
- Per-frame animation metadata (state/transition/timing).
- Dense per-frame animation output values (`nodeId:paramName`).
- Optional per-frame `motion_channels` (S/Q/E/P drivers) when present.

Golden files live at:

- `tests/fixtures/render/<group>/<case>/expected/animation_values.json`

Optional event schedules:

- `tests/fixtures/render/<group>/<case>/events.json`

## Run Tests

```bash
cargo test --test animation_values
```

## Update Goldens

Use the existing golden update flag:

```bash
UPDATE_GOLDENS=1 cargo test --test animation_values
```

This regenerates `animation_values.json` for each case with a `stateMachine`.
