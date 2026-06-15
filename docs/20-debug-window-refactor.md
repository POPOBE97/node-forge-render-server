# Pass Debug Window Refactor

## Status

Draft. This document now contains the top-level architecture plus focused deep dives for several
modules. The current pass tightens the `Reference Workspace` design because it is the boundary where
artifact restore, local filesystem access, editor state, and reference-side Shortwire still overlap.

`src/ui/pass_debug_window.rs` is currently about 10k lines and mixes state transitions, egui
rendering, patch/diff logic, debug artifact persistence, local file I/O, viewport lifecycle, and
tests. The refactor should first make the ownership model explicit. Module-level details should be
filled in only after each area is studied in isolation.

## Table Of Contents

1. Goals
2. Non-Goals
3. Current Shape
4. Target Architecture
5. Single Source Of Truth
6. Event And Effect Model
7. Proposed Module Layout
8. Public Integration Boundary
9. Migration Phases
10. Validation Strategy
11. Module Deep-Dive Backlog
12. Open Questions

## Goals

- Reduce `pass_debug_window.rs` from a mixed implementation file into a small public integration
  facade.
- Establish one canonical state owner for each open pass debug window.
- Make UI rendering a consumer of state and producer of typed events, not a place where domain
  rules are implemented.
- Move patch, diff, merge, reference workspace, file sync, and artifact serialization behind clear
  modules with testable APIs.
- Preserve existing app-facing behavior and the current `PassDebugWindowAction` boundary while the
  internals are being split.
- Keep each migration step small enough to verify with the current tests.

## Non-Goals

- Do not redesign the renderer, app command system, or shader override runtime in this refactor.
- Do not change Shortwire behavior as part of extraction unless a bug is found and documented.
- Do not introduce async file sync or background workers in the first pass.
- Do not split into tiny files by UI widget alone. Split by state ownership and behavior boundary.
- Do not duplicate state to make module extraction easier.

## Current Shape

The current file has five responsibilities living in one mutable object graph:

- Window lifecycle:
  `PassDebugWindowMap`, `PassDebugWindowState`, viewport IDs, close-request filtering, focus
  requests, and pending app actions.
- Domain document state:
  canonical shader source, loaded patch source, editor draft, dirty flags, dependency tree focus,
  expansion state, merge conflict state, Shortwire state, reference workspace state, and render
  caches.
- Domain algorithms:
  hunk computation/application, three-way merge, dependency tree flattening, row identity, source
  range lookup, and reference workspace artifact migration.
- Side effects:
  app actions, debug artifact upserts, local file reads/writes, file picker invocation, viewport
  commands, metric logging, and image diff capture coordination.
- egui rendering:
  titlebar, dependency tree, split panes, code editor, reference editor, diff editor, line gutter,
  merge resolver, and paint helpers.

The main structural problem is not just file size. The problem is that state changes can happen
from rendering functions, document methods, and outer window functions, so the single source of
truth is implicit rather than enforced.

## Target Architecture

Use a store/reducer/effect architecture inside the pass debug feature.

```text
app/frame
  -> pass_debug_window facade
     -> WindowRegistry
        -> PassDebugStore
           -> reducer(PassDebugEvent) -> PassDebugEffects
           -> selectors / view models
        -> effect runner
        -> egui views
```

The design pattern is deliberately simple:

- **Store** owns all durable state for one pass debug window.
- **Reducer** is the only place that mutates durable state.
- **Events** describe user intent, app/runtime callbacks, and timer ticks.
- **Effects** describe work outside the store: apply shader patch, reset patch, upsert artifact,
  read/write reference files, open file dialog, request viewport close, or request diff capture.
- **Selectors** derive view models from store state.
- **Views** render view models and emit events.

This keeps egui immediate-mode rendering, but removes immediate-mode business logic from the UI
functions.

## Single Source Of Truth

There should be one authoritative object per open window:

```rust
struct PassDebugStore {
    identity: PassDebugIdentity,
    shader: ShaderDocumentState,
    dependencies: DependencyTreeState,
    shortwire: ShortwireState,
    reference: ReferenceWorkspaceState,
    merge: MergeState,
    ui: PassDebugUiState,
    caches: PassDebugRenderCaches,
    outbox: Vec<PassDebugEffect>,
}
```

Ownership rules:

- `shader` owns canonical generated WGSL, loaded runtime editor WGSL, draft WGSL, source revision,
  dirty/clean editor state, patch-active runtime state, and shader-facing status/error state.
- `dependencies` owns canonical analysis snapshots, flattened rows, root/focus/expansion/filter
  state, and navigation intents. Dependency rows are derived from canonical generated WGSL, not from
  the draft editor text and not from an applied patch override.
- `shortwire` owns active Shortwire session, stored node patches, diff capture state, and patch
  artifact dirtiness.
- `reference` owns reference files, selected file, reference editor draft, root path, sync state,
  reference Shortwire patches, and local restore bookkeeping.
- `merge` owns only merge-conflict state and pending merge patch rebases.
- `ui` owns viewport/split/editor-only UI state that is not domain state.
- `caches` owns galley/tree-width/visible-row caches and can be dropped without changing behavior.
- `outbox` is transient. It is drained after dispatch and must never be treated as persisted state
  or as a second owner of domain facts.

No rendering function should directly mutate these fields. Rendering functions may only emit
`PassDebugEvent`.

## Event And Effect Model

Events are internal to the pass debug feature:

```rust
enum PassDebugEvent {
    AppSourceChanged(AppSourceSnapshot),
    AppPatchApplied(PatchApplied),
    AppPatchReset(PatchReset),
    AppPatchFailed(String),
    DebugArtifactsChanged(DebugArtifactSnapshot),
    Tick { now_secs: f64 },
    CloseRequested,
    SaveRequested,
    DependencyRowClicked(RowClick),
    DependencyRowContextAction(RowAction),
    DependencyFilterEdited(String),
    ShaderDraftEdited(String),
    ShaderCursorFocused { char_index: usize },
    ReferenceDraftEdited(String),
    ReferenceFileSelected(String),
    ReferenceOpenFolderRequested,
    ReferenceFolderPicked(PathBuf),
    ReferenceReloadRequested,
    ReferenceFolderReadCompleted(ReferenceFolderReadResult),
    ReferenceLocalFilesReadCompleted(ReferenceLocalFilesReadResult),
    ReferenceSyncCompleted(ReferenceSyncResult),
    MergeAction(MergeAction),
    ShortwireAction(ShortwireAction),
    DiffCaptureResult(DiffCaptureResult),
}
```

Effects are the only bridge out:

```rust
enum PassDebugEffect {
    ApplyPatch {
        pass_name: String,
        source: String,
        reference_image: Option<ShortwireReferenceImage>,
    },
    ResetPatch { pass_name: String },
    ResetAllPatches,
    UpsertTextArtifact { item: DebugArtifactItem, content_text: String },
    UpsertBinaryArtifact { item: DebugArtifactItem, bytes: Vec<u8> },
    ReadReferenceManifestFiles { root: PathBuf, manifest: ReferenceWorkspaceManifest },
    ReadReferenceFolder { path: PathBuf },
    WriteReferenceFiles { root: PathBuf, files: Vec<ReferenceFileWrite> },
    WriteReferenceShortwireFile { path: PathBuf, content: String },
    RestoreReferenceShortwireFile { path: PathBuf, content: String },
    PickReferenceFolder,
    RequestDiffCapture(ShortwireDiffCaptureRequest),
    CloseViewport,
    FocusViewport,
}
```

The existing public `PassDebugWindowAction` can remain as the app-facing subset of effects during
migration. Internally, use the richer `PassDebugEffect` so file I/O and viewport actions are no
longer hidden inside render functions.

Important boundary correction from code reading: reference workspace artifact restore can
currently perform local filesystem reads when a manifest has `rootPath`, and reference sync can
currently write local files while collecting debug artifacts. In the target architecture, the
reducer must only decide what should be read or written. The effect runner performs filesystem work
and feeds the result back through completion events.

## Proposed Module Layout

Target directory:

```text
src/ui/pass_debug/
  mod.rs                    # public facade, preserves existing function names during migration
  registry.rs               # PassDebugWindowMap / window open/show/close orchestration
  store.rs                  # PassDebugStore and reducer dispatch
  event.rs                  # PassDebugEvent, PassDebugEffect, app-facing action conversion
  selectors.rs              # derived view models and status text
  shader_document.rs        # canonical/loaded/draft WGSL state and patch apply/reset transitions
  dependency_tree.rs        # row model, flattening, focus, expansion, filtering, row keys
  shortwire.rs              # Shortwire state machine and patch lifecycle
  patch.rs                  # hunk/diff/merge primitives, serde payloads
  merge.rs                  # canonical-change conflict state and pending patch rebase bookkeeping
  reference_workspace.rs    # reference files, selection, sync state, migration state machine
  artifacts.rs              # DebugArtifactItem naming, manifests, payload encode/decode
  file_io.rs                # local reference scan/read/write adapters used by effects
  viewport.rs               # viewport builder, close classification, focus/close effects
  render/
    mod.rs
    root.rs                 # split layout and top-level view composition
    titlebar.rs
    dependency_panel.rs
    code_editor.rs
    reference_editor.rs
    merge_dialog.rs
    tree_paint.rs
    text_layout.rs          # galley cache, line gutter, highlighting helpers
  tests/
    mod.rs                  # optional later; keep colocated tests at first if lower churn
```

Facade compatibility:

- Keep `src/ui/pass_debug_window.rs` temporarily as a re-export/wrapper module, or convert
  `src/ui/mod.rs` to `pub mod pass_debug;` and update call sites in one mechanical step.
- Preserve these current public entrypoints and types until the internal split is stable:
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

## Public Integration Boundary

The app should continue to see pass debug as a narrow feature boundary:

```text
AppCommand::OpenPassDebug
  -> pass_debug::open_pass_debug_window(...)

present frame
  -> pass_debug::show_pass_debug_windows(...)
  -> Vec<PassDebugWindowAction>
  -> AppCommand::{ApplyPassShaderPatch, ResetPassShaderPatch, UpsertDebugArtifact, ...}

shader patch apply completion
  -> pass_debug::mark_patch_applied
  -> PassDebugPatchApplyResult::{artifacts, binary_artifacts, diff_capture}
  -> app upserts returned artifacts and schedules pending Shortwire diff capture

shader patch reset completion
  -> pass_debug::mark_patch_reset / mark_all_patches_reset
  -> no returned artifacts today

shader rebuild failure
  -> pass_debug::record_patch_error / record_all_patch_error
  -> no returned artifacts today

explicit Shortwire diff capture request
  -> pass_debug::request_active_shortwire_diff_capture
  -> PassDebugPatchApplyResult::{artifacts, binary_artifacts, diff_capture}
  -> app upserts returned artifacts and schedules pending Shortwire diff capture

diff analysis completion
  -> pass_debug::record_shortwire_diff_result
  -> Vec<(DebugArtifactItem, String)> for patch artifact update
```

