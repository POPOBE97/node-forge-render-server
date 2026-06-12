# Shortwire Mode for Pass Debug Dependency Tree

## Context

The pass debug window currently has a freely-editable code editor that lets users modify shader source and apply patches. The user wants a **per-node "shortwire" mode** where right-clicking a dependency tree row opens a context menu with a "Shortwire" option. Entering this mode freezes the dep tree, makes the editor editable for free-form changes (labeled by node but not restricted to a specific source range), and stores the edits as a structured diff per node. Only one node's patch is active at a time. When re-entering shortwire on a node with a stored patch, the patch is re-applied (with inline diff highlighting), and if the base shader changed so the patch no longer applies, an error is shown.

## Design

### State Model (in `PassDebugWindowDocument`)

```rust
/// Stable composite key for patch storage.
/// target_id + relation_path + source_range fingerprint.
fn shortwire_patch_key(row: &PassDebugDependencyRow) -> String {
    let range_suffix = row.source_range
        .map(|r| format!("#{}-{}", r.start_byte, r.end_byte))
        .unwrap_or_default();
    match row.target_id.as_deref() {
        Some(target_id) => {
            if row.relation_path.is_empty() {
                format!("target:{target_id}{range_suffix}")
            } else {
                format!("target:{target_id}@{}{range_suffix}", row.relation_path)
            }
        }
        None => format!("label:{}{range_suffix}", row.label),
    }
}

/// Display identity for UI (not used as map key).
struct ShortwireRowIdentity {
    patch_key: String,          // the map key (stable composite)
    row_key_hint: String,       // for tree highlighting during shortwire mode
    label: String,              // for toolbar display
    target_id: Option<String>,
}

/// A stored per-node patch.
struct ShortwireNodePatch {
    hunks: Vec<ShortwireHunk>,
    base_source_hash: u64,
}

/// A single diff hunk with context for fuzzy re-application.
struct ShortwireHunk {
    old_start: usize,           // 0-indexed line in base
    old_lines: Vec<String>,     // lines removed from base (for verification + fuzzy match)
    new_lines: Vec<String>,     // replacement lines
    context_before: Vec<String>,// up to 3 lines before hunk (for fuzzy offset)
    context_after: Vec<String>, // up to 3 lines after hunk (for fuzzy offset)
}

enum ShortwirePhase {
    Editing,
    PendingApply { pending_hunks: Vec<ShortwireHunk> },
    PendingResetThenEnter { next_identity: ShortwireRowIdentity },
}

struct ShortwireActiveState {
    identity: ShortwireRowIdentity,
    /// Snapshot of generated_base_source at entry time.
    base_source: String,
    /// Hash of base_source at entry time.
    base_source_hash: u64,
    /// True when generated_base_source changed after entry (source refresh mid-session).
    base_source_stale: bool,
    diff_view_enabled: bool,
    phase: ShortwirePhase,
}
```

Add to `PassDebugWindowDocument`:
- `shortwire_patches: HashMap<String, ShortwireNodePatch>` — keyed by `shortwire_patch_key()` output
- `shortwire_active: Option<ShortwireActiveState>` — None when not in shortwire mode
- `generated_base_source: String` — the unpatched shader source from the scene pipeline
- `generated_base_source_hash: u64` — hash of above, for fast comparison

**`base_source_stale` lifecycle**: set to `true` by `update_source` when base changes during active shortwire; reset to `false` on entry, cancel, mark_reset, and mark_patch_applied.

**Patch map key**: `target_id + relation_path + source_range fingerprint`. This disambiguates rows that share the same target_id and relation_path but reference different source locations (e.g. a variable used multiple times in the same call chain). `row_key` remains a non-stable display hint only.

**`generated_base_source` provenance:**

The problem: `pass_debug_sources` at the app level is overwritten with override WGSL during `apply_pass_shader_overrides` (api.rs:165). So when `update_source` is called, `source.module_source` may already be the patched source.

