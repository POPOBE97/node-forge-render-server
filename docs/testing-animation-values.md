# Animation Value Trace Testing

This repository includes deterministic animation value-trace testing for scenes with
`stateMachine` definitions.

## What It Verifies

- A fixed timeline: `0.0s .. 10.0s` at `60fps` (inclusive end), i.e. `601` frames.
- Per-frame animation metadata (state/transition/timing).
- Dense per-frame animation output values (`nodeId:paramName`).

Golden files live at:

- `tests/cases/<case>/animation_values.json`

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
