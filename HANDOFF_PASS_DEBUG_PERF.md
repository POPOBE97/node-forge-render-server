# Pass Debug Window Editor Performance — Handoff Doc

## Problem

The pass debug window's WGSL code editor (~29KB, ~500 lines, ~12000 syntax highlight sections) takes ~50ms per frame during editing. Target is <5ms.

## Root Cause (Confirmed)

The debug window uses `ctx.show_viewport_deferred()` which creates a separate OS window. egui's internal `GalleyCache` (per-paragraph layout cache) gets flushed on every `begin_pass()` call. The call sequence per frame is:

1. Main viewport `begin_pass()` → `galley_cache.flush_cache()` → generation G→G+1
2. Main viewport renders
3. **Deferred viewport `begin_pass()`** → `galley_cache.flush_cache()` → generation G+1→G+2

At step 3, entries marked `last_used = G` (from the deferred viewport's *previous* frame) get cleared because G ≠ G+1. So every frame in the deferred viewport starts with an empty paragraph cache.

**Proof**: Pasting the same 29KB code into the official egui demo (which runs in the main viewport, no deferred) → editing is instant. Same code in our deferred viewport → 50ms/frame.

**Why 19ms appears sometimes**: TextEdit calls the layouter multiple times within a single frame (e.g., for event handling then for painting). The second call within the same frame hits the cache (same generation) but still costs ~19ms just for hashing 12000 LayoutSections.

## What's Already Done

File: `src/ui/pass_debug_window.rs`

1. **Focus highlight moved to overlay** — removed `apply_layout_job_highlight()` from the layouter, replaced with `paint_focus_highlight_overlay()` that paints rects after `editor.show()`. This prevents the LayoutJob from changing every frame due to focus changes.

2. **Whole-galley cache for browsing** — `PassDebugGalleyCache` in `PassDebugHighlightCache` stores the `Arc<Galley>` keyed by `(draft_revision, wrap_width)`. When text hasn't changed, the layouter returns the cached galley in ~0.15ms. **Browsing is fixed.**

3. **Perf instrumentation** — `src/perf_log.rs` + `metric_log!` macro writes to `$TMPDIR/node-forge-frame-metrics.log`. Tags: `[frame]`, `[pass-debug]`. On startup prints the log path to stderr.

4. **Per-line galley cache (`LineGalleyCache`)** — Survives egui's inter-viewport `GalleyCache` flush. Hashes text per-line, reuses `Arc<Galley>` for unchanged lines, calls `Galley::concat` only when needed. **Editing reduced from ~50ms to ~7ms per frame.**

5. **Arc<LayoutJob> in highlight cache** — `PassDebugHighlightCache::base_layout_job` is `Arc<LayoutJob>` to avoid cloning 12000 sections (~5ms) on every layouter call. The layouter passes `Arc::clone` instead.

6. **Line cache merged galley as precomputed fallback** — When `highlight_cache.galley` is lost (after `mark_draft_edited` clears it), `existing_galley` falls through to check `line_galley_cache.merged.job.text == draft_source`, providing O(1) return on subsequent non-edit frames.

## What Was Implemented (Per-Line Galley Cache)

Self-managed per-line galley cache (`LineGalleyCache`) that survives egui's inter-viewport flush.

### Data Structure (as implemented)

```rust
struct LineGalleyCache {
    wrap_width: f32,
    pixels_per_point: f32,
    line_hashes: Vec<u64>,                         // ahash of text bytes per line
    line_galleys: Vec<Arc<Galley>>,                 // per-line laid-out galleys
    merged: Arc<Galley>,                            // full concatenated galley
}
```

Stored in `PassDebugWindowDocument::line_galley_cache`. Passed into the layouter via `RefCell` to work around borrow conflicts with `TextEdit::multiline`.

### Algorithm (3 phases inside `layout_with_line_cache_standalone`)

**Phase 1** — Compute per-line text hashes (no allocation):
- Split text on `\n`, hash each line's byte slice with `ahash::RandomState::hash_one`
- Only hashes text bytes (not sections) — syntax highlighting is deterministic from text content

**Phase 2** — Fast path: if `line_hashes == prev_cache.line_hashes` (Vec equality), return `prev_cache.merged` immediately.

**Phase 3** — Partial rebuild on miss:
- Build `HashMap<u64, &Arc<Galley>>` from previous cache (handles line insertions/deletions)
- For each line: HashMap lookup → hit = reuse Arc<Galley>, miss = build per-line LayoutJob + layout
- Call `Galley::concat` to produce merged galley (skip if all hits + prev text matches)

### Key Design Decisions

1. **Hash only text, not sections** — Hashing 12000 `TextFormat` objects cost ~40ms. Text-only hashing costs 0.17ms. Safe because highlighting is deterministic from text content.

2. **`Arc<LayoutJob>` for zero-cost sharing** — `PassDebugHighlightCache::base_layout_job` is `Arc<LayoutJob>`. The layouter passes `Arc::clone` (O(1)) instead of cloning 12000 sections (~5ms).

3. **HashMap lookup instead of index comparison** — Handles line insertion/deletion gracefully. A line at index N in the old cache maps to the same hash at index N+1 in new.

4. **Precomputed galley fallback from line cache** — `existing_galley` checks `line_galley_cache.merged.job.text == draft_source` before entering the layouter, providing O(1) return on non-edit frames even when `highlight_cache.galley` was lost.

### Measured Results

| Scenario | Before | After |
|----------|--------|-------|
| Browsing (no edit) | ~0.15ms | **0.35ms** (slightly worse due to text comparison overhead, but well within budget) |
| Single-char edit (per frame) | ~50ms | **~7ms** (6ms re-highlight + 0.4ms line cache + 0.1ms concat) |
| Multi-line paste/delete | ~50ms | **~7ms** single-char edits; line-shift reuses all galleys via HashMap |
| Cold start (first layout) | ~50ms | ~50ms (unavoidable, builds cache for subsequent frames) |

### Remaining Bottleneck

The **~6ms per edit frame** is dominated by `egui_extras::syntax_highlighting::highlight()` being called inside the layouter when `buf.as_str() != cached_source_text` (text changed by edit). This re-highlights the full 29KB source. This is not cacheable since the text genuinely changed.

### Potential Further Optimizations (not implemented)

1. **Incremental re-highlighting**: Instead of re-highlighting the full 29KB on each edit, only re-highlight the changed paragraph. Requires modifying or wrapping `egui_extras::syntax_highlighting`.

2. **Pre-highlight on edit**: Call `ensure_highlight_cache()` AFTER `mark_draft_edited()` (which clears it) so that by the time the layouter runs, the highlight cache is already warm with the new text. Currently it's rebuilt lazily inside the layouter.

3. **Skip Phase 1 entirely**: Use `draft_revision` as a proxy — if revision matches, return merged. Would eliminate the 0.17ms Phase 1 cost on non-edit frames where Phase 2 fires.

## Key Files

| File | Role |
|------|------|
| `src/ui/pass_debug_window.rs` | All editor rendering code. `render_code_editor` function (~line 2060) |
| `src/perf_log.rs` | `metric_log!` macro, writes to temp file |
| `src/app/frame/mod.rs` | Frame-level timing |

## egui Internals Reference

| Location | What |
|----------|------|
| `epaint-0.34.3/src/text/fonts.rs:850` | `GalleyCache` struct |
| `epaint-0.34.3/src/text/fonts.rs:915` | `should_cache_each_paragraph_individually` — triggers per-line split when text has `\n` |
| `epaint-0.34.3/src/text/fonts.rs:962` | `layout_each_paragraph_individually` — reference implementation for splitting LayoutJob by `\n` |
| `epaint-0.34.3/src/text/fonts.rs:1072` | `flush_cache` — the flush that kills deferred viewport entries |
| `epaint-0.34.3/src/text/text_layout_types.rs:953` | `Galley::concat` — merges sub-galleys |
| `epaint-0.34.3/src/text/text_layout_types.rs:207` | `LayoutJob` Hash impl |
| `egui-0.34.3/src/context.rs:530` | `update_fonts_mut` → calls `fonts.begin_pass` |

## How to Verify

```bash
# Run render server, open a pass debug window, edit code
# Then check the log:
tail -f /var/folders/c3/m02z4_xn1ts1jlljpwhjqw0w0000gn/T/node-forge-frame-metrics.log

# Look for:
# [pass-debug] editor.show=XXms
# Before all fixes (editing): ~50-70ms
# Current (editing, single char): ~7-9ms (6ms highlight + ~1ms line cache)
# Current (browsing): ~0.35ms
# Target: <5ms editing
#
# Key log entries to watch:
# "all_hit (fast path)" — line cache fully cached, no work needed
# "hits=N misses=M" — M lines re-laid-out, N reused
# "p1=Xms p3s=Yms concat=Zms" — phase breakdown when not fast path
```

## Metrics Log Format

```
[frame] #1234 total=1.72ms | ingest=0.01ms advance=0.00ms analysis=0.01ms present=1.68ms finalize=0.00ms
[pass-debug] window=rende.rpass4.pass update_source=0.00ms
[pass-debug] code_editor pass=rende.rpass4.pass highlight_cache=0.00ms source_len=29221
[pass-debug] line_cache lines=767 all_hit (fast path)             ← browsing: text unchanged
[pass-debug] layouter_call=0.18ms wrap_width=818 (line-cache)     ← fast path cost
[pass-debug] editor.show=0.35ms                                   ← browsing total

# On edit frame:
[pass-debug] line_cache lines=767 hits=766 misses=1 p1=0.17ms p3s=0.14ms concat=0.10ms
[pass-debug] layouter_call=6.87ms wrap_width=818 (line-cache)     ← includes ~6ms re-highlight
[pass-debug] editor.show=8.75ms                                   ← edit total (target: <5ms)

[pass-debug] split pass=rende.rpass4.pass dependency_panel=1.39ms code_editor=9.00ms
[pass-debug] viewport-inner pass=rende.rpass4.pass toolbar=0.20ms central_panel=10.00ms
[pass-debug] window=rende.rpass4.pass viewport_render=10.20ms
[pass-debug] show_all total=0.03ms window_count=1
```