Solution: `generated_base_source` is updated **only** when `patch_active == false` in `update_source`. When `patch_active == true`, the incoming source is tainted by the override and must not replace `generated_base_source`. Additionally, `mark_reset` sets `generated_base_source = source.module_source`.

Lifecycle:
- `new(patch_active=false)`: `generated_base_source = source.module_source`
- `new(patch_active=true)`: `generated_base_source = ""` (filled on next non-patched refresh)
- `update_source(patch_active=false)`: `generated_base_source = source.module_source`
- `update_source(patch_active=true)`: **no change** to `generated_base_source`
- `mark_applied`: **no change** to `generated_base_source`
- `mark_reset`: `generated_base_source = source.module_source`

Edge case: document opened while a patch is active → `generated_base_source` is empty → context menu shows "Shortwire (base unavailable)" greyed out until reset or clean rebuild.

### `update_source` During Active Shortwire

`update_source` is called every frame. When `shortwire_active.is_some()`, an early branch prevents the normal `replace_draft_source` path (pass_debug_window.rs:476) from overwriting the user's in-progress edits:

```
if shortwire_active.is_some() {
    // Update generated_base_source if patch_active == false and source changed
    if !patch_active && source.module_source != generated_base_source {
        generated_base_source = source.module_source;
        generated_base_source_hash = hash(generated_base_source);
        shortwire_active.base_source_stale = true;
    }
    // Do NOT call replace_draft_source — user's edits are preserved
    // Do NOT refresh analysis rows
    // Still update source_revision and self.source for metadata
    return;
}
```

