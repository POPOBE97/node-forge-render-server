# Handoff: egui TextEdit double-click selection lag

Date: 2026-06-05

## Summary

The render server pass debug WGSL editor had a large lag, roughly 1.3s, when double-clicking to select a word in a 22KB `TextEdit`. The same visible editor code works in the official egui demo because upstream egui has already changed the text-selection implementation.

This is not caused by the render server viewport, the pass debug window UI, or syntax highlighting. The measured hotspot is inside egui's `TextEdit` double-click word selection path.

## Impacted path

Local render server editor:

- `node-forge-render-server/src/ui/pass_debug_window.rs`
- `render_code_editor`
- `egui::TextEdit::multiline(&mut document.draft_source)`
- `.code_editor()`
- `.layouter(...)`

egui internals:

- `egui/src/widgets/text_edit/builder.rs`
- `galley.cursor_from_pos(...)`
- `TextCursorState::pointer_interaction(...)`
- `select_word_at(...)`
- `ccursor_previous_word(...)`
- `next_word_boundary_char_index(...)`

## Evidence

Temporary timing logs were added to the local cargo registry copy of egui:

- `/Users/ruiyao/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/egui-0.33.3/src/widgets/text_edit/builder.rs`
- `/Users/ruiyao/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/egui-0.33.3/src/text_selection/text_cursor_state.rs`

Before the temporary egui patch:

```text
[egui textedit timing] cursor_from_pos event=double id=289C text_bytes=22453 cursor_index=337 elapsed_us=7
[egui textedit timing] ccursor_previous_word label=select_word_at/both_word text_bytes=22453 num_chars=22447 from=338 to=334 count_us=1 reverse_us=2132 boundary_us=1345044 total_us=1347178
[egui textedit timing] ccursor_next_word label=select_word_at/both_word text_bytes=22453 from=334 to=340 elapsed_us=493
[egui textedit timing] select_word_at branch=both_word text_bytes=22453 cursor_index=337 selected=334..340 probe_us=19 total_us=1347712
[egui textedit timing] pointer_interaction event=double id=289C did_interact=true selected=Some(334..340) elapsed_us=1347725
```

The culprit is `ccursor_previous_word`, especially `boundary_us`. It was taking about `1_345_044us`.

After applying the temporary upstream-style egui patch:

```text
[egui textedit timing] ccursor_previous_word label=select_word_at/line text_bytes=45 num_chars=45 from=10 to=4 count_us=0 reverse_us=9 boundary_us=14 total_us=24
[egui textedit timing] ccursor_next_word label=select_word_at/line text_bytes=45 from=10 to=14 elapsed_us=2
[egui textedit timing] select_word_at branch=line text_bytes=22453 cursor_index=5980 selected=5974..5984 probe_us=509 total_us=552
[egui textedit timing] pointer_interaction event=double id=289C did_interact=true selected=Some(5974..5984) elapsed_us=564
```

Double-click selection dropped from about 1.35s to about 0.57ms.

## Root cause

In egui `0.33.3`, `select_word_at` can call `ccursor_previous_word` against the full `TextEdit` text. `ccursor_previous_word` reverses the whole text, then calls `next_word_boundary_char_index` on the reversed text.

The old `next_word_boundary_char_index` implementation repeatedly calls `char_index_from_byte_index(text, word_byte_index)` for each word boundary. That recomputes a character index from the start of the string every time. For many words, this becomes effectively O(n^2). With a cursor near the beginning of a 22KB document, the reversed-text scan walks almost the whole document and triggers the large lag.

Upstream egui main now avoids this by:

- Limiting double-click `select_word_at` to the current line.
- Making `next_word_boundary_char_index` maintain a running character index instead of recalculating from the start for each word.
- Using a faster `find_line_start` based on byte search for `\n`.

Reference:

- `https://raw.githubusercontent.com/emilk/egui/main/crates/egui/src/text_selection/text_cursor_state.rs`

## Current versions

Observed render-server lockfile versions:

- `egui = 0.33.3`
- `eframe = 0.33.3`
- `egui_extras = 0.33.3`
- `egui-winit = 0.33.3`
- `egui_glow = 0.33.3`
- local patched `egui-wgpu = 0.33.3` from `node-forge-render-server/3rd/egui-wgpu`

Current crates.io search result on 2026-06-05:

- `egui = 0.34.3`
- `eframe = 0.34.3`
- `egui_extras = 0.34.3`

The project currently has:

- `node-forge-render-server/Cargo.toml`: `egui_extras = "0.33.3"`
- `node-forge-render-server/Cargo.toml`: `[patch.crates-io] egui-wgpu = { path = "3rd/egui-wgpu" }`
- `rust-wgpu-fiber/Cargo.toml`: `eframe = "0.33.0"`, which resolves to `0.33.3` in the render-server lockfile.
- `node-forge-render-server/3rd/egui-wgpu/Cargo.toml`: depends on `egui = "0.33.3"` and `epaint = "0.33.3"`.

## Recommended upgrade task

Preferred fix: bump the egui family to a release that includes the upstream text-selection fix.

Likely files to update:

- `node-forge-render-server/Cargo.toml`
- `rust-wgpu-fiber/Cargo.toml`
- `node-forge-render-server/3rd/egui-wgpu/Cargo.toml`
- `node-forge-render-server/Cargo.lock`
- Possibly `rust-wgpu-fiber/Cargo.lock`, if maintained independently.

Suggested direction:

```text
egui/eframe/egui_extras/egui-winit/egui_glow/epaint -> 0.34.3 or newer
```

Because `egui-wgpu` is locally patched under `node-forge-render-server/3rd/egui-wgpu`, that vendored crate must be checked carefully. It may need to be replaced with the matching upstream `egui-wgpu` version or rebased onto `0.34.3`.

## Verification steps

1. Remove the temporary local cargo-registry timing and selection patch, or run in a clean checkout/container.
2. Bump the egui family.
3. Run:

```bash
cargo check --manifest-path node-forge-render-server/Cargo.toml
```

4. Launch the render server.
5. Open the pass debug WGSL editor.
6. Paste or generate a roughly 22KB WGSL module.
7. Double-click words near the beginning, middle, and end of the editor.
8. Expected behavior: word selection should feel instant, with no ~1s stall.

Optional validation if timing logs are temporarily re-added:

- `cursor_from_pos` should stay in microseconds.
- `select_word_at` should be microseconds or low milliseconds.
- `ccursor_previous_word` should run on current-line text, not the full 22KB text.

Expected log shape after the fix:

```text
[egui textedit timing] ccursor_previous_word label=select_word_at/line text_bytes=45 ... boundary_us=14 total_us=24
[egui textedit timing] select_word_at branch=line text_bytes=22453 ... total_us=552
[egui textedit timing] pointer_interaction event=double ... elapsed_us=564
```

## Temporary local state to clean up

During debugging, the local cargo registry source for egui `0.33.3` was modified directly. This is not durable and should not be relied on for production.

Modified local registry files:

- `/Users/ruiyao/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/egui-0.33.3/src/widgets/text_edit/builder.rs`
- `/Users/ruiyao/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/egui-0.33.3/src/text_selection/text_cursor_state.rs`

The app was also returned to `show_viewport_deferred`; `show_viewport_immediate` was tested but did not solve the lag.

If the upgrade is not immediately possible, a fallback is to vendor or `[patch.crates-io]` egui `0.33.3` with only the upstream text-selection fix. That should be treated as temporary technical debt.
