# Pass Debug Window Refactor Implementation Plan

## Status

Draft implementation roadmap. This file turns
`20-debug-window-refactor.md` into PR-sized execution slices.

The architecture document is the contract. This document is the migration plan. If the two disagree,
update the architecture document first, then adjust this plan.

## Scope

This plan covers the staged refactor of `src/ui/pass_debug_window.rs` into
`src/ui/pass_debug/` without changing app-facing behavior.

The plan deliberately keeps the current public boundary during migration:

- `open_pass_debug_window`
- `show_pass_debug_windows`
- `mark_patch_applied`
- `mark_patch_reset`
- `mark_all_patches_reset`
- `record_patch_error`
- `record_all_patch_error`
- `record_shortwire_diff_result`
- `request_active_shortwire_diff_capture`
- `has_active_shortwire`
- `PassDebugWindowMap`
- `PassDebugWindowAction`
- `PassDebugPatchApplyResult`
- `ShortwireDiffCaptureRequest`
- `ShortwireDiffResult`

Do not redesign `AppCommand`, renderer patch application, or Shortwire diff capture scheduling in
this refactor.

Preserve the exact callback return shapes during migration:

- `mark_patch_applied` returns `PassDebugPatchApplyResult`.
- `request_active_shortwire_diff_capture` returns `PassDebugPatchApplyResult`.
- `record_shortwire_diff_result` returns `Vec<(DebugArtifactItem, String)>`.
- `mark_patch_reset`, `mark_all_patches_reset`, `record_patch_error`, and
  `record_all_patch_error` return no artifacts today and should not be widened in this refactor.

## Execution Rules

- Every PR must preserve current behavior at the public pass debug boundary.
- Every PR must keep `pass_debug_window.rs` compiling and callable from existing app code.
- Do not move egui rendering before durable state and effect boundaries exist.
- Do not introduce async workers in the first implementation. Effect APIs should make async possible
  later, but the runner can stay synchronous.
- Do not clear dirty editor/reference/patch state from request emission alone. Clear it only after
  the corresponding acknowledgement event or effect completion.
- Prefer wrapper delegation over wide rewrites. The old method names can stay temporarily if they
  call into extracted modules.
- If a PR touches behavior and extraction at the same time, split it unless the behavior change is a
  documented bug fix with a focused test.

## Module Dependency Order

Use this order unless a blocker forces a smaller local detour:

1. `patch.rs`
   Pure hunk, apply, diff view, and three-way merge primitives.
2. `artifacts.rs`
   Debug artifact naming, slot keys, manifests, and JSON payload encode/decode.
3. `file_io.rs`
   Local reference scan/read/write helpers, still synchronous.
4. `reference_workspace.rs`
   Reference state, reducer/effects, sync plan, and reference patch store.
5. `shader_document.rs`
   Canonical/runtime/draft shader lifecycle and patch apply/reset completion state.
6. `dependency_tree.rs`
   Canonical dependency rows, root/focus/expansion/filter/navigation state.
7. `merge.rs`
   Canonical-change auto-merge, conflict lifecycle, pending Shortwire patch rebase token.
8. `shortwire.rs`
   Shortwire stored patch lifecycle, active session, pending apply acknowledgement, diff capture.
9. `store.rs` and `event.rs`
   Cross-module reducer dispatch, internal effects, and public action conversion.
10. `registry.rs`, `viewport.rs`, and `render/`
    Window map, viewport lifecycle, effect runner, and egui view extraction.

Rationale:

- `patch.rs`, `artifacts.rs`, and `file_io.rs` have the lowest coupling and create safe foundations.
- Reference workspace should move before full Shortwire extraction because shader Shortwire currently
  mutates reference-side state directly.
- Shader and dependency tree can be split before views because current render code can still call old
  wrappers.
- Merge should move before Shortwire completes so pending patch rebase becomes a store handoff rather
  than a hidden shader callback side effect.

