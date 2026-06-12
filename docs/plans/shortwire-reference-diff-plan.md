# Shortwire Reference Auto-Load And Diff View Plan

## Task Context

Shortwire reference workspaces persist local file paths and archived debug artifacts, but reopening a pass debug window only restores artifact content. Diff mode also renders against the full editor source, so large files hide the relevant change context, and the reference editor does not show the same diff.

## Assumptions

- Saved local reference paths should be preferred when they still exist.
- If a workspace has a saved local path, that path is the only source of truth.
- Archived debug artifact text is only used for pathless archive/legacy references.
- Diff mode is a read-only compact view with 3 context lines before and after each change.
- No DSL schema or WebSocket message shape changes are required.

## Implementation Plan

1. Extend reference workspace restore to read `rootPath + relativePath` for each manifest file; pathless references continue to restore from archived artifact text.
2. Add a compact `ShortwireDiffView` row builder based on `similar::TextDiff::grouped_ops(3)`.
3. Render compact read-only diff text when shader shortwire `Diff` is enabled, leaving the full editor untouched when disabled.
4. Reuse the same diff renderer in the reference editor, comparing `shortwire_base_source` to the reference editor source.
5. Add focused tests for local auto-load fallback and compact diff row generation.

## Status

- Implemented.