This ensures:
1. `draft_source` (user's edits) is never clobbered by a source refresh
2. `generated_base_source` tracks the latest truth for Done-time diffing
3. No analysis/dep-tree refresh interrupts the editing session

### Behavior

1. **Editor is read-only by default**: `TextEdit::multiline(...).interactive(matches!(phase, Some(ShortwirePhase::Editing)))`. Outside shortwire mode, the editor is view-only. Toolbar shows "Copy WGSL", "Reset Patch" (when `patch_active`), and "Reset All".

2. **Context menu on dep tree rows**: In `render_scrollable_tree_rows`, add `.context_menu()` on each selectable row's response (pattern from `file_tree_widget.rs:269`). Menu item: "Shortwire". Disabled when `shortwire_active.is_some()` or `generated_base_source.is_empty()`.

3. **Entering shortwire mode**:
   - If `patch_active` (an override is live):
     - Set `shortwire_active` with phase `PendingResetThenEnter { next_identity }`
     - Emit `PassDebugWindowAction::ResetPatch { pass_name }`
     - UI shows "Resetting..." state, editor non-interactive
   - If not patched, enter immediately:
     - Set `shortwire_active` with phase `Editing`, `base_source_stale = false`
     - `base_source = generated_base_source.clone()`, `base_source_hash = generated_base_source_hash`
     - Attempt to apply stored patch (see re-entry below)
     - Copy result into `draft_source`
     - Cancel pending async analysis, clear `draft_analysis_due_secs`

   **`mark_reset` completion path**: When `mark_reset` is called and phase is `PendingResetThenEnter`:
   - `generated_base_source` is now fresh (set by `mark_reset` itself)
   - Transition phase to `Editing`, `base_source_stale = false`
   - Set `base_source = generated_base_source.clone()`
   - Attempt stored patch re-application
   - Copy into `draft_source`

   **`record_error` during `PendingResetThenEnter`**: Reset failed. Clear `shortwire_active = None`, show error "Failed to reset patch: {error}". Return to non-shortwire state.

4. **Stored patch re-application on re-entry**:
   - Lookup patch by `identity.patch_key`
   - If `base_source_hash` matches: apply hunks directly (reverse order)
   - If hash differs: attempt fuzzy application using `context_before`/`context_after` and `old_lines` to locate correct offset. On failure: show error "Shortwire patch outdated — base shader changed", clear stored patch, start fresh
   - On success: set `diff_view_enabled = true`

5. **While in shortwire mode (phase = Editing)**:
   - **Dep tree frozen**: render all rows at 30% opacity except the active row (matched by `row_key_hint`; full opacity + highlight). Row clicks are no-ops.
   - Editor edits go to `draft_source` as normal.
   - **Dependency analysis suppressed**: gate `mark_draft_edited` to skip `schedule_draft_analysis`; `maybe_refresh_pending_draft_analysis` early-returns; `poll_pending_analysis` discards results; editor-click handler skips `focus_target_at_char_index`.
   - Toolbar: "Shortwire: {label}" heading, "Done" button, "Cancel" button, "Diff" toggle (top-right).
   - If `base_source_stale`: show warning banner "Base shader updated — will rebase on Done".

6. **Diff view (inline highlights)**: When `diff_view_enabled` is toggled on, compute line-level diff between `base_source` and `draft_source` using `similar::TextDiff::from_lines`. Paint added/changed lines with green-tinted background. At deletion points, paint a thin red horizontal marker between adjacent lines. Cache keyed by `(draft_source_hash, base_source_hash)` — recompute when either changes (base can change mid-session due to staleness).

7. **Exiting shortwire mode (Done)**:
   - If `base_source_stale` (generated base changed during editing):
     - Compute user's intent as hunks: diff `base_source` (entry snapshot) vs `draft_source`
     - Rebase: apply those hunks onto current `generated_base_source` using `apply_hunks`
     - If rebase succeeds: update `draft_source` to rebased result, proceed normally
     - If rebase fails: block Done, show error "Cannot rebase onto new base — resolve conflicts manually". User remains in Editing and can inspect the new base via Diff toggle.
   - Compute final hunks: diff current `generated_base_source` vs (rebased) `draft_source`
   - Set phase to `PendingApply { pending_hunks: final_hunks }`
   - Emit `PassDebugWindowAction::ApplyPatch { pass_name, source: draft_source }`
   - Editor becomes non-interactive
   - On `mark_patch_applied`: move `pending_hunks` into `shortwire_patches[key]` with `generated_base_source_hash`, clear `shortwire_active`
   - On `record_error` while `PendingApply`: revert phase to `Editing`, show error, `pending_hunks` discarded, nothing stored

8. **Exiting shortwire mode (Cancel)**:
   - Revert `draft_source` back to `generated_base_source`
   - Clear `shortwire_active = None`
   - No patch stored, no apply emitted

9. **Switching nodes**: Disabled while `shortwire_active.is_some()`. User must Done/Cancel first.

### Hunk Application Semantics

`apply_hunks` processes hunks in **reverse order** (highest `old_start` first) so earlier hunks' indices remain valid after later splices.

For each hunk:
1. **Locate target position**:
   - If `base_source_hash` matches the stored patch hash: use `old_start` directly
   - Otherwise (fuzzy mode): search within ±30 lines of `old_start` for a position where `context_before` matches the preceding lines and either `old_lines` matches at the position (for replace/delete hunks) or `context_after` matches the following lines (for insert-only hunks)
2. **Verify**:
   - For hunks with `!old_lines.is_empty()`: verify `old_lines` match at the resolved position
   - For insert-only hunks (`old_lines.is_empty()`): verify `context_before` + `context_after` match. Multiple insert-only hunks at the same line are applied in their original order (reverse-order processing naturally handles this since inserts at the same position don't shift each other when old_count=0)
3. **Apply**: replace `base_lines[position..position+old_lines.len()]` with `new_lines`

If any hunk fails to locate or verify, the entire application fails (no partial apply).

### Diff Library

Add `similar` crate to `Cargo.toml`:
```toml
similar = "2"
```

Usage:
- `similar::TextDiff::from_lines(base, edited)` for computing diffs and inline highlights
- Iterate `diff.ops()` to build `ShortwireHunk` structs (extract context from surrounding Equal ops)
- Custom `apply_hunks(base_lines: &[&str], hunks: &[ShortwireHunk]) -> Result<String, HunkApplyError>`

### Key Files to Modify

- **`node-forge-render-server/Cargo.toml`** — add `similar = "2"`
- **`node-forge-render-server/src/ui/pass_debug_window.rs`** — main implementation:
  - Add `ShortwireRowIdentity`, `ShortwireNodePatch`, `ShortwireHunk`, `ShortwirePhase`, `ShortwireActiveState` structs, `shortwire_patch_key()` function
  - Add `shortwire_patches`, `shortwire_active`, `generated_base_source`, `generated_base_source_hash` fields to `PassDebugWindowDocument`
  - Add early `shortwire_active` branch in `update_source` that skips `replace_draft_source` and analysis refresh
  - Update `new`, `update_source`, `mark_applied`, `mark_reset`, `record_error` to maintain `generated_base_source` and handle all phase transitions
  - Add context menu in `render_scrollable_tree_rows` (on selectable row responses)
  - Modify `render_code_editor`: `.interactive(...)` gated by shortwire phase
  - Modify `render_pass_debug_toolbar`: shortwire toolbar when active, keep "Reset Patch" visible when `patch_active` and not in shortwire
  - Add `paint_shortwire_diff_overlay` (green bg for added lines, red marker for deletions)
  - Add `enter_shortwire`, `complete_shortwire_entry`, `exit_shortwire_done` (with rebase logic), `exit_shortwire_cancel`, `apply_hunks`, `compute_hunks` methods
  - Gate `mark_draft_edited`, `maybe_refresh_pending_draft_analysis`, `poll_pending_analysis`, editor-click focus
  - Modify dep tree row rendering: alpha dimming when `shortwire_active.is_some()`

### Existing Patterns to Reuse

- `response.context_menu(|ui| { ... })` — from `file_tree_widget.rs:269`
- `paint_focus_highlight_overlay` at line 2900 — pattern for painting line-range overlays on the editor
- `push_action` + `PassDebugWindowAction::ApplyPatch` — for applying the final shader override
- `tree_selected_row_bg` / `tree_hovered_row_bg` — for row highlighting colors
- `mark_patch_applied` / `record_error` callbacks — for confirming or rejecting the apply

## Verification

1. `cargo check -p node-forge-render-server` — compiles
2. `cargo test -p node-forge-render-server` — existing tests pass
3. New unit tests:
   - `generated_base_source` not updated when `patch_active=true` in `update_source`
   - `update_source` during active shortwire: does NOT overwrite `draft_source`
   - `update_source` during active shortwire with new base: sets `base_source_stale = true`
   - `mark_reset` triggers `PendingResetThenEnter` → `Editing` transition, `base_source_stale = false`
   - `record_error` during `PendingResetThenEnter` → clears `shortwire_active`, returns to idle
   - Re-entering same node after apply: stored patch re-applies against `generated_base_source`
   - Fuzzy hunk application: patch applies at shifted offset when context lines match
   - Fuzzy application of insert-only hunks: uses context_before + context_after for positioning
   - Multiple insert-only hunks at same line: applied in correct order
   - Failed hunk application: error shown, stored patch cleared, user starts fresh
   - `record_error` during `PendingApply`: phase reverts to `Editing`, no patch stored
   - Cancel during editing: no patch stored, `draft_source` reverted, `base_source_stale` cleared
   - Pending async analysis discarded on shortwire entry and during editing
   - Document opened with `patch_active=true`: `generated_base_source` empty, context menu disabled
   - Done with stale base: rebase user edits onto new base, apply rebased source
   - Done with stale base + rebase failure: Done blocked, error shown, stays in Editing
   - `apply_hunks` reverse-order: later hunks don't shift earlier hunk positions
   - Patch key stability: same target at different relation paths → independent patches
   - Patch key with source_range fingerprint: same target+relation but different source ranges → independent
   - Diff cache invalidated when base_source_hash changes (staleness)
4. Manual: open pass debug window, right-click a dep tree node → "Shortwire" appears
5. Manual: entering shortwire mode makes editor editable, dep tree dimmed
6. Manual: edit code, click "Done" → patch applied to renderer, editor returns to read-only
7. Manual: re-enter shortwire on same node → previous edits shown with diff highlights
8. Manual: change the scene to regenerate shader, re-enter shortwire → error if patch conflicts