## Compatibility Layer

The compatibility layer exists to keep app code stable while internals move.

Keep `src/ui/pass_debug_window.rs` as the facade until the final PR. It can shrink over time, but it
should continue to export the existing public types and functions.

Temporary shape:

```text
src/ui/pass_debug_window.rs
  -> pub use pass_debug::api types/functions, or thin wrappers

src/ui/pass_debug/
  mod.rs
  api.rs or mod.rs          # public facade implementation
  registry.rs
  store.rs
  event.rs
  ...
```

During migration, internal modules may emit `PassDebugEffect`, but public callers still receive one
of the current outputs:

- per-frame `Vec<PassDebugWindowAction>` from `show_pass_debug_windows`
- immediate `PassDebugPatchApplyResult` from patch/diff callback paths
- immediate `Vec<(DebugArtifactItem, String)>` from `record_shortwire_diff_result`

Adapter rules:

- `PassDebugEffect::ApplyPatch` converts to `PassDebugWindowAction::ApplyPatch`.
- `PassDebugEffect::ResetPatch` converts to `PassDebugWindowAction::ResetPatch`.
- `PassDebugEffect::ResetAllPatches` converts to `PassDebugWindowAction::ResetAllPatches`.
- `PassDebugEffect::UpsertTextArtifact` converts to `PassDebugWindowAction::UpsertDebugArtifact`.
- `PassDebugEffect::UpsertBinaryArtifact` stays on callback result paths for now.
- `PassDebugEffect::RequestDiffCapture` stays on `PassDebugPatchApplyResult` for now.
- Reference file/dialog effects are handled inside the pass debug registry/effect runner, not
  surfaced as `AppCommand`.

Do not widen `PassDebugWindowAction` for binary artifacts or diff capture in this refactor. That is
an app command boundary redesign and should be separate.

## Validation Commands

Run from the repository root unless noted.

Focused pass debug tests:

```sh
cargo test --manifest-path node-forge-render-server/Cargo.toml pass_debug_window
```

Focused extraction module tests once modules exist:

```sh
cargo test --manifest-path node-forge-render-server/Cargo.toml patch
cargo test --manifest-path node-forge-render-server/Cargo.toml reference_workspace
cargo test --manifest-path node-forge-render-server/Cargo.toml shader_document
cargo test --manifest-path node-forge-render-server/Cargo.toml dependency_tree
cargo test --manifest-path node-forge-render-server/Cargo.toml shortwire
cargo test --manifest-path node-forge-render-server/Cargo.toml merge
```

Broad render-server safety net:

```sh
cargo test --manifest-path node-forge-render-server/Cargo.toml
```

Format/check before handoff:

```sh
cargo fmt --manifest-path node-forge-render-server/Cargo.toml --check
git -C node-forge-render-server diff --check
```

If the focused `pass_debug_window` filter misses tests after files move, replace it with the relevant
module filters or run the full render-server test suite.

## PR Roadmap

### PR 0: Freeze Current Behavior

Goal: establish the current behavior baseline before moving code.

Actions:

- Run the focused pass debug test filter before extraction starts.
- Run the full render-server test suite if the focused filter misses relevant colocated tests.
- Map each Phase 0 invariant from the architecture document to an existing test or add the missing
  focused test before moving ownership.
- Record any known failing or flaky tests in the PR description before starting extraction work.

Required invariant coverage:

- Dirty shader draft is not overwritten by source refresh.
- Dependency tree tracks canonical source, not patched draft.
- Active Shortwire source refresh marks base stale without clobbering draft.
- Canonical changes rebase a stored patch only after app acknowledgement or enter merge conflict.
- Reference workspace local files write, restore, and skip rooted missing-file fallback correctly.
- Debug artifact payloads round-trip with legacy optional fields.
- Viewport transient close requests are filtered.

Exit criteria:

