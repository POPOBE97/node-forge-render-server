# Pass Debug Window Refactor

## Status

Draft. This document starts with the top-level design only.

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

- `shader` owns canonical generated WGSL, loaded editor WGSL, draft WGSL, source revision, dirty
  state, patch-active state, and parse/dependency source snapshots.
- `dependencies` owns flattened rows, root/focus/expansion/filter state, and navigation intents.
- `shortwire` owns active Shortwire session, stored node patches, diff capture state, and patch
  artifact dirtiness.
- `reference` owns reference files, selected file, reference editor draft, root path, sync state,
  reference Shortwire patches, and local restore bookkeeping.
- `merge` owns only merge-conflict state and pending merge patch rebases.
- `ui` owns viewport/split/editor-only UI state that is not domain state.
- `caches` owns galley/tree-width/visible-row caches and can be dropped without changing behavior.

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
    Tick { now_secs: f64 },
    CloseRequested,
    SaveRequested,
    DependencyRowClicked(RowClick),
    DependencyRowContextAction(RowAction),
    ShaderDraftEdited(String),
    ReferenceDraftEdited(String),
    ReferenceFileSelected(String),
    ReferenceOpenFolderRequested,
    ReferenceReloadRequested,
    MergeAction(MergeAction),
    ShortwireAction(ShortwireAction),
    DiffCaptureResult(DiffCaptureResult),
}
```

Effects are the only bridge out:

```rust
enum PassDebugEffect {
    ApplyPatch { pass_name: String, source: String, reference_image: Option<ShortwireReferenceImage> },
    ResetPatch { pass_name: String },
    ResetAllPatches,
    UpsertTextArtifact { item: DebugArtifactItem, content_text: String },
    UpsertBinaryArtifact { item: DebugArtifactItem, bytes: Vec<u8> },
    ReadReferenceFolder { path: PathBuf },
    WriteReferenceFile { path: PathBuf, content: String },
    PickReferenceFolder,
    RequestDiffCapture(ShortwireDiffCaptureRequest),
    CloseViewport,
    FocusViewport,
}
```

The existing public `PassDebugWindowAction` can remain as the app-facing subset of effects during
migration. Internally, use the richer `PassDebugEffect` so file I/O and viewport actions are no
longer hidden inside render functions.

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
  reference_workspace.rs    # reference files, selection, sync state, artifact migration
  artifacts.rs              # DebugArtifactItem naming, payload encode/decode
  file_io.rs                # local reference read/write/folder scan
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
- Preserve these current public entrypoints until the internal split is stable:
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

## Public Integration Boundary

The app should continue to see pass debug as a narrow feature boundary:

```text
AppCommand::OpenPassDebug
  -> pass_debug::open_pass_debug_window(...)

present frame
  -> pass_debug::show_pass_debug_windows(...)
  -> Vec<PassDebugWindowAction>
  -> AppCommand::{ApplyPassShaderPatch, ResetPassShaderPatch, UpsertDebugArtifact, ...}

shader rebuild completion
  -> pass_debug::mark_patch_applied / mark_patch_reset / record_patch_error

diff analysis completion
  -> pass_debug::record_shortwire_diff_result
```

Do not pull `App` into the pass debug modules. The feature should receive snapshots and return
effects/actions.

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

Exit criteria: file reads/writes are callable through a small adapter, and artifact IDs/slot keys
have one owner.

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

To be filled after studying canonical source, loaded source, draft source, dirty state, patch active
state, source revisions, and analysis snapshots.

### Dependency Tree State

To be filled after studying row flattening, root selection, focus, expansion, filtering, jump
behavior, and visible-row caching.

### Shortwire State Machine

To be filled after studying stored patch lifecycle, active phases, diff capture, reference image
attachment, stale-base rebase, and interaction with app patch apply/reset.

### Reference Workspace

To be filled after studying workspace manifests, legacy migration, local folder/file import,
debounced sync, reference Shortwire patches, and local file restore semantics.

### Merge Conflict Flow

To be filled after studying canonical-change rebase, conflict resolver UI, and pending merge patch
updates.

### View Rendering

To be filled after studying titlebar, split layout, dependency tree, editors, line layout cache,
merge dialogs, and viewport close behavior.

## Open Questions

- Should file dialogs remain as effects handled by the pass debug registry, or be surfaced to the
  app command layer?
- Should `PassDebugWindowAction` grow binary artifact support, or should binary artifact upload
  remain returned through `PassDebugPatchApplyResult` for now?
- Should the reference workspace support single-file import publicly, or is folder import the only
  user-facing path?
- Should tests move gradually with modules, or stay in one integration test module until all
  extraction is complete?