Do not pull `App` into the pass debug modules. The feature should receive snapshots and return
effects/actions.

Important boundary correction from code reading: not every pass debug output currently travels through
the per-frame `Vec<PassDebugWindowAction>`. Patch apply completion and explicit diff-capture requests
return `PassDebugPatchApplyResult`, because they may need to persist patch artifacts immediately,
upload a pasted reference image as a binary artifact, and schedule renderer-side diff analysis in the
same app command. Reset and error callbacks do not currently return `PassDebugPatchApplyResult`; the
refactor should not widen those public signatures unless the app command layer is deliberately
redesigned.

## Migration Phases

### Phase 0: Freeze Behavior

Before moving code, add or preserve tests for current invariants:

- dirty shader draft is not overwritten by source refresh
- dependency tree tracks canonical source, not patched draft
- active Shortwire source refresh marks base stale without clobbering draft
- canonical changes rebase patch or enter merge conflict
- reference workspace local files write and restore correctly
- debug artifact payloads round-trip
- viewport transient close requests are filtered

Exit criteria: current tests pass before extraction starts.

### Phase 1: Extract Pure Patch Primitives

Move hunk, diff, apply, and merge primitives into `patch.rs`.

Exit criteria: no egui, file I/O, or window state types are imported by patch primitives.

### Phase 2: Extract Artifact And Reference File Utilities

Move artifact naming/serialization to `artifacts.rs` and local reference file operations to
`file_io.rs`.

Exit criteria: artifact IDs/slot keys have one owner, file reads/writes are callable through a
small adapter, and no newly extracted state transition performs filesystem I/O directly.

### Phase 3: Split Domain State From UI Caches

Introduce `PassDebugStore` while still rendering through existing functions.

Exit criteria: durable state and disposable render caches are separate fields with explicit
ownership.

### Phase 4: Introduce Reducer Events

Replace direct document method calls from views with `PassDebugEvent` dispatch.

Exit criteria: views no longer call patch/apply/reference mutation methods directly.

### Phase 5: Move Views Into `render/`

Move egui rendering by panel, using selectors/view models instead of raw store mutation.

Exit criteria: each render module emits events and uses derived view models for status/enablement.

### Phase 6: Retire `pass_debug_window.rs`

Replace the original file with a facade or remove it after call sites move to `ui::pass_debug`.

Exit criteria: no 10k-line mixed module remains; public behavior is unchanged.

## Validation Strategy

Use the existing tests as the first safety net, then move them next to the owning modules as code
moves.

Validation levels:

- Pure unit tests for `patch.rs`, `dependency_tree.rs`, `artifacts.rs`, and `file_io.rs`.
- Reducer tests for store transitions and emitted effects.
- Integration tests through the public facade for app-facing behavior.
- Manual egui smoke test only after view extraction, because the first phases should not change UI.

## Module Deep-Dive Backlog

These sections should be filled one at a time after focused code reading.

Deep-dive rule:

- Study one module boundary per pass.
- Add invariants, state ownership, events, effects, tests, and extraction order for that module only.
- Do not start implementation from this document until the current module section has concrete exit
  criteria.

### Shader Document State

Current code facts:

- `PassDebugWindowDocument` stores the shader text lifecycle in these sibling fields:
  `source`, `analysis_source`, `analysis_source_text`, `source_revision`, `draft_source`,
  `loaded_source`, `dirty`, `patch_active`, `generated_base_source`,
  `generated_base_source_hash`, `last_error`, and `last_status`.
- `open_pass_debug_window` and `show_pass_debug_windows` call `update_source` every frame with the
  current `pass_sources_revision` and any `pass_shader_overrides` entry. The renderer has a tested
  invariant that shader overrides do not replace `pass_debug_sources`; debug sources remain
  canonical even while runtime rendering uses an override.
- `loaded_source` means the last runtime-acknowledged editor source. When `patch_active` is false it
  matches canonical generated WGSL; when `patch_active` is true it is the override source currently
  accepted by the app.
- `draft_source` is the editor buffer. `dirty` is currently stored, but behaviorally it is
  `draft_source != loaded_source`.
- `generated_base_source` is the canonical base used by Shortwire and merge logic. It must track
  canonical source refreshes even when a patch override is active.
- `analysis_source` / `analysis_source_text` are canonical analysis inputs for the dependency tree.
  Existing tests assert that draft edits and applied patch overrides do not replace the dependency
  tree source.
- `draft_analysis_due_secs` is effectively disabled today: draft edits clear it, the refresh helper
  clears it, and forced draft analysis still keeps canonical dependency source. The refactor should
  not accidentally reintroduce draft-based dependency analysis.
- `update_source_inner` has one special same-revision path: when the document is clean, Shortwire is
  inactive, and a patch override appears or changes, it updates `loaded_source` and `draft_source`
  even if `source_revision` did not change.
- Source refresh while Shortwire is active updates canonical analysis state and marks the active
  Shortwire base stale when canonical text changes, but it must not overwrite the draft.
- A canonical source change while a patch override is active attempts a three-way merge from
  `generated_base_source` + incoming canonical + local override. It emits app patch/reset actions on
  clean auto-merge, or creates `PassDebugMergeConflict` for manual resolution.
- `ApplyPatch` / `ResetPatch` actions are app-facing effects. The document only becomes
  `patch_active` or clean-reset after the app rebuild succeeds and calls `mark_patch_applied` or
  `mark_patch_reset`.

Target state shape:

```rust
struct ShaderDocumentState {
    canonical: CanonicalShaderSnapshot,
    runtime: RuntimeShaderState,
    draft: ShaderDraftState,
    status: ShaderStatusState,
}

struct CanonicalShaderSnapshot {
    source: Option<PassDebugSource>,
    source_text: String,
    source_hash: u64,
    source_revision: Option<u64>,
}

enum RuntimeShaderState {
    Loaded(RuntimeLoadedShaderState),
    PendingApply {
        previous: RuntimeLoadedShaderState,
        requested_source: String,
    },
    PendingReset {
        previous: RuntimeLoadedShaderState,
    },
}

enum RuntimeLoadedShaderState {
    NoSource,
    CanonicalLoaded,
    PatchLoaded { loaded_source: String },
}

struct ShaderDraftState {
    source: String,
    revision: u64,
}

struct ShaderStatusState {
    error: Option<String>,
    status: Option<String>,
}
```

Selectors should expose the current conveniences:

```rust
impl ShaderDocumentState {
    fn loaded_source(&self) -> &str;
    fn draft_source(&self) -> &str;
    fn is_dirty(&self) -> bool;
    fn patch_active(&self) -> bool;
    fn canonical_dependency_snapshot(&self) -> Option<&PassDebugSource>;
}
```

`is_dirty` should be derived from `draft.source != loaded_source()`. If the migration keeps a stored
`dirty` flag temporarily, all writes must go through one helper and tests should assert it matches
the derived value.

State ownership rules:

- Canonical WGSL has one owner: `ShaderDocumentState::canonical`. `DependencyTreeState` may store a
  cloned or reduced analysis snapshot, but it is updated only from canonical source-change events.
- `RuntimeShaderState` models what the app has acknowledged, not what the user requested. Emitting
  `ApplyPatch` must enter a pending state; it must not set `Loaded(PatchLoaded)` until
  `AppPatchApplied`. Pending states must carry the previous loaded state so failed app rebuilds can
  roll back without guessing.
- `draft.source` is the only editable WGSL buffer for the main editor. Views can display it and emit
  `ShaderDraftEdited`, but they cannot mutate it directly once reducer events are introduced.
- `PatchLoaded` identity is the presence of a runtime override, not text inequality. A patch source
  that happens to equal canonical still requires a reset effect to remove the override.
- Dependency-tree analysis must never use `draft.source` as input. Editor-click focus can map a
  cursor in draft text to canonical ranges only as a best-effort selector; it cannot refresh rows
  from draft text.
- Shortwire owns active edit session state, but shader owns canonical base refresh. When canonical
  changes during Shortwire, shader emits a `CanonicalBaseChanged` fact and Shortwire decides whether
  to mark `base_source_stale` or rebase on save.
- Merge conflict state belongs to `merge.rs`. Shader provides base/incoming/local sources and applies
  the result after `merge` emits an apply/reset effect.
- `ShaderDocumentState` should not import `egui`, `rfd`, `std::fs`, debug artifact storage, or
  reference workspace types.

Event and effect contract:

```rust
enum ShaderDocumentEvent {
    AppSourceChanged {
        source: Option<PassDebugSource>,
        source_revision: u64,
        patch_source: Option<String>,
    },
    DraftEdited { source: String },
    ApplyRequested { source: String, reason: ShaderApplyReason },
    ResetRequested { reason: ShaderResetReason },
    AppPatchApplied {
        source: Option<PassDebugSource>,
        source_revision: u64,
        applied_source: String,
        status: String,
    },
    AppPatchReset {
        source: Option<PassDebugSource>,
        source_revision: u64,
        status: String,
    },
    AppPatchFailed(String),
}

enum ShaderDocumentEffect {
    ApplyPatch { pass_name: String, source: String, reference_image: Option<ShortwireReferenceImage> },
    ResetPatch { pass_name: String },
    CanonicalAnalysisChanged { source: Option<PassDebugSource>, source_text: String },
    CanonicalBaseChanged { previous_hash: u64, next_hash: u64 },
    MergeCanonicalPatchChange { base: String, incoming: String, local: String },
}
```

These names are internal sketches. The required contract is that app-facing patch effects are
acknowledged later through completion events, while canonical analysis changes are delivered to the
dependency reducer synchronously inside the pass debug store.

Behavioral transitions to preserve:

- New clean window: canonical source initializes `canonical`, `runtime = Loaded(CanonicalLoaded)`,
  `draft.source = canonical.source_text`, and dependencies are built from canonical source.
- New window with an existing override: canonical source initializes `canonical`,
  `runtime = Loaded(PatchLoaded { loaded_source: patch_source })`, and
  `draft.source = patch_source`, while dependencies are still built from canonical source.
- Same `source_revision`: do not refresh canonical analysis. The only allowed edit is accepting a
  changed `patch_source` into a clean, inactive document so runtime override state stays in sync.
