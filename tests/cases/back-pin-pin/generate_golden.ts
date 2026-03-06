#!/usr/bin/env npx tsx
/**
 * Golden value generator for the back-pin-pin animation test case.
 *
 * State machine:
 *   EntryState --[mousedown, delay:0.3, duration:0.3, ease:linear]--> TimeCycleMutationNode
 *
 * TimeCycleMutation passes sceneElapsedTime through to FloatInput_53:value.
 *
 * Timeline (60 fps, 0–10s, include_end=true → 601 frames):
 *   - Frames 0–59:  Entry state, value=0, no transition
 *   - Frame 60:     mousedown fires at t=1.0s, transition begins (delay phase)
 *   - Frames 60–78: Transition active, blend=0 (delay=0.3s → 18 frames of delay)
 *   - Frames 79–95: Blend ramps linearly from 1/18 to 17/18
 *                    value = lerp(entryValue=0, scene_time, blend)
 *   - Frame 96:     Transition completes, enter TimeCycle state, localElapsedTime resets to 0
 *   - Frames 97+:   value = state_local_time_secs (= localElapsedTime)
 *
 * Usage:
 *   npx tsx tests/cases/back-pin-pin/generate_golden.ts
 */

import { writeFileSync } from "fs";
import { join, dirname } from "path";
import { fileURLToPath } from "url";

// --- Constants matching the Rust TickSchedule and scene definition ---

const FPS = 60;
const START_SECS = 0.0;
const END_SECS = 10.0;
const INCLUDE_END = true;

// Raw step — NOT rounded. Matches Rust: `let step = 1.0 / (self.fps as f64);`
const STEP = 1.0 / FPS;

const MOUSEDOWN_FRAME = 60; // t = 1.0s
const DELAY = 0.3;          // seconds
const DURATION = 0.3;       // seconds

const DELAY_FRAMES = Math.round(DELAY * FPS);       // 18
const DURATION_FRAMES = Math.round(DURATION * FPS);  // 18
const TRANSITION_END_FRAME = MOUSEDOWN_FRAME + DELAY_FRAMES + DURATION_FRAMES; // 96

const ENTRY_STATE = "st_mmamj2am_3";
const TARGET_STATE = "st_mmamj4me_7";
const TRANSITION_ID = "tr_mmbig5yq_d";
const TRACKED_KEY = "FloatInput_53:value";

// Entry-state value for FloatInput_53 (from scene.json params).
const ENTRY_VALUE = 0;

// Match Rust's round_f64: round to 6 decimal places.
const ROUND_PRECISION = 1_000_000;
function round6(v: number): number {
  const r = Math.round(v * ROUND_PRECISION) / ROUND_PRECISION;
  return Object.is(r, -0) ? 0 : r;
}

interface Frame {
  frame_index: number;
  time_secs: number;
  dt_secs: number;
  current_state_id: string;
  state_local_time_secs: number;
  scene_time_secs: number;
  active_transition_id: string | null;
  transition_blend: number | null;
  finished: boolean;
  diagnostics: string[];
  values: Record<string, number>;
}

// Total frame count (matches Rust TickSchedule::frame_count).
const totalFrames = Math.round((END_SECS - START_SECS) * FPS) + (INCLUDE_END ? 1 : 0);

// --- Simulate the runtime ---
//
// The Rust runtime accumulates scene_time by adding the raw dt (= STEP)
// each tick. We mirror that here by computing scene_time = i * STEP
// (equivalent to accumulation without drift since STEP is exact 1/60 in f64).
//
// Rounding (round6) is applied only at the output stage, matching the
// trace serialization in Rust.

const frames: Frame[] = [];

for (let i = 0; i < totalFrames; i++) {
  // time_secs: from TickSchedule — start + idx * step (raw f64).
  const rawTimeSecs = START_SECS + i * STEP;
  const rawDtSecs = i === 0 ? 0.0 : STEP;

  // scene_time: the runtime accumulates dt each tick.
  // Since dt is constant STEP for all frames > 0, scene_time = i * STEP.
  const rawSceneTime = i * STEP;

  let currentStateId: string;
  let activeTransitionId: string | null = null;
  let rawTransitionBlend: number | null = null;
  let rawStateLocalTime: number;
  let rawValue: number;

  if (i < MOUSEDOWN_FRAME) {
    // Before mousedown: entry state, no transition.
    // Entry state is static — local_time stays 0.
    currentStateId = ENTRY_STATE;
    rawStateLocalTime = 0.0;
    rawValue = ENTRY_VALUE;

  } else if (i < TRANSITION_END_FRAME) {
    // Transition active (delay + blend phases).
    // Still in source (entry) state during transition.
    currentStateId = ENTRY_STATE;
    activeTransitionId = TRANSITION_ID;

    // Transition elapsed = ticks since armed * STEP.
    const ticksSinceArmed = i - MOUSEDOWN_FRAME;
    const transitionElapsed = ticksSinceArmed * STEP;

    const BLEND_START_FRAME = MOUSEDOWN_FRAME + DELAY_FRAMES;

    if (transitionElapsed < DELAY - 1e-9) {
      // Delay phase: blend = 0, value stays at entry, local_time = 0.
      rawTransitionBlend = 0.0;
      rawStateLocalTime = 0.0;
      rawValue = ENTRY_VALUE;
    } else {
      // Blend phase: target state starts running, local_time counts from 0.
      const ticksSinceBlendStart = i - BLEND_START_FRAME;
      rawStateLocalTime = ticksSinceBlendStart * STEP;

      const blendTime = transitionElapsed - DELAY;
      const rawBlend = Math.min(blendTime / DURATION, 1.0);
      rawTransitionBlend = rawBlend;

      // value = lerp(entryValue, sceneTime, blend)
      rawValue = ENTRY_VALUE + (rawSceneTime - ENTRY_VALUE) * rawBlend;
    }

  } else {
    // Post-transition: in target state (TimeCycle).
    currentStateId = TARGET_STATE;

    // local_time started counting at BLEND_START_FRAME and continues.
    const BLEND_START_FRAME = MOUSEDOWN_FRAME + DELAY_FRAMES;
    const ticksSinceBlendStart = i - BLEND_START_FRAME;
    rawStateLocalTime = ticksSinceBlendStart * STEP;

    // Mutation passes sceneElapsedTime → FloatInput_53:value.
    rawValue = rawSceneTime;
  }

  frames.push({
    frame_index: i,
    time_secs: round6(rawTimeSecs),
    dt_secs: round6(rawDtSecs),
    current_state_id: currentStateId,
    state_local_time_secs: round6(rawStateLocalTime),
    scene_time_secs: round6(rawSceneTime),
    active_transition_id: activeTransitionId,
    transition_blend: rawTransitionBlend !== null ? round6(rawTransitionBlend) : null,
    finished: false,
    diagnostics: [],
    values: { [TRACKED_KEY]: round6(rawValue) },
  });
}

const traceLog = {
  schema_version: 1,
  start_secs: START_SECS,
  end_secs: END_SECS,
  fps: FPS,
  include_end: INCLUDE_END,
  frame_count: totalFrames,
  tracked_keys: [TRACKED_KEY],
  frames,
};

const outPath = join(dirname(fileURLToPath(import.meta.url)), "animation_values.json");
writeFileSync(outPath, JSON.stringify(traceLog, null, 2) + "\n");
console.log(`Wrote ${totalFrames} frames to ${outPath}`);