- Current tests pass or any failures are documented as unrelated pre-existing failures.
- Missing invariant tests are added before PR 1 changes ownership.

### PR 1: Extract Pure Patch Primitives

Goal: create `src/ui/pass_debug/patch.rs` and move pure patch algorithms without changing behavior.

Files:

- Update `src/ui/mod.rs` to declare `pass_debug`
- Add `src/ui/pass_debug/mod.rs`
- Add `src/ui/pass_debug/patch.rs`
- Keep `src/ui/pass_debug_window.rs` as the public facade and old test host for now

Move:

- `ShortwireHunk`
- `HunkApplyError`
- `compute_hunks`
- `apply_hunks`
- `locate_hunk_position`
- `verify_hunk_at_position`
- `three_way_merge_sources`
- compact diff view helpers only if they can move without importing egui/window state

Do not move:

- Shortwire session state
- `DebugArtifactItem` construction
- reference workspace state
- app-facing actions
- render functions

Validation:

- Existing hunk, diff, Shortwire rebase, and merge tests pass.
- `patch.rs` imports no `egui`, `rfd`, `std::fs`, app shell, viewport, or debug artifact store types.

Exit criteria:

- All moved functions are pure or depend only on data types in `patch.rs`.
- `pass_debug_window.rs` delegates to `patch.rs`.
- Public API is unchanged.

### PR 2: Extract Artifact Naming And Payloads

Goal: make debug artifact IDs, slot keys, manifests, and patch payload encode/decode have one owner.

Files:

- Add `src/ui/pass_debug/artifacts.rs`
- Reuse `src/ui/pass_debug/mod.rs`

Move:

- `DEBUG_ARTIFACT_*` constants that are pass-debug specific
- `REFERENCE_WORKSPACE_VERSION`
- `ShortwirePatchesPayload`
- `ReferenceWorkspaceManifest`
- `ReferenceWorkspaceManifestFile`
- `ReferencePatchesPayload`
- pass reference workspace/file/patch artifact id helpers
- Shortwire reference image artifact id/item helpers if they can remain free of app state

Keep temporarily:

- Dirty flags on `PassDebugWindowDocument`
- `take_*_dirty_artifact` wrapper methods

Validation:

- Artifact round-trip tests pass.
- Legacy patch JSON restore still accepts missing `referenceImage` and `diffResult`.
- Reference workspace migration still emits the same slot keys.

Exit criteria:

- Artifact slot keys are defined once.
- Payload encode/decode is callable without window/document state.
- Public artifact IDs remain stable.

### PR 3: Extract Local Reference File I/O

Goal: isolate local filesystem operations behind a small synchronous adapter.

Files:

- Add `src/ui/pass_debug/file_io.rs`

Move:

- `read_manifest_reference_file`
- `read_reference_file`
- `read_reference_folder`
- `write_reference_workspace_file`
- `reference_relative_path`
- max file byte and max folder file limits, unless they stay in a shared config module

State of the world after PR:

- Existing document methods may still call these helpers synchronously.
- No new reducer/state-transition code may call `std::fs` directly.

Validation:

- Rooted manifest local-load tests pass.
- Rooted missing-file no-fallback tests pass.
- Rooted sync write tests pass.
- Reference reload missing root keeps the archive snapshot.

Exit criteria:

- `file_io.rs` imports no pass debug window/store/document types.
- File I/O has typed result structs or explicit `Result<_, String>` boundaries.

### PR 4: Introduce Reference Workspace Reducer Skeleton

Goal: create `ReferenceWorkspaceState` ownership without moving every caller at once.

Files:

- Add `src/ui/pass_debug/reference_workspace.rs`

Introduce:

- `ReferenceWorkspaceState`
- `ReferenceLocation`
- `ReferenceFileState`
- `ReferenceSyncState`
- `ReferencePendingSync`
- `ReferenceSyncPlan`
- `ReferenceSyncResult`
- `ReferenceEffect`
- `ReferenceEvent`
- `ReferencePatchStore`