- Dirty draft + source refresh: update canonical snapshot and dependency analysis, but preserve
  `draft.source` and dirty state.
- Clean draft + source refresh without patch: replace loaded and draft source with canonical.
- Active Shortwire + source refresh: update canonical snapshot and dependency analysis, emit
  `CanonicalBaseChanged` when the canonical hash changes, and do not replace the draft.
- Patch active + canonical text changed + no active Shortwire: emit `MergeCanonicalPatchChange`.
  If the merge auto-resolves to incoming canonical, emit `ResetPatch`; if it resolves to a non-empty
  local patch, emit `ApplyPatch`; if it conflicts, leave runtime state as the old patch and let
  `MergeState` own the resolver.
- `AppPatchApplied`: set `runtime = Loaded(PatchLoaded)`, set `draft.source = applied_source`,
  clear dirty state, update canonical snapshot from the callback, clear shader error, and let
  Shortwire/merge reducers commit any pending patch-store updates.
- `AppPatchReset`: set `runtime = Loaded(CanonicalLoaded)` or `Loaded(NoSource)`, set
  `draft.source` to canonical text or empty text, clear dirty state, update canonical snapshot, and
  let Shortwire finish `PendingResetThenEnter` if needed.
- `AppPatchFailed`: keep the requested draft intact. During Shortwire pending apply, return the
  active session to editing and recompute dirty from draft vs loaded source. During pending reset,
  clear the pending Shortwire entry. Other failures only update shader error/status.
- Source disappears while clean: clear loaded/draft/canonical/dependencies. Source disappears while
  dirty: preserve draft text, clear canonical/dependencies, and keep dirty true.

Extraction order:

1. Add `shader_document.rs` with `CanonicalShaderSnapshot`, `RuntimeShaderState`,
   `ShaderDraftState`, status types, and pure selectors. Keep `PassDebugWindowDocument` fields in
   place and populate the new state from them first to avoid a wide rewrite.
2. Move `replace_draft_source`, `mark_draft_edited`, same-revision refresh handling, and clean/dirty
   `AppSourceChanged` behavior behind `ShaderDocumentState` methods. Existing render code may still
   call through document wrappers.
3. Introduce a transition result that returns `ShaderDocumentEffect` values instead of pushing
   `PassDebugWindowAction` directly. During migration, the document wrapper converts
   `ApplyPatch`/`ResetPatch` effects back into the existing pending action queue.
4. Move `mark_applied`, `mark_reset`, and normal `record_error` handling into shader reducer
   completion events. Leave Shortwire-specific completion hooks as callbacks until the Shortwire
   module is extracted, but make the ordering explicit: shader updates runtime state first, then
   Shortwire/merge commit their pending stores.
5. Move canonical-analysis refresh into a store-level handoff:
   `ShaderDocumentEffect::CanonicalAnalysisChanged` feeds `DependencyTreeState`. Remove direct calls
   from shader methods to dependency row rebuild helpers.
6. Move canonical-change auto-merge invocation to `merge.rs`. Shader detects the condition and
   provides base/incoming/local sources; merge produces `ApplyPatch`, `ResetPatch`, or conflict
   state.
7. Delete temporary duplicated `dirty`, `patch_active`, `analysis_source`, and
   `generated_base_source` fields from `PassDebugWindowDocument` once call sites use the new state.

Focused validation:

- Preserve existing tests covering:
  - dirty draft is not replaced by source refresh
  - clean document tracks source refresh and existing patch override
  - same source revision does not refresh canonical source
  - patch source updates editor while dependency tree stays canonical
  - canonical source refresh updates dependency tree while patch exists
  - generated base source tracks canonical when patch is active
  - source refresh during active Shortwire does not overwrite draft and marks base stale
  - app patch apply/reset callbacks update `patch_active`, `loaded_source`, and `draft_source`
  - record error during pending apply/reset preserves the current Shortwire semantics
- Add reducer tests before view extraction:
  - `ApplyPatch` effect leaves `runtime` pending and does not report `patch_active` until
    `AppPatchApplied`
  - failed apply returns from pending to the previous loaded runtime state and keeps draft dirty
  - patch override text equal to canonical still reports `patch_active`
  - source disappearance preserves a dirty draft but clears canonical dependency analysis
  - `DraftEdited` increments draft revision/cache invalidation data without emitting dependency
    analysis refresh
  - canonical-change auto-merge emits effects but does not mutate loaded runtime state before the app
    completion callback

Exit criteria:

- `shader_document.rs` imports no `egui`, `rfd`, `std::fs`, reference workspace, or render-cache
  types.
- `dependency_tree.rs` rebuilds only from `CanonicalAnalysisChanged` or equivalent canonical
  snapshots.
- Applying or resetting a patch is represented as pending until app callbacks arrive.
- Dirty and patch-active semantics are available through selectors with no uncontrolled duplicate
  state.
- All current pass debug tests pass after each extraction step.

### Dependency Tree State

Current code facts:

- `PassDebugWindowDocument` stores dependency-tree state in sibling fields:
  `analysis_source`, `analysis_source_text`, `dependency_rows`, `focused_target_id`,
  `focused_dependency_row_key`, `dependency_root_target_id`, `dependency_expanded_row_keys`,
  `pending_editor_jump`, `pending_dependency_reveal_row_key`, `filter_text`,
  `dependency_rows_generation`, `dependency_expansion_generation`,
  `dependency_expandable_row_keys_cache`, `visible_dependency_row_indices_cache`, and
  `dependency_tree_width_cache`.
- `analysis_source` is the canonical `PassDebugSource` snapshot used by the tree. Existing behavior
  deliberately ignores draft WGSL and runtime patch overrides for dependency analysis. `draft_*`
  analysis hooks are currently disabled and must not be revived accidentally during extraction.
- `refresh_analysis_rows` runs after canonical source changes, app apply/reset callbacks, Shortwire
  exits that replace runtime state, and source disappearance. It calls `ensure_navigation_targets`
  before `refresh_dependency_rows`.
- Root selection is canonical-source driven. `ensure_navigation_targets` prefers
  `source.dependency_root_target_id` when that target still exists, then falls back to the first
  dependency target. Focusing a child target does not change the root tree.
- `dependency_rows` are a flattened view of `source.dependency_trees[root_target_id]`. Row identity
  is path based (`"0/0/1"`), not target based, because the same target can appear multiple times in
  different call-site paths and those rows must remain independently selectable.
- `push_dependency_rows` hides non-target intermediate nodes, records their relation labels into
  `relation_path`, follows `reference` nodes into the referenced target tree, and uses a
  `reference_stack` to avoid recursive reference loops.
- Row `source_range` means the occurrence/call-site range for row clicks. `source_jump_range` means
  the definition or reaching-definition jump target, and is shown only when it differs from the row
  occurrence range.
- Focus state has two identities: `focused_target_id` for semantic target focus and
  `focused_dependency_row_key` for the exact visible occurrence. The row key wins for painting and
  focused source range when present.
- Editor-click focus maps a draft editor char index to canonical row ranges as a best effort:
  first the smallest/deepest matching dependency row range, then a target source range, then an
  identifier-name match. This mapping uses `draft_source` byte indices today, but the ranges come
  from canonical analysis.
- Expansion state is durable: the root row is always expanded; editor-originated focus collapses to
  the shortest path from root to the focused row; tree-originated clicks focus without queuing reveal
  scroll.
- Filtering is currently implemented in `render_dependency_rows`, not in domain code. It keeps rows
  whose labels match the lowercase filter plus their ancestors. It does not change expansion state.
- Cached visible row indices are invalidated by row-generation or expansion-generation changes.
  Expandable-row and tree-width caches are invalidated by row-generation changes only. The width
  cache depends on `egui` text measurement and belongs with render caches, not durable dependency
  state.
- Shortwire tree affordances are derived from dependency rows plus `ShortwireState`: right-clicking
  a row enters Shortwire or applies a stored Shortwire patch; clicking another row while Shortwire is
  active exits current Shortwire first.

Target state shape:

```rust
struct DependencyTreeState {
    canonical: DependencyAnalysisSnapshot,
    rows: Vec<DependencyRow>,
    root_target_id: Option<String>,
    focus: DependencyFocusState,
    expansion: DependencyExpansionState,
    filter: DependencyFilterState,
    navigation: DependencyNavigationState,
    generation: DependencyGenerationState,
}

struct DependencyAnalysisSnapshot {
    source: Option<PassDebugSource>,
    source_text: String,
}

struct DependencyRow {
    depth: usize,
    row_key: String,
    parent_row_key: Option<String>,
    label: String,
    relation_path: String,
    target_id: Option<String>,
    occurrence_range: Option<PassDebugSourceRange>,
    source_jump_range: Option<PassDebugSourceRange>,
    selectable: bool,
}

struct DependencyFocusState {
    target_id: Option<String>,
    row_key: Option<String>,
}

struct DependencyExpansionState {
    expanded_row_keys: HashSet<String>,
}

struct DependencyFilterState {
    text: String,
}

struct DependencyNavigationState {
    pending_editor_jump: Option<PassDebugSourceRange>,
    pending_reveal_row_key: Option<String>,
}

struct DependencyGenerationState {
    rows_generation: u64,
    expansion_generation: u64,
}
```

`DependencyTreeState` should own canonical analysis snapshots only until `ShaderDocumentState`
becomes the single canonical owner. After `ShaderDocumentState::canonical` is fully introduced,
`DependencyAnalysisSnapshot` can be reduced to dependency-specific analysis data or receive cloned
canonical snapshots through `CanonicalAnalysisChanged`.

Render-only cache shape:

```rust
struct DependencyTreeRenderCaches {
    expandable_row_keys: Option<ExpandableRowsCache>,
    visible_row_indices: Option<VisibleRowsCache>,
    intrinsic_width: Option<TreeWidthCache>,
}
```

Selectors should expose the current conveniences without giving views mutable access to state:

```rust
impl DependencyTreeState {
    fn visible_row_indices(&self, caches: &mut DependencyTreeRenderCaches) -> &[usize];
    fn filtered_visible_row_indices(&self, visible_indices: &[usize]) -> Vec<usize>;
    fn focus_path_row_keys(&self) -> Vec<String>;
    fn focused_source_range(&self) -> Option<PassDebugSourceRange>;
    fn focus_is_in_root(&self) -> bool;
    fn row_for_key(&self, row_key: &str) -> Option<&DependencyRow>;
}
```

State ownership rules:

- Dependency rows are derived from canonical generated WGSL only. `ShaderDraftEdited`,
  `ApplyPatch`, and runtime override callbacks must not rebuild rows unless they also carry a new
  canonical source snapshot.
- `root_target_id` is owned by dependencies and changes only when the canonical root target
  disappears or when a future explicit root-selection event is added. Ordinary focus changes must
  not replace the root tree.
- Row identity is the dependency path from the root tree. Never key row selection, expansion, or
  Shortwire row identity by `target_id` alone.
- `focused_target_id` may point outside the current root map. In that case `focused_row_key` is
  `None`, `focus_is_in_root` is false, and the UI shows the existing warning instead of silently
  changing roots.
- `pending_editor_jump` and `pending_reveal_row_key` are one-shot navigation intents owned by the
  dependency module. Views may consume them through selectors/events, but should not clear arbitrary
  dependency fields.
- Filtering is dependency UI state, not render cache state. It affects the selected visible row set
  but does not mutate expansion. Matching should include ancestors so filtered children remain
  understandable in context.
- Expandable rows, visible row indices, and intrinsic width are caches. They may be recomputed from
  `rows`, `expansion`, and render metrics at any time. They must not be serialized or used as facts
  by other reducers.
- Shortwire may read dependency row identities and selected row metadata, but it should not mutate
  dependency state directly. Right-click and row navigation should be represented as dependency or
  Shortwire events coordinated by the store.
- Source-range lookup should remain byte-range based because `PassDebugSourceRange` is byte based.
  Any selector that receives an editor char index must convert through the same UTF-8 helpers before
  comparing ranges.

Event and effect contract:

```rust
enum DependencyEvent {
    CanonicalAnalysisChanged {
        source: Option<PassDebugSource>,
        source_text: String,
    },
    RowClicked {
        row_key: Option<String>,
        target_id: Option<String>,
        source_range: Option<PassDebugSourceRange>,
        toggle_row_key: Option<String>,
    },
    EditorCursorFocused { char_index: usize },
    FilterEdited(String),
    RevealConsumed { row_key: String },
    EditorJumpConsumed,
}

enum DependencyEffect {
    FocusedRowChanged { row_key: Option<String>, target_id: Option<String> },
    EditorJumpRequested(PassDebugSourceRange),
    DependencyRevealRequested(String),
}
```

These effects are store-internal navigation effects. They should usually be folded into
`DependencyNavigationState` rather than surfaced as app-facing `PassDebugWindowAction` values. The
important contract is that render functions emit `DependencyEvent` values instead of calling
`focus_tree_click`, `toggle_dependency_row_expanded`, or clearing pending jumps directly.

Behavioral transitions to preserve:

- New canonical source: choose the canonical root target if valid, otherwise the first target;
  flatten rows from that root; ensure root row `"0"` is expanded; choose a fallback focus only if the
  existing focus target is missing.
- Source disappears: clear canonical analysis, rows, root, focus, expansion, pending editor jump,
  pending reveal, and row caches.
- Canonical source refresh with same root: rebuild rows, prune expansion to existing expandable
  row keys, keep a valid focused row when possible, and recompute the shortest row for the focused
  target if the previous row key disappeared.
- Canonical source refresh with changed root: replace `root_target_id`, reset expansion to root,
  and then repair focus as above.
- Draft edit: do not rebuild dependency rows, do not schedule draft dependency analysis, and do not
  clear existing dependency focus solely because the draft changed.
- Tree row click: toggle expansion when `toggle_row_key` is present. Otherwise focus the exact row
  key, set `focused_target_id` from that row when present, and queue `pending_editor_jump` from the
  row occurrence range unless the click supplied a `source_jump_range` override.
- Editor cursor focus: prefer the deepest and smallest matching row occurrence range, then fall back
  to target source ranges, then identifier-name matching. When a target maps to a row, reveal and
  collapse to the shortest path from root.
- Source jump button: focus remains on the clicked row, but the queued editor jump uses
  `source_jump_range` rather than row occurrence range.
- Filter edit: update filter text only. Visible rows for the view are derived from current visible
  rows plus label matches and ancestors; expansion state is unchanged.
- Focusing a target outside the current root map: keep the current `root_target_id`, set
  `focused_target_id`, clear `focused_row_key`, and report the "Focus is outside the current
  dependency map" view state.

Extraction order:

1. Move `PassDebugDependencyRow`, `PassDebugTreeClick`, flattening helpers, row-label helpers,
   source-range lookup, identifier lookup, and row-path helpers to `dependency_tree.rs`.
2. Introduce `DependencyTreeState` inside `PassDebugWindowDocument` while keeping wrapper methods
   with the old names. This keeps the first change mechanical and preserves existing tests.
3. Move `refresh_analysis_rows`, `ensure_navigation_targets`, `refresh_dependency_rows`, focus,
   expansion, reveal, and visible-row-index logic behind dependency methods that return typed
   navigation effects or mutate only `DependencyTreeState`.
4. Split caches into `PassDebugRenderCaches` / `DependencyTreeRenderCaches`. Keep
   `cached_dependency_tree_intrinsic_width` in render code because it depends on `egui::Ui` and font
   measurement.
5. Move filter matching out of `render_dependency_rows` into a selector so render code passes
   `DependencyFilterEdited` and receives row indices/view models.
6. Replace direct render mutations:
   - filter text edit emits `DependencyFilterEdited`
   - row/toggle/source-jump clicks emit `DependencyRowClicked`
   - code-editor cursor clicks emit `ShaderCursorFocused` or `DependencyEvent::EditorCursorFocused`
   - pending reveal/editor jump are consumed through explicit view-model fields
7. After `ShaderDocumentState` owns canonical snapshots, make the store dispatch
   `CanonicalAnalysisChanged` to dependencies whenever shader canonical analysis changes. Remove
   direct shader/document calls to dependency refresh helpers.

Focused validation:

- Preserve existing tests covering:
  - target list refresh after canonical source update
  - dirty draft and forced draft analysis do not replace canonical dependency source
  - render caches invalidate on expansion and source refresh
  - focusing a dependency child does not replace the root tree
  - root defaults to the fragment return target
  - only the root row is expanded by default
  - editor focus expands only the shortest path from root
  - tree click focuses without queuing reveal scroll
  - focusing a target outside the root map keeps the root and reports out-of-map focus
  - hidden intermediate relation nodes preserve relation path and parent linkage
  - duplicate target rows keep distinct row keys and occurrence ranges
  - source jump ranges jump to definitions or reaching definitions
  - editor click prefers the dependency access-path occurrence over the broader target range
- Add reducer/selector tests before view extraction:
  - `CanonicalAnalysisChanged(None)` clears rows, root, focus, expansion, and pending navigation
  - canonical refresh with a deleted focused row remaps by focused target when possible
  - filter selector includes matching rows and ancestors without modifying expansion
  - row-toggle events ignore non-expandable row keys
  - source-jump click preserves focused row identity while emitting the override editor jump
  - editor cursor focus uses UTF-8 byte conversion correctly for non-ASCII WGSL comments/identifiers
  - cached visible rows are recomputed from generations and never required for reducer correctness

Exit criteria:

- `dependency_tree.rs` imports no `egui`, `rfd`, `std::fs`, Shortwire patch payloads, reference
  workspace, merge state, viewport state, or debug artifact types.
- Dependency rows rebuild only from canonical analysis snapshots.
- Views can render dependency panels from selectors/view models and emit typed dependency events.
- Row selection, expansion, filter text, editor jump, and reveal semantics are covered by unit tests.
- All current pass debug tests pass after each extraction step.

### Shortwire State Machine

Current code facts:

- Shortwire state is split across sibling fields on `PassDebugWindowDocument`:
  `shortwire_patches`, `shortwire_patches_dirty`, `shortwire_active`,
  `shortwire_exit_on_apply`, `generated_base_source`, and `generated_base_source_hash`.
  Reference-side Shortwire state is currently stored under `reference_workspace` plus the sibling
  `reference_patches` store.
- `shortwire_patch_key(row)` is the durable shader patch identity. It is derived from target id,
  relation path, and source-range byte fingerprint. `row_key_hint` is only a display/current-tree
  hint used for active-row highlighting and must not be used as the stored patch key.
- `ShortwireNodePatch` stores `hunks`, `base_source_hash`, optional `reference_image`, and optional
  `diff_result`. Legacy patch artifacts may omit `referenceImage` and `diffResult`; restore must keep
  that backward compatibility.
- `ShortwireActiveState` has three phases today:
  `Editing`, `PendingApply { pending_hunks }`, and
  `PendingResetThenEnter { next_identity }`.
- Entering Shortwire while a runtime patch override is active first emits `ResetPatch`; the actual
  edit session starts only after `mark_reset` acknowledges the reset and `complete_shortwire_entry`
  rehydrates the session against the fresh canonical base.
- Entering Shortwire while no patch override is active starts editing immediately, snapshots
  `generated_base_source`, attempts to apply the stored patch, and calls
  `enter_reference_shortwire` so the reference editor follows the same row identity.
- `generated_base_source` tracks canonical debug WGSL, not runtime override WGSL. Current tests assert
  it is initialized even when a window opens with an existing patch override and that source refreshes
  while a patch exists still update the canonical base and dependency tree.
- Source refresh while Shortwire is active updates canonical analysis state and `generated_base_source`
  but does not replace the draft. If the canonical text changed, it marks
  `active.base_source_stale = true`; the user edit is rebased on Done.
- Shortwire Done computes hunks from current canonical base to draft. If the active base is stale, it
  first computes user intent from the entry base to draft and applies those hunks onto the latest
  canonical base. Rebase failure keeps the session in `Editing`.
- Main shader patch storage is acknowledged by the app. `exit_shortwire_done` only enters
  `PendingApply` and emits `ApplyPatch`; stored patch hunks move into `shortwire_patches` only after
  `mark_patch_applied`.
- Reference Shortwire patch storage is coupled to the left shader apply. Reference hunks are prepared
  before the left apply request, but committed to `reference_patches` only after the left
  `mark_patch_applied` succeeds. On apply error they remain pending and the editor returns to
  `Editing`.
- Closing/navigating away from an active Shortwire session is a save-without-left-apply path. It
  stores non-empty shader hunks, preserves matching existing `diff_result`, restores the editor to
  canonical generated WGSL, and emits a reset if a runtime patch is still active.
- Potential defect found during code reading: the current `PendingApply` navigate/close branch clears
  `shortwire_active` after setting `shortwire_exit_on_apply`, which can leave the later
  `mark_patch_applied` callback without the pending hunks it needs to commit. The target state should
  split "active UI session is closed" from "pending app acknowledgement still owns hunks".