Migrate:

- Existing `ReferenceWorkspaceState` fields into the module shape.
- Old document methods become wrappers around reference methods where possible.

Do not yet:

- Move all reference Shortwire handoffs.
- Move all render controls to events.

Validation:

- Reference file selection and editor dirty tests pass.
- Existing reference workspace restore/import/reload tests pass.

Exit criteria:

- Reference state is held as one child object.
- Wrapper methods preserve old call sites.
- No behavior change in sync timing or status text unless tests are updated for an intentional bug
  fix.

### PR 5: Convert Reference Artifact Restore To Effects

Goal: remove local reads from artifact restore and model rooted manifest load as an effect.

Implement:

- `ReferenceArtifactSnapshot` pure decode.
- `ReferenceEffect::ReadManifestLocalFiles`.
- Completion event for local manifest reads.
- Temporary synchronous `ReferenceEffect` / `ReferenceEvent` runner in the pass debug facade.

Do not introduce the full `PassDebugEffect` / `PassDebugEvent` cross-module model in this PR unless
PR 12 is deliberately pulled forward. The temporary runner should have the same completion-boundary
shape so it can be folded into the store-level effect runner later.

Behavior:

- Pathless manifests restore from archived `file:*` artifacts.
- Rooted manifests emit a local read effect.
- Missing rooted files do not fall back to archived artifact text.
- Dirty local state blocks replacement and only permits acknowledgements.

Validation:

- Add reducer/effect test: rooted restore emits `ReadManifestLocalFiles` and does not read files
  during decode.
- Add stale local-load completion test: completion after dirty edit does not clobber draft.
- Preserve existing rooted/pathless restore tests.

Exit criteria:

- `reference_workspace.rs` imports no `std::fs`.
- Rooted manifest local reads happen only in `file_io.rs` via the effect runner.

### PR 6: Convert Reference Sync To Sync Plan

Goal: replace inline sync mutation with plan/effect/completion semantics.

Implement:

- `ReferenceSyncPlan` construction for rooted and archived workspaces.
- `ReferenceEffect::RunSyncPlan`.
- `ReferenceEvent::SyncCompleted`.
- Plan id and workspace revision checks.

Behavior:

- Rooted sync writes local dirty files and upserts only the manifest artifact.
- Archived sync upserts manifest and dirty `ReferenceCode` file artifacts.
- Dirty flags clear only after successful completion.
- Stale completion cannot mark newer edits clean.

Validation:

- Existing rooted sync write test passes.
- Add tests for write failure retaining dirty state.
- Add tests for stale sync completion.
- Add tests for rooted vs archived artifact emission.

Exit criteria:

- `take_reference_workspace_dirty_artifacts` is either gone or delegates through sync plan execution.
- Reducer code does not write local files directly.

### PR 7: Move Reference Shortwire Store Under Reference Workspace

Goal: stop storing reference patches as siblings of `reference_workspace`.

Move:

- `reference_patches`
- `reference_patches_dirty`
- reference patch restore/collect/take logic
- `reference_shortwire_patch_key`

Implement reference reducer events:

- `EnterShortwire`
- `PrepareShortwireSave`
- `LeftApplySucceeded`
- `LeftApplyFailed`
- `ExitShortwire`

Keep temporary adapters from shader Shortwire methods if full Shortwire extraction is not ready.

Validation:

- Reference patch key isolation by relative path.
- Reference Shortwire apply success commits after left apply.
- Apply failure keeps pending draft uncommitted.
- Re-enter writes stored patch to local file.
- Close restores local file.
- `mark_patch_applied` still returns shader and reference patch artifacts immediately.

Exit criteria:

- `PassDebugWindowDocument` no longer has `reference_patches` as a sibling field.
- Reference patch artifacts have one dirty flag owner.

### PR 8: Extract Shader Document State