- `enter_shortwire_and_apply` is the fast path for a row with a stored patch. It applies stored hunks
  to the current canonical base, enters `PendingApply`, emits `ApplyPatch`, and forwards any stored
  `reference_image` so the app can load it into the canvas before diff capture.
- Diff capture is a separate lifecycle from patch apply. `mark_patch_applied` can return
  `ShortwireDiffCaptureRequest`; render analysis later computes `ShortwireDiffResult`, calls
  `record_shortwire_diff_result`, and persists an updated patch artifact with the result.
- Pasted Shortwire reference images are binary debug artifacts. `request_active_shortwire_diff_capture`
  can create an image-only patch with empty hunks, upsert the patch artifact, return a binary artifact,
  clear any stale diff result, and queue diff capture.
- Patch artifacts are restored opportunistically from debug artifacts, but restore is skipped while a
  Shortwire session is active, while local patch artifacts are dirty, or while the shader draft is
  dirty.
- Tree UI affordances depend on Shortwire selectors: rows with stored patches show a status dot, active
  Shortwire dims other rows, active-row clicks are ignored, and clicking another row exits the current
  session before normal dependency focus handling.

Target state shape:

```rust
struct ShortwireState {
    patches: ShortwirePatchStore,
    active: Option<ShortwireSession>,
    pending_apply: Option<ShortwirePendingApply>,
    loaded_artifact_hash: Option<u64>,
}

struct ShortwirePatchStore {
    patches: HashMap<String, ShortwireNodePatch>,
    dirty: bool,
}

struct ShortwireSession {
    identity: ShortwireRowIdentity,
    base: ShortwireBaseSnapshot,
    base_stale: bool,
    reference_image: Option<ShortwireReferenceImage>,
    phase: ShortwirePhase,
    ui: ShortwireUiState,
}

struct ShortwireBaseSnapshot {
    source: String,
    source_hash: u64,
}

struct ShortwireRowIdentity {
    patch_key: String,
    row_key_hint: String,
    label: String,
    target_id: Option<String>,
}

enum ShortwirePhase {
    Editing,
    PendingApply,
    PendingResetThenEnter { next_identity: ShortwireRowIdentity },
}

struct ShortwirePendingApply {
    identity: ShortwireRowIdentity,
    pending_hunks: Vec<ShortwireHunk>,
    reference_image: Option<ShortwireReferenceImage>,
    exit_after_apply: bool,
}

struct ShortwireUiState {
    diff_view_enabled: bool,
}

struct ShortwireNodePatch {
    hunks: Vec<ShortwireHunk>,
    base_source_hash: u64,
    reference_image: Option<ShortwireReferenceImage>,
    diff_result: Option<ShortwireDiffResult>,
}
```

`generated_base_source` should move to `ShaderDocumentState::canonical` as part of the shader
extraction. Shortwire should hold only the entry snapshot for an active session and read the current
canonical base through store coordination.

Selectors should expose the current conveniences:

```rust
impl ShortwireState {
    fn active_row_key_hint(&self) -> Option<&str>;
    fn is_editor_interactive(&self) -> bool;
    fn can_enter(&self, canonical_base_available: bool, merge_conflict_open: bool) -> bool;
    fn patch_for_row(&self, row: &DependencyRow) -> Option<&ShortwireNodePatch>;
    fn dot_info_by_patch_key(&self) -> HashMap<String, ShortwireDotInfo>;
    fn active_diff_base<'a>(&'a self, canonical_base: &'a str) -> Option<&'a str>;
}
```

State ownership rules:

- Shortwire owns stored shader patches and the active shader-patch edit session. It does not own the
  canonical generated WGSL, runtime patch-loaded fact, dependency rows, reference files, merge
  conflicts, canvas reference texture, or debug artifact store.
- Stored patch identity is `ShortwireRowIdentity::patch_key`. `row_key_hint` is volatile and should be
  refreshed or tolerated as stale after dependency rows rebuild.
- `ShortwirePatchStore::dirty` means the in-memory patch artifact differs from the last persisted
  artifact. It must be cleared only when an artifact payload has been produced or an incoming artifact
  has been accepted.
- `pending_apply` owns hunks after `ApplyPatch` has been emitted and before the app acknowledgement
  arrives. It is separate from `active` so closing or hiding the active UI cannot lose the apply
  completion token.
- `ShortwireNodePatch::diff_result` is valid only for the exact stored `{ base_source_hash, hunks,
  reference_image }` tuple. Any patch edit, rebase, reference image replacement, or explicit capture
  request must clear it unless the code proves the tuple is unchanged.
- Apply/reset requests are app-facing effects. Shortwire must not commit pending hunks to the stored
  patch map until the app acknowledges with `AppPatchApplied`.
- Reference Shortwire is a coordinated child flow. The shader Shortwire reducer may emit handoff
  effects such as `ReferenceEnterShortwire`, `ReferencePrepareShortwireSave`, and
  `ReferenceCommitAfterLeftApply`, but it should not mutate reference workspace fields directly.
- Merge may ask Shortwire to rebase a stored patch after a matching app acknowledgement. The matching
  heuristic and patch-store rewrite belong to Shortwire, not merge.
- Diff capture request state belongs outside the pass debug store at the app shell today. Internally,
  Shortwire can emit `RequestDiffCapture`, but completion still arrives as a later
  `DiffCaptureResult`/`ShortwireDiffResultRecorded` event.
- Binary reference-image artifact construction belongs with Shortwire artifacts, but binary upload is
  an effect. Reducer code should produce metadata and bytes as an effect payload, not write to the app
  debug artifact store.
- Active Shortwire blocks ordinary save semantics, merge resolver actions, and file selection, but it
  should not block canonical dependency analysis refresh. The dependency tree may update from
  canonical source while the draft remains untouched.

Event and effect contract:

```rust
enum ShortwireEvent {
    ArtifactSnapshotChanged(Option<String>),
    EnterRequested { row: DependencyRow },
    EnterAndApplyRequested { row: DependencyRow },
    ResetBeforeEnterAcknowledged { canonical_base: String, canonical_hash: u64 },
    ResetBeforeEnterFailed(String),
    CanonicalBaseChanged { source: String, source_hash: u64 },
    DraftEdited,
    DiffViewToggled,
    SaveRequested { draft_source: String, canonical_base: String, canonical_hash: u64 },
    NavigateOrCloseRequested { draft_source: String, canonical_base: String, canonical_hash: u64 },
    CancelRequested,
    AppPatchApplied { canonical_base: String, canonical_hash: u64, applied_source: String },
    AppPatchReset { canonical_base: String, canonical_hash: u64 },
    AppPatchFailed(String),
    PastedReferenceCaptureRequested(Option<ShortwirePastedReferenceImage>),
    DiffCaptureCompleted { request: ShortwireDiffCaptureRequest, result: ShortwireDiffResult },
    MergePatchRebaseAcknowledged(PendingMergePatchRebase),
}

enum ShortwireEffect {
    ApplyPatch { source: String, reference_image: Option<ShortwireReferenceImage> },
    ResetPatch,
    UpsertPatchArtifact { item: DebugArtifactItem, content_text: String },
    UpsertReferenceImageArtifact { item: DebugArtifactItem, bytes: Vec<u8> },
    RequestDiffCapture(ShortwireDiffCaptureRequest),
    ReferenceEnterShortwire(ShortwireRowIdentity),
    ReferencePrepareShortwireSave,
    ReferenceCommitAfterLeftApply { exit_after_apply: bool },
    ReferenceCancelWithoutSave,
    ReferenceSaveWithoutLeftApply,
    DependencyRefreshRequested,
}
```

The exact enum names can change. The required contract is:

- `ApplyPatch` / `ResetPatch` remain app-facing operations and are acknowledged later.
- `UpsertPatchArtifact` may be emitted from both frame rendering and apply/diff callbacks; callback
  paths must persist immediately so patch/diff metadata is not delayed until the next visible frame.
- `RequestDiffCapture` is only emitted after a stored patch exists or an image-only patch has been
  created for the active row.
- Reference handoff effects are store-internal coordination until `ReferenceWorkspaceState` is
  extracted.

Behavioral transitions to preserve:

- New window or artifact snapshot with no active/dirty Shortwire: restore patch artifact if version is
  supported; do not restore over active Shortwire, dirty stored patches, or dirty shader draft.
- Enter row with no runtime patch active: create an `Editing` session from the current canonical base,
  enter matching reference Shortwire, apply stored hunks if possible, enable diff view on successful
  re-entry, and clear stale draft-analysis scheduling.
- Enter row with runtime patch active: create `PendingResetThenEnter`, emit `ResetPatch`, and keep the
  editor non-interactive until `AppPatchReset` completes the entry.
- Reset-before-enter failure: clear active Shortwire, keep stored patches unchanged, clear
  any pending apply token, and show reset failure status.
- Stored patch re-entry: apply hunks against current canonical base. If hunk application fails, remove
  the stored patch, mark patch store dirty, surface an outdated-patch error, and continue in fresh
  `Editing` mode.
- Enter-and-apply stored patch: apply stored hunks, enter `PendingApply`, emit `ApplyPatch` with stored
  `reference_image`, and load matching reference Shortwire state. If hunk application fails, remove the
  stored patch and fall back to normal edit entry.
- Canonical base change during `Editing`: update the current canonical base via shader/dependency
  state, mark active base stale, keep the draft untouched, and keep dependency rows canonical.
- Save while editing with stale base: compute user hunks against the entry base and apply them to the
  latest canonical base. On success, replace the draft with the rebased source and continue; on
  failure, stay in `Editing` and keep pending stores unchanged.
- Save while editing: compute final hunks from latest canonical base to draft, ask reference workspace
  to prepare its pending hunks, create `pending_apply`, enter `PendingApply`, emit `ApplyPatch`, and
  keep stored patch maps unchanged until acknowledgement.
- `AppPatchApplied` with `pending_apply`: store pending hunks under the pending patch key with the
  latest canonical base hash and pending reference image, clear old diff result, mark patch store
  dirty, ask reference workspace to commit its prepared hunks, emit patch artifact upserts
  immediately, and emit a diff-capture request. If `pending_apply.exit_after_apply` is true, clear
  active Shortwire and refresh dependency navigation; otherwise return to `Editing` with the new base
  snapshot.
- `AppPatchFailed` with `pending_apply`: return to `Editing` when the active session is still visible,
  keep the draft text, clear `pending_apply`, keep reference pending hunks uncommitted, and clear
  active diff view.