Goal: introduce canonical/runtime/draft shader state and pending app acknowledgement modeling.

Files:

- Add `src/ui/pass_debug/shader_document.rs`

Implement:

- `ShaderDocumentState`
- canonical snapshot
- runtime loaded/pending state
- draft state
- status state
- selectors for loaded source, dirty, patch active, canonical dependency snapshot

Migrate:

- `replace_draft_source`
- `mark_draft_edited`
- same-revision patch-source refresh behavior
- clean/dirty source refresh behavior
- normal app patch applied/reset/failed state updates

Keep temporary wrappers:

- `PassDebugWindowDocument::update_source`
- `mark_applied`
- `mark_reset`
- `record_error`

Validation:

- Dirty draft is not overwritten by source refresh.
- Same source revision does not refresh canonical analysis.
- Patch override text equal to canonical still reports patch-active.
- Apply/reset is pending until app callbacks arrive.

Exit criteria:

- Shader state imports no `egui`, `rfd`, `std::fs`, reference workspace, or debug artifact store.
- Dirty and patch-active semantics are selector-owned.

### PR 9: Extract Dependency Tree State

Goal: isolate canonical dependency analysis, flattened rows, focus, expansion, filter, and navigation.

Files:

- Add `src/ui/pass_debug/dependency_tree.rs`

Move:

- `PassDebugDependencyRow`
- `PassDebugTreeClick`
- row flattening helpers
- focus and source-range lookup helpers
- expansion and visible row index logic that is not egui-measurement dependent
- filter selector

Do not move:

- Tree width cache that depends on `egui::Ui`
- paint/render code

Validation:

- Dependency tree tracks canonical source, not draft or runtime override.
- Duplicate target rows keep distinct row keys.
- Editor click focuses best matching canonical row.
- Filter selector includes matching rows and ancestors.
- Root focus behavior remains stable.

Exit criteria:

- `dependency_tree.rs` imports no `egui`, `rfd`, `std::fs`, Shortwire patch payloads, reference
  workspace, merge state, viewport state, or debug artifact types.

### PR 10: Extract Merge Flow

Goal: make canonical-change merge and pending Shortwire rebase token explicit.

Files:

- Add `src/ui/pass_debug/merge.rs`

Move:

- `PassDebugMergeConflict`
- `PassDebugPendingMergePatchUpdate`
- merge resolver state transitions
- pending patch rebase verification

Implement:

- `MergeState`
- `MergeEvent`
- `MergeEffect`
- handoff token to Shortwire after app acknowledgement

Validation:

- Clean auto-merge emits apply/reset.
- Conflict resolver behavior remains unchanged.
- Pending rebase is not committed on app failure or mismatched acknowledgement source.
- Conflict status selector wins over status/error string ambiguity.

Exit criteria:

- Merge imports no egui, fs, rfd, debug artifact payloads, or render caches.
- Stored Shortwire patch rebase happens only after app acknowledgement.

### PR 11: Extract Shortwire State Machine

Goal: make Shortwire active session, pending apply, stored patches, reference image, and diff capture
state explicit.

Files:

- Add `src/ui/pass_debug/shortwire.rs`

Implement:

- `ShortwireState`
- `ShortwirePatchStore`
- `ShortwireSession`
- `ShortwirePendingApply`
- artifact restore/collect/take for shader Shortwire patches
- selectors for active row, dot info, editor interactivity, and diff view

Correct during extraction:

- Keep pending apply hunks separate from active UI session so navigate/close during pending apply
  cannot lose the later app acknowledgement token.

Validation:

- Existing Shortwire tests pass.
- Add reducer test for navigate/close during pending apply preserving enough state for later
  acknowledgement.
- Add tests for explicit diff capture and image-only patch creation.
- Add tests for diff result clearing when reference image changes.

Exit criteria:

- `shortwire.rs` imports no `egui`, `rfd`, `std::fs`, viewport types, reference workspace concrete
  fields, merge UI state, or app shell/canvas types.
- Stored patches commit only on app acknowledgement.

### PR 12: Introduce Store, Events, And Internal Effects

Goal: coordinate extracted modules through `PassDebugStore`.

Files:

- Add `src/ui/pass_debug/store.rs`
- Add `src/ui/pass_debug/event.rs`
- Add `src/ui/pass_debug/selectors.rs` if needed

Implement:

- `PassDebugStore`
- `PassDebugEvent`
- `PassDebugEffect`
- dispatch/drain loop
- store-level coordination between shader, dependency, Shortwire, reference, and merge

Compatibility:

- `PassDebugWindowDocument` may become a wrapper around `PassDebugStore`.
- Existing render functions may still receive a document/store lock.
- Effects convert to current public action/result outputs.

Validation:

- Reducer tests for cross-module handoffs:
  - shader canonical analysis feeds dependency rows
  - Shortwire save asks reference workspace to prepare/commit
  - merge app acknowledgement hands rebase token to Shortwire
  - reference sync effects drain through registry

Exit criteria:

- Durable state mutations happen through dispatch helpers, not arbitrary render methods.
- Public API unchanged.

### PR 13: Move Registry And Effect Runner

Goal: move window map and per-frame orchestration out of the legacy file.

Files:

- Add `src/ui/pass_debug/registry.rs`
- Add `src/ui/pass_debug/viewport.rs`

Move:

- `PassDebugWindowMap`
- `PassDebugWindowState`
- `show_pass_debug_windows` internals
- `open_pass_debug_window` internals
- effect runner for file dialogs, local reference reads/writes, sync plans, viewport focus/close
- viewport close classification

Keep:

- public facade in `pass_debug_window.rs`

Validation:

- Viewport transient close request tests pass.
- Public open/show behavior unchanged.
- Reference folder picker and sync effects are handled internally.

Exit criteria:

- App frame code still imports `ui::pass_debug_window`.
- Registry/effect runner owns non-app local side effects.

### PR 14: Extract Render Modules

Goal: move egui views after state/effect boundaries are stable.

Files:

- Add `src/ui/pass_debug/render/`
- Add panel-specific modules as listed in the architecture document

Move in order:

1. titlebar and toolbar
2. dependency panel
3. code editor and line gutter
4. reference editor
5. merge dialog
6. tree/text paint helpers
7. root split layout

Rules:

- Views render selectors/view models and emit events.
- Views do not mutate durable state directly.
- Render caches stay disposable and separate from domain state.

Validation:

- Manual egui smoke test after each visible move.
- Existing unit tests should remain green.
- No visual behavior changes beyond unavoidable module movement.

Exit criteria:

- Render code does not call domain mutation methods directly.

### PR 15: Retire Legacy Facade Internals

Goal: leave `pass_debug_window.rs` as a thin compatibility facade or remove it after call sites move.

Options:

- Keep `src/ui/pass_debug_window.rs` as `pub use crate::ui::pass_debug::*;`
- Or update call sites to `ui::pass_debug` and remove the old module

Choose the lower-churn option for the first completion pass.

Validation:

- Full render-server tests pass.
- App frame command/present call sites are unchanged or mechanically updated.
- No large mixed module remains.

Exit criteria:

- `pass_debug_window.rs` is no longer a mixed implementation file.
- Public behavior is unchanged.

## First Implementation Slice

After PR 0 preflight, start code movement with PR 1. It has the lowest state risk and creates a
foundation for merge, Shortwire, and reference patches.

Detailed steps:

1. Run PR 0 preflight and confirm the current behavior baseline.
2. Update `src/ui/mod.rs` to declare `pass_debug`.
3. Create `src/ui/pass_debug/mod.rs` with `pub mod patch;`.
4. Create `src/ui/pass_debug/patch.rs`.
5. Move hunk and merge primitives into `patch.rs`.
6. Keep types private or `pub(crate)` unless an extracted module needs them.
7. Update `pass_debug_window.rs` to import from `crate::ui::pass_debug::patch`.
8. Move the focused hunk/merge tests only if doing so is mechanical. Otherwise leave tests in place
   for the first PR.