- Navigate/close while editing: save reference Shortwire without left apply, store non-empty shader
  hunks or image-only patch metadata, preserve an existing `diff_result` only when base hash and hunks
  are unchanged, restore shader editor to canonical base, and emit `ResetPatch` if runtime patch is
  active.
- Navigate/close while pending apply: target correction from current code reading. Set
  `pending_apply.exit_after_apply`, hide/close active UI if needed, restore editor to canonical base,
  and emit reset only if a runtime patch remains active. The reducer must keep a pending
  acknowledgement token containing the hunks so the later `AppPatchApplied` event can still commit
  the patch store.
- Cancel while editing: cancel reference Shortwire without save, clear active Shortwire, restore draft
  to canonical base, do not store hunks, and do not emit app apply/reset unless a runtime patch needs
  separate cleanup through the normal close path.
- Explicit diff capture for active Shortwire: attach any pasted reference image to the active session
  and stored patch, create an image-only patch if there are no hunks yet, clear stale `diff_result`,
  emit patch artifact and binary image artifact upserts, then emit `RequestDiffCapture`.
- Diff capture completion: update only the matching stored patch. If the patch is missing or the
  request no longer matches an active/stored patch, drop the result. Persist the updated patch artifact
  immediately.
- Merge rebase acknowledgement: find exactly one stored patch whose hunks apply from old base to old
  local source. Recompute hunks from incoming base to acknowledged source; remove the patch if the new
  hunks are empty; otherwise preserve `reference_image`, clear `diff_result`, update base hash, and
  mark dirty. If matching is zero or ambiguous, do not mutate stored patches.

Extraction order:

1. Move `ShortwireRowIdentity`, `ShortwireNodePatch`, `ShortwireHunk`, `ShortwirePatchesPayload`,
   diff-result types, patch-key helpers, dot-status helpers, compact diff-view builders, and
   patch-artifact encode/decode into `shortwire.rs` / `patch.rs` without changing call sites.
2. Move `compute_hunks`, `apply_hunks`, `locate_hunk_position`, `verify_hunk_at_position`, and
   `HunkApplyError` to `patch.rs`. Keep the functions free of window, egui, artifact, and app types.
3. Introduce `ShortwireState` inside `PassDebugWindowDocument` while keeping old wrapper methods
   (`enter_shortwire`, `exit_shortwire_done`, `record_error`, etc.) delegating to transition helpers.
4. Move stored patch artifact restore/collect/take logic behind `ShortwirePatchStore`. Keep
   `PassDebugWindowState::loaded_shortwire_patches_artifact_hash` temporarily until registry/effect
   extraction decides whether artifact snapshot dedupe belongs in the registry or store.
5. Convert entry, save, navigate/close, cancel, `mark_applied`, `mark_reset`, and `record_error` paths
   to `ShortwireEvent` transitions that return `ShortwireEffect` values. During migration, wrappers
   convert `ApplyPatch`, `ResetPatch`, and artifact effects back to existing
   `PassDebugWindowAction` / `PassDebugPatchApplyResult` outputs.
6. Replace direct calls from Shortwire methods into reference workspace with store-level coordination
   effects. This should happen after the reference reducer exists, or behind temporary adapter
   functions if Shortwire is extracted first.
7. Move merge patch rebase from `commit_pending_merge_patch_update` into a Shortwire transition that
   consumes `PendingMergePatchRebase` after merge verifies the app acknowledgement.
8. Move tree/view coupling to selectors: render code receives active row hint, can-enter state, dot
   info, diff-view enabled state, and emits events instead of mutating Shortwire fields.
9. After shader extraction, remove `generated_base_source` / `generated_base_source_hash` from
   `PassDebugWindowDocument`; Shortwire receives current canonical base through event payloads or
   store selectors.

Focused validation:

- Preserve existing tests covering:
  - legacy Shortwire patch JSON restore without `diffResult`
  - diff result and reference image round-trip in patch artifacts
  - dot status threshold and hover information
  - active diff capture clearing stale result and persisting a patch artifact
  - pasted reference image creating an image-only patch plus binary artifact
  - late artifact restore being skipped while active/dirty
  - `mark_patch_applied` returning shader/reference patch artifacts immediately
  - apply failure returning active Shortwire to editing and leaving reference patches uncommitted
  - dependency tree remains canonical while patches are applied
  - active-row click no-op and other-row navigation exit
  - pending reset-then-enter success and failure
  - pending apply failure reverts to editing
  - cancel, close/navigate, re-enter, enter-and-apply, stale-base rebase, and patch-key stability
  - merge rebase of matching stored patch only after app apply success
- Add reducer/effect tests before view extraction:
  - `EnterRequested` with patch active emits only `ResetPatch` and does not mutate patch store
  - `AppPatchApplied` is the only event that commits `PendingApply` hunks to the patch store
  - callback artifact effects are emitted immediately from `AppPatchApplied` and
    `DiffCaptureCompleted`
  - `NavigateOrCloseRequested` during `PendingApply` preserves enough pending state for the later app
    acknowledgement
  - changing only `reference_image` clears `diff_result` and marks the patch artifact dirty
  - explicit capture without hunks creates an image-only patch and request
  - stale-base rebase failure leaves phase `Editing` and preserves the draft
  - merge rebase with zero or multiple matching patches does not mutate stored patches
  - artifact snapshot restore does not overwrite active/dirty state

Exit criteria:

- `shortwire.rs` imports no `egui`, `rfd`, `std::fs`, viewport types, reference workspace concrete
  fields, merge UI state, or app shell/canvas types.
- `patch.rs` owns hunk computation/application and is independent of Shortwire state.
- Shortwire patch artifacts have one encoder/decoder and one dirty flag owner.
- Stored shader patches are committed only after app acknowledgement, never when `ApplyPatch` is first
  emitted.
- Reference-image binary artifacts and diff-capture requests are represented as effects and still
  flow through the existing public callback result during migration.
- Views render Shortwire state from selectors and emit typed events for enter, apply stored patch,
  save, cancel/close, diff toggle, and capture.
- All current pass debug tests pass after each extraction step.

### Reference Workspace

Current code facts:

- `ReferenceWorkspaceState` owns `root_path`, `root_label`, selected file, file list,
  `editor_source`, sync debounce/status, skipped count, manifest dirty flag, active reference
  Shortwire fields, pending reference patch hunks, and local-file restore bookkeeping.
- Reference Shortwire patches are stored separately on `PassDebugWindowDocument` as
  `reference_patches` plus `reference_patches_dirty`, but behaviorally they belong to the reference
  workspace. The target state should move them under the reference module.
- There are four artifact inputs:
  - workspace manifest: `DebugArtifactRole::Attachment`, slot `reference-workspace`
  - archived reference files: `DebugArtifactRole::ReferenceCode`, slot prefix `file:`
  - legacy single reference: `DebugArtifactRole::ReferenceCode`, default slot
  - reference Shortwire patches: `DebugArtifactRole::Patch`, slot `reference-patches`
- Local side effects currently happen in domain methods:
  - manifest restore reads local files when `rootPath` exists
  - folder import and reload scan local folders synchronously
  - sync writes dirty rooted files before emitting the workspace manifest artifact
  - reference Shortwire snapshots, writes, and restores the selected local file
  - the Open button calls `rfd::FileDialog` directly from the render function
- `load_reference_workspace_from_artifacts` currently mixes pure artifact decoding with rooted
  manifest local reads. If a rooted manifest file is missing, the code intentionally skips it and
  does not fall back to archived artifact text.
- `take_reference_workspace_dirty_artifacts` currently performs a full sync transaction inline: it
  commits the editor draft, writes dirty rooted files, marks successfully written files clean,
  builds the manifest artifact, and returns app-facing artifact upsert actions.
- For rooted workspaces, only the manifest artifact is emitted after sync. The local files are the
  source of truth and `ReferenceCode` file artifacts are not re-emitted.

Target state shape:

```rust
struct ReferenceWorkspaceState {
    location: ReferenceLocation,
    files: Vec<ReferenceFileState>,
    selected_file: Option<String>,
    editor_draft: String,
    sync: ReferenceSyncState,
    shortwire: Option<ReferenceShortwireSession>,
    patches: ReferencePatchStore,
}

enum ReferenceLocation {
    Empty,
    Archived { label: String },
    LocalRoot { root: PathBuf, label: String },
}

struct ReferenceFileState {
    relative_path: String,
    artifact_id: String,
    source: String,
    loaded_source: String,
}

struct ReferenceSyncState {
    due_secs: Option<f64>,
    status: Option<String>,
    skipped_files: usize,
    manifest_dirty: bool,
    pending: Option<ReferencePendingSync>,
    revision: u64,
}

struct ReferencePendingSync {
    id: ReferenceSyncId,
    workspace_revision: u64,
    expected_file_hashes: HashMap<String, String>,
}

struct ReferenceShortwireSession {
    patch_key: String,
    base_source: String,
    pre_session_source: String,
    pending_hunks: Option<Vec<ShortwireHunk>>,
    local_restore: Option<ReferenceLocalRestore>,
}

struct ReferencePatchStore {
    patches: HashMap<String, ShortwireNodePatch>,
    dirty: bool,
}

struct ReferenceSyncPlan {
    id: ReferenceSyncId,
    workspace_revision: u64,
    writes: Vec<ReferenceFileWrite>,
    text_artifacts: Vec<(DebugArtifactItem, String)>,
    manifest_dirty_after_failure: bool,
}

struct ReferenceFileWrite {
    relative_path: String,
    path: PathBuf,
    content: String,
}

struct ReferenceSyncResult {
    id: ReferenceSyncId,
    workspace_revision: u64,
    written_files: Vec<String>,
    failed_writes: Vec<(String, String)>,
    emitted_artifact_ids: Vec<String>,
}
```

State ownership rules:

- `editor_draft` is the only mutable text buffer for the selected reference file. The selected
  file's `source` is updated when selecting another file, scheduling sync, entering Shortwire, or
  collecting a sync plan.
- File selection is disabled while reference Shortwire is active. This preserves the invariant that
  `pre_session_source`, `base_source`, and `patch_key` all refer to the same relative path.
- `ReferencePatchStore` is keyed by `"{relative_path}::{row_patch_key}"`; the relative path is part
  of identity so two reference files can hold independent patches for the same shader row.
- Dirty workspace state shields local edits from incoming artifact snapshots. Incoming artifacts may
  only acknowledge matching file contents and clear dirty flags; they must not replace a dirty
  draft, a dirty manifest, or an active reference Shortwire session.
- Rooted workspaces treat the local root as authoritative. If a manifest has `rootPath`, missing or
  unreadable local files are skipped and counted; the loader must not fall back to archived artifact
  text for those files. Pathless manifests use archived artifact text.
- Legacy default reference text migrates to a pathless workspace named `reference.txt`, with
  `loaded_source` empty so the first sync emits both the new manifest and the file artifact.
- `ReferenceSyncState::revision` increments whenever file contents, selection, location, manifest
  metadata, or patch-store state changes. Sync completion events must carry the plan id/revision they
  were produced from so stale completions cannot mark newer edits clean.
- The reducer may build a `ReferenceSyncPlan`, but it must not mark files clean when the plan is
  created. Clean markers move only after `SyncCompleted` reports successful local writes
  and the effect runner has handed text artifact upserts to the app-facing action queue.
- `ReferenceSyncPlan::manifest_dirty_after_failure` exists because rooted sync has a mixed outcome:
  local file write failures should keep manifest/file dirty state, while successful writes may still
  require a manifest upsert that records updated content hashes.

Event and effect contract:

```rust
enum ReferenceEvent {
    ArtifactSnapshotChanged(ReferenceArtifactSnapshot),
    ManifestLocalFilesLoaded(ReferenceManifestLoadResult),
    FolderPicked(PathBuf),
    FolderRead(ReferenceFolderReadResult),
    ReloadRequested,
    FileSelected(String),
    DraftEdited { text: String, now_secs: f64 },
    SyncDue { now_secs: f64 },
    SyncCompleted(ReferenceSyncResult),
    EnterShortwire(ShortwireRowIdentity),
    PrepareShortwireSave,
    LeftApplySucceeded { exit_after_apply: bool },
    LeftApplyFailed(String),
    ExitShortwire { save: bool },
}

enum ReferenceEffect {
    PickFolder,
    ReadManifestLocalFiles {
        request_id: ReferenceLoadRequestId,
        root: PathBuf,
        manifest_files: Vec<ReferenceWorkspaceManifestFile>,
    },
    ReadFolder { root: PathBuf, pass_name: String, mark_dirty: bool },
    RunSyncPlan(ReferenceSyncPlan),
    WriteShortwireLocalFile { path: PathBuf, content: String },
    RestoreShortwireLocalFile { path: PathBuf, content: String },
}
```

The event names do not need to become public API, but the separation must be preserved:

- Artifact decode and payload construction are pure.
- Filesystem reads/writes and folder dialogs are effects.
- Effects report completion through events before state marks files clean or clears restore
  bookkeeping.
- `RunSyncPlan` is a registry/effect-runner effect. For rooted plans it writes local files first,
  then queues text artifact upserts. For archived plans it only queues text artifact upserts and
  reports immediate completion.
- The public app-facing output can remain `PassDebugWindowAction::UpsertDebugArtifact` for text
  artifacts during migration.

Effect runner contract:

- `PickFolder` owns all `rfd` usage. It returns either `FolderPicked(path)` or a no-op cancellation
  event; render code never calls the dialog directly.
- `ReadManifestLocalFiles` reads only the manifest-declared relative paths under `root`, enforces the
  existing UTF-8 and max-size limits, counts missing/unreadable files, and returns
  `ManifestLocalFilesLoaded`. It must not inspect archived `ReferenceCode` artifacts as fallback.
- `ReadFolder` scans local folders with the current max-file and max-size limits, preserving the
  existing skipped-file behavior.
- `RunSyncPlan` is allowed to run synchronously in the first migration pass, but its API should look
  like an effect completion boundary. This prevents reducer code from depending on `std::fs` and
  keeps a future async runner possible without redesigning state.
- `WriteShortwireLocalFile` and `RestoreShortwireLocalFile` are separate from normal workspace sync.
  They are paired to the active reference Shortwire session and must complete before the session
  clears its local-restore token.

Behavioral transitions to preserve:

- `ArtifactSnapshotChanged` with a pathless manifest restores reference files from `file:*`
  artifacts and sets status to "Loaded archived reference".
- `ArtifactSnapshotChanged` with a rooted manifest emits `ReadManifestLocalFiles`; the completion
  event replaces clean state with local file contents, reports missing files in `skipped_files`, and
  does not use archived fallback text for missing rooted files.
- If `ManifestLocalFilesLoaded` arrives after the workspace became dirty, the reducer treats it as an
  acknowledgement candidate only. It may clear matching loaded sources, but it must not replace the
  editor draft, dirty file contents, or active reference Shortwire state.
- `DraftEdited` commits the draft to the selected file, schedules sync at
  `now + PASS_DEBUG_REFERENCE_SYNC_DEBOUNCE_SECS`, and sets sync-pending status. During reference
  Shortwire it only updates the draft/status and does not schedule normal workspace sync.
- `SyncDue` for a rooted workspace emits writes for dirty local files and then a manifest upsert.
  It must not emit `ReferenceCode` file artifacts for rooted files. Dirty flags are cleared only
  after `SyncCompleted` confirms successful writes.
- `SyncDue` for an archived workspace emits the manifest when dirty and emits one `ReferenceCode`
  artifact for each dirty file.
- `SyncCompleted` with failed rooted writes keeps those files dirty, keeps or restores
  `manifest_dirty`, and reports the first failure in status text. Successfully written files may be
  marked clean if their source still matches the planned content.
- `SyncCompleted` with a stale plan id or stale workspace revision is ignored except for optional
  status logging. It must not overwrite newer local edits.
- `ReloadRequested` keeps the existing archive snapshot if the root path is missing. A reload that
  finds no UTF-8 files reports status and leaves current files intact.
- Entering reference Shortwire snapshots the selected local file when possible. If a stored
  reference patch applies, the patched reference draft is also written to the local file so external
  diff tooling sees the paired reference change.
- Reference patch hunks are prepared when the left shader patch is submitted, but committed to
  `ReferencePatchStore` only after the left `AppPatchApplied` callback succeeds. On apply failure,
  the pending hunks remain in the active reference Shortwire session and the editor returns to
  editing.
- Exiting or closing while reference Shortwire wrote a local file restores the pre-session local
  content. Restore failure should become status text and log output, but it should not keep the
  window from closing.

Extraction order:

1. Move manifest structs, reference file state, artifact ID/slot helpers, and reference patch
   payload encode/decode behind `artifacts.rs` and `reference_workspace.rs` without changing call
   sites.
2. Move local scan/read/write helpers to `file_io.rs` as stateless functions. At this step they may
   still be called synchronously by existing document methods, but new state-transition functions
   must not call them.
3. Introduce `ReferenceArtifactSnapshot` decoding as a pure function. Rooted manifest restore should
   return `ReferenceEffect::ReadManifestLocalFiles` instead of calling local read helpers directly.
4. Introduce pure reference reducer functions that turn artifact snapshots, editor edits, file
   selection, reload, and sync timer events into state changes plus `ReferenceEffect`.
5. Add `ReferenceSyncPlan` construction for rooted and archived workspaces. Keep the old
   `take_reference_workspace_dirty_artifacts` wrapper temporarily, but make it delegate to plan
   construction plus a synchronous effect runner.
6. Add a synchronous effect runner inside the pass debug registry for `PickFolder`, local reads, and
   local writes. This preserves current behavior while removing filesystem calls from the store.
7. Move titlebar/reference editor controls to events: Open emits `ReferenceOpenFolderRequested`,
   Reload emits `ReferenceReloadRequested`, combo selection emits `ReferenceFileSelected`, and text
   edits emit `ReferenceDraftEdited`.
8. Fold `reference_patches` and `reference_patches_dirty` into `ReferenceWorkspaceState` or a
   child `ReferencePatchStore`, then update `mark_patch_applied`, `record_patch_error`, and
   Shortwire navigation to call the reference reducer instead of mutating reference fields directly.

Focused validation:

- Preserve existing tests covering:
  - legacy default reference artifact migration
  - multiple-file workspace restore from artifacts
  - rooted manifest loading local files instead of artifact text
  - rooted missing files not falling back to artifact text
  - pathless manifest restoration from archived text
  - rooted sync writing the local file and emitting only the manifest
  - reference patch keys isolated by relative path
  - reference Shortwire save, apply success, apply failure, re-enter, close, and local restore
  - reload with a missing root preserving the current archive snapshot
- Add reducer/effect tests before view extraction:
  - rooted sync does not clear dirty state until `SyncCompleted`
  - write failure keeps dirty file state and manifest dirty state
  - incoming artifact snapshots acknowledge matching clean files without clobbering dirty drafts
  - rooted manifest restore emits `ReadManifestLocalFiles` and never reads files during artifact
    decode
  - rooted manifest local-load completion is ignored or treated as acknowledgement when a dirty edit
    was made before the read completed
  - stale `ReferenceSyncResult` cannot mark newer draft content clean
  - archived sync emits dirty `ReferenceCode` file artifacts, while rooted sync emits only the
    manifest artifact plus local write effects
  - `CloseRequested` during reference Shortwire emits local restore before viewport close
  - `ReferenceOpenFolderRequested` emits `PickFolder` and never calls `rfd` from render code

Exit criteria:

- `reference_workspace.rs` imports no `egui`, `rfd`, or `std::fs`.
- `file_io.rs` imports no pass debug store/window types.
- Reference artifact slot keys are defined in one place and match `debug_artifacts.rs`.
- `PassDebugWindowDocument` no longer contains `reference_patches` as a sibling of
  `reference_workspace`.
- No reducer method performs rooted manifest reads, folder scans, file writes, file-dialog calls, or
  local Shortwire restore writes directly.
- All current pass debug tests pass after each extraction step.

### Merge Conflict Flow

Current code facts:

- Merge is triggered only when a patch override is active, the app source revision changes, and the
  incoming canonical WGSL differs from `generated_base_source`.
- `handle_patch_canonical_change` updates canonical analysis state before attempting the merge. This
  means the dependency tree already tracks the incoming generated shader while the runtime may still
  be showing the previous patch override.
- The current three-way merge is hunk based:
  `local_hunks = compute_hunks(base, local)` and then `apply_hunks(incoming, local_hunks)`.
  `local == base` resolves to incoming; `incoming == base` resolves to local.
- Clean auto-merge emits an app-facing `ApplyPatch` or `ResetPatch` action and records
  `PassDebugPendingMergePatchUpdate`. It does not immediately rewrite the stored Shortwire patch.
- Pending Shortwire patch rebase is committed only from `mark_applied` / `mark_reset`, after the app
  rebuild callback confirms the requested source. `commit_pending_merge_patch_update` drops the
  pending update if the acknowledged source does not match the requested merged source.
- Shortwire patch rebase currently finds the one stored patch whose hunks applied to the old base
  produce the old local source. If exactly one patch matches, it recomputes hunks from incoming to
  the acknowledged merged source. If the new hunks are empty, the stored patch is removed.