9. Run focused tests.
10. Run `cargo fmt`.

First-slice non-goals:

- Do not introduce `PassDebugStore`.
- Do not move Shortwire session fields.
- Do not move artifact payloads.
- Do not change `PassDebugWindowAction`.
- Do not change UI rendering.

First-slice acceptance checklist:

- Hunk application behavior is unchanged.
- Three-way merge behavior is unchanged.
- No extracted patch function imports egui or filesystem code.
- `pass_debug_window.rs` is smaller and only delegates for moved primitives.

## Minimal Effect Runner Design

The first effect runner can live in `registry.rs` once that module exists. Before then, it can be a
private helper in the facade.

PR 5-11 may use module-specific temporary effect/event types, such as
`ReferenceEffect` / `ReferenceEvent`, before the store-level `PassDebugEffect` /
`PassDebugEvent` exists. Those temporary runners must preserve the same ordering and completion
boundary described here so they can be folded into `PassDebugEffectRunner` in PR 12.

Shape:

```rust
struct PassDebugEffectRunner;

impl PassDebugEffectRunner {
    fn run(
        effect: PassDebugEffect,
        completions: &mut Vec<PassDebugEvent>,
        public_actions: &mut Vec<PassDebugWindowAction>,
        callback_result: Option<&mut PassDebugPatchApplyResultBuilder>,
    ) {
        // synchronous for first pass
    }
}
```

Responsibilities:

- Convert app-facing patch effects into `PassDebugWindowAction`.
- Run local reference reads/writes via `file_io.rs`.
- Queue completion events back into the store.
- Convert text artifact effects into `PassDebugWindowAction::UpsertDebugArtifact`.
- Keep binary artifact and diff-capture effects on callback result paths until the app command
  boundary is redesigned.
- Own `rfd` file dialog calls for reference folder picking.
- Own viewport focus/close commands once viewport extraction happens.

Non-responsibilities:

- It does not own durable state.
- It does not decide whether a file is dirty.
- It does not compute patches.
- It does not import `App`.

Ordering rule:

1. Dispatch user/app event into store.
2. Drain effects.
3. Run effects synchronously.
4. Dispatch completion events generated by the effect runner.
5. Drain any follow-up effects.
6. Return public actions/results.

Use a bounded loop or explicit queue drain to avoid accidental infinite effect/event cycles.

## PR Review Checklist

Every refactor PR should answer these questions in its description:

- What behavior owner moved?
- What public boundary stayed unchanged?
- What old wrappers remain?
- What tests prove behavior was preserved?
- Did any state become duplicated temporarily?
- If yes, what assertion or follow-up removes the duplicate?
- Did any reducer/state transition import `egui`, `rfd`, or `std::fs`?
- Did any dirty flag start clearing before acknowledgement/completion?

## Rollback Strategy

Each PR should be independently revertible.

Safe rollback points:

- PR 1-3 are pure utility extractions and should not require data migration.
- PR 4-7 may leave compatibility wrappers; revert the PR rather than trying to partially restore
  old state fields.
- PR 8-12 change ownership semantics; only merge them after focused tests cover pending/app-callback
  behavior.
- PR 14 render extraction should be deferred until domain reducers are stable because visual regressions
  are harder to isolate.

## Done Definition

The refactor is complete when:

- `pass_debug_window.rs` is a facade or gone.
- Each durable state owner lives in its target module.
- Render functions emit events and consume selectors/view models.
- Patch, merge, artifact, and file I/O primitives are independently testable.
- Local filesystem work and file dialogs run only through effects.
- App-facing public behavior remains compatible with the pre-refactor boundary.
- Full render-server tests pass.