- Merge conflict UI state is currently stored inside `PassDebugMergeConflict`:
  base/incoming/local sources, editable `resolved_source`, error text, and two popup booleans.
- Resolver actions are:
  - "Discard Shortwire Patch" / "Use Incoming" emits `ResetPatch`.
  - "Keep Local" emits `ApplyPatch(local)`.
  - "Apply Resolved" emits `ApplyPatch(resolved_source)`.
  - "Cancel" restores the local patch text into the editor and drops pending merge state.
- The status selector currently checks `last_error` before `merge_conflict`, so conflicts generally
  surface as patch-failed text rather than the plain `Conflict` branch. The refactor should make
  conflict a first-class view status rather than relying on error-string priority.
- Active Shortwire is a separate path. Source refresh during active Shortwire marks the active base
  stale and does not enter this merge flow until the user saves or leaves Shortwire.

Target state shape:

```rust
struct MergeState {
    conflict: Option<MergeConflictState>,
    pending_patch_rebase: Option<PendingMergePatchRebase>,
}

struct MergeConflictState {
    base_source: String,
    incoming_source: String,
    local_source: String,
    resolved_source: String,
    error: MergeError,
    ui: MergeConflictUiState,
}

struct MergeConflictUiState {
    choice_popup_open: bool,
    resolver_window_open: bool,
}

struct PendingMergePatchRebase {
    base_source: String,
    incoming_source: String,
    local_source: String,
    requested_source: String,
    requested_action: MergePatchRuntimeAction,
}

enum MergePatchRuntimeAction {
    ApplyPatch,
    ResetPatch,
}
```

`MergeState` owns the conflict lifecycle and the pending rebase token, but it must not own the
runtime patch-loaded fact, the editor draft, or the Shortwire patch store. Those belong to shader
and Shortwire respectively.

State ownership rules:

- `ShaderDocumentState` detects the canonical-change condition and provides
  `{ base, incoming, local }` to merge. Merge decides whether the result is auto-merge, reset,
  apply, or conflict.
- `MergeState::pending_patch_rebase` is a token for a requested app mutation. It becomes valid only
  after an `ApplyPatch` or `ResetPatch` effect is emitted. It must be cleared on matching app
  acknowledgement, app failure, cancel, or any newer canonical-change merge request.
- The stored Shortwire patch is rebased after app acknowledgement, not when the merge decision is
  made. This preserves the current rule that debug artifacts are not updated for patch requests that
  fail to compile or are superseded.
- The matching heuristic for rebasing stored Shortwire patches belongs to Shortwire, not merge:
  merge can provide `PendingMergePatchRebase`, but Shortwire decides which patch key, if any, maps
  old base to old local.
- Conflict UI booleans are disposable UI state. The durable conflict fact is the
  `{ base, incoming, local, resolved, error }` tuple.
- `last_error` / `last_status` should not be the source of truth for merge. Selectors may derive
  user-facing text from merge state, but clearing status text must not clear the conflict.
- While a merge conflict is present, normal save and Shortwire entry remain disabled. Resolver
  actions are the only paths that may emit patch apply/reset from that conflict.
- A source refresh that arrives while a conflict is open must either replace the conflict with a new
  merge attempt against the latest incoming canonical source or explicitly ignore stale revisions.
  The reducer should never merge against an older incoming source after a newer source snapshot has
  arrived.

Event and effect contract:

```rust
enum MergeEvent {
    CanonicalPatchChanged {
        base_source: String,
        incoming_source: String,
        local_source: String,
    },
    OpenResolver,
    CloseResolver,
    CancelResolution,
    UseIncoming,
    KeepLocal,
    ResolvedEdited(String),
    ApplyResolved,
    AppPatchApplied { applied_source: String },
    AppPatchReset { reset_source: String },
    AppPatchFailed(String),
}

enum MergeEffect {
    ApplyPatch { source: String },
    ResetPatch,
    RebaseStoredShortwirePatch(PendingMergePatchRebase),
}
```

The exact enum names can change during implementation. The required contract is:

- `CanonicalPatchChanged` is pure and synchronous. It may emit `ApplyPatch` / `ResetPatch`, or it may
  create a `MergeConflictState`.
- App-facing patch effects still flow through `PassDebugWindowAction` during migration.
- `RebaseStoredShortwirePatch` is a store-internal handoff after a matching app acknowledgement; it
  is not a renderer command and is not a debug artifact upsert by itself.

Behavioral transitions to preserve:

- `local == base`: emit `ResetPatch`, store a pending rebase with `requested_source = incoming`, and
  show "Canonical source changed - clearing empty patch" or equivalent status.
- `incoming == base`: the pure merge primitive resolves to local. The canonical-change entrypoint
  normally avoids this path because it only runs when incoming differs from base, but `patch.rs`
  should preserve the primitive behavior for direct unit tests and future callers.
- Clean auto-merge to a non-incoming source: emit `ApplyPatch(merged)`, keep runtime state pending,
  store a pending rebase token, and leave the editor draft/runtime loaded source unchanged until the
  app callback.
- Auto-merge conflict: create `MergeConflictState`, set `resolved_source = local_source`, keep the
  editor showing the local source, and block ordinary save/Shortwire entry.
- "Use Incoming": emit `ResetPatch`, store a pending token with `requested_source = incoming`, and
  close resolver UI only after the effect is queued.
- "Keep Local": emit `ApplyPatch(local)`, store a pending token with `requested_source = local`, and
  keep debug artifacts unchanged until `AppPatchApplied`.
- "Apply Resolved": emit `ApplyPatch(resolved)`, store a pending token with
  `requested_source = resolved`, and keep the conflict visible or marked pending until the app
  acknowledgement arrives.
- "Cancel": drop conflict and pending token, restore the local patch as the loaded/draft editor text
  for the current runtime patch, and do not emit an app action.
- `AppPatchApplied(applied_source)`: if it matches `pending_patch_rebase.requested_source`, clear the
  conflict, hand the rebase token to Shortwire, clear pending merge state, and let shader mark the
  runtime patch loaded.
- `AppPatchReset(reset_source)`: same as apply, but removes or rebases the stored patch according to
  the pending token and lets shader mark runtime canonical loaded.
- `AppPatchFailed`: do not commit the pending rebase token. If the failure came from a resolver
  action, keep the conflict and resolved editor text so the user can edit and retry. If it came from
  an auto-merge request with no open conflict, clear the pending token and let shader status report
  the failed apply/reset without rewriting stored Shortwire patches.

Extraction order:

1. Move `three_way_merge_sources` to `patch.rs` with `compute_hunks`, `apply_hunks`,
   `locate_hunk_position`, and `HunkApplyError`. Keep it free of pass debug document types.
2. Introduce `merge.rs` with `MergeState`, `MergeConflictState`, and pure transition helpers.
   Existing `PassDebugWindowDocument` can delegate to this state while still holding the fields.
3. Replace `handle_patch_canonical_change` internals with a call to merge transition logic. Convert
   returned `MergeEffect::ApplyPatch` / `ResetPatch` back into `PassDebugWindowAction` during
   migration.
4. Move `open_merge_resolver`, `apply_merge_resolved`, `use_merge_incoming`,
   `keep_merge_local`, and `cancel_merge_resolution` into merge events. The render functions should
   emit events and mutate no merge fields directly.
5. Move `commit_pending_merge_patch_update` out of shader completion handling. Shader should emit an
   app-acknowledged event; merge verifies the pending token; Shortwire performs the stored patch
   rebase.
6. Update selectors so conflict status and resolver enablement derive from `MergeState` instead of
   `last_error` string content.

Focused validation:

- Preserve existing tests covering:
  - clean auto-merge emits `ApplyPatch`
  - matching stored Shortwire patch is rebased only after apply success
  - conflicting canonical change opens the choice popup and resolver state
  - dependency tree tracks incoming canonical source while the patch runtime is unresolved
- Add reducer tests before view extraction:
  - pending rebase token is not committed when app acknowledgement source differs
  - app failure keeps conflict/resolved text and clears pending token
  - cancel emits no patch action and restores local runtime text
  - use incoming emits reset and eventually clears or removes the matching stored patch
  - apply resolved emits apply and rebases the matching stored patch from incoming to resolved
  - ambiguous stored patch matches do not rewrite any Shortwire patch and leave artifacts dirty only
    when a patch actually changed
  - conflict status selector reports conflict even when an error/status string is present

Exit criteria:

- `merge.rs` imports no `egui`, `rfd`, `std::fs`, debug artifact payload types, or render-cache
  types.
- `patch.rs` owns hunk application and three-way merge primitives.
- Merge UI rendering only reads view models and emits merge events.
- Shortwire patch rebase happens only after matching app apply/reset acknowledgement.
- All current pass debug tests pass after each extraction step.

### View Rendering

View rendering is intentionally left as a later deep dive. The first implementation pass should
extract pure state, reducer, artifact, and file-I/O boundaries before moving egui code. Once domain
state no longer mutates directly from render functions, study titlebar, split layout, dependency
tree, editors, line layout cache, merge dialogs, and viewport close behavior as their own rendering
pass.

## Design Closure

This document is now the architectural contract for the refactor:

- ownership rules for durable state and disposable render caches
- internal event/effect model
- proposed module boundaries
- public app integration boundary during migration
- module-level invariants for shader document, dependency tree, Shortwire, reference workspace, and
  merge conflict flow

The remaining gap is not more top-level architecture. The next artifact should be an implementation
roadmap with PR-sized slices, compatibility adapters, exact validation commands, and first-slice
entry criteria. Keep that execution plan adjacent to this document as
`21-debug-window-refactor-implementation-plan.md`.

## Open Questions

- Should tests move gradually with modules, or stay in one integration test module until all
  extraction is complete? The implementation plan should default to colocated module tests for newly
  extracted pure modules while preserving existing integration-style tests until each behavior owner
  is fully moved.

Resolved for the first extraction pass:

- File dialogs should be `PassDebugEffect::PickReferenceFolder` handled by the pass debug registry
  or effect runner, not called from render code and not surfaced to the broader app command layer.
- Reference workspace import remains folder-only in the public UI. Single-file read support stays
  internal for reload/tests unless product requirements change.
- Binary Shortwire reference-image uploads and diff-capture scheduling should remain returned through
  `PassDebugPatchApplyResult` during migration. Internal reducers may emit `UpsertBinaryArtifact` and
  `RequestDiffCapture`, but widening `PassDebugWindowAction` should wait for an app command boundary
  redesign.
