use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use rust_wgpu_fiber::eframe::egui;
use serde::{Deserialize, Serialize};

use crate::app::{DiffStats, ShortwirePastedReferenceImage, ShortwireReferenceImage};
use crate::dsl::{DebugArtifactAnchor, DebugArtifactItem, DebugArtifactRole};
use crate::metric_log;
use crate::renderer::{PassDebugDependencyNode, PassDebugSource, PassDebugSourceRange};

const SIDE_PANEL_DEFAULT_WIDTH: f32 = 340.0;
const SIDE_PANEL_MIN_WIDTH: f32 = 220.0;
const SIDE_PANEL_MAX_WIDTH: f32 = 560.0;
const TREE_ROW_INDENT_WIDTH: f32 = 14.0;
const PASS_DEBUG_SPLIT_HANDLE_WIDTH: f32 = 6.0;
const PASS_DEBUG_SPLIT_LINE_WIDTH: f32 = 1.0;
const PASS_DEBUG_EDITOR_MIN_WIDTH: f32 = 320.0;
const PASS_DEBUG_COLUMN_HEADER_HEIGHT: f32 = 28.0;
const TREE_ROW_TRAILING_PADDING: f32 = 24.0;
const TREE_ROW_SOURCE_JUMP_GAP: f32 = 8.0;
const TREE_ROW_SOURCE_JUMP_LABEL: &str = "src";
const TREE_ROW_SOURCE_JUMP_HORIZONTAL_PADDING: f32 = 5.0;
const TREE_ROW_SOURCE_JUMP_VERTICAL_PADDING: f32 = 2.0;
const PASS_DEBUG_CLOSE_RESIZE_DELTA_THRESHOLD: f32 = 48.0;
const PASS_DEBUG_TREE_FONT_SIZE: f32 = 13.0;
const PASS_DEBUG_CODE_FONT_SIZE: f32 = 13.0;
const PASS_DEBUG_LINE_NUMBER_FONT_SIZE: f32 = 11.5;
const PASS_DEBUG_CODE_EDITOR_MARGIN_Y: i8 = 3;
const PASS_DEBUG_CODE_EDITOR_MARGIN_X: i8 = 6;
const PASS_DEBUG_LINE_NUMBER_GUTTER_MIN_WIDTH: f32 = 30.0;
const PASS_DEBUG_LINE_NUMBER_GUTTER_MAX_WIDTH: f32 = 96.0;
const PASS_DEBUG_LINE_NUMBER_GUTTER_DIGIT_WIDTH: f32 = 7.0;
const PASS_DEBUG_LINE_NUMBER_GUTTER_RIGHT_PADDING: f32 = 8.0;
const PASS_DEBUG_WINDOW_DEFAULT_WIDTH: f32 = 1480.0;
const PASS_DEBUG_WINDOW_DEFAULT_HEIGHT: f32 = 760.0;
const PASS_DEBUG_WINDOW_MIN_WIDTH: f32 = 960.0;
const PASS_DEBUG_WINDOW_MIN_HEIGHT: f32 = 360.0;
const PASS_DEBUG_REFERENCE_SYNC_DEBOUNCE_SECS: f64 = 0.250;
const PASS_DEBUG_PATCH_ERROR_SUMMARY_CHARS: usize = 140;
const PASS_DEBUG_REFERENCE_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const PASS_DEBUG_REFERENCE_MAX_FOLDER_FILES: usize = 512;
const SHORTWIRE_DIFF_CONTEXT_LINES: usize = 3;
const DEBUG_ARTIFACT_DEFAULT_SLOT: &str = "default";
const DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT: &str = "reference-workspace";
const DEBUG_ARTIFACT_REFERENCE_PATCHES_SLOT: &str = "reference-patches";
const DEBUG_ARTIFACT_REFERENCE_FILE_SLOT_PREFIX: &str = "file:";
const REFERENCE_WORKSPACE_VERSION: u32 = 1;

// --- Shortwire mode types ---

fn shortwire_patch_key(row: &PassDebugDependencyRow) -> String {
    let range_suffix = row
        .source_range
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

#[derive(Clone, Debug)]
struct ShortwireRowIdentity {
    patch_key: String,
    row_key_hint: String,
    label: String,
    #[allow(dead_code)]
    target_id: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ShortwireNodePatch {
    hunks: Vec<ShortwireHunk>,
    base_source_hash: u64,
    #[serde(
        rename = "referenceImage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    reference_image: Option<ShortwireReferenceImage>,
    #[serde(
        rename = "diffResult",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    diff_result: Option<ShortwireDiffResult>,
}

const SHORTWIRE_DIFF_PASS_MAX_AE: f32 = 2.0 / 255.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortwireDiffResult {
    pub metric: String,
    pub max_ae: f32,
    pub min: f32,
    pub avg: f32,
    pub rms: f32,
    pub p95_abs: f32,
    pub sample_count: u64,
    pub non_finite_count: u64,
    pub render_size: [u32; 2],
    pub reference_size: [u32; 2],
    pub reference_offset: [i32; 2],
}

impl ShortwireDiffResult {
    pub fn from_stats(
        stats: DiffStats,
        render_size: [u32; 2],
        reference_size: [u32; 2],
        reference_offset: [i32; 2],
    ) -> Option<Self> {
        if !stats.max.is_finite()
            || !stats.min.is_finite()
            || !stats.avg.is_finite()
            || !stats.rms.is_finite()
            || !stats.p95_abs.is_finite()
        {
            return None;
        }
        Some(Self {
            metric: "AE".to_string(),
            max_ae: stats.max,
            min: stats.min,
            avg: stats.avg,
            rms: stats.rms,
            p95_abs: stats.p95_abs,
            sample_count: stats.sample_count,
            non_finite_count: stats.non_finite_count,
            render_size,
            reference_size,
            reference_offset,
        })
    }
}

fn shortwire_diff_result_summary(diff_result: Option<&ShortwireDiffResult>) -> String {
    match diff_result {
        Some(result) => format!(
            "diff=max_ae:{:.6},min:{:.6},avg:{:.6},rms:{:.6},p95:{:.6},n:{},nonfinite:{},render:{}x{},ref:{}x{},offset:{},{}",
            result.max_ae,
            result.min,
            result.avg,
            result.rms,
            result.p95_abs,
            result.sample_count,
            result.non_finite_count,
            result.render_size[0],
            result.render_size[1],
            result.reference_size[0],
            result.reference_size[1],
            result.reference_offset[0],
            result.reference_offset[1],
        ),
        None => "diff:none".to_string(),
    }
}

fn shortwire_patch_summary(patch: Option<&ShortwireNodePatch>) -> String {
    match patch {
        Some(patch) => format!(
            "patch=hunks:{},base_hash:{},status:{:?},{}",
            patch.hunks.len(),
            patch.base_source_hash,
            shortwire_dot_info_for_patch(patch).status,
            shortwire_diff_result_summary(patch.diff_result.as_ref()),
        ),
        None => "patch:none".to_string(),
    }
}

fn shortwire_diff_status(diff_result: &ShortwireDiffResult) -> ShortwireDotStatus {
    if diff_result.max_ae < SHORTWIRE_DIFF_PASS_MAX_AE {
        ShortwireDotStatus::Passing
    } else {
        ShortwireDotStatus::Failing
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShortwireDiffCaptureRequest {
    pub pass_name: String,
    pub patch_key: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct ShortwireHunk {
    old_start: usize,
    old_lines: Vec<String>,
    new_lines: Vec<String>,
    context_before: Vec<String>,
    context_after: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShortwireDiffRowKind {
    Context,
    Added,
    Removed,
    Separator,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShortwireDiffRow {
    kind: ShortwireDiffRowKind,
    old_line: Option<usize>,
    new_line: Option<usize>,
    text: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ShortwireDiffView {
    rows: Vec<ShortwireDiffRow>,
    old_line_count: usize,
    new_line_count: usize,
}

impl ShortwireDiffView {
    fn to_display_text(&self) -> String {
        if self.rows.is_empty() {
            return "No changes\n".to_string();
        }

        let old_width = self.old_line_count.max(1).to_string().len();
        let new_width = self.new_line_count.max(1).to_string().len();
        let mut text = String::new();
        for row in &self.rows {
            if row.kind == ShortwireDiffRowKind::Separator {
                text.push_str(&format!(
                    "{:old_width$} {:new_width$}   ...\n",
                    "",
                    "",
                    old_width = old_width,
                    new_width = new_width
                ));
                continue;
            }

            let old_line = row
                .old_line
                .map(|line| line.to_string())
                .unwrap_or_default();
            let new_line = row
                .new_line
                .map(|line| line.to_string())
                .unwrap_or_default();
            let prefix = match row.kind {
                ShortwireDiffRowKind::Context => " ",
                ShortwireDiffRowKind::Added => "+",
                ShortwireDiffRowKind::Removed => "-",
                ShortwireDiffRowKind::Separator => unreachable!(),
            };
            text.push_str(&format!(
                "{old_line:>old_width$} {new_line:>new_width$} {prefix} {}\n",
                row.text,
                old_width = old_width,
                new_width = new_width
            ));
        }
        text
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ShortwirePatchesPayload {
    version: u32,
    patches: HashMap<String, ShortwireNodePatch>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShortwireDotStatus {
    PendingDiff,
    Passing,
    Failing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ShortwireDotInfo {
    status: ShortwireDotStatus,
    max_ae: Option<f32>,
    sample_count: Option<u64>,
}

#[derive(Clone, Debug)]
enum ShortwirePhase {
    Editing,
    PendingApply { pending_hunks: Vec<ShortwireHunk> },
    PendingResetThenEnter { next_identity: ShortwireRowIdentity },
}

#[derive(Clone, Debug)]
struct ShortwireActiveState {
    identity: ShortwireRowIdentity,
    base_source: String,
    base_source_hash: u64,
    base_source_stale: bool,
    diff_view_enabled: bool,
    reference_image: Option<ShortwireReferenceImage>,
    phase: ShortwirePhase,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceWorkspaceManifest {
    version: u32,
    root_path: Option<String>,
    root_label: String,
    selected_file: Option<String>,
    files: Vec<ReferenceWorkspaceManifestFile>,
    #[serde(default)]
    skipped_files: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReferenceWorkspaceManifestFile {
    relative_path: String,
    artifact_id: String,
    content_hash: String,
    size: u64,
}

#[derive(Clone, Debug)]
struct ReferenceWorkspaceFile {
    relative_path: String,
    artifact_id: String,
    source: String,
    loaded_source: String,
}

#[derive(Clone, Debug)]
struct ReferenceShortwireLocalFile {
    path: PathBuf,
    restore_source: String,
    wrote_patch: bool,
}

impl ReferenceWorkspaceFile {
    fn size(&self) -> u64 {
        self.source.len() as u64
    }

    fn content_hash(&self) -> String {
        debug_artifact_content_hash(self.source.as_bytes())
    }

    fn is_dirty(&self) -> bool {
        self.source != self.loaded_source
    }
}

#[derive(Clone, Debug)]
struct ReferenceWorkspaceState {
    root_path: Option<String>,
    root_label: String,
    selected_file: Option<String>,
    files: Vec<ReferenceWorkspaceFile>,
    editor_source: String,
    sync_due_secs: Option<f64>,
    last_status: Option<String>,
    skipped_files: usize,
    manifest_dirty: bool,
    pre_shortwire_source: Option<String>,
    shortwire_active_key: Option<String>,
    shortwire_base_source: Option<String>,
    pending_shortwire_patch: Option<(String, Vec<ShortwireHunk>)>,
    local_shortwire_file: Option<ReferenceShortwireLocalFile>,
}

impl Default for ReferenceWorkspaceState {
    fn default() -> Self {
        Self {
            root_path: None,
            root_label: "Reference".to_string(),
            selected_file: None,
            files: Vec::new(),
            editor_source: String::new(),
            sync_due_secs: None,
            last_status: None,
            skipped_files: 0,
            manifest_dirty: false,
            pre_shortwire_source: None,
            shortwire_active_key: None,
            shortwire_base_source: None,
            pending_shortwire_patch: None,
            local_shortwire_file: None,
        }
    }
}

impl ReferenceWorkspaceState {
    fn selected_file_index(&self) -> Option<usize> {
        let selected = self.selected_file.as_deref()?;
        self.files
            .iter()
            .position(|file| file.relative_path == selected)
    }

    fn selected_file(&self) -> Option<&ReferenceWorkspaceFile> {
        self.selected_file_index()
            .and_then(|index| self.files.get(index))
    }

    fn selected_file_mut(&mut self) -> Option<&mut ReferenceWorkspaceFile> {
        let index = self.selected_file_index()?;
        self.files.get_mut(index)
    }

    fn selected_local_path(&self) -> Option<PathBuf> {
        let root_path = self.root_path.as_deref()?;
        let selected_file = self.selected_file.as_deref()?;
        Some(PathBuf::from(root_path).join(selected_file))
    }

    fn selected_file_dirty(&self) -> bool {
        self.selected_file()
            .is_some_and(ReferenceWorkspaceFile::is_dirty)
    }

    fn has_dirty_files(&self) -> bool {
        self.files.iter().any(ReferenceWorkspaceFile::is_dirty)
    }

    fn has_content(&self) -> bool {
        !self.files.is_empty()
    }

    fn commit_editor_to_selected(&mut self) {
        let editor_source = self.editor_source.clone();
        if let Some(file) = self.selected_file_mut() {
            file.source = editor_source;
        }
    }

    fn select_file(&mut self, relative_path: &str) -> bool {
        if self.shortwire_active_key.is_some() {
            return false;
        }
        self.commit_editor_to_selected();
        let Some(file) = self
            .files
            .iter()
            .find(|file| file.relative_path == relative_path)
        else {
            return false;
        };
        self.selected_file = Some(file.relative_path.clone());
        self.editor_source = file.source.clone();
        self.manifest_dirty = true;
        true
    }

    fn replace_files(
        &mut self,
        root_path: Option<String>,
        root_label: String,
        files: Vec<ReferenceWorkspaceFile>,
        selected_file: Option<String>,
        skipped_files: usize,
        mark_dirty: bool,
    ) {
        self.root_path = root_path;
        self.root_label = root_label;
        self.files = files;
        self.files
            .sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        self.skipped_files = skipped_files;
        self.selected_file = selected_file
            .filter(|selected| {
                self.files
                    .iter()
                    .any(|file| &file.relative_path == selected)
            })
            .or_else(|| self.files.first().map(|file| file.relative_path.clone()));
        self.editor_source = self
            .selected_file()
            .map(|file| file.source.clone())
            .unwrap_or_default();
        self.sync_due_secs = None;
        self.last_status = None;
        self.manifest_dirty = mark_dirty;
        self.pre_shortwire_source = None;
        self.shortwire_active_key = None;
        self.shortwire_base_source = None;
        self.pending_shortwire_patch = None;
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ReferencePatchesPayload {
    version: u32,
    patches: HashMap<String, ShortwireNodePatch>,
}

#[derive(Clone, Debug)]
struct PassDebugMergeConflict {
    base_source: String,
    incoming_source: String,
    local_source: String,
    resolved_source: String,
    error: String,
    choice_popup_open: bool,
    resolver_window_open: bool,
}

#[derive(Clone, Debug)]
struct PassDebugPendingMergePatchUpdate {
    base_source: String,
    incoming_source: String,
    local_source: String,
    merged_source: String,
}

#[derive(Clone, Debug)]
enum HunkApplyError {
    HunkNotFound { hunk_index: usize },
    VerificationFailed { hunk_index: usize },
}

impl std::fmt::Display for HunkApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HunkApplyError::HunkNotFound { hunk_index } => {
                write!(f, "hunk {hunk_index}: could not locate target position")
            }
            HunkApplyError::VerificationFailed { hunk_index } => {
                write!(
                    f,
                    "hunk {hunk_index}: old lines do not match at resolved position"
                )
            }
        }
    }
}

fn hash_source(source: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = ahash::AHasher::default();
    source.hash(&mut hasher);
    hasher.finish()
}

trait PatchSourceArg {
    fn into_patch_source(self) -> Option<String>;
}

impl PatchSourceArg for bool {
    fn into_patch_source(self) -> Option<String> {
        debug_assert!(
            !self,
            "pass debug patch state should pass the actual patch source"
        );
        None
    }
}

impl<'a> PatchSourceArg for Option<&'a str> {
    fn into_patch_source(self) -> Option<String> {
        self.map(str::to_string)
    }
}

#[derive(Clone, Debug)]
pub enum PassDebugWindowAction {
    ApplyPatch {
        pass_name: String,
        source: String,
        reference_image: Option<ShortwireReferenceImage>,
    },
    ResetPatch {
        pass_name: String,
    },
    ResetAllPatches,
    UpsertDebugArtifact {
        item: DebugArtifactItem,
        content_text: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PassDebugDependencyRow {
    depth: usize,
    row_key: String,
    parent_row_key: Option<String>,
    label: String,
    relation_path: String,
    target_id: Option<String>,
    source_range: Option<PassDebugSourceRange>,
    source_jump_range: Option<PassDebugSourceRange>,
    selectable: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PassDebugTreeClick {
    row_key: Option<String>,
    target_id: Option<String>,
    source_range: Option<PassDebugSourceRange>,
    toggle_row_key: Option<String>,
}

#[derive(Clone, Debug)]
struct LineGalleyCache {
    wrap_width: f32,
    pixels_per_point: f32,
    line_hashes: Vec<u64>,
    line_sections: Vec<Vec<egui::text::LayoutSection>>,
    line_galleys: Vec<std::sync::Arc<egui::Galley>>,
    merged: std::sync::Arc<egui::Galley>,
}

#[derive(Clone, Debug)]
struct PassDebugExpandableRowsCache {
    rows_generation: u64,
    row_keys: HashSet<String>,
}

#[derive(Clone, Debug)]
struct PassDebugVisibleRowsCache {
    rows_generation: u64,
    expansion_generation: u64,
    row_indices: Vec<usize>,
}

#[derive(Clone, Debug)]
struct PassDebugTreeWidthCache {
    rows_generation: u64,
    intrinsic_width: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct PassDebugViewportSnapshot {
    inner_rect: Option<egui::Rect>,
    outer_rect: Option<egui::Rect>,
    monitor_size: Option<egui::Vec2>,
    native_pixels_per_point: Option<f32>,
    focused: Option<bool>,
    visible: Option<bool>,
}

impl PassDebugViewportSnapshot {
    fn from_info(info: &egui::ViewportInfo) -> Self {
        Self {
            inner_rect: info.inner_rect,
            outer_rect: info.outer_rect,
            monitor_size: info.monitor_size,
            native_pixels_per_point: info.native_pixels_per_point,
            focused: info.focused,
            visible: info.visible(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassDebugCloseDecision {
    Accept,
    Cancel(PassDebugCloseCancelReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PassDebugCloseCancelReason {
    FocusLost,
    Hidden,
    MonitorChanged,
    ScaleChanged,
    ViewportJumped,
}

trait PassDebugTreeRow {
    fn depth(&self) -> usize;
    fn row_key(&self) -> Option<&str>;
    fn label(&self) -> &str;
    fn relation_path(&self) -> Option<&str>;
    fn target_id(&self) -> Option<&str>;
    fn source_range(&self) -> Option<PassDebugSourceRange>;
    fn source_jump_range(&self) -> Option<PassDebugSourceRange>;
    fn selectable(&self) -> bool;
}

impl PassDebugTreeRow for PassDebugDependencyRow {
    fn depth(&self) -> usize {
        self.depth
    }

    fn row_key(&self) -> Option<&str> {
        Some(&self.row_key)
    }

    fn label(&self) -> &str {
        &self.label
    }

    fn relation_path(&self) -> Option<&str> {
        if self.relation_path.is_empty() {
            None
        } else {
            Some(&self.relation_path)
        }
    }

    fn target_id(&self) -> Option<&str> {
        self.target_id.as_deref()
    }

    fn source_range(&self) -> Option<PassDebugSourceRange> {
        self.source_range
    }

    fn source_jump_range(&self) -> Option<PassDebugSourceRange> {
        self.source_jump_range
    }

    fn selectable(&self) -> bool {
        self.selectable
    }
}

#[derive(Clone, Debug)]
pub struct PassDebugWindowDocument {
    pub pass_name: String,
    pub source: Option<PassDebugSource>,
    analysis_source: Option<PassDebugSource>,
    analysis_source_text: String,
    source_revision: Option<u64>,
    dependency_rows: Vec<PassDebugDependencyRow>,
    focused_target_id: Option<String>,
    focused_dependency_row_key: Option<String>,
    dependency_root_target_id: Option<String>,
    dependency_expanded_row_keys: HashSet<String>,
    pending_editor_jump: Option<PassDebugSourceRange>,
    pending_dependency_reveal_row_key: Option<String>,
    pub draft_source: String,
    loaded_source: String,
    reference_workspace: ReferenceWorkspaceState,
    reference_patches: HashMap<String, ShortwireNodePatch>,
    reference_patches_dirty: bool,
    reference_line_galley_cache: Option<LineGalleyCache>,
    draft_revision: u64,
    draft_analysis_due_secs: Option<f64>,
    line_galley_cache: Option<LineGalleyCache>,
    dependency_rows_generation: u64,
    dependency_expansion_generation: u64,
    dependency_expandable_row_keys_cache: Option<PassDebugExpandableRowsCache>,
    visible_dependency_row_indices_cache: Option<PassDebugVisibleRowsCache>,
    dependency_tree_width_cache: Option<PassDebugTreeWidthCache>,
    dirty: bool,
    patch_active: bool,
    last_error: Option<String>,
    last_status: Option<String>,
    // Shortwire state
    shortwire_patches: HashMap<String, ShortwireNodePatch>,
    shortwire_patches_dirty: bool,
    shortwire_active: Option<ShortwireActiveState>,
    shortwire_exit_on_apply: bool,
    generated_base_source: String,
    generated_base_source_hash: u64,
    merge_conflict: Option<PassDebugMergeConflict>,
    pending_merge_patch_update: Option<PassDebugPendingMergePatchUpdate>,
    filter_text: String,
}

impl PassDebugWindowDocument {
    fn new(
        pass_name: String,
        source: Option<PassDebugSource>,
        source_revision: u64,
        patch_source: impl PatchSourceArg,
    ) -> Self {
        let canonical_source = source
            .as_ref()
            .map(|s| s.module_source.clone())
            .unwrap_or_default();
        let patch_source = patch_source.into_patch_source();
        let patch_active = patch_source.is_some();
        let loaded_source = patch_source
            .as_deref()
            .unwrap_or(canonical_source.as_str())
            .to_string();
        let analysis_source = source.clone();
        let generated_base_source = canonical_source.clone();
        let generated_base_source_hash = hash_source(&generated_base_source);
        let mut document = Self {
            pass_name,
            source,
            analysis_source,
            analysis_source_text: canonical_source,
            source_revision: Some(source_revision),
            dependency_rows: Vec::new(),
            focused_target_id: None,
            focused_dependency_row_key: None,
            dependency_root_target_id: None,
            dependency_expanded_row_keys: HashSet::new(),
            pending_editor_jump: None,
            pending_dependency_reveal_row_key: None,
            draft_source: loaded_source.clone(),
            loaded_source,
            reference_workspace: ReferenceWorkspaceState::default(),
            reference_patches: HashMap::new(),
            reference_patches_dirty: false,
            reference_line_galley_cache: None,
            draft_revision: 0,
            draft_analysis_due_secs: None,
            line_galley_cache: None,
            dependency_rows_generation: 0,
            dependency_expansion_generation: 0,
            dependency_expandable_row_keys_cache: None,
            visible_dependency_row_indices_cache: None,
            dependency_tree_width_cache: None,
            dirty: false,
            patch_active,
            last_error: None,
            last_status: None,
            shortwire_patches: HashMap::new(),
            shortwire_patches_dirty: false,
            shortwire_active: None,
            shortwire_exit_on_apply: false,
            generated_base_source,
            generated_base_source_hash,
            merge_conflict: None,
            pending_merge_patch_update: None,
            filter_text: String::new(),
        };
        document.refresh_analysis_rows();
        document
    }

    fn replace_draft_source(&mut self, next_source: String) {
        if self.draft_source == next_source {
            return;
        }
        self.draft_source = next_source;
        self.invalidate_draft_render_cache();
    }

    fn invalidate_draft_render_cache(&mut self) {
        self.draft_revision = self.draft_revision.wrapping_add(1);
    }

    fn mark_draft_edited(&mut self, _now_secs: f64) {
        self.invalidate_draft_render_cache();
        self.dirty = self.draft_source != self.loaded_source;
        self.last_status = None;
        self.draft_analysis_due_secs = None;
    }

    fn update_reference_workspace(
        &mut self,
        workspace_text: Option<&str>,
        reference_files: &[crate::debug_artifacts::DebugArtifactTextSnapshot],
        legacy_reference_source: Option<&str>,
        reference_patches_text: Option<&str>,
    ) {
        self.restore_reference_patches_from_text(reference_patches_text);

        let Some((incoming, migrated_legacy)) = load_reference_workspace_from_artifacts(
            &self.pass_name,
            workspace_text,
            reference_files,
            legacy_reference_source,
        ) else {
            if !self.reference_workspace.has_dirty_files()
                && !self.reference_workspace.manifest_dirty
                && self.reference_workspace.shortwire_active_key.is_none()
                && self.reference_workspace.has_content()
            {
                self.reference_workspace = ReferenceWorkspaceState::default();
                self.reference_line_galley_cache = None;
            }
            return;
        };

        if self.reference_workspace.shortwire_active_key.is_some()
            || self.reference_workspace.has_dirty_files()
            || self.reference_workspace.manifest_dirty
        {
            self.acknowledge_reference_workspace_sync(&incoming);
            return;
        }

        if !reference_workspace_loaded_matches(&self.reference_workspace, &incoming) {
            self.reference_workspace = incoming;
            if migrated_legacy {
                self.reference_workspace.manifest_dirty = true;
                if let Some(file) = self.reference_workspace.selected_file_mut() {
                    file.loaded_source.clear();
                }
                self.reference_workspace.sync_due_secs = Some(0.0);
                self.reference_workspace.last_status =
                    Some("Migrating legacy reference".to_string());
            }
            self.reference_line_galley_cache = None;
        }
    }

    fn acknowledge_reference_workspace_sync(&mut self, incoming: &ReferenceWorkspaceState) {
        let mut any_ack = false;
        for incoming_file in &incoming.files {
            if let Some(current_file) = self
                .reference_workspace
                .files
                .iter_mut()
                .find(|file| file.relative_path == incoming_file.relative_path)
                && current_file.source == incoming_file.source
            {
                current_file.loaded_source = incoming_file.source.clone();
                any_ack = true;
            }
        }

        if self.reference_workspace.root_path == incoming.root_path
            && self.reference_workspace.root_label == incoming.root_label
            && self.reference_workspace.selected_file == incoming.selected_file
            && self.reference_workspace.files.len() == incoming.files.len()
        {
            self.reference_workspace.manifest_dirty = false;
        }

        if any_ack && !self.reference_workspace.has_dirty_files() {
            self.reference_workspace.sync_due_secs = None;
            self.reference_workspace.last_status = Some("Synced".to_string());
        }
    }

    fn mark_reference_edited(&mut self, now_secs: f64) {
        if self.reference_workspace.shortwire_active_key.is_some() {
            self.reference_workspace.last_status = Some("Reference patch draft".to_string());
            return;
        }

        self.reference_workspace.commit_editor_to_selected();
        if self.reference_workspace.selected_file().is_some() {
            self.reference_workspace.sync_due_secs =
                Some(now_secs + PASS_DEBUG_REFERENCE_SYNC_DEBOUNCE_SECS);
            self.reference_workspace.last_status = Some("Sync pending".to_string());
        }
    }

    fn maybe_emit_reference_upsert(
        &mut self,
        now_secs: f64,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        if self.reference_workspace.shortwire_active_key.is_some() {
            return;
        }
        let Some(due_secs) = self.reference_workspace.sync_due_secs else {
            return;
        };
        if now_secs < due_secs {
            return;
        }

        self.reference_workspace.sync_due_secs = None;
        let artifacts = self.take_reference_workspace_dirty_artifacts();
        if artifacts.is_empty() {
            self.reference_workspace.last_status = None;
            return;
        }

        for (item, content_text) in artifacts {
            push_action(
                pending_actions,
                PassDebugWindowAction::UpsertDebugArtifact { item, content_text },
            );
        }
        self.reference_workspace.last_status = Some("Syncing...".to_string());
    }

    fn maybe_refresh_pending_draft_analysis(&mut self, _now_secs: f64, _ctx: &egui::Context) {
        self.draft_analysis_due_secs = None;
    }

    #[cfg(test)]
    fn update_source(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_source: impl PatchSourceArg,
    ) {
        self.update_source_inner(source, source_revision, patch_source, None);
    }

    fn update_source_with_actions(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_source: impl PatchSourceArg,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        self.update_source_inner(source, source_revision, patch_source, Some(pending_actions));
    }

    fn update_source_inner(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_source: impl PatchSourceArg,
        pending_actions: Option<&Arc<Mutex<Vec<PassDebugWindowAction>>>>,
    ) {
        let patch_source = patch_source.into_patch_source();
        let patch_active = patch_source.is_some();
        let canonical_source_text = source.map(|s| s.module_source.as_str()).unwrap_or_default();
        let next_editor_source = patch_source
            .as_deref()
            .unwrap_or(canonical_source_text)
            .to_string();
        self.patch_active = patch_active;
        if self.source_revision == Some(source_revision) {
            if self.shortwire_active.is_none()
                && !self.dirty
                && patch_active
                && self.loaded_source != next_editor_source
            {
                self.loaded_source = next_editor_source.clone();
                self.replace_draft_source(next_editor_source);
                self.last_error = None;
            }
            return;
        }

        self.source_revision = Some(source_revision);
        self.source = source.cloned();

        if self.shortwire_active.is_some() {
            self.analysis_source = source.cloned();
            self.analysis_source_text = canonical_source_text.to_string();
            if canonical_source_text != self.generated_base_source {
                self.generated_base_source = canonical_source_text.to_string();
                self.generated_base_source_hash = hash_source(&self.generated_base_source);
                if let Some(ref mut active) = self.shortwire_active {
                    active.base_source_stale = true;
                }
            }
            return;
        }

        if patch_active
            && pending_actions.is_some()
            && canonical_source_text != self.generated_base_source
        {
            self.handle_patch_canonical_change(
                source,
                source_revision,
                canonical_source_text,
                patch_source.as_deref().unwrap_or_default(),
                pending_actions.expect("pending actions checked above"),
            );
            return;
        }

        if canonical_source_text != self.generated_base_source {
            self.generated_base_source = canonical_source_text.to_string();
            self.generated_base_source_hash = hash_source(&self.generated_base_source);
        }

        if !self.dirty {
            if source.is_none() {
                self.loaded_source.clear();
                self.replace_draft_source(String::new());
                self.analysis_source = None;
                self.analysis_source_text.clear();
                self.generated_base_source.clear();
                self.generated_base_source_hash = hash_source("");
                self.draft_analysis_due_secs = None;
                self.refresh_analysis_rows();
                self.last_error = None;
                return;
            }
            self.loaded_source = next_editor_source.clone();
            self.replace_draft_source(next_editor_source);
            self.last_error = None;
        }

        self.analysis_source = source.cloned();
        self.analysis_source_text = canonical_source_text.to_string();
        self.draft_analysis_due_secs = None;
        self.refresh_analysis_rows();
    }

    fn handle_patch_canonical_change(
        &mut self,
        source: Option<&PassDebugSource>,
        _source_revision: u64,
        incoming_source: &str,
        local_source: &str,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        let base_source = self.generated_base_source.clone();
        self.generated_base_source = incoming_source.to_string();
        self.generated_base_source_hash = hash_source(&self.generated_base_source);
        self.analysis_source = source.cloned();
        self.analysis_source_text = incoming_source.to_string();
        self.draft_analysis_due_secs = None;
        self.refresh_analysis_rows();

        match three_way_merge_sources(&base_source, incoming_source, local_source) {
            Ok(merged_source) => {
                self.merge_conflict = None;
                self.last_error = None;
                self.pending_merge_patch_update = Some(PassDebugPendingMergePatchUpdate {
                    base_source: base_source.clone(),
                    incoming_source: incoming_source.to_string(),
                    local_source: local_source.to_string(),
                    merged_source: merged_source.clone(),
                });
                if merged_source == incoming_source {
                    push_action(
                        pending_actions,
                        PassDebugWindowAction::ResetPatch {
                            pass_name: self.pass_name.clone(),
                        },
                    );
                    self.last_status =
                        Some("Canonical source changed — clearing empty patch".to_string());
                } else {
                    push_action(
                        pending_actions,
                        PassDebugWindowAction::ApplyPatch {
                            pass_name: self.pass_name.clone(),
                            source: merged_source,
                            reference_image: None,
                        },
                    );
                    self.last_status =
                        Some("Canonical source changed — rebasing patch...".to_string());
                }
            }
            Err(error) => {
                self.merge_conflict = Some(PassDebugMergeConflict {
                    base_source,
                    incoming_source: incoming_source.to_string(),
                    local_source: local_source.to_string(),
                    resolved_source: local_source.to_string(),
                    error: error.to_string(),
                    choice_popup_open: true,
                    resolver_window_open: false,
                });
                self.replace_draft_source(local_source.to_string());
                self.loaded_source = local_source.to_string();
                self.dirty = false;
                self.last_error = Some(
                    "Patch conflicts with updated generated shader — resolve merge".to_string(),
                );
                self.last_status = None;
            }
        }
    }

    fn mark_applied(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        draft_source: String,
        status: String,
    ) -> Option<ShortwireDiffCaptureRequest> {
        let is_shortwire_completion = matches!(
            self.shortwire_active.as_ref().map(|a| &a.phase),
            Some(ShortwirePhase::PendingApply { .. })
        );
        let mut diff_capture_request = None;
        let canonical_source_text = source
            .map(|s| s.module_source.as_str())
            .unwrap_or_default()
            .to_string();

        self.source_revision = Some(source_revision);
        self.source = source.cloned();
        self.loaded_source = draft_source.clone();
        self.replace_draft_source(draft_source);
        self.analysis_source = source.cloned();
        self.analysis_source_text = canonical_source_text.clone();
        self.generated_base_source = canonical_source_text;
        self.generated_base_source_hash = hash_source(&self.generated_base_source);
        self.draft_analysis_due_secs = None;
        let applied_source = self.loaded_source.clone();
        self.commit_pending_merge_patch_update(&applied_source);
        self.merge_conflict = None;

        if is_shortwire_completion {
            if let Some(ref mut active) = self.shortwire_active {
                if let ShortwirePhase::PendingApply { ref pending_hunks } = active.phase {
                    let patch_key = active.identity.patch_key.clone();
                    self.shortwire_patches.insert(
                        patch_key.clone(),
                        ShortwireNodePatch {
                            hunks: pending_hunks.clone(),
                            base_source_hash: self.generated_base_source_hash,
                            reference_image: active.reference_image.clone(),
                            diff_result: None,
                        },
                    );
                    self.shortwire_patches_dirty = true;
                    eprintln!(
                        "[shortwire-diff] apply_success_queue_capture pass={} patch_key={} hunks={} base_hash={} exit_on_apply={}",
                        self.pass_name,
                        patch_key,
                        pending_hunks.len(),
                        self.generated_base_source_hash,
                        self.shortwire_exit_on_apply,
                    );
                    diff_capture_request = Some(ShortwireDiffCaptureRequest {
                        pass_name: self.pass_name.clone(),
                        patch_key,
                    });
                }
            }
            let should_exit_shortwire = self.shortwire_exit_on_apply;
            self.commit_reference_shortwire_after_left_apply(should_exit_shortwire);
            if self.shortwire_exit_on_apply {
                self.shortwire_exit_on_apply = false;
                self.shortwire_active = None;
                self.refresh_analysis_rows();
            } else {
                if let Some(ref mut active) = self.shortwire_active {
                    active.base_source = self.generated_base_source.clone();
                    active.base_source_hash = self.generated_base_source_hash;
                    active.base_source_stale = false;
                    active.phase = ShortwirePhase::Editing;
                }
            }
        } else {
            self.refresh_analysis_rows();
            if let Some(active) = self.shortwire_active.take() {
                if let ShortwirePhase::PendingApply { pending_hunks } = active.phase {
                    self.shortwire_patches.insert(
                        active.identity.patch_key.clone(),
                        ShortwireNodePatch {
                            hunks: pending_hunks,
                            base_source_hash: self.generated_base_source_hash,
                            reference_image: active.reference_image,
                            diff_result: None,
                        },
                    );
                    self.shortwire_patches_dirty = true;
                    eprintln!(
                        "[shortwire-diff] apply_success_store_without_capture pass={} patch_key={} base_hash={}",
                        self.pass_name, active.identity.patch_key, self.generated_base_source_hash,
                    );
                }
            }
        }

        self.dirty = false;
        self.patch_active = true;
        self.last_error = None;
        self.last_status = Some(status);
        diff_capture_request
    }

    fn mark_reset(
        &mut self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        status: String,
    ) {
        self.source_revision = Some(source_revision);
        self.source = source.cloned();
        if let Some(source) = source {
            self.loaded_source = source.module_source.clone();
            self.replace_draft_source(source.module_source.clone());
            self.analysis_source = Some(source.clone());
            self.analysis_source_text = self.draft_source.clone();
            self.generated_base_source = source.module_source.clone();
            self.generated_base_source_hash = hash_source(&self.generated_base_source);
        } else {
            self.analysis_source = None;
            self.analysis_source_text.clear();
            self.generated_base_source.clear();
            self.generated_base_source_hash = hash_source("");
        }
        self.draft_analysis_due_secs = None;
        self.refresh_analysis_rows();
        self.dirty = false;
        self.patch_active = false;
        let reset_source = self.generated_base_source.clone();
        self.commit_pending_merge_patch_update(&reset_source);
        self.merge_conflict = None;
        self.last_error = None;
        self.last_status = Some(status);

        let pending_enter = matches!(
            self.shortwire_active.as_ref().map(|a| &a.phase),
            Some(ShortwirePhase::PendingResetThenEnter { .. })
        );
        if pending_enter {
            self.complete_shortwire_entry();
        }
    }

    fn refresh_draft_analysis(&mut self) {
        self.draft_analysis_due_secs = None;
    }

    fn refresh_analysis_rows(&mut self) {
        self.ensure_navigation_targets();
        self.refresh_dependency_rows();
    }

    fn invalidate_dependency_row_caches(&mut self) {
        self.dependency_rows_generation = self.dependency_rows_generation.wrapping_add(1);
        self.dependency_expandable_row_keys_cache = None;
        self.visible_dependency_row_indices_cache = None;
        self.dependency_tree_width_cache = None;
    }

    fn invalidate_dependency_visibility_cache(&mut self) {
        self.dependency_expansion_generation = self.dependency_expansion_generation.wrapping_add(1);
        self.visible_dependency_row_indices_cache = None;
    }

    fn ensure_navigation_targets(&mut self) {
        let Some(source) = self.analysis_source.as_ref() else {
            self.focused_target_id = None;
            self.focused_dependency_row_key = None;
            self.dependency_root_target_id = None;
            self.dependency_expanded_row_keys.clear();
            self.invalidate_dependency_visibility_cache();
            self.pending_editor_jump = None;
            self.pending_dependency_reveal_row_key = None;
            return;
        };

        let next_root_target_id = source
            .dependency_root_target_id
            .clone()
            .filter(|target_id| target_exists(source, Some(target_id)))
            .or_else(|| {
                source
                    .dependency_targets
                    .first()
                    .map(|target| target.id.clone())
            });
        let focused_target_exists = target_exists(source, self.focused_target_id.as_deref());
        let fallback_focus_target_id = next_root_target_id.clone().or_else(|| {
            source
                .dependency_targets
                .first()
                .map(|target| target.id.clone())
        });
        if self.dependency_root_target_id != next_root_target_id {
            self.dependency_root_target_id = next_root_target_id;
            self.reset_dependency_expansion_to_root();
        }

        if !focused_target_exists {
            self.focused_target_id = fallback_focus_target_id;
        }
    }

    fn refresh_dependency_rows(&mut self) {
        self.dependency_rows = self
            .analysis_source
            .as_ref()
            .and_then(|source| {
                self.dependency_root_target_id
                    .as_ref()
                    .and_then(|target_id| {
                        source
                            .dependency_trees
                            .get(target_id)
                            .map(|tree| flatten_dependency_tree(tree, source))
                    })
            })
            .unwrap_or_default();
        self.invalidate_dependency_row_caches();
        self.ensure_focused_dependency_row();
        self.prune_dependency_expansion();
        self.ensure_dependency_root_expanded();
    }

    fn focus_target(&mut self, target_id: impl Into<String>, _show_dependencies: bool) {
        self.focus_target_inner(target_id, true);
    }

    fn focus_target_from_editor(&mut self, target_id: impl Into<String>) {
        let target_id = target_id.into();
        self.focus_target_inner(target_id.clone(), false);
        if let Some(row_key) = self.shortest_dependency_row_key_for_target(&target_id) {
            self.focused_dependency_row_key = Some(row_key.clone());
            self.pending_dependency_reveal_row_key = Some(row_key.clone());
            self.reveal_dependency_row_key(&row_key, true);
        }
    }

    fn focus_target_inner(&mut self, target_id: impl Into<String>, jump_editor: bool) {
        let target_id = target_id.into();
        let Some(source) = self.analysis_source.as_ref() else {
            return;
        };
        if let Some(source_range) = source
            .dependency_targets
            .iter()
            .find(|target| target.id == target_id)
            .and_then(|target| target.source_range)
        {
            self.focused_target_id = Some(target_id.clone());
            if let Some(row_key) = self.shortest_dependency_row_key_for_target(&target_id) {
                self.focused_dependency_row_key = Some(row_key.clone());
                self.pending_dependency_reveal_row_key = Some(row_key.clone());
                self.reveal_dependency_row_key(&row_key, false);
            } else {
                self.focused_dependency_row_key = None;
            }
            if jump_editor {
                self.pending_editor_jump = Some(source_range);
            }
        } else if source
            .dependency_targets
            .iter()
            .any(|target| target.id == target_id)
        {
            self.focused_target_id = Some(target_id.clone());
            if let Some(row_key) = self.shortest_dependency_row_key_for_target(&target_id) {
                self.focused_dependency_row_key = Some(row_key.clone());
                self.pending_dependency_reveal_row_key = Some(row_key.clone());
                self.reveal_dependency_row_key(&row_key, false);
            } else {
                self.focused_dependency_row_key = None;
            }
        }
    }

    fn focus_tree_click(&mut self, click: PassDebugTreeClick, show_dependencies: bool) {
        if let Some(row_key) = click.toggle_row_key {
            self.toggle_dependency_row_expanded(&row_key);
        } else if let Some(row_key) = click.row_key {
            let jump_override = click.source_range;
            self.focus_dependency_row_key(
                row_key,
                show_dependencies,
                jump_override.is_none(),
                false,
            );
            if let Some(source_range) = jump_override {
                self.pending_editor_jump = Some(source_range);
            }
        } else if let Some(target_id) = click.target_id {
            self.focus_target(target_id, show_dependencies);
        } else if let Some(source_range) = click.source_range {
            self.pending_editor_jump = Some(source_range);
        }
    }

    fn focus_dependency_row_key(
        &mut self,
        row_key: impl Into<String>,
        _show_dependencies: bool,
        jump_editor: bool,
        reveal_row: bool,
    ) {
        let row_key = row_key.into();
        let Some(row) = self
            .dependency_rows
            .iter()
            .find(|row| row.row_key == row_key)
            .cloned()
        else {
            return;
        };
        self.focused_dependency_row_key = Some(row_key.clone());
        if reveal_row {
            self.pending_dependency_reveal_row_key = Some(row_key.clone());
            self.reveal_dependency_row_key(&row_key, false);
        }
        if let Some(target_id) = row.target_id {
            self.focused_target_id = Some(target_id.clone());
        }
        if jump_editor {
            self.pending_editor_jump = row.source_range;
        }
    }

    fn focus_target_at_char_index(&mut self, char_index: usize) {
        let byte_index = char_index_to_byte_index(&self.draft_source, char_index);
        let matching_dependency_row_key = self
            .dependency_rows
            .iter()
            .filter_map(|row| {
                let range = row.source_range?;
                if range.start_byte <= byte_index && byte_index < range.end_byte {
                    Some((
                        range.end_byte.saturating_sub(range.start_byte),
                        row.depth,
                        row.row_key.clone(),
                    ))
                } else {
                    None
                }
            })
            .min_by(
                |(left_len, left_depth, left_key), (right_len, right_depth, right_key)| {
                    right_depth
                        .cmp(left_depth)
                        .then_with(|| left_len.cmp(right_len))
                        .then_with(|| left_key.cmp(right_key))
                },
            )
            .map(|(_, _, row_key)| row_key);
        if let Some(row_key) = matching_dependency_row_key {
            self.focus_dependency_row_key(row_key, true, false, false);
            return;
        }

        let matching_target_id = self.analysis_source.as_ref().and_then(|source| {
            source
                .dependency_targets
                .iter()
                .find(|target| {
                    target
                        .source_range
                        .map(|range| range.start_byte <= byte_index && byte_index < range.end_byte)
                        .unwrap_or(false)
                })
                .map(|target| target.id.clone())
        });

        if let Some(target_id) = matching_target_id {
            self.focus_target_from_editor(target_id);
            return;
        }

        let matching_target_id =
            identifier_at_char_index(&self.draft_source, char_index).and_then(|identifier| {
                self.analysis_source.as_ref().and_then(|source| {
                    source
                        .dependency_targets
                        .iter()
                        .find(|target| target.name == identifier)
                        .map(|target| target.id.clone())
                })
            });
        if let Some(target_id) = matching_target_id {
            self.focus_target_from_editor(target_id);
        }
    }

    fn focused_source_range(&self) -> Option<PassDebugSourceRange> {
        if let Some(row_source_range) = self
            .focused_dependency_row_key
            .as_deref()
            .and_then(|row_key| {
                self.dependency_rows
                    .iter()
                    .find(|row| row.row_key == row_key)
            })
            .and_then(|row| row.source_range)
        {
            return Some(row_source_range);
        }

        let source = self.analysis_source.as_ref()?;
        let focused_target_id = self.focused_target_id.as_deref()?;
        source
            .dependency_targets
            .iter()
            .find(|target| target.id == focused_target_id)
            .and_then(|target| target.source_range)
    }

    fn focus_is_in_dependency_root(&self) -> bool {
        if self.focused_target_id.is_none() {
            return true;
        }
        let Some(row_key) = self.focused_dependency_row_key.as_deref() else {
            return false;
        };
        self.dependency_rows
            .iter()
            .any(|row| row.row_key == row_key)
    }

    fn ensure_focused_dependency_row(&mut self) {
        let focused_row_exists = self
            .focused_dependency_row_key
            .as_deref()
            .map(|row_key| {
                self.dependency_rows
                    .iter()
                    .any(|row| row.row_key == row_key)
            })
            .unwrap_or(false);
        if focused_row_exists {
            return;
        }

        self.focused_dependency_row_key = self
            .focused_target_id
            .as_deref()
            .and_then(|target_id| self.shortest_dependency_row_key_for_target(target_id));
    }

    fn shortest_dependency_row_key_for_target(&self, target_id: &str) -> Option<String> {
        self.dependency_rows
            .iter()
            .filter(|row| row.target_id.as_deref() == Some(target_id))
            .map(|row| (row.depth, row.row_key.clone()))
            .min_by(|(left_depth, left_key), (right_depth, right_key)| {
                left_depth
                    .cmp(right_depth)
                    .then_with(|| left_key.cmp(right_key))
            })
            .map(|(_, row_key)| row_key)
    }

    #[cfg(test)]
    fn dependency_expandable_row_keys(&self) -> HashSet<String> {
        self.compute_dependency_expandable_row_keys()
    }

    fn compute_dependency_expandable_row_keys(&self) -> HashSet<String> {
        self.dependency_rows
            .iter()
            .filter_map(|row| row.parent_row_key.clone())
            .collect()
    }

    fn ensure_dependency_expandable_row_keys_cache(&mut self) {
        let cache_valid = self
            .dependency_expandable_row_keys_cache
            .as_ref()
            .map(|cache| cache.rows_generation == self.dependency_rows_generation)
            .unwrap_or(false);
        if !cache_valid {
            self.dependency_expandable_row_keys_cache = Some(PassDebugExpandableRowsCache {
                rows_generation: self.dependency_rows_generation,
                row_keys: self.compute_dependency_expandable_row_keys(),
            });
        }
    }

    fn reset_dependency_expansion_to_root(&mut self) {
        self.dependency_expanded_row_keys.clear();
        self.invalidate_dependency_visibility_cache();
        self.ensure_dependency_root_expanded();
    }

    fn ensure_dependency_root_expanded(&mut self) {
        if let Some(root_row_key) = self.dependency_rows.first().map(|row| row.row_key.clone()) {
            if self.dependency_expanded_row_keys.insert(root_row_key) {
                self.invalidate_dependency_visibility_cache();
            }
        }
    }

    fn prune_dependency_expansion(&mut self) {
        let expandable_row_keys = self.compute_dependency_expandable_row_keys();
        let before_len = self.dependency_expanded_row_keys.len();
        self.dependency_expanded_row_keys
            .retain(|row_key| expandable_row_keys.contains(row_key));
        if self.dependency_expanded_row_keys.len() != before_len {
            self.invalidate_dependency_visibility_cache();
        }
    }

    fn toggle_dependency_row_expanded(&mut self, row_key: &str) {
        let expandable_row_keys = self.compute_dependency_expandable_row_keys();
        if !expandable_row_keys.contains(row_key) {
            return;
        }
        if !self.dependency_expanded_row_keys.remove(row_key) {
            self.dependency_expanded_row_keys
                .insert(row_key.to_string());
        }
        self.invalidate_dependency_visibility_cache();
    }

    fn reveal_dependency_row_key(&mut self, row_key: &str, collapse_to_path: bool) {
        let path = dependency_path_for_row_key(&self.dependency_rows, row_key);
        if path.is_empty() {
            return;
        }
        let expandable_row_keys = self.compute_dependency_expandable_row_keys();
        let ancestor_keys = path
            .iter()
            .take(path.len().saturating_sub(1))
            .filter(|row_key| expandable_row_keys.contains(*row_key))
            .cloned()
            .collect::<HashSet<_>>();
        let before = self.dependency_expanded_row_keys.clone();
        if collapse_to_path {
            self.dependency_expanded_row_keys = ancestor_keys;
        } else {
            self.dependency_expanded_row_keys.extend(ancestor_keys);
        }
        if self.dependency_expanded_row_keys != before {
            self.invalidate_dependency_visibility_cache();
        }
        self.ensure_dependency_root_expanded();
    }

    #[cfg(test)]
    fn visible_dependency_rows(&self) -> Vec<PassDebugDependencyRow> {
        self.compute_visible_dependency_row_indices()
            .into_iter()
            .map(|index| self.dependency_rows[index].clone())
            .collect()
    }

    fn cached_visible_dependency_row_indices(&mut self) -> &[usize] {
        let cache_valid = self
            .visible_dependency_row_indices_cache
            .as_ref()
            .map(|cache| {
                cache.rows_generation == self.dependency_rows_generation
                    && cache.expansion_generation == self.dependency_expansion_generation
            })
            .unwrap_or(false);
        if !cache_valid {
            self.visible_dependency_row_indices_cache = Some(PassDebugVisibleRowsCache {
                rows_generation: self.dependency_rows_generation,
                expansion_generation: self.dependency_expansion_generation,
                row_indices: self.compute_visible_dependency_row_indices(),
            });
        }
        &self
            .visible_dependency_row_indices_cache
            .as_ref()
            .expect("visible dependency row cache must be initialized")
            .row_indices
    }

    fn cached_dependency_tree_intrinsic_width(
        &mut self,
        ui: &egui::Ui,
        font_id: &egui::FontId,
    ) -> f32 {
        let cache_valid = self
            .dependency_tree_width_cache
            .as_ref()
            .map(|cache| cache.rows_generation == self.dependency_rows_generation)
            .unwrap_or(false);
        if !cache_valid {
            let text_color = ui.visuals().text_color();
            let source_jump_button_width = source_jump_button_size(ui, font_id).x;
            let intrinsic_width = self
                .dependency_rows
                .iter()
                .map(|row| {
                    let indent = row.depth as f32 * TREE_ROW_INDENT_WIDTH;
                    let toggle_slot = TREE_ROW_INDENT_WIDTH;
                    let label_width = ui
                        .painter()
                        .layout_no_wrap(row.label.clone(), font_id.clone(), text_color)
                        .size()
                        .x;
                    let source_jump_width = if row.source_jump_range.is_some() {
                        TREE_ROW_SOURCE_JUMP_GAP + source_jump_button_width
                    } else {
                        0.0
                    };
                    indent
                        + toggle_slot
                        + label_width
                        + source_jump_width
                        + TREE_ROW_TRAILING_PADDING
                })
                .fold(0.0, f32::max);
            self.dependency_tree_width_cache = Some(PassDebugTreeWidthCache {
                rows_generation: self.dependency_rows_generation,
                intrinsic_width,
            });
        }
        self.dependency_tree_width_cache
            .as_ref()
            .map(|cache| cache.intrinsic_width)
            .unwrap_or(0.0)
    }

    fn compute_visible_dependency_row_indices(&self) -> Vec<usize> {
        let mut visible_rows = Vec::new();
        let mut hidden_depth: Option<usize> = None;
        for (row_index, row) in self.dependency_rows.iter().enumerate() {
            if let Some(depth) = hidden_depth {
                if row.depth > depth {
                    continue;
                }
                hidden_depth = None;
            }

            visible_rows.push(row_index);
            if self
                .dependency_rows
                .iter()
                .any(|child| child.parent_row_key.as_deref() == Some(row.row_key.as_str()))
                && !self.dependency_expanded_row_keys.contains(&row.row_key)
            {
                hidden_depth = Some(row.depth);
            }
        }
        visible_rows
    }

    fn dependency_focus_path_row_keys(&self) -> Vec<String> {
        let Some(row_key) = self.focused_dependency_row_key.as_deref() else {
            return Vec::new();
        };
        dependency_path_for_row_key(&self.dependency_rows, row_key)
    }

    fn record_error(&mut self, error: String) {
        if let Some(ref active) = self.shortwire_active {
            match &active.phase {
                ShortwirePhase::PendingResetThenEnter { .. } => {
                    self.shortwire_active = None;
                    self.last_error = Some(format!("Failed to reset patch: {error}"));
                    self.last_status = None;
                    return;
                }
                ShortwirePhase::PendingApply { .. } => {
                    if let Some(ref mut active) = self.shortwire_active {
                        active.phase = ShortwirePhase::Editing;
                        active.diff_view_enabled = false;
                    }
                    self.shortwire_exit_on_apply = false;
                    self.dirty = self.draft_source != self.loaded_source;
                    self.last_error = Some(error);
                    self.last_status = None;
                    return;
                }
                ShortwirePhase::Editing => {}
            }
        }
        self.last_error = Some(error);
        self.last_status = None;
    }

    fn commit_pending_merge_patch_update(&mut self, applied_source: &str) {
        let Some(pending) = self.pending_merge_patch_update.take() else {
            return;
        };
        if pending.merged_source != applied_source {
            return;
        }

        let mut matching_keys = self
            .shortwire_patches
            .iter()
            .filter_map(|(key, patch)| {
                apply_hunks(&pending.base_source, &patch.hunks)
                    .ok()
                    .filter(|patched| patched == &pending.local_source)
                    .map(|_| key.clone())
            })
            .collect::<Vec<_>>();
        matching_keys.sort();
        matching_keys.dedup();

        if matching_keys.len() != 1 {
            return;
        }

        let key = matching_keys.remove(0);
        let next_hunks = compute_hunks(&pending.incoming_source, applied_source);
        if next_hunks.is_empty() {
            self.shortwire_patches.remove(&key);
        } else {
            let reference_image = self
                .shortwire_patches
                .get(&key)
                .and_then(|patch| patch.reference_image.clone());
            self.shortwire_patches.insert(
                key,
                ShortwireNodePatch {
                    hunks: next_hunks,
                    base_source_hash: hash_source(&pending.incoming_source),
                    reference_image,
                    diff_result: None,
                },
            );
        }
        self.shortwire_patches_dirty = true;
    }

    fn open_merge_resolver(&mut self) {
        if let Some(conflict) = self.merge_conflict.as_mut() {
            conflict.choice_popup_open = false;
            conflict.resolver_window_open = true;
        }
    }

    fn apply_merge_resolved(&mut self, pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>) {
        let Some(conflict) = self.merge_conflict.as_ref() else {
            return;
        };
        self.pending_merge_patch_update = Some(PassDebugPendingMergePatchUpdate {
            base_source: conflict.base_source.clone(),
            incoming_source: conflict.incoming_source.clone(),
            local_source: conflict.local_source.clone(),
            merged_source: conflict.resolved_source.clone(),
        });
        push_action(
            pending_actions,
            PassDebugWindowAction::ApplyPatch {
                pass_name: self.pass_name.clone(),
                source: conflict.resolved_source.clone(),
                reference_image: None,
            },
        );
        self.last_error = None;
        self.last_status = Some("Applying resolved shader...".to_string());
    }

    fn use_merge_incoming(&mut self, pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>) {
        let Some(conflict) = self.merge_conflict.as_ref() else {
            return;
        };
        self.pending_merge_patch_update = Some(PassDebugPendingMergePatchUpdate {
            base_source: conflict.base_source.clone(),
            incoming_source: conflict.incoming_source.clone(),
            local_source: conflict.local_source.clone(),
            merged_source: conflict.incoming_source.clone(),
        });
        push_action(
            pending_actions,
            PassDebugWindowAction::ResetPatch {
                pass_name: self.pass_name.clone(),
            },
        );
        self.last_error = None;
        self.last_status = Some("Using incoming generated shader...".to_string());
    }

    fn keep_merge_local(&mut self, pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>) {
        let Some(conflict) = self.merge_conflict.as_ref() else {
            return;
        };
        self.pending_merge_patch_update = Some(PassDebugPendingMergePatchUpdate {
            base_source: conflict.base_source.clone(),
            incoming_source: conflict.incoming_source.clone(),
            local_source: conflict.local_source.clone(),
            merged_source: conflict.local_source.clone(),
        });
        push_action(
            pending_actions,
            PassDebugWindowAction::ApplyPatch {
                pass_name: self.pass_name.clone(),
                source: conflict.local_source.clone(),
                reference_image: None,
            },
        );
        self.last_error = None;
        self.last_status = Some("Keeping local patch...".to_string());
    }

    fn cancel_merge_resolution(&mut self) {
        let Some(conflict) = self.merge_conflict.take() else {
            return;
        };
        self.loaded_source = conflict.local_source.clone();
        self.replace_draft_source(conflict.local_source);
        self.dirty = false;
        self.pending_merge_patch_update = None;
        self.last_error = None;
        self.last_status = Some("Merge resolution cancelled".to_string());
    }

    #[cfg(test)]
    fn import_reference_file_from_path(&mut self, path: &Path, now_secs: f64) {
        if self.reference_workspace.shortwire_active_key.is_some() {
            self.reference_workspace.last_status =
                Some("Close shortwire before opening a file".to_string());
            return;
        }
        let Some(parent) = path.parent() else {
            self.reference_workspace.last_status =
                Some("Cannot open file without parent".to_string());
            return;
        };
        match read_reference_file(path, parent, &self.pass_name, true) {
            Ok(file) => {
                let selected_file = Some(file.relative_path.clone());
                self.reference_workspace.replace_files(
                    Some(parent.to_string_lossy().to_string()),
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Reference file")
                        .to_string(),
                    vec![file],
                    selected_file,
                    0,
                    true,
                );
                self.reference_workspace.sync_due_secs = Some(now_secs);
                self.reference_workspace.last_status = Some("File imported".to_string());
                self.reference_line_galley_cache = None;
            }
            Err(error) => {
                self.reference_workspace.last_status = Some(error);
            }
        }
    }

    fn import_reference_folder_from_path(&mut self, path: &Path, now_secs: f64) {
        if self.reference_workspace.shortwire_active_key.is_some() {
            self.reference_workspace.last_status =
                Some("Close shortwire before opening a folder".to_string());
            return;
        }
        match read_reference_folder(path, &self.pass_name, true) {
            Ok((files, skipped_files)) => {
                if files.is_empty() {
                    self.reference_workspace.last_status =
                        Some("No UTF-8 text files found".to_string());
                    return;
                }
                self.reference_workspace.replace_files(
                    Some(path.to_string_lossy().to_string()),
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("Reference folder")
                        .to_string(),
                    files,
                    None,
                    skipped_files,
                    true,
                );
                self.reference_workspace.sync_due_secs = Some(now_secs);
                self.reference_workspace.last_status =
                    Some(format!("Folder imported ({skipped_files} skipped)"));
                self.reference_line_galley_cache = None;
            }
            Err(error) => {
                self.reference_workspace.last_status = Some(error);
            }
        }
    }

    fn reload_reference_workspace(&mut self, now_secs: f64) {
        if self.reference_workspace.shortwire_active_key.is_some() {
            self.reference_workspace.last_status =
                Some("Close shortwire before reloading".to_string());
            return;
        }
        let Some(root_path) = self.reference_workspace.root_path.clone() else {
            self.reference_workspace.last_status = Some("No local path to reload".to_string());
            return;
        };
        let root = PathBuf::from(root_path);
        if !root.exists() {
            self.reference_workspace.last_status =
                Some("Local path missing — keeping archive snapshot".to_string());
            return;
        }

        if self.reference_workspace.files.len() <= 1 {
            let Some(relative_path) = self.reference_workspace.selected_file.clone() else {
                self.reference_workspace.last_status =
                    Some("No selected file to reload".to_string());
                return;
            };
            let path = root.join(&relative_path);
            match read_reference_file(&path, &root, &self.pass_name, true) {
                Ok(file) => {
                    self.reference_workspace.replace_files(
                        Some(root.to_string_lossy().to_string()),
                        self.reference_workspace.root_label.clone(),
                        vec![file],
                        Some(relative_path),
                        0,
                        true,
                    );
                    self.reference_workspace.sync_due_secs = Some(now_secs);
                    self.reference_workspace.last_status = Some("File reloaded".to_string());
                    self.reference_line_galley_cache = None;
                }
                Err(error) => {
                    self.reference_workspace.last_status = Some(format!("Reload failed — {error}"));
                }
            }
            return;
        }

        match read_reference_folder(&root, &self.pass_name, true) {
            Ok((files, skipped_files)) => {
                if files.is_empty() {
                    self.reference_workspace.last_status =
                        Some("Reload found no UTF-8 text files".to_string());
                    return;
                }
                let selected_file = self.reference_workspace.selected_file.clone();
                self.reference_workspace.replace_files(
                    Some(root.to_string_lossy().to_string()),
                    self.reference_workspace.root_label.clone(),
                    files,
                    selected_file,
                    skipped_files,
                    true,
                );
                self.reference_workspace.sync_due_secs = Some(now_secs);
                self.reference_workspace.last_status =
                    Some(format!("Folder reloaded ({skipped_files} skipped)"));
                self.reference_line_galley_cache = None;
            }
            Err(error) => {
                self.reference_workspace.last_status = Some(format!("Reload failed — {error}"));
            }
        }
    }

    fn take_reference_workspace_dirty_artifacts(&mut self) -> Vec<(DebugArtifactItem, String)> {
        self.reference_workspace.commit_editor_to_selected();
        let mut artifacts = Vec::new();

        if let Some(root_path) = self.reference_workspace.root_path.clone() {
            let root = PathBuf::from(root_path);
            let mut wrote_any_file = false;
            let mut had_write_error = false;
            for file in &mut self.reference_workspace.files {
                if !file.is_dirty() {
                    continue;
                }
                match write_reference_workspace_file(&root, file) {
                    Ok(()) => {
                        file.loaded_source = file.source.clone();
                        wrote_any_file = true;
                    }
                    Err(error) => {
                        had_write_error = true;
                        self.reference_workspace.last_status = Some(error);
                    }
                }
            }

            if wrote_any_file {
                self.reference_workspace.manifest_dirty = true;
            }
            if self.reference_workspace.manifest_dirty {
                if let Some((item, content_text)) = self.collect_reference_workspace_artifact() {
                    artifacts.push((item, content_text));
                }
                self.reference_workspace.manifest_dirty = had_write_error;
            }
            return artifacts;
        }

        if self.reference_workspace.manifest_dirty {
            if let Some((item, content_text)) = self.collect_reference_workspace_artifact() {
                artifacts.push((item, content_text));
            }
            self.reference_workspace.manifest_dirty = false;
        }

        let pass_name = self.pass_name.clone();
        for file in &mut self.reference_workspace.files {
            if !file.is_dirty() {
                continue;
            }
            let item = pass_reference_file_artifact_item(&pass_name, file);
            let content_text = file.source.clone();
            file.loaded_source = file.source.clone();
            artifacts.push((item, content_text));
        }

        artifacts
    }

    fn collect_reference_workspace_artifact(&self) -> Option<(DebugArtifactItem, String)> {
        let manifest = ReferenceWorkspaceManifest {
            version: REFERENCE_WORKSPACE_VERSION,
            root_path: self.reference_workspace.root_path.clone(),
            root_label: self.reference_workspace.root_label.clone(),
            selected_file: self.reference_workspace.selected_file.clone(),
            files: self
                .reference_workspace
                .files
                .iter()
                .map(|file| ReferenceWorkspaceManifestFile {
                    relative_path: file.relative_path.clone(),
                    artifact_id: file.artifact_id.clone(),
                    content_hash: file.content_hash(),
                    size: file.size(),
                })
                .collect(),
            skipped_files: self.reference_workspace.skipped_files,
        };
        let content_text = serde_json::to_string(&manifest).ok()?;
        let artifact_id = pass_reference_workspace_artifact_id(&self.pass_name);
        let file_name = format!(
            "{}.reference-workspace.json",
            safe_debug_artifact_segment(&self.pass_name, "pass")
        );
        let item = DebugArtifactItem {
            id: artifact_id.clone(),
            anchor: DebugArtifactAnchor::Pass {
                pass_name: self.pass_name.clone(),
            },
            role: DebugArtifactRole::Attachment,
            name: "Reference workspace".to_string(),
            mime_type: "text/plain".to_string(),
            path: format!(
                "debug-artifacts/{}/{}",
                safe_debug_artifact_segment(&artifact_id, "artifact"),
                safe_debug_artifact_segment(&file_name, "artifact.json")
            ),
            size: Some(content_text.len() as u64),
            content_hash: Some(debug_artifact_content_hash(content_text.as_bytes())),
            slot_key: Some(DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT.to_string()),
        };
        Some((item, content_text))
    }

    fn restore_reference_patches_from_text(&mut self, text: Option<&str>) {
        if self.reference_patches_dirty {
            return;
        }
        let Some(text) = text else {
            return;
        };
        let Ok(payload) = serde_json::from_str::<ReferencePatchesPayload>(text) else {
            return;
        };
        if payload.version != REFERENCE_WORKSPACE_VERSION {
            return;
        }
        self.reference_patches = payload.patches;
    }

    fn collect_reference_patches_artifact(&self) -> Option<(DebugArtifactItem, String)> {
        let payload = ReferencePatchesPayload {
            version: REFERENCE_WORKSPACE_VERSION,
            patches: self.reference_patches.clone(),
        };
        let content_text = serde_json::to_string(&payload).ok()?;
        let artifact_id = pass_reference_patches_artifact_id(&self.pass_name);
        let file_name = format!(
            "{}.reference-patches.json",
            safe_debug_artifact_segment(&self.pass_name, "pass")
        );
        let item = DebugArtifactItem {
            id: artifact_id.clone(),
            anchor: DebugArtifactAnchor::Pass {
                pass_name: self.pass_name.clone(),
            },
            role: DebugArtifactRole::Patch,
            name: "Reference shortwire patches".to_string(),
            mime_type: "text/plain".to_string(),
            path: format!(
                "debug-artifacts/{}/{}",
                safe_debug_artifact_segment(&artifact_id, "artifact"),
                safe_debug_artifact_segment(&file_name, "artifact.json")
            ),
            size: Some(content_text.len() as u64),
            content_hash: Some(debug_artifact_content_hash(content_text.as_bytes())),
            slot_key: Some(DEBUG_ARTIFACT_REFERENCE_PATCHES_SLOT.to_string()),
        };
        Some((item, content_text))
    }

    fn take_reference_patches_dirty_artifact(&mut self) -> Option<(DebugArtifactItem, String)> {
        if !self.reference_patches_dirty {
            return None;
        }
        self.reference_patches_dirty = false;
        self.collect_reference_patches_artifact()
    }

    fn enter_reference_shortwire(&mut self, identity: &ShortwireRowIdentity) {
        if self.reference_workspace.shortwire_active_key.is_some() {
            return;
        }
        self.reference_workspace.commit_editor_to_selected();
        let Some(file) = self.reference_workspace.selected_file() else {
            return;
        };
        let relative_path = file.relative_path.clone();
        let base_source = file.source.clone();
        let patch_key = reference_shortwire_patch_key(&relative_path, &identity.patch_key);
        let mut draft = base_source.clone();
        let mut applied_stored_patch = false;

        if let Some(patch) = self.reference_patches.get(&patch_key) {
            match apply_hunks(&base_source, &patch.hunks) {
                Ok(patched) => {
                    draft = patched;
                    applied_stored_patch = true;
                }
                Err(_) => {
                    self.reference_patches.remove(&patch_key);
                    self.reference_patches_dirty = true;
                    self.reference_workspace.last_status =
                        Some("Reference patch outdated — removed".to_string());
                }
            }
        }

        self.reference_workspace.pre_shortwire_source = Some(base_source.clone());
        self.reference_workspace.shortwire_active_key = Some(patch_key);
        self.reference_workspace.shortwire_base_source = Some(base_source);
        self.reference_workspace.pending_shortwire_patch = None;
        self.reference_workspace.local_shortwire_file = self
            .reference_workspace
            .selected_local_path()
            .and_then(|path| match fs::read_to_string(&path) {
                Ok(restore_source) => Some(ReferenceShortwireLocalFile {
                    path,
                    restore_source,
                    wrote_patch: false,
                }),
                Err(error) => {
                    self.reference_workspace.last_status =
                        Some(format!("Reference local restore unavailable: {error}"));
                    None
                }
            });
        self.reference_workspace.editor_source = draft;
        if applied_stored_patch {
            self.write_reference_shortwire_to_local_file();
        }
        self.reference_line_galley_cache = None;
    }

    fn prepare_reference_shortwire_save(&mut self) {
        let Some(patch_key) = self.reference_workspace.shortwire_active_key.clone() else {
            return;
        };
        let Some(base_source) = self.reference_workspace.shortwire_base_source.clone() else {
            return;
        };
        let hunks = compute_hunks(&base_source, &self.reference_workspace.editor_source);
        self.reference_workspace.pending_shortwire_patch = Some((patch_key, hunks));
    }

    fn commit_reference_shortwire_after_left_apply(&mut self, restore_after: bool) {
        let mut should_write_local_file = false;
        if let Some((patch_key, hunks)) = self.reference_workspace.pending_shortwire_patch.take() {
            should_write_local_file = !hunks.is_empty()
                || self
                    .reference_workspace
                    .local_shortwire_file
                    .as_ref()
                    .is_some_and(|local| local.wrote_patch);
            if hunks.is_empty() {
                self.reference_patches.remove(&patch_key);
            } else {
                let base_hash = self
                    .reference_workspace
                    .shortwire_base_source
                    .as_deref()
                    .map(hash_source)
                    .unwrap_or_else(|| hash_source(""));
                self.reference_patches.insert(
                    patch_key,
                    ShortwireNodePatch {
                        hunks,
                        base_source_hash: base_hash,
                        reference_image: None,
                        diff_result: None,
                    },
                );
            }
            self.reference_patches_dirty = true;
        }
        if restore_after {
            self.restore_reference_after_shortwire();
        } else if should_write_local_file {
            self.write_reference_shortwire_to_local_file();
        }
    }

    fn write_reference_shortwire_to_local_file(&mut self) {
        let Some(local_path) = self
            .reference_workspace
            .local_shortwire_file
            .as_ref()
            .map(|local| local.path.clone())
        else {
            return;
        };
        let content = self.reference_workspace.editor_source.clone();
        match fs::write(&local_path, content) {
            Ok(()) => {
                if let Some(local) = self.reference_workspace.local_shortwire_file.as_mut() {
                    local.wrote_patch = true;
                }
                self.reference_workspace.last_status =
                    Some(format!("Wrote {}", local_path.display()));
            }
            Err(error) => {
                self.reference_workspace.last_status =
                    Some(format!("Reference file write failed: {error}"));
            }
        }
    }

    fn restore_reference_shortwire_local_file(&mut self) {
        let Some(local) = self.reference_workspace.local_shortwire_file.take() else {
            return;
        };
        if !local.wrote_patch {
            return;
        }
        if let Err(error) = fs::write(&local.path, local.restore_source) {
            let message = format!("Reference file restore failed: {error}");
            eprintln!("[pass-debug] {message}");
            self.reference_workspace.last_status = Some(message);
        }
    }

    fn save_and_exit_reference_shortwire_without_left_apply(&mut self) {
        self.prepare_reference_shortwire_save();
        if let Some((patch_key, hunks)) = self.reference_workspace.pending_shortwire_patch.take() {
            if !hunks.is_empty() {
                let base_hash = self
                    .reference_workspace
                    .shortwire_base_source
                    .as_deref()
                    .map(hash_source)
                    .unwrap_or_else(|| hash_source(""));
                self.reference_patches.insert(
                    patch_key,
                    ShortwireNodePatch {
                        hunks,
                        base_source_hash: base_hash,
                        reference_image: None,
                        diff_result: None,
                    },
                );
                self.reference_patches_dirty = true;
            }
        }
        self.restore_reference_after_shortwire();
    }

    #[cfg(test)]
    fn cancel_reference_shortwire_without_save(&mut self) {
        self.reference_workspace.pending_shortwire_patch = None;
        self.restore_reference_after_shortwire();
    }

    fn restore_reference_after_shortwire(&mut self) {
        self.restore_reference_shortwire_local_file();
        let Some(pre_shortwire_source) = self.reference_workspace.pre_shortwire_source.take()
        else {
            return;
        };
        self.reference_workspace.editor_source = pre_shortwire_source.clone();
        if let Some(file) = self.reference_workspace.selected_file_mut() {
            file.source = pre_shortwire_source;
        }
        self.reference_workspace.shortwire_active_key = None;
        self.reference_workspace.shortwire_base_source = None;
        self.reference_workspace.pending_shortwire_patch = None;
        self.reference_line_galley_cache = None;
    }

    // --- Shortwire methods ---

    fn enter_shortwire(
        &mut self,
        row: &PassDebugDependencyRow,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        if self.shortwire_active.is_some() || self.generated_base_source.is_empty() {
            return;
        }

        let identity = ShortwireRowIdentity {
            patch_key: shortwire_patch_key(row),
            row_key_hint: row.row_key.clone(),
            label: row.label.clone(),
            target_id: row.target_id.clone(),
        };
        eprintln!(
            "[shortwire-diff] enter_shortwire pass={} label={} target={:?} patch_key={} patch_active={} base_empty={} {}",
            self.pass_name,
            identity.label,
            identity.target_id,
            identity.patch_key,
            self.patch_active,
            self.generated_base_source.is_empty(),
            shortwire_patch_summary(self.shortwire_patches.get(&identity.patch_key)),
        );
        let existing_reference_image = self
            .shortwire_patches
            .get(&identity.patch_key)
            .and_then(|patch| patch.reference_image.clone());

        if self.patch_active {
            self.shortwire_active = Some(ShortwireActiveState {
                identity: identity.clone(),
                base_source: String::new(),
                base_source_hash: 0,
                base_source_stale: false,
                diff_view_enabled: false,
                reference_image: existing_reference_image,
                phase: ShortwirePhase::PendingResetThenEnter {
                    next_identity: identity,
                },
            });
            push_action(
                pending_actions,
                PassDebugWindowAction::ResetPatch {
                    pass_name: self.pass_name.clone(),
                },
            );
            self.last_status = Some("Resetting...".to_string());
        } else {
            self.shortwire_active = Some(ShortwireActiveState {
                identity,
                base_source: self.generated_base_source.clone(),
                base_source_hash: self.generated_base_source_hash,
                base_source_stale: false,
                diff_view_enabled: false,
                reference_image: existing_reference_image,
                phase: ShortwirePhase::Editing,
            });
            self.complete_shortwire_entry();
        }
    }

    fn complete_shortwire_entry(&mut self) {
        let identity = {
            let Some(ref mut active) = self.shortwire_active else {
                return;
            };

            let identity = match &active.phase {
                ShortwirePhase::PendingResetThenEnter { next_identity } => next_identity.clone(),
                ShortwirePhase::Editing => active.identity.clone(),
                _ => return,
            };

            active.identity = identity.clone();
            active.base_source = self.generated_base_source.clone();
            active.base_source_hash = self.generated_base_source_hash;
            active.base_source_stale = false;
            active.phase = ShortwirePhase::Editing;
            identity
        };
        self.enter_reference_shortwire(&identity);

        let mut draft = self.generated_base_source.clone();
        eprintln!(
            "[shortwire-diff] complete_entry pass={} patch_key={} current_base_hash={} {}",
            self.pass_name,
            identity.patch_key,
            self.generated_base_source_hash,
            shortwire_patch_summary(self.shortwire_patches.get(&identity.patch_key)),
        );
        if let Some(patch) = self.shortwire_patches.get(&identity.patch_key) {
            if patch.base_source_hash == self.generated_base_source_hash {
                match apply_hunks(&self.generated_base_source, &patch.hunks) {
                    Ok(patched) => {
                        draft = patched;
                        if let Some(ref mut active) = self.shortwire_active {
                            active.diff_view_enabled = true;
                        }
                    }
                    Err(_) => {
                        self.shortwire_patches.remove(&identity.patch_key);
                        self.shortwire_patches_dirty = true;
                        self.last_error =
                            Some("Shortwire patch outdated — base shader changed".to_string());
                    }
                }
            } else {
                match apply_hunks(&self.generated_base_source, &patch.hunks) {
                    Ok(patched) => {
                        draft = patched;
                        if let Some(ref mut active) = self.shortwire_active {
                            active.diff_view_enabled = true;
                        }
                    }
                    Err(_) => {
                        self.shortwire_patches.remove(&identity.patch_key);
                        self.shortwire_patches_dirty = true;
                        self.last_error =
                            Some("Shortwire patch outdated — base shader changed".to_string());
                    }
                }
            }
        }

        self.replace_draft_source(draft);
        self.dirty = self.draft_source != self.loaded_source;
        self.draft_analysis_due_secs = None;
        self.last_status = None;
        eprintln!(
            "[shortwire-diff] active pass={} label={} target={:?} patch_key={} draft_len={} dirty={}",
            self.pass_name,
            identity.label,
            identity.target_id,
            identity.patch_key,
            self.draft_source.len(),
            self.dirty,
        );
    }

    fn exit_shortwire_done(&mut self, pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>) {
        let Some(ref active) = self.shortwire_active else {
            return;
        };
        if !matches!(active.phase, ShortwirePhase::Editing) {
            return;
        }

        let mut final_draft = self.draft_source.clone();
        let base_source_stale = active.base_source_stale;
        let base_source = active.base_source.clone();
        let active_label = active.identity.label.clone();
        let active_target_id = active.identity.target_id.clone();

        if base_source_stale {
            let user_hunks = compute_hunks(&base_source, &self.draft_source);
            if user_hunks.is_empty() {
                final_draft = self.generated_base_source.clone();
            } else {
                match apply_hunks(&self.generated_base_source, &user_hunks) {
                    Ok(rebased) => {
                        final_draft = rebased;
                    }
                    Err(_) => {
                        self.last_error = Some(
                            "Cannot rebase onto new base — resolve conflicts manually".to_string(),
                        );
                        return;
                    }
                }
            }
            self.replace_draft_source(final_draft.clone());
        }

        let final_hunks = compute_hunks(&self.generated_base_source, &final_draft);
        eprintln!(
            "[shortwire-diff] save_apply pass={} label={} target={:?} hunks={} base_stale={} previous_{}",
            self.pass_name,
            active_label,
            active_target_id,
            final_hunks.len(),
            base_source_stale,
            shortwire_patch_summary(
                self.shortwire_active
                    .as_ref()
                    .and_then(|active| self.shortwire_patches.get(&active.identity.patch_key)),
            ),
        );
        self.prepare_reference_shortwire_save();
        self.shortwire_exit_on_apply = false;
        if let Some(ref mut active) = self.shortwire_active {
            active.phase = ShortwirePhase::PendingApply {
                pending_hunks: final_hunks,
            };
        }

        push_action(
            pending_actions,
            PassDebugWindowAction::ApplyPatch {
                pass_name: self.pass_name.clone(),
                source: final_draft,
                reference_image: self
                    .shortwire_active
                    .as_ref()
                    .and_then(|active| active.reference_image.clone()),
            },
        );
        self.last_error = None;
        self.last_status = Some("Saving...".to_string());
    }

    #[cfg(test)]
    fn exit_shortwire_cancel(&mut self) {
        self.cancel_reference_shortwire_without_save();
        self.shortwire_active = None;
        self.replace_draft_source(self.generated_base_source.clone());
        self.dirty = self.draft_source != self.loaded_source;
        self.refresh_analysis_rows();
        self.last_error = None;
        self.last_status = None;
    }

    fn exit_shortwire_navigate(
        &mut self,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        let Some(active) = self.shortwire_active.clone() else {
            return;
        };
        eprintln!(
            "[shortwire-diff] close pass={} label={} target={:?} phase={:?}",
            self.pass_name, active.identity.label, active.identity.target_id, active.phase,
        );

        match &active.phase {
            ShortwirePhase::Editing => {
                self.save_and_exit_reference_shortwire_without_left_apply();
                let hunks = compute_hunks(&self.generated_base_source, &self.draft_source);
                if !hunks.is_empty() || active.reference_image.is_some() {
                    let patch_key = active.identity.patch_key.clone();
                    let previous_patch = self.shortwire_patches.get(&patch_key);
                    let preserved_diff_result = previous_patch
                        .filter(|patch| {
                            patch.base_source_hash == self.generated_base_source_hash
                                && patch.hunks == hunks
                        })
                        .and_then(|patch| patch.diff_result.clone());
                    let reference_image = active
                        .reference_image
                        .clone()
                        .or_else(|| previous_patch.and_then(|patch| patch.reference_image.clone()));
                    eprintln!(
                        "[shortwire-diff] close_store_pending pass={} patch_key={} hunks={} base_hash={} preserved_diff={} previous_{}",
                        self.pass_name,
                        patch_key,
                        hunks.len(),
                        self.generated_base_source_hash,
                        preserved_diff_result.is_some(),
                        shortwire_patch_summary(self.shortwire_patches.get(&patch_key)),
                    );
                    self.shortwire_patches.insert(
                        patch_key,
                        ShortwireNodePatch {
                            hunks,
                            base_source_hash: self.generated_base_source_hash,
                            reference_image,
                            diff_result: preserved_diff_result,
                        },
                    );
                    self.shortwire_patches_dirty = true;
                }
                self.shortwire_active = None;
                self.loaded_source = self.generated_base_source.clone();
                self.replace_draft_source(self.generated_base_source.clone());
                self.dirty = false;
                if self.patch_active {
                    push_action(
                        pending_actions,
                        PassDebugWindowAction::ResetPatch {
                            pass_name: self.pass_name.clone(),
                        },
                    );
                    self.patch_active = false;
                    self.last_status = Some("Resetting...".to_string());
                }
                self.refresh_analysis_rows();
            }
            ShortwirePhase::PendingApply { .. } => {
                self.shortwire_exit_on_apply = true;
                self.shortwire_active = None;
                self.loaded_source = self.generated_base_source.clone();
                self.replace_draft_source(self.generated_base_source.clone());
                self.dirty = false;
                if self.patch_active {
                    push_action(
                        pending_actions,
                        PassDebugWindowAction::ResetPatch {
                            pass_name: self.pass_name.clone(),
                        },
                    );
                    self.patch_active = false;
                    self.last_status = Some("Resetting...".to_string());
                }
                self.refresh_analysis_rows();
            }
            ShortwirePhase::PendingResetThenEnter { .. } => {
                self.shortwire_active = None;
                self.loaded_source = self.generated_base_source.clone();
                self.replace_draft_source(self.generated_base_source.clone());
                self.dirty = false;
                self.refresh_analysis_rows();
            }
        }
        self.last_error = None;
        if !self.patch_active && self.last_status.as_deref() != Some("Resetting...") {
            self.last_status = None;
        }
    }

    fn prepare_debug_window_close(
        &mut self,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        if self.shortwire_active.is_some() {
            self.exit_shortwire_navigate(pending_actions);
        }
    }

    fn enter_shortwire_and_apply(
        &mut self,
        row: &PassDebugDependencyRow,
        pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    ) {
        if self.shortwire_active.is_some() || self.generated_base_source.is_empty() {
            return;
        }

        let patch_key = shortwire_patch_key(row);
        let patch = match self.shortwire_patches.get(&patch_key) {
            Some(p) => p.clone(),
            None => {
                self.enter_shortwire(row, pending_actions);
                return;
            }
        };
        eprintln!(
            "[shortwire-diff] enter_and_apply_stored pass={} patch_key={} {}",
            self.pass_name,
            patch_key,
            shortwire_patch_summary(Some(&patch)),
        );

        match apply_hunks(&self.generated_base_source, &patch.hunks) {
            Ok(patched) => {
                let identity = ShortwireRowIdentity {
                    patch_key: patch_key.clone(),
                    row_key_hint: row.row_key.clone(),
                    label: row.label.clone(),
                    target_id: row.target_id.clone(),
                };
                self.shortwire_active = Some(ShortwireActiveState {
                    identity,
                    base_source: self.generated_base_source.clone(),
                    base_source_hash: self.generated_base_source_hash,
                    base_source_stale: false,
                    diff_view_enabled: false,
                    reference_image: patch.reference_image.clone(),
                    phase: ShortwirePhase::PendingApply {
                        pending_hunks: patch.hunks.clone(),
                    },
                });
                let identity = self
                    .shortwire_active
                    .as_ref()
                    .map(|active| active.identity.clone());
                if let Some(identity) = identity.as_ref() {
                    self.enter_reference_shortwire(identity);
                }
                self.replace_draft_source(patched.clone());
                self.dirty = true;
                push_action(
                    pending_actions,
                    PassDebugWindowAction::ApplyPatch {
                        pass_name: self.pass_name.clone(),
                        source: patched,
                        reference_image: patch.reference_image.clone(),
                    },
                );
                self.last_error = None;
                self.last_status = Some("Applying stored patch...".to_string());
            }
            Err(_) => {
                self.shortwire_patches.remove(&patch_key);
                self.shortwire_patches_dirty = true;
                self.enter_shortwire(row, pending_actions);
                self.last_error = Some("Stored patch outdated — entering edit mode".to_string());
            }
        }
    }

    fn shortwire_is_editor_interactive(&self) -> bool {
        matches!(
            self.shortwire_active.as_ref().map(|a| &a.phase),
            Some(ShortwirePhase::Editing)
        )
    }

    fn collect_shortwire_patches_artifact(&self) -> Option<(DebugArtifactItem, String)> {
        let payload = ShortwirePatchesPayload {
            version: 1,
            patches: self.shortwire_patches.clone(),
        };
        let content_text = serde_json::to_string(&payload).ok()?;
        let artifact_id = pass_patches_artifact_id(&self.pass_name);
        let file_name = format!(
            "{}.patches.json",
            safe_debug_artifact_segment(&self.pass_name, "pass")
        );
        let item = DebugArtifactItem {
            id: artifact_id.clone(),
            anchor: DebugArtifactAnchor::Pass {
                pass_name: self.pass_name.clone(),
            },
            role: DebugArtifactRole::Patch,
            name: "Shortwire patches".to_string(),
            mime_type: "text/plain".to_string(),
            path: format!(
                "debug-artifacts/{}/{}",
                safe_debug_artifact_segment(&artifact_id, "artifact"),
                safe_debug_artifact_segment(&file_name, "artifact.json")
            ),
            size: Some(content_text.len() as u64),
            content_hash: Some(debug_artifact_content_hash(content_text.as_bytes())),
            slot_key: Some(DEBUG_ARTIFACT_DEFAULT_SLOT.to_string()),
        };
        Some((item, content_text))
    }

    fn restore_shortwire_patches_from_text(&mut self, text: &str) -> bool {
        let Ok(payload) = serde_json::from_str::<ShortwirePatchesPayload>(text) else {
            return false;
        };
        if payload.version != 1 {
            return false;
        }
        self.shortwire_patches = payload.patches;
        self.shortwire_patches_dirty = false;
        true
    }

    fn take_patches_dirty_artifact(&mut self) -> Option<(DebugArtifactItem, String)> {
        if !self.shortwire_patches_dirty {
            return None;
        }
        self.shortwire_patches_dirty = false;
        self.collect_shortwire_patches_artifact()
    }
}

fn compute_hunks(base: &str, edited: &str) -> Vec<ShortwireHunk> {
    use similar::TextDiff;

    if base == edited {
        return Vec::new();
    }

    let diff = TextDiff::from_lines(base, edited);
    let base_lines: Vec<&str> = base.lines().collect();
    let mut hunks = Vec::new();

    for group in diff.grouped_ops(SHORTWIRE_DIFF_CONTEXT_LINES) {
        for op in &group {
            match op {
                similar::DiffOp::Equal { .. } => {}
                similar::DiffOp::Delete {
                    old_index, old_len, ..
                }
                | similar::DiffOp::Replace {
                    old_index, old_len, ..
                } => {
                    let old_start = *old_index;
                    let old_lines_slice: Vec<String> = base_lines[old_start..old_start + old_len]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();

                    let new_lines_slice: Vec<String> = match op {
                        similar::DiffOp::Replace {
                            new_index, new_len, ..
                        } => {
                            let edited_lines: Vec<&str> = edited.lines().collect();
                            edited_lines[*new_index..*new_index + new_len]
                                .iter()
                                .map(|s| s.to_string())
                                .collect()
                        }
                        _ => Vec::new(),
                    };

                    let context_before: Vec<String> = base_lines
                        [old_start.saturating_sub(SHORTWIRE_DIFF_CONTEXT_LINES)..old_start]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    let after_end = (old_start + old_len).min(base_lines.len());
                    let context_after: Vec<String> = base_lines[after_end
                        ..(after_end + SHORTWIRE_DIFF_CONTEXT_LINES).min(base_lines.len())]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();

                    hunks.push(ShortwireHunk {
                        old_start,
                        old_lines: old_lines_slice,
                        new_lines: new_lines_slice,
                        context_before,
                        context_after,
                    });
                }
                similar::DiffOp::Insert {
                    old_index,
                    new_index,
                    new_len,
                } => {
                    let edited_lines: Vec<&str> = edited.lines().collect();
                    let new_lines_slice: Vec<String> = edited_lines
                        [*new_index..*new_index + new_len]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();

                    let context_before: Vec<String> = base_lines
                        [old_index.saturating_sub(SHORTWIRE_DIFF_CONTEXT_LINES)..*old_index]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();
                    let context_after: Vec<String> = base_lines[*old_index
                        ..(*old_index + SHORTWIRE_DIFF_CONTEXT_LINES).min(base_lines.len())]
                        .iter()
                        .map(|s| s.to_string())
                        .collect();

                    hunks.push(ShortwireHunk {
                        old_start: *old_index,
                        old_lines: Vec::new(),
                        new_lines: new_lines_slice,
                        context_before,
                        context_after,
                    });
                }
            }
        }
    }
    hunks
}

fn build_shortwire_diff_view(base: &str, edited: &str) -> ShortwireDiffView {
    use similar::TextDiff;

    let old_lines: Vec<&str> = base.lines().collect();
    let new_lines: Vec<&str> = edited.lines().collect();
    let mut rows = Vec::new();

    if base != edited {
        let diff = TextDiff::from_lines(base, edited);
        for (group_index, group) in diff
            .grouped_ops(SHORTWIRE_DIFF_CONTEXT_LINES)
            .into_iter()
            .enumerate()
        {
            if group_index > 0 {
                rows.push(ShortwireDiffRow {
                    kind: ShortwireDiffRowKind::Separator,
                    old_line: None,
                    new_line: None,
                    text: String::new(),
                });
            }

            for op in group {
                match op {
                    similar::DiffOp::Equal {
                        old_index,
                        new_index,
                        len,
                    } => {
                        for offset in 0..len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Context,
                                old_line: Some(old_index + offset + 1),
                                new_line: Some(new_index + offset + 1),
                                text: old_lines
                                    .get(old_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                    }
                    similar::DiffOp::Delete {
                        old_index, old_len, ..
                    } => {
                        for offset in 0..old_len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Removed,
                                old_line: Some(old_index + offset + 1),
                                new_line: None,
                                text: old_lines
                                    .get(old_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                    }
                    similar::DiffOp::Insert {
                        new_index, new_len, ..
                    } => {
                        for offset in 0..new_len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Added,
                                old_line: None,
                                new_line: Some(new_index + offset + 1),
                                text: new_lines
                                    .get(new_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                    }
                    similar::DiffOp::Replace {
                        old_index,
                        old_len,
                        new_index,
                        new_len,
                    } => {
                        for offset in 0..old_len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Removed,
                                old_line: Some(old_index + offset + 1),
                                new_line: None,
                                text: old_lines
                                    .get(old_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                        for offset in 0..new_len {
                            rows.push(ShortwireDiffRow {
                                kind: ShortwireDiffRowKind::Added,
                                old_line: None,
                                new_line: Some(new_index + offset + 1),
                                text: new_lines
                                    .get(new_index + offset)
                                    .copied()
                                    .unwrap_or_default()
                                    .to_string(),
                            });
                        }
                    }
                }
            }
        }
    }

    ShortwireDiffView {
        rows,
        old_line_count: old_lines.len(),
        new_line_count: new_lines.len(),
    }
}

fn apply_hunks(base: &str, hunks: &[ShortwireHunk]) -> Result<String, HunkApplyError> {
    if hunks.is_empty() {
        return Ok(base.to_string());
    }

    let mut base_lines: Vec<String> = base.lines().map(|s| s.to_string()).collect();

    let mut sorted_indices: Vec<usize> = (0..hunks.len()).collect();
    sorted_indices.sort_by(|a, b| hunks[*b].old_start.cmp(&hunks[*a].old_start));

    for &hunk_index in &sorted_indices {
        let hunk = &hunks[hunk_index];
        let position = locate_hunk_position(&base_lines, hunk, hunk_index)?;

        if !hunk.old_lines.is_empty() {
            if position + hunk.old_lines.len() > base_lines.len() {
                return Err(HunkApplyError::VerificationFailed { hunk_index });
            }
            for (i, old_line) in hunk.old_lines.iter().enumerate() {
                if base_lines[position + i] != *old_line {
                    return Err(HunkApplyError::VerificationFailed { hunk_index });
                }
            }
            base_lines.splice(
                position..position + hunk.old_lines.len(),
                hunk.new_lines.iter().cloned(),
            );
        } else {
            base_lines.splice(position..position, hunk.new_lines.iter().cloned());
        }
    }

    let mut result = base_lines.join("\n");
    if base.ends_with('\n') && !result.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

fn three_way_merge_sources(
    base: &str,
    incoming: &str,
    local: &str,
) -> Result<String, HunkApplyError> {
    if local == base {
        return Ok(incoming.to_string());
    }
    if incoming == base {
        return Ok(local.to_string());
    }

    let local_hunks = compute_hunks(base, local);
    if local_hunks.is_empty() {
        return Ok(incoming.to_string());
    }
    apply_hunks(incoming, &local_hunks)
}

fn locate_hunk_position(
    base_lines: &[String],
    hunk: &ShortwireHunk,
    hunk_index: usize,
) -> Result<usize, HunkApplyError> {
    if verify_hunk_at_position(base_lines, hunk, hunk.old_start) {
        return Ok(hunk.old_start);
    }

    let search_range = 30;
    let start = hunk.old_start.saturating_sub(search_range);
    let end = (hunk.old_start + search_range).min(base_lines.len());

    for offset in 1..=search_range {
        if hunk.old_start + offset < end
            && verify_hunk_at_position(base_lines, hunk, hunk.old_start + offset)
        {
            return Ok(hunk.old_start + offset);
        }
        if hunk.old_start >= offset + start.min(hunk.old_start) {
            let pos = hunk.old_start - offset;
            if pos >= start && verify_hunk_at_position(base_lines, hunk, pos) {
                return Ok(pos);
            }
        }
    }

    Err(HunkApplyError::HunkNotFound { hunk_index })
}

fn verify_hunk_at_position(base_lines: &[String], hunk: &ShortwireHunk, position: usize) -> bool {
    if !hunk.old_lines.is_empty() {
        if position + hunk.old_lines.len() > base_lines.len() {
            return false;
        }
        for (i, old_line) in hunk.old_lines.iter().enumerate() {
            if base_lines[position + i] != *old_line {
                return false;
            }
        }
        if !hunk.context_before.is_empty() {
            let ctx_start = position.saturating_sub(hunk.context_before.len());
            let available = &base_lines[ctx_start..position];
            let expected_suffix =
                &hunk.context_before[hunk.context_before.len().saturating_sub(available.len())..];
            if available.len() >= expected_suffix.len() {
                let tail = &available[available.len() - expected_suffix.len()..];
                if tail.iter().zip(expected_suffix.iter()).any(|(a, b)| a != b) {
                    return false;
                }
            }
        }
        true
    } else {
        if position > base_lines.len() {
            return false;
        }
        if !hunk.context_before.is_empty() {
            let ctx_start = position.saturating_sub(hunk.context_before.len());
            let available = &base_lines[ctx_start..position];
            let expected_suffix =
                &hunk.context_before[hunk.context_before.len().saturating_sub(available.len())..];
            if available.len() >= expected_suffix.len() {
                let tail = &available[available.len() - expected_suffix.len()..];
                if tail.iter().zip(expected_suffix.iter()).any(|(a, b)| a != b) {
                    return false;
                }
            } else {
                return false;
            }
        }
        if !hunk.context_after.is_empty() {
            let available_after =
                &base_lines[position..(position + hunk.context_after.len()).min(base_lines.len())];
            let expected_prefix =
                &hunk.context_after[..hunk.context_after.len().min(available_after.len())];
            if available_after.len() >= expected_prefix.len() {
                if available_after[..expected_prefix.len()]
                    .iter()
                    .zip(expected_prefix.iter())
                    .any(|(a, b)| a != b)
                {
                    return false;
                }
            } else {
                return false;
            }
        }
        true
    }
}

fn target_exists(source: &PassDebugSource, target_id: Option<&str>) -> bool {
    let Some(target_id) = target_id else {
        return false;
    };
    source
        .dependency_targets
        .iter()
        .any(|target| target.id == target_id)
}

fn consume_tree_reveal_row_key<Row: PassDebugTreeRow>(
    pending_row_key: &mut Option<String>,
    rows: &[Row],
) -> Option<String> {
    let row_key = pending_row_key.clone()?;
    if rows
        .iter()
        .any(|row| row.row_key().map(|key| key == row_key).unwrap_or(false))
    {
        *pending_row_key = None;
        Some(row_key)
    } else {
        None
    }
}

fn shortwire_click_matches_active_row(active_row_key: &str, click: &PassDebugTreeClick) -> bool {
    click.row_key.as_deref() == Some(active_row_key)
        || click.toggle_row_key.as_deref() == Some(active_row_key)
}

struct PassDebugTreeRenderState<'a> {
    focused_target_id: Option<&'a str>,
    focused_row_key: Option<&'a str>,
    reveal_row_key: Option<&'a str>,
    path_row_keys: &'a [String],
    expandable_row_keys: Option<&'a HashSet<String>>,
    expanded_row_keys: Option<&'a HashSet<String>>,
    shortwire_active_row_key: Option<&'a str>,
    shortwire_can_enter: bool,
    shortwire_dot_info: &'a HashMap<String, ShortwireDotInfo>,
}

struct ShortwireTreeResult {
    click: Option<PassDebugTreeClick>,
    context_menu_row_index: Option<usize>,
}

pub struct PassDebugWindowState {
    pass_name: String,
    viewport_id: egui::ViewportId,
    document: Arc<Mutex<PassDebugWindowDocument>>,
    close_requested: Arc<AtomicBool>,
    pending_actions: Arc<Mutex<Vec<PassDebugWindowAction>>>,
    last_viewport_snapshot: Arc<Mutex<Option<PassDebugViewportSnapshot>>>,
    loaded_shortwire_patches_artifact_hash: Option<u64>,
    viewport_initialized: bool,
    focus_requested: bool,
}

impl PassDebugWindowState {
    fn new(
        pass_name: String,
        source: Option<PassDebugSource>,
        source_revision: u64,
        patch_source: Option<&str>,
    ) -> Self {
        let viewport_id = egui::ViewportId::from_hash_of(("pass-debug", pass_name.as_str()));
        Self {
            document: Arc::new(Mutex::new(PassDebugWindowDocument::new(
                pass_name.clone(),
                source,
                source_revision,
                patch_source,
            ))),
            close_requested: Arc::new(AtomicBool::new(false)),
            pending_actions: Arc::new(Mutex::new(Vec::new())),
            last_viewport_snapshot: Arc::new(Mutex::new(None)),
            loaded_shortwire_patches_artifact_hash: None,
            viewport_initialized: false,
            pass_name,
            viewport_id,
            focus_requested: true,
        }
    }

    fn update_source(
        &self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_source: Option<&str>,
    ) {
        if let Ok(mut document) = self.document.lock() {
            document.update_source_with_actions(
                source,
                source_revision,
                patch_source,
                &self.pending_actions,
            );
        }
    }

    fn update_reference_workspace(
        &self,
        workspace_text: Option<&str>,
        reference_files: &[crate::debug_artifacts::DebugArtifactTextSnapshot],
        legacy_reference_source: Option<&str>,
        reference_patches_text: Option<&str>,
    ) {
        if let Ok(mut document) = self.document.lock() {
            document.update_reference_workspace(
                workspace_text,
                reference_files,
                legacy_reference_source,
                reference_patches_text,
            );
        }
    }

    fn sync_shortwire_patches_from_artifact(&mut self, text: Option<&str>) {
        let Some(text) = text else {
            return;
        };
        let artifact_hash = hash_source(text);
        if self.loaded_shortwire_patches_artifact_hash == Some(artifact_hash) {
            return;
        }
        if let Ok(mut document) = self.document.lock() {
            if document.shortwire_active.is_some()
                || document.shortwire_patches_dirty
                || document.dirty
            {
                return;
            }
            if document.restore_shortwire_patches_from_text(text) {
                eprintln!(
                    "[shortwire-diff] restore_patches_from_artifact pass={} artifact_hash={} patches={}",
                    self.pass_name,
                    artifact_hash,
                    document.shortwire_patches.len(),
                );
                self.loaded_shortwire_patches_artifact_hash = Some(artifact_hash);
            }
        }
    }

    fn drain_actions(&self, out: &mut Vec<PassDebugWindowAction>) {
        if let Ok(mut pending) = self.pending_actions.lock() {
            out.extend(pending.drain(..));
        }
    }
}

pub type PassDebugWindowMap = HashMap<String, PassDebugWindowState>;

pub struct PassDebugPatchApplyResult {
    pub artifacts: Vec<(DebugArtifactItem, String)>,
    pub binary_artifacts: Vec<(DebugArtifactItem, Vec<u8>)>,
    pub diff_capture: Option<ShortwireDiffCaptureRequest>,
}

fn pass_debug_default_window_size() -> egui::Vec2 {
    egui::vec2(
        PASS_DEBUG_WINDOW_DEFAULT_WIDTH,
        PASS_DEBUG_WINDOW_DEFAULT_HEIGHT,
    )
}

fn pass_debug_min_window_size() -> egui::Vec2 {
    egui::vec2(PASS_DEBUG_WINDOW_MIN_WIDTH, PASS_DEBUG_WINDOW_MIN_HEIGHT)
}

fn pass_debug_viewport_builder(title: String, include_initial_size: bool) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_min_inner_size(pass_debug_min_window_size());

    if include_initial_size {
        builder.with_inner_size(pass_debug_default_window_size())
    } else {
        builder
    }
}

pub fn open_pass_debug_window(
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
    pass_sources_revision: u64,
    pass_shader_overrides: &HashMap<String, String>,
    debug_artifacts: &crate::debug_artifacts::DebugArtifactStore,
    pass_name: String,
) {
    let source = pass_sources.get(pass_name.as_str());
    let patch_source = pass_shader_overrides
        .get(pass_name.as_str())
        .map(String::as_str);
    let reference_workspace_text =
        debug_artifacts.pass_reference_workspace_text(pass_name.as_str());
    let reference_files = debug_artifacts.pass_reference_file_texts(pass_name.as_str());
    let legacy_reference_source = debug_artifacts.pass_reference_text(pass_name.as_str());
    let reference_patches_text = debug_artifacts.pass_reference_patches_text(pass_name.as_str());
    if let Some(existing) = windows.get_mut(pass_name.as_str()) {
        existing.update_source(source, pass_sources_revision, patch_source);
        existing.update_reference_workspace(
            reference_workspace_text,
            &reference_files,
            legacy_reference_source,
            reference_patches_text,
        );
        existing.sync_shortwire_patches_from_artifact(
            debug_artifacts.pass_patches_text(pass_name.as_str()),
        );
        existing.focus_requested = true;
        existing.close_requested.store(false, Ordering::Relaxed);
        return;
    }

    let mut state = PassDebugWindowState::new(
        pass_name.clone(),
        source.cloned(),
        pass_sources_revision,
        patch_source,
    );
    state.update_reference_workspace(
        reference_workspace_text,
        &reference_files,
        legacy_reference_source,
        reference_patches_text,
    );
    state.sync_shortwire_patches_from_artifact(
        debug_artifacts.pass_patches_text(pass_name.as_str()),
    );
    windows.insert(pass_name.clone(), state);
}

pub fn show_pass_debug_windows(
    ctx: &egui::Context,
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
    pass_sources_revision: u64,
    pass_shader_overrides: &HashMap<String, String>,
    debug_artifacts: &crate::debug_artifacts::DebugArtifactStore,
) -> Vec<PassDebugWindowAction> {
    let fn_start = Instant::now();
    windows.retain(|_, state| !state.close_requested.load(Ordering::Relaxed));

    let mut actions = Vec::new();
    let window_count = windows.len();
    for state in windows.values_mut() {
        let window_start = Instant::now();
        let patch_source = pass_shader_overrides
            .get(state.pass_name.as_str())
            .map(String::as_str);
        state.update_source(
            pass_sources.get(state.pass_name.as_str()),
            pass_sources_revision,
            patch_source,
        );
        let reference_files = debug_artifacts.pass_reference_file_texts(state.pass_name.as_str());
        state.update_reference_workspace(
            debug_artifacts.pass_reference_workspace_text(state.pass_name.as_str()),
            &reference_files,
            debug_artifacts.pass_reference_text(state.pass_name.as_str()),
            debug_artifacts.pass_reference_patches_text(state.pass_name.as_str()),
        );
        state.sync_shortwire_patches_from_artifact(
            debug_artifacts.pass_patches_text(state.pass_name.as_str()),
        );
        let update_source_dur = window_start.elapsed();

        let viewport_id = state.viewport_id;
        let document = Arc::clone(&state.document);
        let close_requested = Arc::clone(&state.close_requested);
        let pending_actions = Arc::clone(&state.pending_actions);
        let last_viewport_snapshot = Arc::clone(&state.last_viewport_snapshot);
        let title = format!("RenderPass Debug - {}", state.pass_name);
        let viewport_builder =
            pass_debug_viewport_builder(title.clone(), !state.viewport_initialized);
        state.viewport_initialized = true;

        let pass_name_for_log = state.pass_name.clone();
        ctx.show_viewport_deferred(viewport_id, viewport_builder, move |ui, class| {
            let viewport_render_start = Instant::now();
            match class {
                egui::ViewportClass::EmbeddedWindow => {
                    let mut open = true;
                    egui::Window::new(title.as_str())
                        .id(egui::Id::new(("pass-debug-embedded", title.as_str())))
                        .open(&mut open)
                        .title_bar(false)
                        .default_size(pass_debug_default_window_size())
                        .show(ui.ctx(), |window_ui| {
                            render_pass_debug_embedded_content(
                                window_ui,
                                &document,
                                &pending_actions,
                                close_requested.as_ref(),
                            );
                        });
                    if !open {
                        if let Ok(mut document) = document.lock() {
                            document.prepare_debug_window_close(&pending_actions);
                        }
                        close_requested.store(true, Ordering::Relaxed);
                    }
                }
                _ => {
                    if handle_pass_debug_viewport_close_request(
                        ui.ctx(),
                        &close_requested,
                        &last_viewport_snapshot,
                        &document,
                        &pending_actions,
                    ) {
                        let viewport_render_dur = viewport_render_start.elapsed();
                        metric_log!(
                            "[pass-debug] window={} viewport_render={:.2}ms (close-handled)",
                            pass_name_for_log,
                            viewport_render_dur.as_secs_f64() * 1000.0,
                        );
                        return;
                    }
                    render_pass_debug_viewport(
                        ui,
                        &document,
                        &pending_actions,
                        close_requested.as_ref(),
                    );
                }
            }
            let viewport_render_dur = viewport_render_start.elapsed();
            metric_log!(
                "[pass-debug] window={} viewport_render={:.2}ms",
                pass_name_for_log,
                viewport_render_dur.as_secs_f64() * 1000.0,
            );
        });

        if state.focus_requested {
            ctx.send_viewport_cmd_to(state.viewport_id, egui::ViewportCommand::Focus);
            state.focus_requested = false;
        }

        state.drain_actions(&mut actions);
        if let Ok(mut document) = state.document.lock() {
            if let Some((item, content_text)) = document.take_patches_dirty_artifact() {
                actions.push(PassDebugWindowAction::UpsertDebugArtifact { item, content_text });
            }
            if let Some((item, content_text)) = document.take_reference_patches_dirty_artifact() {
                actions.push(PassDebugWindowAction::UpsertDebugArtifact { item, content_text });
            }
        }
        metric_log!(
            "[pass-debug] window={} update_source={:.2}ms",
            state.pass_name,
            update_source_dur.as_secs_f64() * 1000.0,
        );
    }

    let total_dur = fn_start.elapsed();
    metric_log!(
        "[pass-debug] show_all total={:.2}ms window_count={}",
        total_dur.as_secs_f64() * 1000.0,
        window_count,
    );
    actions
}

pub fn mark_patch_applied(
    windows: &mut PassDebugWindowMap,
    pass_name: &str,
    source: Option<&PassDebugSource>,
    source_revision: u64,
    draft_source: String,
    status: String,
) -> PassDebugPatchApplyResult {
    if let Some(state) = windows.get(pass_name)
        && let Ok(mut document) = state.document.lock()
    {
        let diff_capture = document.mark_applied(source, source_revision, draft_source, status);
        let mut artifacts = Vec::new();
        if let Some(artifact) = document.take_patches_dirty_artifact() {
            artifacts.push(artifact);
        }
        if let Some(artifact) = document.take_reference_patches_dirty_artifact() {
            artifacts.push(artifact);
        }
        return PassDebugPatchApplyResult {
            artifacts,
            binary_artifacts: Vec::new(),
            diff_capture,
        };
    }
    PassDebugPatchApplyResult {
        artifacts: Vec::new(),
        binary_artifacts: Vec::new(),
        diff_capture: None,
    }
}

pub fn record_shortwire_diff_result(
    windows: &mut PassDebugWindowMap,
    request: &ShortwireDiffCaptureRequest,
    diff_result: ShortwireDiffResult,
) -> Vec<(DebugArtifactItem, String)> {
    let Some(state) = windows.get(request.pass_name.as_str()) else {
        return Vec::new();
    };
    let Ok(mut document) = state.document.lock() else {
        return Vec::new();
    };
    let Some(patch) = document
        .shortwire_patches
        .get_mut(request.patch_key.as_str())
    else {
        eprintln!(
            "[shortwire-diff] record_result_missing_patch pass={} patch_key={} {}",
            request.pass_name,
            request.patch_key,
            shortwire_diff_result_summary(Some(&diff_result)),
        );
        return Vec::new();
    };
    let status = shortwire_diff_status(&diff_result);
    eprintln!(
        "[shortwire-diff] record_result pass={} patch_key={} status={:?} pass_threshold={:.6} {}",
        request.pass_name,
        request.patch_key,
        status,
        SHORTWIRE_DIFF_PASS_MAX_AE,
        shortwire_diff_result_summary(Some(&diff_result)),
    );
    patch.diff_result = Some(diff_result);
    document.shortwire_patches_dirty = true;
    document.take_patches_dirty_artifact().into_iter().collect()
}

fn shortwire_reference_image_artifact_id(pass_name: &str, patch_key: &str) -> String {
    format!(
        "pass__{}__shortwire-reference-image__{}",
        safe_debug_artifact_segment(pass_name, "pass"),
        debug_artifact_content_hash(patch_key.as_bytes()),
    )
}

fn shortwire_reference_image_artifact(
    pass_name: &str,
    patch_key: &str,
    pasted: ShortwirePastedReferenceImage,
) -> (ShortwireReferenceImage, DebugArtifactItem, Vec<u8>) {
    let artifact_id = shortwire_reference_image_artifact_id(pass_name, patch_key);
    let patch_key_hash = debug_artifact_content_hash(patch_key.as_bytes());
    let file_name = format!(
        "{}.shortwire-ref.{}.png",
        safe_debug_artifact_segment(pass_name, "pass"),
        patch_key_hash
    );
    let item = DebugArtifactItem {
        id: artifact_id.clone(),
        anchor: DebugArtifactAnchor::Pass {
            pass_name: pass_name.to_string(),
        },
        role: DebugArtifactRole::Image,
        name: "Shortwire reference image".to_string(),
        mime_type: "image/png".to_string(),
        path: format!(
            "debug-artifacts/{}/{}",
            safe_debug_artifact_segment(&artifact_id, "artifact"),
            safe_debug_artifact_segment(&file_name, "shortwire-ref.png"),
        ),
        size: Some(pasted.png_bytes.len() as u64),
        content_hash: Some(debug_artifact_content_hash(pasted.png_bytes.as_slice())),
        slot_key: Some(format!("shortwire-reference:{patch_key_hash}")),
    };
    let image = ShortwireReferenceImage {
        artifact_id,
        name: pasted.name,
        width: pasted.width,
        height: pasted.height,
        alpha_mode: pasted.alpha_mode,
        mode: pasted.mode,
        opacity: pasted.opacity,
        offset: pasted.offset,
    };
    (image, item, pasted.png_bytes)
}

pub fn request_active_shortwire_diff_capture(
    windows: &mut PassDebugWindowMap,
    pasted_reference: Option<ShortwirePastedReferenceImage>,
) -> PassDebugPatchApplyResult {
    let mut active_count = 0usize;
    let mut missing_patch_count = 0usize;
    let mut pending_pasted_reference = pasted_reference;
    for state in windows.values() {
        let Ok(mut document) = state.document.lock() else {
            continue;
        };
        let Some(active) = document.shortwire_active.as_ref() else {
            continue;
        };
        active_count += 1;
        let patch_key = active.identity.patch_key.clone();
        let pass_name = document.pass_name.clone();
        let mut binary_artifacts = Vec::new();
        let reference_image = pending_pasted_reference.take().map(|pasted| {
            let (image, item, bytes) =
                shortwire_reference_image_artifact(&pass_name, &patch_key, pasted);
            binary_artifacts.push((item, bytes));
            image
        });
        if let Some(reference_image) = reference_image.clone()
            && let Some(active) = document.shortwire_active.as_mut()
        {
            active.reference_image = Some(reference_image);
        }
        if !document.shortwire_patches.contains_key(patch_key.as_str()) && reference_image.is_some()
        {
            let hunks = compute_hunks(&document.generated_base_source, &document.draft_source);
            let base_source_hash = document.generated_base_source_hash;
            eprintln!(
                "[shortwire-diff] request_capture_create_image_patch pass={} patch_key={} hunks={} has_image=true",
                pass_name,
                patch_key,
                hunks.len(),
            );
            document.shortwire_patches.insert(
                patch_key.clone(),
                ShortwireNodePatch {
                    hunks,
                    base_source_hash,
                    reference_image: reference_image.clone(),
                    diff_result: None,
                },
            );
        }
        let Some(patch) = document.shortwire_patches.get_mut(patch_key.as_str()) else {
            missing_patch_count += 1;
            eprintln!(
                "[shortwire-diff] request_capture_no_patch pass={} patch_key={}",
                pass_name, patch_key,
            );
            continue;
        };

        eprintln!(
            "[shortwire-diff] request_capture_clear_previous pass={} patch_key={} {}",
            pass_name,
            patch_key,
            shortwire_patch_summary(Some(patch)),
        );
        if let Some(reference_image) = reference_image {
            patch.reference_image = Some(reference_image);
        }
        patch.diff_result = None;
        document.shortwire_patches_dirty = true;
        let artifacts = document.take_patches_dirty_artifact().into_iter().collect();
        eprintln!(
            "[shortwire-diff] request_capture_queued pass={} patch_key={}",
            pass_name, patch_key,
        );
        return PassDebugPatchApplyResult {
            artifacts,
            binary_artifacts,
            diff_capture: Some(ShortwireDiffCaptureRequest {
                pass_name,
                patch_key,
            }),
        };
    }

    eprintln!(
        "[shortwire-diff] request_capture_no_capturable_patch active_count={} missing_patch_count={}",
        active_count, missing_patch_count,
    );
    PassDebugPatchApplyResult {
        artifacts: Vec::new(),
        binary_artifacts: Vec::new(),
        diff_capture: None,
    }
}

pub fn has_active_shortwire(windows: &PassDebugWindowMap) -> bool {
    windows.values().any(|state| {
        state
            .document
            .lock()
            .map(|document| document.shortwire_active.is_some())
            .unwrap_or(false)
    })
}

pub fn mark_patch_reset(
    windows: &mut PassDebugWindowMap,
    pass_name: &str,
    source: Option<&PassDebugSource>,
    source_revision: u64,
    status: String,
) {
    if let Some(state) = windows.get(pass_name)
        && let Ok(mut document) = state.document.lock()
    {
        document.mark_reset(source, source_revision, status);
    }
}

pub fn mark_all_patches_reset(
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
    pass_sources_revision: u64,
    status: String,
) {
    for (pass_name, state) in windows.iter() {
        if let Ok(mut document) = state.document.lock() {
            document.mark_reset(
                pass_sources.get(pass_name),
                pass_sources_revision,
                status.clone(),
            );
        }
    }
}

pub fn record_patch_error(windows: &mut PassDebugWindowMap, pass_name: &str, error: String) {
    if let Some(state) = windows.get(pass_name)
        && let Ok(mut document) = state.document.lock()
    {
        document.record_error(error);
    }
}

pub fn record_all_patch_error(windows: &mut PassDebugWindowMap, error: String) {
    for state in windows.values() {
        if let Ok(mut document) = state.document.lock() {
            document.record_error(error.clone());
        }
    }
}

fn handle_pass_debug_viewport_close_request(
    ctx: &egui::Context,
    close_requested: &AtomicBool,
    last_snapshot: &Mutex<Option<PassDebugViewportSnapshot>>,
    document: &Arc<Mutex<PassDebugWindowDocument>>,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) -> bool {
    let viewport = ctx.input(|input| input.viewport().clone());
    let current_snapshot = PassDebugViewportSnapshot::from_info(&viewport);
    let previous_snapshot = last_snapshot.lock().ok().and_then(|guard| *guard);
    if let Ok(mut guard) = last_snapshot.lock() {
        *guard = Some(current_snapshot);
    }

    if !viewport.close_requested() {
        return false;
    }

    match classify_pass_debug_close_request(previous_snapshot, current_snapshot) {
        PassDebugCloseDecision::Accept => {
            if let Ok(mut document) = document.lock() {
                document.prepare_debug_window_close(pending_actions);
            }
            close_requested.store(true, Ordering::Relaxed);
            true
        }
        PassDebugCloseDecision::Cancel(reason) => {
            eprintln!("[pass-debug] canceling transient close request: {reason:?}");
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            false
        }
    }
}

#[cfg(test)]
fn is_close_request_during_large_viewport_resize(
    previous: Option<egui::Rect>,
    current: Option<egui::Rect>,
) -> bool {
    let (Some(previous), Some(current)) = (previous, current) else {
        return false;
    };
    let width_delta = (previous.width() - current.width()).abs();
    let height_delta = (previous.height() - current.height()).abs();
    width_delta.max(height_delta) >= PASS_DEBUG_CLOSE_RESIZE_DELTA_THRESHOLD
}

fn classify_pass_debug_close_request(
    previous: Option<PassDebugViewportSnapshot>,
    current: PassDebugViewportSnapshot,
) -> PassDebugCloseDecision {
    if current.focused == Some(false) {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::FocusLost);
    }

    if current.visible == Some(false) {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::Hidden);
    }

    let Some(previous) = previous else {
        return PassDebugCloseDecision::Accept;
    };

    if previous.monitor_size != current.monitor_size {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::MonitorChanged);
    }

    if viewport_scale_changed(
        previous.native_pixels_per_point,
        current.native_pixels_per_point,
    ) {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::ScaleChanged);
    }

    if viewport_rect_jumped(previous.inner_rect, current.inner_rect)
        || viewport_rect_jumped(previous.outer_rect, current.outer_rect)
    {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::ViewportJumped);
    }

    PassDebugCloseDecision::Accept
}

fn viewport_scale_changed(previous: Option<f32>, current: Option<f32>) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => {
            (previous - current).abs() >= f32::EPSILON && current.is_finite()
        }
        _ => false,
    }
}

fn viewport_rect_jumped(previous: Option<egui::Rect>, current: Option<egui::Rect>) -> bool {
    let (Some(previous), Some(current)) = (previous, current) else {
        return false;
    };
    let position_delta = previous.min.distance(current.min);
    let size_delta = (previous.width() - current.width())
        .abs()
        .max((previous.height() - current.height()).abs());
    position_delta.max(size_delta) >= PASS_DEBUG_CLOSE_RESIZE_DELTA_THRESHOLD
}

fn render_pass_debug_viewport(
    ui: &mut egui::Ui,
    document: &Arc<Mutex<PassDebugWindowDocument>>,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
) {
    let pass_name = document
        .lock()
        .map(|document| document.pass_name.clone())
        .unwrap_or_else(|_| "unavailable".to_string());

    let t_central = Instant::now();
    egui::CentralPanel::default().show_inside(ui, |ui| {
        let Ok(mut document) = document.lock() else {
            ui.label("Debug document unavailable");
            return;
        };
        if document.source.is_none() {
            render_missing_source_message(ui);
            return;
        }
        render_dependency_editor_split(ui, &mut document, pending_actions, close_requested, true);
    });
    let central_dur = t_central.elapsed();

    metric_log!(
        "[pass-debug] viewport-inner pass={} central_panel={:.2}ms",
        pass_name,
        central_dur.as_secs_f64() * 1000.0,
    );
}

fn render_pass_debug_embedded_content(
    ui: &mut egui::Ui,
    document: &Arc<Mutex<PassDebugWindowDocument>>,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
) {
    let Ok(mut document) = document.lock() else {
        ui.label("Debug document unavailable");
        return;
    };

    if document.source.is_none() {
        ui.add_space(8.0);
        render_missing_source_message(ui);
        return;
    }

    render_dependency_editor_split(ui, &mut document, pending_actions, close_requested, false);
}

fn render_side_panel(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    render_dependency_panel(ui, document, pending_actions);
}

fn render_dependency_panel(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let Some(source) = document.analysis_source.as_ref() else {
        ui.colored_label(
            egui::Color32::from_rgb(255, 180, 120),
            "Pass no longer exists",
        );
        return;
    };

    if let Some(error) = source.parse_error.as_ref() {
        ui.add_space(8.0);
        ui.colored_label(egui::Color32::from_rgb(255, 118, 118), "WGSL parse failed");
        ui.label(egui::RichText::new(error.as_str()).monospace().small());
        ui.add_space(8.0);
        return;
    }

    if let Some(error) = source.dependency_error.as_ref() {
        ui.colored_label(
            egui::Color32::from_rgb(255, 180, 120),
            "Dependency analysis failed",
        );
        ui.label(egui::RichText::new(error.as_str()).monospace().small());
        ui.add_space(8.0);
    }

    if document.dependency_rows.is_empty() {
        ui.label(
            egui::RichText::new("Select a dependency target")
                .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE)),
        );
        return;
    }

    render_dependency_rows(ui, document, pending_actions);
}

fn render_dependency_rows(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    if !document.focus_is_in_dependency_root() {
        ui.label(
            egui::RichText::new("Focus is outside the current dependency map")
                .font(pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE))
                .color(egui::Color32::from_rgb(255, 180, 120)),
        );
    }

    let filter_font = pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE);
    ui.add(
        egui::TextEdit::singleline(&mut document.filter_text)
            .font(filter_font)
            .hint_text("Filter...")
            .desired_width(ui.available_width()),
    );
    ui.add_space(4.0);

    let reveal_row_key = consume_tree_reveal_row_key(
        &mut document.pending_dependency_reveal_row_key,
        &document.dependency_rows,
    );
    let path_row_keys = document.dependency_focus_path_row_keys();
    let mut visible_dependency_row_indices =
        document.cached_visible_dependency_row_indices().to_vec();

    if !document.filter_text.is_empty() {
        let filter_lower = document.filter_text.to_lowercase();
        let matched_row_keys: HashSet<String> = document
            .dependency_rows
            .iter()
            .filter(|row| row.label.to_lowercase().contains(&filter_lower))
            .map(|row| row.row_key.clone())
            .collect();

        let mut keep_row_keys: HashSet<String> = matched_row_keys.clone();
        for row_key in &matched_row_keys {
            let mut current = document
                .dependency_rows
                .iter()
                .find(|r| &r.row_key == row_key)
                .and_then(|r| r.parent_row_key.clone());
            while let Some(parent_key) = current {
                if !keep_row_keys.insert(parent_key.clone()) {
                    break;
                }
                current = document
                    .dependency_rows
                    .iter()
                    .find(|r| r.row_key == parent_key)
                    .and_then(|r| r.parent_row_key.clone());
            }
        }

        visible_dependency_row_indices = document
            .dependency_rows
            .iter()
            .enumerate()
            .filter(|(_, row)| keep_row_keys.contains(&row.row_key))
            .map(|(idx, _)| idx)
            .collect();
    }
    let font_id = pass_debug_mono_font(PASS_DEBUG_TREE_FONT_SIZE);
    let content_width = document.cached_dependency_tree_intrinsic_width(ui, &font_id);
    document.ensure_dependency_expandable_row_keys_cache();
    let shortwire_active_row_key = document
        .shortwire_active
        .as_ref()
        .map(|a| a.identity.row_key_hint.as_str());
    let shortwire_can_enter = document.shortwire_active.is_none()
        && document.merge_conflict.is_none()
        && !document.generated_base_source.is_empty();
    let shortwire_dot_info: HashMap<String, ShortwireDotInfo> = document
        .shortwire_patches
        .iter()
        .map(|(key, patch)| (key.clone(), shortwire_dot_info_for_patch(patch)))
        .collect();
    let result = {
        let expandable_row_keys = &document
            .dependency_expandable_row_keys_cache
            .as_ref()
            .expect("dependency expandable row cache must be initialized")
            .row_keys;
        let tree_state = PassDebugTreeRenderState {
            focused_target_id: document.focused_target_id.as_deref(),
            focused_row_key: document.focused_dependency_row_key.as_deref(),
            reveal_row_key: reveal_row_key.as_deref(),
            path_row_keys: &path_row_keys,
            expandable_row_keys: Some(expandable_row_keys),
            expanded_row_keys: Some(&document.dependency_expanded_row_keys),
            shortwire_active_row_key,
            shortwire_can_enter,
            shortwire_dot_info: &shortwire_dot_info,
        };
        render_scrollable_tree_rows(
            ui,
            egui::Id::new(("pass-debug-dependencies", document.pass_name.as_str())),
            &document.dependency_rows,
            &visible_dependency_row_indices,
            &tree_state,
            &font_id,
            content_width,
        )
    };
    if let Some(click) = result.click {
        let mut handle_click = true;
        if let Some(active_row_key) = document
            .shortwire_active
            .as_ref()
            .map(|active| active.identity.row_key_hint.as_str())
        {
            if shortwire_click_matches_active_row(active_row_key, &click) {
                handle_click = false;
            } else {
                document.exit_shortwire_navigate(pending_actions);
            }
        }
        if handle_click {
            document.refresh_draft_analysis();
            document.focus_tree_click(click, true);
        }
    }
    if let Some(row_idx) = result.context_menu_row_index {
        let row = document.dependency_rows[row_idx].clone();
        let patch_key = shortwire_patch_key(&row);
        let has_stored_patch = document.shortwire_patches.contains_key(&patch_key);

        if has_stored_patch && !document.patch_active {
            document.enter_shortwire_and_apply(&row, pending_actions);
        } else {
            document.enter_shortwire(&row, pending_actions);
        }
    }
}

fn render_scrollable_tree_rows(
    ui: &mut egui::Ui,
    id: egui::Id,
    rows: &[PassDebugDependencyRow],
    row_indices: &[usize],
    tree_state: &PassDebugTreeRenderState<'_>,
    font_id: &egui::FontId,
    intrinsic_content_width: f32,
) -> ShortwireTreeResult {
    let row_height = ui.fonts_mut(|fonts| fonts.row_height(&font_id));
    let row_height_with_spacing = row_height + ui.spacing().item_spacing.y;
    let mut clicked_row: Option<PassDebugTreeClick> = None;
    let mut context_menu_row_index: Option<usize> = None;
    let is_shortwire_active = tree_state.shortwire_active_row_key.is_some();

    egui::ScrollArea::both()
        .id_salt(id)
        .auto_shrink([false, false])
        .show_viewport(ui, |ui, viewport| {
            let total_height = row_height_with_spacing * row_indices.len() as f32;
            let content_width = ui.available_width().max(intrinsic_content_width).max(0.0);
            ui.set_min_size(egui::vec2(content_width, total_height));

            let min_row = (viewport.min.y / row_height_with_spacing).floor().max(0.0) as usize;
            let max_row = ((viewport.max.y / row_height_with_spacing).ceil() as usize + 1)
                .min(row_indices.len());
            let content_origin = ui.min_rect().min;

            let reveal_row_index = tree_state.reveal_row_key.and_then(|reveal_row_key| {
                row_indices.iter().position(|row_index| {
                    rows[*row_index]
                        .row_key()
                        .map(|row_key| row_key == reveal_row_key)
                        .unwrap_or(false)
                })
            });
            if let Some(row_index) = reveal_row_index {
                let row_top = content_origin.y + row_index as f32 * row_height_with_spacing;
                let visible_reveal_rect = egui::Rect::from_min_max(
                    egui::pos2(content_origin.x + viewport.min.x, row_top),
                    egui::pos2(
                        content_origin.x + viewport.max.x,
                        row_top + row_height_with_spacing,
                    ),
                );
                ui.scroll_to_rect(visible_reveal_rect, Some(egui::Align::Center));
            }

            for row_index in min_row..max_row {
                let actual_row_index = row_indices[row_index];
                let row = &rows[actual_row_index];
                let row_top = content_origin.y + row_index as f32 * row_height_with_spacing;
                let row_rect = egui::Rect::from_min_size(
                    egui::pos2(content_origin.x, row_top),
                    egui::vec2(content_width, row_height_with_spacing),
                );

                let is_active_shortwire_row = tree_state
                    .shortwire_active_row_key
                    .zip(row.row_key())
                    .map(|(active, current)| active == current)
                    .unwrap_or(false);

                let row_alpha = if is_shortwire_active && !is_active_shortwire_row {
                    0.3
                } else {
                    1.0
                };

                let selected = tree_state
                    .focused_row_key
                    .zip(row.row_key())
                    .map(|(selected, row_key)| selected == row_key)
                    .or_else(|| {
                        tree_state
                            .focused_target_id
                            .zip(row.target_id())
                            .map(|(selected, target)| selected == target)
                    })
                    .unwrap_or(false);
                let row_key = row.row_key();
                let expandable = row_key
                    .zip(tree_state.expandable_row_keys)
                    .map(|(row_key, expandable_row_keys)| expandable_row_keys.contains(row_key))
                    .unwrap_or(false);
                let expanded = row_key
                    .zip(tree_state.expanded_row_keys)
                    .map(|(row_key, expanded_row_keys)| expanded_row_keys.contains(row_key))
                    .unwrap_or(false);
                let path_index = row_key.and_then(|row_key| {
                    tree_state
                        .path_row_keys
                        .iter()
                        .position(|path_row_key| path_row_key == row_key)
                });
                let response = if row.selectable() {
                    ui.interact(row_rect, id.with(row_index), egui::Sense::click())
                } else {
                    ui.interact(row_rect, id.with(row_index), egui::Sense::hover())
                };

                if row.selectable() && !is_shortwire_active {
                    response.context_menu(|ui| {
                        let enabled = tree_state.shortwire_can_enter;
                        if ui
                            .add_enabled(enabled, egui::Button::new("Shortwire"))
                            .clicked()
                        {
                            context_menu_row_index = Some(actual_row_index);
                            ui.close();
                        }
                    });
                }

                let row_patch_key = shortwire_patch_key(row);
                let shortwire_dot_info = row
                    .selectable()
                    .then(|| tree_state.shortwire_dot_info.get(&row_patch_key))
                    .flatten()
                    .copied();
                let mut hover_lines = Vec::new();
                if let Some(relation_path) = row.relation_path() {
                    hover_lines.push(format!("Path: {relation_path}"));
                }
                if let Some(dot_info) = shortwire_dot_info {
                    hover_lines.push(shortwire_dot_hover_text(dot_info));
                }
                let response = if hover_lines.is_empty() {
                    response
                } else {
                    response.on_hover_text(hover_lines.join("\n"))
                };
                let indent = row.depth() as f32 * TREE_ROW_INDENT_WIDTH;
                let toggle_slot = if tree_state.expandable_row_keys.is_some() && row_key.is_some() {
                    TREE_ROW_INDENT_WIDTH
                } else {
                    0.0
                };
                let text_x = row_rect.left() + indent + toggle_slot;
                let label_width = ui
                    .painter()
                    .layout_no_wrap(
                        row.label().to_string(),
                        font_id.clone(),
                        ui.visuals().text_color(),
                    )
                    .size()
                    .x;
                let source_jump_range = row.source_jump_range();
                let source_jump_rect = source_jump_range.map(|_| {
                    let button_size = source_jump_button_size(ui, &font_id);
                    egui::Rect::from_min_size(
                        egui::pos2(
                            text_x + label_width + TREE_ROW_SOURCE_JUMP_GAP,
                            row_rect.center().y - button_size.y * 0.5,
                        ),
                        button_size,
                    )
                });
                let source_jump_response = source_jump_rect.map(|button_rect| {
                    ui.interact(
                        button_rect,
                        id.with(("source-jump", row_key.unwrap_or_default(), row_index)),
                        egui::Sense::click(),
                    )
                    .on_hover_text("Jump to source")
                });
                let mut toggle_clicked = false;
                let mut toggle_hovered = false;
                let mut toggle_rect = None;
                let toggle_symbol = if expandable {
                    let next_toggle_rect = egui::Rect::from_min_size(
                        egui::pos2(row_rect.left() + indent, row_rect.top()),
                        egui::vec2(TREE_ROW_INDENT_WIDTH, row_height_with_spacing),
                    );
                    let toggle_id = id.with(("toggle", row_key.unwrap_or_default().to_string()));
                    let toggle_response =
                        ui.interact(next_toggle_rect, toggle_id, egui::Sense::click());
                    toggle_clicked = toggle_response.clicked();
                    toggle_hovered = toggle_response.hovered();
                    toggle_rect = Some(next_toggle_rect);
                    Some(if expanded { "-" } else { "+" })
                } else {
                    None
                };

                if let Some(path_index) = path_index {
                    ui.painter().rect_filled(
                        row_rect,
                        0.0,
                        dependency_path_color(ui, path_index, tree_state.path_row_keys.len()),
                    );
                }
                if is_active_shortwire_row {
                    ui.painter()
                        .rect_filled(row_rect, 0.0, tree_selected_row_bg(ui));
                } else if selected {
                    ui.painter()
                        .rect_filled(row_rect, 0.0, tree_selected_row_bg(ui));
                } else if row.selectable() && response.hovered() {
                    ui.painter()
                        .rect_filled(row_rect, 0.0, tree_hovered_row_bg(ui));
                }

                if toggle_clicked {
                    clicked_row = Some(PassDebugTreeClick {
                        row_key: None,
                        target_id: None,
                        source_range: None,
                        toggle_row_key: row.row_key().map(str::to_string),
                    });
                } else if source_jump_response
                    .as_ref()
                    .map(|response| response.clicked())
                    .unwrap_or(false)
                {
                    clicked_row = Some(PassDebugTreeClick {
                        row_key: row.row_key().map(str::to_string),
                        target_id: row.target_id().map(str::to_string),
                        source_range: source_jump_range,
                        toggle_row_key: None,
                    });
                } else if response.clicked()
                    && (row.target_id().is_some() || row.source_range().is_some())
                {
                    clicked_row = Some(PassDebugTreeClick {
                        row_key: row.row_key().map(str::to_string),
                        target_id: row.target_id().map(str::to_string),
                        source_range: row.source_range(),
                        toggle_row_key: None,
                    });
                }

                let text_color = if selected || is_active_shortwire_row {
                    tree_highlight_text_color(ui)
                } else {
                    let base_color = ui.visuals().text_color();
                    if row_alpha < 1.0 {
                        let [r, g, b, _] = base_color.to_srgba_unmultiplied();
                        egui::Color32::from_rgba_unmultiplied(r, g, b, (255.0 * row_alpha) as u8)
                    } else {
                        base_color
                    }
                };
                let has_stored_patch = shortwire_dot_info.is_some();
                let dot_offset = if has_stored_patch { 8.0 } else { 0.0 };

                if let Some(dot_info) = shortwire_dot_info {
                    let dot_radius = 3.0;
                    let dot_center = egui::pos2(text_x + dot_radius, row_rect.center().y);
                    let dot_color = shortwire_dot_color(ui, dot_info.status, row_alpha);
                    ui.painter()
                        .circle_filled(dot_center, dot_radius, dot_color);
                }

                let galley = ui.painter().layout_no_wrap(
                    row.label().to_string(),
                    font_id.clone(),
                    text_color,
                );
                let text_pos = egui::pos2(
                    text_x + dot_offset,
                    row_rect.center().y - galley.size().y * 0.5,
                );
                ui.painter().galley(text_pos, galley, text_color);
                if let (Some(toggle_rect), Some(toggle_symbol)) = (toggle_rect, toggle_symbol) {
                    paint_tree_toggle_symbol(
                        ui,
                        toggle_rect,
                        toggle_symbol,
                        toggle_hovered,
                        &font_id,
                    );
                }
                if let (Some(button_rect), Some(button_response)) =
                    (source_jump_rect, source_jump_response.as_ref())
                {
                    paint_source_jump_button(ui, button_rect, button_response.hovered(), &font_id);
                }
            }
        });

    ShortwireTreeResult {
        click: clicked_row,
        context_menu_row_index,
    }
}

fn source_jump_button_size(ui: &egui::Ui, font_id: &egui::FontId) -> egui::Vec2 {
    let text_color = ui.visuals().text_color();
    let label_size = ui
        .painter()
        .layout_no_wrap(
            TREE_ROW_SOURCE_JUMP_LABEL.to_string(),
            font_id.clone(),
            text_color,
        )
        .size();
    egui::vec2(
        label_size.x + TREE_ROW_SOURCE_JUMP_HORIZONTAL_PADDING * 2.0,
        label_size.y + TREE_ROW_SOURCE_JUMP_VERTICAL_PADDING * 2.0,
    )
}

fn paint_tree_toggle_symbol(
    ui: &egui::Ui,
    rect: egui::Rect,
    symbol: &str,
    hovered: bool,
    font_id: &egui::FontId,
) {
    let symbol_color = if hovered {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let symbol_galley =
        ui.painter()
            .layout_no_wrap(symbol.to_string(), font_id.clone(), symbol_color);
    let symbol_pos = egui::pos2(
        rect.center().x - symbol_galley.size().x * 0.5,
        rect.center().y - symbol_galley.size().y * 0.5,
    );
    ui.painter().galley(symbol_pos, symbol_galley, symbol_color);
}

fn paint_source_jump_button(
    ui: &egui::Ui,
    rect: egui::Rect,
    hovered: bool,
    font_id: &egui::FontId,
) {
    let fill = if hovered {
        source_jump_button_hover_bg(ui)
    } else {
        source_jump_button_bg(ui)
    };
    let text_color = if hovered {
        tree_highlight_text_color(ui)
    } else {
        ui.visuals().weak_text_color()
    };
    ui.painter().rect_filled(rect, 3.0, fill);
    let galley = ui.painter().layout_no_wrap(
        TREE_ROW_SOURCE_JUMP_LABEL.to_string(),
        font_id.clone(),
        text_color,
    );
    let text_pos = egui::pos2(
        rect.center().x - galley.size().x * 0.5,
        rect.center().y - galley.size().y * 0.5,
    );
    ui.painter().galley(text_pos, galley, text_color);
}

fn dependency_path_color(ui: &egui::Ui, index: usize, len: usize) -> egui::Color32 {
    let t = if len <= 1 {
        1.0
    } else {
        index as f32 / (len - 1) as f32
    };
    let (start, end) = if ui.visuals().dark_mode {
        (
            egui::Color32::from_rgba_unmultiplied(96, 165, 250, 26),
            egui::Color32::from_rgba_unmultiplied(245, 158, 11, 38),
        )
    } else {
        (
            egui::Color32::from_rgba_unmultiplied(37, 99, 235, 20),
            egui::Color32::from_rgba_unmultiplied(180, 83, 9, 28),
        )
    };
    lerp_color(start, end, t)
}

fn tree_selected_row_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(44, 58, 76)
    } else {
        egui::Color32::from_rgb(218, 231, 248)
    }
}

fn tree_hovered_row_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 18)
    } else {
        egui::Color32::from_rgba_unmultiplied(0, 0, 0, 10)
    }
}

fn source_jump_button_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(148, 163, 184, 30)
    } else {
        egui::Color32::from_rgba_unmultiplied(71, 85, 105, 20)
    }
}

fn source_jump_button_hover_bg(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(96, 165, 250, 62)
    } else {
        egui::Color32::from_rgba_unmultiplied(37, 99, 235, 42)
    }
}

fn tree_highlight_text_color(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgb(238, 242, 247)
    } else {
        egui::Color32::from_rgb(20, 31, 46)
    }
}

fn shortwire_dot_info_for_patch(patch: &ShortwireNodePatch) -> ShortwireDotInfo {
    match patch.diff_result.as_ref() {
        Some(diff_result) => ShortwireDotInfo {
            status: shortwire_diff_status(diff_result),
            max_ae: Some(diff_result.max_ae),
            sample_count: Some(diff_result.sample_count),
        },
        None => ShortwireDotInfo {
            status: ShortwireDotStatus::PendingDiff,
            max_ae: None,
            sample_count: None,
        },
    }
}

fn shortwire_dot_hover_text(dot_info: ShortwireDotInfo) -> String {
    match (dot_info.max_ae, dot_info.sample_count) {
        (Some(max_ae), Some(sample_count)) => {
            format!(
                "Shortwire diff: max AE {max_ae:.6} ({:.2}/255), threshold < 2/255, n {sample_count}",
                max_ae * 255.0
            )
        }
        _ => "Shortwire diff: pending".to_string(),
    }
}

fn shortwire_dot_color(ui: &egui::Ui, status: ShortwireDotStatus, alpha: f32) -> egui::Color32 {
    let base = match (status, ui.visuals().dark_mode) {
        (ShortwireDotStatus::PendingDiff, true) => egui::Color32::from_rgb(250, 204, 21),
        (ShortwireDotStatus::PendingDiff, false) => egui::Color32::from_rgb(202, 138, 4),
        (ShortwireDotStatus::Passing, true) => egui::Color32::from_rgb(74, 222, 128),
        (ShortwireDotStatus::Passing, false) => egui::Color32::from_rgb(22, 163, 74),
        (ShortwireDotStatus::Failing, true) => egui::Color32::from_rgb(248, 113, 113),
        (ShortwireDotStatus::Failing, false) => egui::Color32::from_rgb(220, 38, 38),
    };
    let [r, g, b, _] = base.to_srgba_unmultiplied();
    egui::Color32::from_rgba_unmultiplied(r, g, b, (220.0 * alpha) as u8)
}

fn lerp_color(a: egui::Color32, b: egui::Color32, t: f32) -> egui::Color32 {
    let t = t.clamp(0.0, 1.0);
    let [ar, ag, ab, aa] = a.to_srgba_unmultiplied();
    let [br, bg, bb, ba] = b.to_srgba_unmultiplied();
    let lerp = |a: u8, b: u8| -> u8 { (a as f32 + (b as f32 - a as f32) * t).round() as u8 };
    egui::Color32::from_rgba_unmultiplied(lerp(ar, br), lerp(ag, bg), lerp(ab, bb), lerp(aa, ba))
}

fn pass_shader_status(document: &PassDebugWindowDocument) -> String {
    if let Some(error) = document.last_error.as_ref() {
        return format!("Patch failed: {}", compact_patch_error(error));
    }
    if let Some(status) = document.last_status.as_ref() {
        return status.clone();
    }
    if let Some(active) = document.shortwire_active.as_ref() {
        if active.base_source_stale {
            "Shortwire stale".to_string()
        } else {
            "Shortwire".to_string()
        }
    } else if document.dirty {
        "Dirty".to_string()
    } else if document.merge_conflict.is_some() {
        "Conflict".to_string()
    } else if document.patch_active {
        "Patched".to_string()
    } else {
        "Generated".to_string()
    }
}

fn reference_status(document: &PassDebugWindowDocument) -> String {
    if let Some(status) = document.reference_workspace.last_status.as_deref() {
        return status.to_string();
    }
    if document.reference_workspace.shortwire_active_key.is_some() {
        "Shortwire".to_string()
    } else if document.reference_workspace.selected_file_dirty()
        || document.reference_workspace.has_dirty_files()
        || document.reference_workspace.manifest_dirty
    {
        "Syncing".to_string()
    } else if !document.reference_workspace.has_content() {
        "Empty".to_string()
    } else if document.reference_workspace.skipped_files > 0 {
        format!(
            "Saved, {} skipped",
            document.reference_workspace.skipped_files
        )
    } else {
        "Saved".to_string()
    }
}

fn pass_debug_save_enabled(document: &PassDebugWindowDocument) -> bool {
    if document.shortwire_active.is_some() {
        document.shortwire_is_editor_interactive()
    } else {
        document.dirty && document.merge_conflict.is_none()
    }
}

fn request_pass_debug_save(
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    if document.shortwire_active.is_some() {
        if document.shortwire_is_editor_interactive() {
            document.exit_shortwire_done(pending_actions);
        }
        return;
    }

    if !document.dirty || document.merge_conflict.is_some() {
        return;
    }

    document.refresh_draft_analysis();
    document.last_error = None;
    document.last_status = Some("Applying patch...".to_string());
    push_action(
        pending_actions,
        PassDebugWindowAction::ApplyPatch {
            pass_name: document.pass_name.clone(),
            source: document.draft_source.clone(),
            reference_image: None,
        },
    );
}

fn request_pass_debug_close(
    ui: &egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
    send_viewport_close: bool,
) {
    if document.shortwire_active.is_some() {
        document.exit_shortwire_navigate(pending_actions);
        return;
    }

    document.prepare_debug_window_close(pending_actions);
    close_requested.store(true, Ordering::Relaxed);
    if send_viewport_close {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

fn render_status_badge(ui: &mut egui::Ui, text: impl Into<String>) {
    ui.label(egui::RichText::new(text.into()).monospace().small());
}

fn render_reference_file_selector(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    let now_secs = ui.input(|input| input.time);
    let shortwire_active = document.reference_workspace.shortwire_active_key.is_some();
    let selected_label = document
        .reference_workspace
        .selected_file
        .as_deref()
        .unwrap_or("No file")
        .to_string();
    let file_choices = document
        .reference_workspace
        .files
        .iter()
        .map(|file| file.relative_path.clone())
        .collect::<Vec<_>>();
    let pass_name = document.pass_name.clone();

    ui.add_enabled_ui(!shortwire_active && !file_choices.is_empty(), |ui| {
        egui::ComboBox::from_id_salt(("pass-debug-reference-file", pass_name.as_str()))
            .selected_text(selected_label.as_str())
            .width(220.0)
            .show_ui(ui, |ui| {
                for relative_path in file_choices {
                    let selected = document.reference_workspace.selected_file.as_deref()
                        == Some(relative_path.as_str());
                    if ui
                        .selectable_label(selected, relative_path.as_str())
                        .clicked()
                        && document.reference_workspace.select_file(&relative_path)
                    {
                        document.reference_workspace.sync_due_secs = Some(now_secs);
                        document.reference_line_galley_cache = None;
                    }
                }
            });
    });
}

fn render_pass_debug_titlebar(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
    send_viewport_close: bool,
) {
    let save_requested =
        ui.input(|input| input.modifiers.command && input.key_pressed(egui::Key::S));
    ui.horizontal(|ui| {
        ui.heading(format!("RenderPass Debug - {}", document.pass_name));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let diff_enabled = document.shortwire_active.is_some();
            let diff_active = document
                .shortwire_active
                .as_ref()
                .is_some_and(|active| active.diff_view_enabled);
            ui.add_enabled_ui(diff_enabled, |ui| {
                if ui.selectable_label(diff_active, "Diff").clicked()
                    && let Some(active) = document.shortwire_active.as_mut()
                {
                    active.diff_view_enabled = !active.diff_view_enabled;
                }
            });

            if ui.button("Close").clicked() {
                request_pass_debug_close(
                    ui,
                    document,
                    pending_actions,
                    close_requested,
                    send_viewport_close,
                );
            }

            let save_enabled = pass_debug_save_enabled(document);
            if ui
                .add_enabled(save_enabled, egui::Button::new("Save"))
                .clicked()
            {
                request_pass_debug_save(document, pending_actions);
            }
        });
    });

    if save_requested && pass_debug_save_enabled(document) {
        request_pass_debug_save(document, pending_actions);
    }
}

fn render_pass_debug_column_headers(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    panel_rect: egui::Rect,
    current_rect: egui::Rect,
    reference_rect: egui::Rect,
) {
    let mut panel_header_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-side-header", document.pass_name.as_str()))
            .max_rect(panel_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    panel_header_ui.set_clip_rect(panel_rect.intersect(ui.clip_rect()));
    panel_header_ui.label(egui::RichText::new("Deps Tree").strong());

    let mut current_header_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-editor-header", document.pass_name.as_str()))
            .max_rect(current_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    current_header_ui.set_clip_rect(current_rect.intersect(ui.clip_rect()));
    current_header_ui.label(egui::RichText::new("Pass Shader").strong());
    render_status_badge(&mut current_header_ui, pass_shader_status(document));

    let mut reference_header_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-reference-header", document.pass_name.as_str()))
            .max_rect(reference_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    reference_header_ui.set_clip_rect(reference_rect.intersect(ui.clip_rect()));
    reference_header_ui.label(egui::RichText::new("Reference").strong());
    render_status_badge(&mut reference_header_ui, reference_status(document));
    reference_header_ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let now_secs = ui.input(|input| input.time);
        let shortwire_active = document.reference_workspace.shortwire_active_key.is_some();
        if ui
            .add_enabled(
                !shortwire_active && document.reference_workspace.root_path.is_some(),
                egui::Button::new("Reload"),
            )
            .clicked()
        {
            document.reload_reference_workspace(now_secs);
        }
        if ui
            .add_enabled(!shortwire_active, egui::Button::new("Open"))
            .on_hover_text("Open reference folder")
            .clicked()
            && let Some(path) = rfd::FileDialog::new().pick_folder()
        {
            document.import_reference_folder_from_path(&path, now_secs);
        }
        render_reference_file_selector(ui, document);
    });
}

fn render_shortwire_diff_editor(
    ui: &mut egui::Ui,
    id_salt: (&'static str, &str),
    base_source: &str,
    current_source: &str,
) {
    let view = build_shortwire_diff_view(base_source, current_source);
    let mut diff_text = view.to_display_text();

    ui.scope(|ui| {
        ui.visuals_mut().text_cursor.preview = false;
        egui::ScrollArea::both()
            .id_salt(id_salt)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add(
                    egui::TextEdit::multiline(&mut diff_text)
                        .id_salt((id_salt.0, "text", id_salt.1))
                        .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
                        .code_editor()
                        .interactive(false)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin {
                            left: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            right: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            top: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                            bottom: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                        })
                        .desired_rows(24)
                        .desired_width(f32::INFINITY)
                        .lock_focus(true),
                );
            });
    });
}

fn compact_patch_error(error: &str) -> String {
    let first_line = error
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("Unknown patch error");
    let compact = first_line.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= PASS_DEBUG_PATCH_ERROR_SUMMARY_CHARS {
        return compact;
    }

    let keep = PASS_DEBUG_PATCH_ERROR_SUMMARY_CHARS.saturating_sub(3);
    let mut out = compact.chars().take(keep).collect::<String>();
    out.push_str("...");
    out
}

fn render_missing_source_message(ui: &mut egui::Ui) {
    ui.colored_label(
        egui::Color32::from_rgb(255, 180, 120),
        "Pass no longer exists in the current scene.",
    );
}

fn render_dependency_editor_split(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    close_requested: &AtomicBool,
    send_viewport_close: bool,
) {
    render_pass_debug_titlebar(
        ui,
        document,
        pending_actions,
        close_requested,
        send_viewport_close,
    );
    ui.separator();

    let full_rect = ui.available_rect_before_wrap();
    if full_rect.width() <= 0.0 || full_rect.height() <= PASS_DEBUG_COLUMN_HEADER_HEIGHT {
        return;
    }
    let header_height = PASS_DEBUG_COLUMN_HEADER_HEIGHT.min(full_rect.height());
    let body_rect = egui::Rect::from_min_max(
        egui::pos2(full_rect.left(), full_rect.top() + header_height),
        full_rect.right_bottom(),
    );

    let tree_split_id = egui::Id::new(("pass-debug-split-width", document.pass_name.as_str()));
    let editor_split_id =
        egui::Id::new(("pass-debug-editor-split-width", document.pass_name.as_str()));
    let available_for_panel = (body_rect.width() - PASS_DEBUG_SPLIT_HANDLE_WIDTH * 2.0).max(0.0);
    let max_panel_width = SIDE_PANEL_MAX_WIDTH
        .min(
            (available_for_panel - PASS_DEBUG_EDITOR_MIN_WIDTH * 2.0)
                .max(SIDE_PANEL_MIN_WIDTH)
                .min(available_for_panel),
        )
        .max(0.0);
    let min_panel_width = SIDE_PANEL_MIN_WIDTH.min(max_panel_width);
    let panel_width = ui
        .ctx()
        .data_mut(|data| {
            data.get_persisted::<f32>(tree_split_id)
                .unwrap_or(SIDE_PANEL_DEFAULT_WIDTH)
        })
        .clamp(min_panel_width, max_panel_width);

    let panel_rect = egui::Rect::from_min_max(
        body_rect.min,
        egui::pos2(body_rect.left() + panel_width, body_rect.bottom()),
    );
    let handle_rect = egui::Rect::from_min_max(
        egui::pos2(panel_rect.right(), full_rect.top()),
        egui::pos2(
            panel_rect.right() + PASS_DEBUG_SPLIT_HANDLE_WIDTH,
            full_rect.bottom(),
        ),
    );
    let editors_rect = egui::Rect::from_min_max(
        egui::pos2(handle_rect.right(), body_rect.top()),
        body_rect.right_bottom(),
    );

    let handle_response = ui.interact(
        handle_rect,
        tree_split_id.with("handle"),
        egui::Sense::click_and_drag(),
    );
    if handle_response.hovered() || handle_response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if handle_response.dragged() {
        let next_width =
            (panel_width + handle_response.drag_delta().x).clamp(min_panel_width, max_panel_width);
        ui.ctx()
            .data_mut(|data| data.insert_persisted(tree_split_id, next_width));
        ui.ctx().request_repaint();
    }

    let line_x = handle_rect.center().x;
    let line_color = if handle_response.hovered() || handle_response.dragged() {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    ui.painter().rect_filled(
        egui::Rect::from_center_size(
            egui::pos2(line_x, handle_rect.center().y),
            egui::vec2(PASS_DEBUG_SPLIT_LINE_WIDTH, handle_rect.height()),
        ),
        0.0,
        line_color,
    );

    let editors_available_width = (editors_rect.width() - PASS_DEBUG_SPLIT_HANDLE_WIDTH).max(0.0);
    let max_current_width = (editors_available_width - PASS_DEBUG_EDITOR_MIN_WIDTH)
        .max(PASS_DEBUG_EDITOR_MIN_WIDTH)
        .min(editors_available_width);
    let min_current_width = PASS_DEBUG_EDITOR_MIN_WIDTH.min(max_current_width);
    let current_width = ui
        .ctx()
        .data_mut(|data| {
            data.get_persisted::<f32>(editor_split_id)
                .unwrap_or(editors_available_width * 0.5)
        })
        .clamp(min_current_width, max_current_width);

    let current_rect = egui::Rect::from_min_max(
        editors_rect.min,
        egui::pos2(editors_rect.left() + current_width, editors_rect.bottom()),
    );
    let editor_handle_rect = egui::Rect::from_min_max(
        egui::pos2(current_rect.right(), full_rect.top()),
        egui::pos2(
            current_rect.right() + PASS_DEBUG_SPLIT_HANDLE_WIDTH,
            full_rect.bottom(),
        ),
    );
    let reference_rect = egui::Rect::from_min_max(
        egui::pos2(editor_handle_rect.right(), editors_rect.top()),
        editors_rect.right_bottom(),
    );

    let header_top = full_rect.top();
    let header_bottom = header_top + header_height;
    render_pass_debug_column_headers(
        ui,
        document,
        egui::Rect::from_min_max(
            egui::pos2(panel_rect.left(), header_top),
            egui::pos2(panel_rect.right(), header_bottom),
        ),
        egui::Rect::from_min_max(
            egui::pos2(current_rect.left(), header_top),
            egui::pos2(current_rect.right(), header_bottom),
        ),
        egui::Rect::from_min_max(
            egui::pos2(reference_rect.left(), header_top),
            egui::pos2(reference_rect.right(), header_bottom),
        ),
    );

    let editor_handle_response = ui.interact(
        editor_handle_rect,
        editor_split_id.with("handle"),
        egui::Sense::click_and_drag(),
    );
    if editor_handle_response.hovered() || editor_handle_response.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    if editor_handle_response.dragged() {
        let next_width = (current_width + editor_handle_response.drag_delta().x)
            .clamp(min_current_width, max_current_width);
        ui.ctx()
            .data_mut(|data| data.insert_persisted(editor_split_id, next_width));
        ui.ctx().request_repaint();
    }

    let editor_line_x = editor_handle_rect.center().x;
    let editor_line_color = if editor_handle_response.hovered() || editor_handle_response.dragged()
    {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    ui.painter().rect_filled(
        egui::Rect::from_center_size(
            egui::pos2(editor_line_x, editor_handle_rect.center().y),
            egui::vec2(PASS_DEBUG_SPLIT_LINE_WIDTH, editor_handle_rect.height()),
        ),
        0.0,
        editor_line_color,
    );
    ui.painter().rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(full_rect.left(), header_bottom),
            egui::pos2(
                full_rect.right(),
                header_bottom + PASS_DEBUG_SPLIT_LINE_WIDTH,
            ),
        ),
        0.0,
        ui.visuals().widgets.noninteractive.bg_stroke.color,
    );

    let t_dep = Instant::now();
    let mut panel_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-side-child", document.pass_name.as_str()))
            .max_rect(panel_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    panel_ui.set_clip_rect(panel_rect.intersect(ui.clip_rect()));
    render_side_panel(&mut panel_ui, document, pending_actions);
    let dep_dur = t_dep.elapsed();

    let t_editor = Instant::now();
    let mut editor_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-editor-child", document.pass_name.as_str()))
            .max_rect(current_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    editor_ui.set_clip_rect(current_rect.intersect(ui.clip_rect()));
    render_current_editor_column(&mut editor_ui, document, pending_actions);
    let editor_dur = t_editor.elapsed();

    let t_reference = Instant::now();
    let mut reference_ui = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("pass-debug-reference-child", document.pass_name.as_str()))
            .max_rect(reference_rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    reference_ui.set_clip_rect(reference_rect.intersect(ui.clip_rect()));
    render_reference_editor_column(&mut reference_ui, document, pending_actions);
    let reference_dur = t_reference.elapsed();

    metric_log!(
        "[pass-debug] split pass={} dependency_panel={:.2}ms code_editor={:.2}ms reference_editor={:.2}ms",
        document.pass_name,
        dep_dur.as_secs_f64() * 1000.0,
        editor_dur.as_secs_f64() * 1000.0,
        reference_dur.as_secs_f64() * 1000.0,
    );

    ui.advance_cursor_after_rect(full_rect);
}

fn render_current_editor_column(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    render_code_editor(ui, document);
    render_merge_conflict_popups(ui.ctx(), document, pending_actions);
}

fn render_reference_editor_column(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let now_secs = ui.input(|input| input.time);
    render_reference_editor(ui, document);
    document.maybe_emit_reference_upsert(now_secs, pending_actions);
}

fn layout_with_line_cache_incremental(
    ui: &egui::Ui,
    text: &str,
    wrap_width: f32,
    theme: &crate::ui::wgsl_highlight::WgslTheme,
    cache_cell: &std::cell::RefCell<Option<LineGalleyCache>>,
) -> std::sync::Arc<egui::Galley> {
    let pixels_per_point = ui.ctx().pixels_per_point();
    let rounded_wrap = wrap_width.round();
    let hasher_state = ahash::RandomState::with_seeds(1, 2, 3, 4);

    let cache_reusable = cache_cell.borrow().as_ref().is_some_and(|c| {
        (c.wrap_width - rounded_wrap).abs() < 0.5
            && (c.pixels_per_point - pixels_per_point).abs() < f32::EPSILON
    });

    let t_phase1 = Instant::now();
    let mut line_hashes_new: Vec<u64> = Vec::with_capacity(800);
    let line_boundaries = line_boundaries_for_layout(text);
    for &(start, end) in &line_boundaries {
        let hash = hasher_state.hash_one(&text[start..end]);
        line_hashes_new.push(hash);
    }
    let phase1_ms = t_phase1.elapsed().as_secs_f64() * 1000.0;

    if cache_reusable {
        let cache_ref = cache_cell.borrow();
        if let Some(ref c) = *cache_ref {
            if c.line_hashes.len() == line_hashes_new.len() && c.line_hashes == line_hashes_new {
                let merged = std::sync::Arc::clone(&c.merged);
                drop(cache_ref);
                metric_log!(
                    "[pass-debug] line_cache lines={} all_hit (fast path)",
                    line_hashes_new.len(),
                );
                return merged;
            }
        }
    }

    let t_phase3 = Instant::now();
    let prev_cache = if cache_reusable {
        cache_cell.borrow_mut().take()
    } else {
        None
    };

    struct PrevEntry<'a> {
        galley: &'a std::sync::Arc<egui::Galley>,
        sections: &'a Vec<egui::text::LayoutSection>,
    }
    let prev_lookup: HashMap<u64, PrevEntry<'_>> = prev_cache
        .as_ref()
        .map(|c| {
            c.line_hashes
                .iter()
                .zip(c.line_galleys.iter().zip(c.line_sections.iter()))
                .map(|(&h, (g, s))| {
                    (
                        h,
                        PrevEntry {
                            galley: g,
                            sections: s,
                        },
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let phase3_setup_ms = t_phase3.elapsed().as_secs_f64() * 1000.0;

    let num_lines = line_boundaries.len();
    let mut line_galleys: Vec<std::sync::Arc<egui::Galley>> = Vec::with_capacity(num_lines);
    let mut line_sections_vec: Vec<Vec<egui::text::LayoutSection>> = Vec::with_capacity(num_lines);
    let mut cache_hits = 0usize;

    for (i, &(start, end)) in line_boundaries.iter().enumerate() {
        if let Some(entry) = prev_lookup.get(&line_hashes_new[i]) {
            line_galleys.push(std::sync::Arc::clone(entry.galley));
            line_sections_vec.push(entry.sections.clone());
            cache_hits += 1;
            continue;
        }

        let line_text = &text[start..end];
        let sections = highlighted_line_sections_for_layout(line_text, theme);

        let paragraph_job = egui::text::LayoutJob {
            text: line_text.to_owned(),
            wrap: egui::text::TextWrapping {
                max_width: rounded_wrap,
                max_rows: usize::MAX,
                ..Default::default()
            },
            sections: sections.clone(),
            break_on_newline: true,
            halign: egui::Align::LEFT,
            justify: false,
            first_row_min_height: 0.0,
            round_output_to_gui: true,
        };

        let galley = ui.fonts_mut(|fonts| fonts.layout_job(paragraph_job));
        line_galleys.push(galley);
        line_sections_vec.push(sections);
    }

    let t_concat = Instant::now();
    let full_job = build_full_layout_job(
        text,
        &line_boundaries,
        &line_sections_vec,
        rounded_wrap,
        theme,
    );
    let full_job_arc = Arc::new(full_job);

    let merged = if cache_hits == num_lines {
        if let Some(ref prev) = prev_cache {
            if prev.merged.job.text == text {
                std::sync::Arc::clone(&prev.merged)
            } else {
                std::sync::Arc::new(egui::Galley::concat(
                    full_job_arc,
                    &line_galleys,
                    pixels_per_point,
                ))
            }
        } else {
            std::sync::Arc::new(egui::Galley::concat(
                full_job_arc,
                &line_galleys,
                pixels_per_point,
            ))
        }
    } else {
        std::sync::Arc::new(egui::Galley::concat(
            full_job_arc,
            &line_galleys,
            pixels_per_point,
        ))
    };
    let concat_ms = t_concat.elapsed().as_secs_f64() * 1000.0;

    metric_log!(
        "[pass-debug] line_cache lines={} hits={} misses={} p1={:.2}ms p3s={:.2}ms concat={:.2}ms",
        num_lines,
        cache_hits,
        num_lines - cache_hits,
        phase1_ms,
        phase3_setup_ms,
        concat_ms,
    );

    *cache_cell.borrow_mut() = Some(LineGalleyCache {
        wrap_width: rounded_wrap,
        pixels_per_point,
        line_hashes: line_hashes_new,
        line_sections: line_sections_vec,
        line_galleys,
        merged: std::sync::Arc::clone(&merged),
    });

    merged
}

fn line_boundaries_for_layout(text: &str) -> Vec<(usize, usize)> {
    let mut boundaries = Vec::with_capacity(text.lines().count().saturating_add(1));
    let mut start = 0usize;

    for (idx, ch) in text.char_indices() {
        if ch == '\n' {
            boundaries.push((start, idx));
            start = idx + ch.len_utf8();
        }
    }

    if start < text.len() || text.ends_with('\n') || text.is_empty() {
        boundaries.push((start, text.len()));
    }

    boundaries
}

fn highlighted_line_sections_for_layout(
    line: &str,
    theme: &crate::ui::wgsl_highlight::WgslTheme,
) -> Vec<egui::text::LayoutSection> {
    let mut sections = crate::ui::wgsl_highlight::highlight_wgsl_line(line, theme);
    if sections.is_empty() {
        sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: 0..0,
            format: egui::text::TextFormat {
                font_id: theme.font_id.clone(),
                color: theme.default,
                ..Default::default()
            },
        });
    }
    sections
}

fn build_full_layout_job(
    text: &str,
    line_boundaries: &[(usize, usize)],
    line_sections: &[Vec<egui::text::LayoutSection>],
    wrap_width: f32,
    theme: &crate::ui::wgsl_highlight::WgslTheme,
) -> egui::text::LayoutJob {
    let default_fmt = egui::text::TextFormat {
        font_id: theme.font_id.clone(),
        color: theme.default,
        ..Default::default()
    };
    let mut all_sections = Vec::with_capacity(
        line_sections.iter().map(|s| s.len()).sum::<usize>() + line_boundaries.len(),
    );
    for (line_idx, &(start, end)) in line_boundaries.iter().enumerate() {
        for section in &line_sections[line_idx] {
            all_sections.push(egui::text::LayoutSection {
                leading_space: section.leading_space,
                byte_range: (section.byte_range.start + start)..(section.byte_range.end + start),
                format: section.format.clone(),
            });
        }
        if end < text.len() {
            all_sections.push(egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: end..end + 1,
                format: default_fmt.clone(),
            });
        }
    }
    let last_covered = all_sections.last().map_or(0, |s| s.byte_range.end);
    if last_covered < text.len() {
        all_sections.push(egui::text::LayoutSection {
            leading_space: 0.0,
            byte_range: last_covered..text.len(),
            format: default_fmt,
        });
    }
    egui::text::LayoutJob {
        text: text.to_owned(),
        wrap: egui::text::TextWrapping {
            max_width: wrap_width,
            max_rows: usize::MAX,
            ..Default::default()
        },
        sections: all_sections,
        break_on_newline: true,
        halign: egui::Align::LEFT,
        justify: false,
        first_row_min_height: 0.0,
        round_output_to_gui: true,
    }
}

fn render_code_editor(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    let now_secs = ui.input(|input| input.time);
    document.maybe_refresh_pending_draft_analysis(now_secs, ui.ctx());
    let focused_source_range = document.focused_source_range();

    metric_log!(
        "[pass-debug] code_editor pass={} source_len={}",
        document.pass_name,
        document.draft_source.len(),
    );

    if let Some(active) = document
        .shortwire_active
        .as_ref()
        .filter(|active| active.diff_view_enabled)
    {
        render_shortwire_diff_editor(
            ui,
            ("pass-debug-source-diff", document.pass_name.as_str()),
            &active.base_source,
            &document.draft_source,
        );
        return;
    }

    let existing_galley = document.line_galley_cache.as_ref().and_then(|c| {
        if c.merged.job.text == document.draft_source {
            Some(std::sync::Arc::clone(&c.merged))
        } else {
            None
        }
    });
    let precomputed_galley: std::cell::RefCell<Option<std::sync::Arc<egui::Galley>>> =
        std::cell::RefCell::new(existing_galley);

    let line_cache_cell: std::cell::RefCell<Option<LineGalleyCache>> =
        std::cell::RefCell::new(document.line_galley_cache.take());

    let font_id = pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE);
    let wgsl_theme = if ui.visuals().dark_mode {
        crate::ui::wgsl_highlight::WgslTheme::dark(font_id)
    } else {
        crate::ui::wgsl_highlight::WgslTheme::light(font_id)
    };

    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        if let Some(ref galley) = *precomputed_galley.borrow() {
            if galley.job.text == buf.as_str()
                && (galley.job.wrap.max_width - wrap_width).abs() < 0.5
            {
                return std::sync::Arc::clone(galley);
            }
        }

        let t_layouter = Instant::now();
        let galley = layout_with_line_cache_incremental(
            ui,
            buf.as_str(),
            wrap_width,
            &wgsl_theme,
            &line_cache_cell,
        );

        let layouter_ms = t_layouter.elapsed().as_secs_f64() * 1000.0;
        metric_log!(
            "[pass-debug] layouter_call={:.2}ms wrap_width={:.0} (incremental)",
            layouter_ms,
            wrap_width,
        );
        *precomputed_galley.borrow_mut() = Some(std::sync::Arc::clone(&galley));
        galley
    };

    ui.scope(|ui| {
        ui.visuals_mut().text_cursor.preview = false;
        egui::ScrollArea::vertical()
            .id_salt(("pass-debug-source-editor", document.pass_name.as_str()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let initial_line_count = line_boundaries_for_layout(&document.draft_source).len();
                let gutter_width = line_number_gutter_width(initial_line_count);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let gutter_top_left = ui.cursor().left_top();
                    ui.add_space(gutter_width);

                    let editor_interactive = (document.shortwire_is_editor_interactive()
                        || document.shortwire_active.is_none())
                        && document.merge_conflict.is_none();

                    let editor = egui::TextEdit::multiline(&mut document.draft_source)
                        .id_salt(("pass-debug-source-text", document.pass_name.as_str()))
                        .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
                        .code_editor()
                        .interactive(editor_interactive)
                        .frame(egui::Frame::NONE)
                        .margin(egui::Margin {
                            left: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            right: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                            top: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                            bottom: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                        })
                        .desired_rows(24)
                        .desired_width(f32::INFINITY)
                        .lock_focus(true)
                        .layouter(&mut layouter);

                    let t_show = Instant::now();
                    let output = editor.show(ui);
                    let show_ms = t_show.elapsed().as_secs_f64() * 1000.0;
                    metric_log!("[pass-debug] editor.show={:.2}ms", show_ms,);

                    let gutter_rect = egui::Rect::from_min_max(
                        gutter_top_left,
                        egui::pos2(
                            gutter_top_left.x + gutter_width,
                            output.response.rect.bottom(),
                        ),
                    );
                    let line_boundaries = line_boundaries_for_layout(&document.draft_source);
                    paint_line_number_gutter(
                        ui,
                        &output,
                        &document.draft_source,
                        &line_boundaries,
                        gutter_rect,
                    );

                    if document.shortwire_active.is_none() {
                        if let Some(source_range) = focused_source_range {
                            paint_focus_highlight_overlay(
                                ui,
                                &output,
                                &document.draft_source,
                                source_range,
                            );
                        }
                    }

                    if output.response.changed() {
                        document.mark_draft_edited(now_secs);
                    }
                    if let Some(source_range) = document.pending_editor_jump.take() {
                        jump_editor_to_source_range(
                            ui,
                            &output,
                            &document.draft_source,
                            source_range,
                        );
                    }
                    if document.shortwire_active.is_none()
                        && output.response.clicked()
                        && let Some(cursor_range) = output.cursor_range
                    {
                        document.refresh_draft_analysis();
                        document.focus_target_at_char_index(cursor_range.primary.index);
                    }
                });
            });
    });

    document.line_galley_cache = line_cache_cell.into_inner();
}

fn render_merge_conflict_popups(
    ctx: &egui::Context,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let choice_popup_open = document
        .merge_conflict
        .as_ref()
        .map(|conflict| conflict.choice_popup_open)
        .unwrap_or(false);
    if choice_popup_open {
        egui::Window::new("Shader patch conflict")
            .id(egui::Id::new((
                "pass-debug-merge-choice",
                document.pass_name.as_str(),
            )))
            .collapsible(false)
            .resizable(false)
            .default_width(420.0)
            .show(ctx, |ui| {
                ui.label("Generated WGSL changed while a shortwire patch is applied.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Discard Shortwire Patch").clicked() {
                        document.use_merge_incoming(pending_actions);
                        if let Some(conflict) = document.merge_conflict.as_mut() {
                            conflict.choice_popup_open = false;
                            conflict.resolver_window_open = false;
                        }
                    }
                    if ui.button("Resolve Conflict").clicked() {
                        document.open_merge_resolver();
                    }
                });
            });
    }

    let resolver_open = document
        .merge_conflict
        .as_ref()
        .map(|conflict| conflict.resolver_window_open)
        .unwrap_or(false);
    if resolver_open {
        let mut open = true;
        egui::Window::new(format!("Resolve Shader Conflict - {}", document.pass_name))
            .id(egui::Id::new((
                "pass-debug-merge-resolver",
                document.pass_name.as_str(),
            )))
            .default_size(egui::vec2(1180.0, 720.0))
            .min_size(egui::vec2(760.0, 420.0))
            .open(&mut open)
            .show(ctx, |ui| {
                render_merge_conflict_resolver(ui, document, pending_actions);
            });
        if !open {
            if let Some(conflict) = document.merge_conflict.as_mut() {
                conflict.resolver_window_open = false;
                conflict.choice_popup_open = true;
            }
        }
    }
}

fn render_merge_conflict_resolver(
    ui: &mut egui::Ui,
    document: &mut PassDebugWindowDocument,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let Some(conflict) = document.merge_conflict.as_ref() else {
        return;
    };
    let base_source = conflict.base_source.clone();
    let incoming_source = conflict.incoming_source.clone();
    let local_source = conflict.local_source.clone();
    let conflict_error = conflict.error.clone();

    ui.horizontal(|ui| {
        ui.heading("Resolve conflict");
        ui.label(
            egui::RichText::new("Base / Incoming / Local")
                .monospace()
                .small(),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button("Cancel").clicked() {
                document.cancel_merge_resolution();
            }
            if ui.button("Keep Local").clicked() {
                document.keep_merge_local(pending_actions);
            }
            if ui.button("Use Incoming").clicked() {
                document.use_merge_incoming(pending_actions);
            }
            if ui.button("Apply Resolved").clicked() {
                document.apply_merge_resolved(pending_actions);
            }
        });
    });

    ui.label(
        egui::RichText::new(format!("Automatic merge failed: {conflict_error}"))
            .monospace()
            .small(),
    );
    ui.add_space(6.0);

    ui.columns(3, |columns| {
        render_readonly_merge_panel(&mut columns[0], "Base", &base_source);
        render_readonly_merge_panel(&mut columns[1], "Incoming", &incoming_source);
        render_readonly_merge_panel(&mut columns[2], "Local Patch", &local_source);
    });

    ui.separator();
    ui.heading("Resolved");
    if let Some(conflict) = document.merge_conflict.as_mut() {
        let font_id = pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE);
        ui.add(
            egui::TextEdit::multiline(&mut conflict.resolved_source)
                .id_salt(("pass-debug-merge-resolved", document.pass_name.as_str()))
                .font(font_id)
                .code_editor()
                .desired_rows(16)
                .desired_width(f32::INFINITY)
                .lock_focus(true),
        );
    }
}

fn render_readonly_merge_panel(ui: &mut egui::Ui, title: &str, source: &str) {
    ui.label(egui::RichText::new(title).monospace().strong());
    let mut text = source.to_string();
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
            .code_editor()
            .interactive(false)
            .desired_rows(12)
            .desired_width(f32::INFINITY),
    );
}

fn render_reference_editor(ui: &mut egui::Ui, document: &mut PassDebugWindowDocument) {
    let now_secs = ui.input(|input| input.time);
    if document.reference_workspace.selected_file().is_none() {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Open a UTF-8 text file or folder to add reference code.")
                .monospace()
                .small(),
        );
        return;
    }

    if let Some(base_source) = document
        .shortwire_active
        .as_ref()
        .filter(|active| active.diff_view_enabled)
        .and_then(|_| {
            document
                .reference_workspace
                .shortwire_base_source
                .as_deref()
        })
    {
        render_shortwire_diff_editor(
            ui,
            ("pass-debug-reference-diff", document.pass_name.as_str()),
            base_source,
            &document.reference_workspace.editor_source,
        );
        return;
    }

    let existing_galley = document.reference_line_galley_cache.as_ref().and_then(|c| {
        if c.merged.job.text == document.reference_workspace.editor_source {
            Some(std::sync::Arc::clone(&c.merged))
        } else {
            None
        }
    });
    let precomputed_galley: std::cell::RefCell<Option<std::sync::Arc<egui::Galley>>> =
        std::cell::RefCell::new(existing_galley);

    let line_cache_cell: std::cell::RefCell<Option<LineGalleyCache>> =
        std::cell::RefCell::new(document.reference_line_galley_cache.take());

    let font_id = pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE);
    let wgsl_theme = if ui.visuals().dark_mode {
        crate::ui::wgsl_highlight::WgslTheme::dark(font_id)
    } else {
        crate::ui::wgsl_highlight::WgslTheme::light(font_id)
    };

    let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
        if let Some(ref galley) = *precomputed_galley.borrow() {
            if galley.job.text == buf.as_str()
                && (galley.job.wrap.max_width - wrap_width).abs() < 0.5
            {
                return std::sync::Arc::clone(galley);
            }
        }

        let galley = layout_with_line_cache_incremental(
            ui,
            buf.as_str(),
            wrap_width,
            &wgsl_theme,
            &line_cache_cell,
        );
        *precomputed_galley.borrow_mut() = Some(std::sync::Arc::clone(&galley));
        galley
    };

    ui.scope(|ui| {
        ui.visuals_mut().text_cursor.preview = false;
        egui::ScrollArea::vertical()
            .id_salt(("pass-debug-reference-editor", document.pass_name.as_str()))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let initial_line_count =
                    line_boundaries_for_layout(&document.reference_workspace.editor_source).len();
                let gutter_width = line_number_gutter_width(initial_line_count);

                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let gutter_top_left = ui.cursor().left_top();
                    ui.add_space(gutter_width);

                    let editor =
                        egui::TextEdit::multiline(&mut document.reference_workspace.editor_source)
                            .id_salt(("pass-debug-reference-text", document.pass_name.as_str()))
                            .font(pass_debug_mono_font(PASS_DEBUG_CODE_FONT_SIZE))
                            .code_editor()
                            .frame(egui::Frame::NONE)
                            .margin(egui::Margin {
                                left: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                                right: PASS_DEBUG_CODE_EDITOR_MARGIN_X,
                                top: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                                bottom: PASS_DEBUG_CODE_EDITOR_MARGIN_Y,
                            })
                            .desired_rows(24)
                            .desired_width(f32::INFINITY)
                            .lock_focus(true)
                            .layouter(&mut layouter);

                    let output = editor.show(ui);
                    let gutter_rect = egui::Rect::from_min_max(
                        gutter_top_left,
                        egui::pos2(
                            gutter_top_left.x + gutter_width,
                            output.response.rect.bottom(),
                        ),
                    );
                    let line_boundaries =
                        line_boundaries_for_layout(&document.reference_workspace.editor_source);
                    paint_line_number_gutter(
                        ui,
                        &output,
                        &document.reference_workspace.editor_source,
                        &line_boundaries,
                        gutter_rect,
                    );

                    if output.response.changed() {
                        document.mark_reference_edited(now_secs);
                    }
                });
            });
    });

    document.reference_line_galley_cache = line_cache_cell.into_inner();
}

fn line_number_gutter_width(line_count: usize) -> f32 {
    let digits = line_count.max(1).to_string().len() as f32;
    (digits * PASS_DEBUG_LINE_NUMBER_GUTTER_DIGIT_WIDTH
        + PASS_DEBUG_LINE_NUMBER_GUTTER_RIGHT_PADDING
        + 10.0)
        .clamp(
            PASS_DEBUG_LINE_NUMBER_GUTTER_MIN_WIDTH,
            PASS_DEBUG_LINE_NUMBER_GUTTER_MAX_WIDTH,
        )
        .ceil()
}

fn paint_line_number_gutter(
    ui: &egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    source: &str,
    line_boundaries: &[(usize, usize)],
    gutter_rect: egui::Rect,
) {
    if line_boundaries.is_empty() {
        return;
    }

    let clip_rect = gutter_rect.intersect(ui.clip_rect());
    if clip_rect.is_negative() {
        return;
    }

    let painter = ui.painter_at(clip_rect);
    let separator_x = gutter_rect.right() - 0.5;
    painter.line_segment(
        [
            egui::pos2(separator_x, gutter_rect.top()),
            egui::pos2(separator_x, gutter_rect.bottom()),
        ],
        egui::Stroke::new(1.0, line_number_separator_color(ui)),
    );

    let active_line = output
        .cursor_range
        .and_then(|range| line_index_at_char_index(source, range.primary.index, line_boundaries));
    let line_start_chars = line_start_char_indices_for_layout(source, line_boundaries);
    let number_x = gutter_rect.right() - PASS_DEBUG_LINE_NUMBER_GUTTER_RIGHT_PADDING;
    let font_id = pass_debug_mono_font(PASS_DEBUG_LINE_NUMBER_FONT_SIZE);

    for (line_idx, &start_char) in line_start_chars.iter().enumerate() {
        let cursor_rect = output
            .galley
            .pos_from_cursor(egui::text::CCursor::new(start_char))
            .translate(output.galley_pos.to_vec2());
        if cursor_rect.bottom() < clip_rect.top() || cursor_rect.top() > clip_rect.bottom() {
            continue;
        }

        let is_active = active_line == Some(line_idx);
        painter.text(
            egui::pos2(number_x, cursor_rect.center().y),
            egui::Align2::RIGHT_CENTER,
            (line_idx + 1).to_string(),
            font_id.clone(),
            line_number_text_color(ui, is_active),
        );
    }
}

fn line_start_char_indices_for_layout(
    source: &str,
    line_boundaries: &[(usize, usize)],
) -> Vec<usize> {
    let mut starts = Vec::with_capacity(line_boundaries.len());
    let mut char_index = 0usize;

    for &(start, end) in line_boundaries {
        starts.push(char_index);
        char_index += source[start..end].chars().count();
        if end < source.len() {
            char_index += 1;
        }
    }

    starts
}

fn line_index_at_char_index(
    source: &str,
    char_index: usize,
    line_boundaries: &[(usize, usize)],
) -> Option<usize> {
    let byte_index = char_index_to_byte_index(source, char_index);
    for (line_idx, &(start, end)) in line_boundaries.iter().enumerate() {
        let line_end_exclusive = if end < source.len() { end + 1 } else { end };
        if byte_index >= start && byte_index < line_end_exclusive {
            return Some(line_idx);
        }
    }
    if byte_index == source.len() {
        return line_boundaries.len().checked_sub(1);
    }
    None
}

fn line_number_separator_color(ui: &egui::Ui) -> egui::Color32 {
    if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(255, 255, 255, 26)
    } else {
        egui::Color32::from_rgba_unmultiplied(15, 23, 42, 30)
    }
}

fn line_number_text_color(ui: &egui::Ui, active: bool) -> egui::Color32 {
    if active {
        if ui.visuals().dark_mode {
            egui::Color32::from_rgb(191, 219, 254)
        } else {
            egui::Color32::from_rgb(30, 64, 175)
        }
    } else if ui.visuals().dark_mode {
        egui::Color32::from_rgba_unmultiplied(203, 213, 225, 96)
    } else {
        egui::Color32::from_rgba_unmultiplied(51, 65, 85, 106)
    }
}

fn paint_focus_highlight_overlay(
    ui: &egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    source: &str,
    source_range: PassDebugSourceRange,
) {
    let highlight_start = source_range.start_byte;
    let highlight_end = source_range.end_byte;
    if highlight_start >= highlight_end
        || highlight_end > source.len()
        || !source.is_char_boundary(highlight_start)
        || !source.is_char_boundary(highlight_end)
    {
        return;
    }

    let start_char = byte_index_to_char_index(source, highlight_start);
    let end_char = byte_index_to_char_index(source, highlight_end);
    let highlight_color = egui::Color32::from_rgba_premultiplied(251, 191, 36, 56);
    let galley = &output.galley;
    let galley_pos = output.galley_pos;

    let start_cursor = galley.layout_from_cursor(egui::text::CCursor::new(start_char));
    let end_cursor = galley.layout_from_cursor(egui::text::CCursor::new(end_char));

    if start_cursor.row == end_cursor.row {
        let start_rect = galley.pos_from_layout_cursor(&start_cursor);
        let end_rect = galley.pos_from_layout_cursor(&end_cursor);
        let row = &galley.rows[start_cursor.row];
        let rect = egui::Rect::from_min_max(
            egui::pos2(start_rect.left() + galley_pos.x, row.pos.y + galley_pos.y),
            egui::pos2(
                end_rect.left() + galley_pos.x,
                row.pos.y + row.row.size.y + galley_pos.y,
            ),
        );
        ui.painter().rect_filled(rect, 0.0, highlight_color);
    } else {
        for row_idx in start_cursor.row..=end_cursor.row {
            let Some(row) = galley.rows.get(row_idx) else {
                break;
            };
            let row_top = row.pos.y + galley_pos.y;
            let row_bottom = row_top + row.row.size.y;

            let left = if row_idx == start_cursor.row {
                let cursor_rect = galley.pos_from_layout_cursor(&start_cursor);
                cursor_rect.left() + galley_pos.x
            } else {
                row.pos.x + galley_pos.x
            };
            let right = if row_idx == end_cursor.row {
                let cursor_rect = galley.pos_from_layout_cursor(&end_cursor);
                cursor_rect.left() + galley_pos.x
            } else {
                row.pos.x + row.row.size.x + galley_pos.x
            };

            if right > left {
                let rect = egui::Rect::from_min_max(
                    egui::pos2(left, row_top),
                    egui::pos2(right, row_bottom),
                );
                ui.painter().rect_filled(rect, 0.0, highlight_color);
            }
        }
    }
}

fn jump_editor_to_source_range(
    ui: &mut egui::Ui,
    output: &egui::widgets::text_edit::TextEditOutput,
    source: &str,
    source_range: PassDebugSourceRange,
) {
    if source_range.start_byte >= source_range.end_byte || source_range.end_byte > source.len() {
        return;
    }

    let start_char = byte_index_to_char_index(source, source_range.start_byte);
    let end_char = byte_index_to_char_index(source, source_range.end_byte).max(start_char + 1);
    let selection = egui::text::CCursorRange::two(
        egui::text::CCursor::new(start_char),
        egui::text::CCursor::new(end_char),
    );
    let mut state = output.state.clone();
    state.cursor.set_char_range(Some(selection));
    state.store(ui.ctx(), output.response.id);
    output.response.request_focus();

    let cursor_rect = output
        .galley
        .pos_from_cursor(egui::text::CCursor::new(start_char))
        .translate(output.galley_pos.to_vec2())
        .expand2(egui::vec2(0.0, 64.0));
    ui.scroll_to_rect(cursor_rect, Some(egui::Align::Center));
}

fn reference_workspace_loaded_matches(
    current: &ReferenceWorkspaceState,
    incoming: &ReferenceWorkspaceState,
) -> bool {
    current.root_path == incoming.root_path
        && current.root_label == incoming.root_label
        && current.selected_file == incoming.selected_file
        && current.skipped_files == incoming.skipped_files
        && current.files.len() == incoming.files.len()
        && current
            .files
            .iter()
            .zip(incoming.files.iter())
            .all(|(a, b)| {
                a.relative_path == b.relative_path
                    && a.artifact_id == b.artifact_id
                    && a.source == b.source
                    && a.loaded_source == b.loaded_source
            })
}

fn load_reference_workspace_from_artifacts(
    pass_name: &str,
    workspace_text: Option<&str>,
    reference_files: &[crate::debug_artifacts::DebugArtifactTextSnapshot],
    legacy_reference_source: Option<&str>,
) -> Option<(ReferenceWorkspaceState, bool)> {
    let file_text_by_id = reference_files
        .iter()
        .map(|snapshot| (snapshot.item.id.as_str(), snapshot.text.as_str()))
        .collect::<HashMap<_, _>>();

    if let Some(workspace_text) = workspace_text
        && let Ok(manifest) = serde_json::from_str::<ReferenceWorkspaceManifest>(workspace_text)
        && manifest.version == REFERENCE_WORKSPACE_VERSION
    {
        let mut files = Vec::new();
        let root_path = manifest.root_path.clone();
        let root = root_path.as_deref().map(PathBuf::from);
        let mut local_loaded_count = 0usize;
        let mut archive_fallback_count = 0usize;
        let mut missing_count = 0usize;
        for manifest_file in manifest.files {
            let (source, loaded_from_local) = if let Some(root) = root.as_deref() {
                match read_manifest_reference_file(root, &manifest_file) {
                    Ok(source) => (source, true),
                    Err(_) => {
                        missing_count += 1;
                        continue;
                    }
                }
            } else {
                match file_text_by_id.get(manifest_file.artifact_id.as_str()) {
                    Some(text) => ((*text).to_string(), false),
                    None => {
                        missing_count += 1;
                        continue;
                    }
                }
            };
            if loaded_from_local {
                local_loaded_count += 1;
            } else {
                archive_fallback_count += 1;
            }
            files.push(ReferenceWorkspaceFile {
                relative_path: manifest_file.relative_path,
                artifact_id: manifest_file.artifact_id,
                source: source.clone(),
                loaded_source: source,
            });
        }

        if !files.is_empty() || root.is_some() {
            let mut state = ReferenceWorkspaceState::default();
            state.replace_files(
                root_path,
                manifest.root_label,
                files,
                manifest.selected_file,
                manifest.skipped_files + missing_count,
                false,
            );
            if root.is_some() {
                state.last_status = if missing_count > 0 {
                    Some(if local_loaded_count > 0 {
                        format!("Loaded local reference ({missing_count} missing)")
                    } else {
                        format!("Local reference missing ({missing_count} missing)")
                    })
                } else if local_loaded_count > 0 {
                    Some("Loaded local reference".to_string())
                } else {
                    None
                };
            } else if archive_fallback_count > 0 {
                state.last_status = Some("Loaded archived reference".to_string());
            }
            return Some((state, false));
        }
    }

    if !reference_files.is_empty() {
        let mut files = reference_files
            .iter()
            .map(|snapshot| {
                let relative_path = snapshot
                    .item
                    .name
                    .strip_prefix("Reference: ")
                    .unwrap_or(snapshot.item.name.as_str())
                    .to_string();
                ReferenceWorkspaceFile {
                    relative_path,
                    artifact_id: snapshot.item.id.clone(),
                    source: snapshot.text.clone(),
                    loaded_source: snapshot.text.clone(),
                }
            })
            .collect::<Vec<_>>();
        files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
        let mut state = ReferenceWorkspaceState::default();
        state.replace_files(None, "Reference archive".to_string(), files, None, 0, false);
        return Some((state, false));
    }

    let legacy_reference_source = legacy_reference_source.unwrap_or_default();
    if legacy_reference_source.is_empty() {
        return None;
    }

    let relative_path = "reference.txt".to_string();
    let file = ReferenceWorkspaceFile {
        artifact_id: pass_reference_file_artifact_id(pass_name, &relative_path),
        relative_path: relative_path.clone(),
        source: legacy_reference_source.to_string(),
        loaded_source: String::new(),
    };
    let mut state = ReferenceWorkspaceState::default();
    state.replace_files(
        None,
        "Legacy reference".to_string(),
        vec![file],
        Some(relative_path),
        0,
        true,
    );
    Some((state, true))
}

fn read_manifest_reference_file(
    root: &Path,
    manifest_file: &ReferenceWorkspaceManifestFile,
) -> Result<String, String> {
    let path = root.join(&manifest_file.relative_path);
    let metadata = fs::metadata(&path)
        .map_err(|error| format!("Failed to inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("Not a file: {}", path.display()));
    }
    if metadata.len() > PASS_DEBUG_REFERENCE_MAX_FILE_BYTES {
        return Err(format!(
            "File is larger than {} MB: {}",
            PASS_DEBUG_REFERENCE_MAX_FILE_BYTES / (1024 * 1024),
            path.display()
        ));
    }
    let bytes =
        fs::read(&path).map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    String::from_utf8(bytes).map_err(|_| format!("File is not UTF-8 text: {}", path.display()))
}

fn write_reference_workspace_file(
    root: &Path,
    file: &ReferenceWorkspaceFile,
) -> Result<(), String> {
    let path = root.join(&file.relative_path);
    fs::write(&path, file.source.as_bytes())
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

fn read_reference_file(
    path: &Path,
    root: &Path,
    pass_name: &str,
    mark_dirty: bool,
) -> Result<ReferenceWorkspaceFile, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("Failed to inspect {}: {error}", path.display()))?;
    if !metadata.is_file() {
        return Err(format!("Not a file: {}", path.display()));
    }
    if metadata.len() > PASS_DEBUG_REFERENCE_MAX_FILE_BYTES {
        return Err(format!(
            "File is larger than {} MB: {}",
            PASS_DEBUG_REFERENCE_MAX_FILE_BYTES / (1024 * 1024),
            path.display()
        ));
    }
    let bytes =
        fs::read(path).map_err(|error| format!("Failed to read {}: {error}", path.display()))?;
    let source = String::from_utf8(bytes)
        .map_err(|_| format!("File is not UTF-8 text: {}", path.display()))?;
    let relative_path = reference_relative_path(root, path);
    Ok(ReferenceWorkspaceFile {
        artifact_id: pass_reference_file_artifact_id(pass_name, &relative_path),
        relative_path,
        loaded_source: if mark_dirty {
            String::new()
        } else {
            source.clone()
        },
        source,
    })
}

fn read_reference_folder(
    root: &Path,
    pass_name: &str,
    mark_dirty: bool,
) -> Result<(Vec<ReferenceWorkspaceFile>, usize), String> {
    if !root.is_dir() {
        return Err(format!("Not a folder: {}", root.display()));
    }

    let mut files = Vec::new();
    let mut skipped_files = 0usize;
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => {
                skipped_files += 1;
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(file_type) = entry.file_type() else {
                skipped_files += 1;
                continue;
            };
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                skipped_files += 1;
                continue;
            }
            if files.len() >= PASS_DEBUG_REFERENCE_MAX_FOLDER_FILES {
                skipped_files += 1;
                continue;
            }
            match read_reference_file(&path, root, pass_name, mark_dirty) {
                Ok(file) => files.push(file),
                Err(_) => skipped_files += 1,
            }
        }
    }

    files.sort_by(|a, b| a.relative_path.cmp(&b.relative_path));
    Ok((files, skipped_files))
}

fn reference_relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn reference_shortwire_patch_key(relative_path: &str, row_patch_key: &str) -> String {
    format!("{relative_path}::{row_patch_key}")
}

fn pass_reference_workspace_artifact_id(pass_name: &str) -> String {
    [
        "pass".to_string(),
        safe_debug_artifact_segment(pass_name, "unnamed"),
        "reference-workspace".to_string(),
        DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT.to_string(),
    ]
    .join("__")
}

fn pass_reference_file_artifact_id(pass_name: &str, relative_path: &str) -> String {
    [
        "pass".to_string(),
        safe_debug_artifact_segment(pass_name, "unnamed"),
        "reference-code".to_string(),
        format!(
            "file-{}",
            debug_artifact_content_hash(relative_path.as_bytes())
        ),
    ]
    .join("__")
}

fn pass_reference_file_slot_key(relative_path: &str) -> String {
    format!(
        "{}{}",
        DEBUG_ARTIFACT_REFERENCE_FILE_SLOT_PREFIX,
        debug_artifact_content_hash(relative_path.as_bytes())
    )
}

fn pass_reference_file_artifact_item(
    pass_name: &str,
    file: &ReferenceWorkspaceFile,
) -> DebugArtifactItem {
    let file_name =
        safe_debug_artifact_segment(&file.relative_path.replace('/', "__"), "reference");
    DebugArtifactItem {
        id: file.artifact_id.clone(),
        anchor: DebugArtifactAnchor::Pass {
            pass_name: pass_name.to_string(),
        },
        role: DebugArtifactRole::ReferenceCode,
        name: format!("Reference: {}", file.relative_path),
        mime_type: "text/plain".to_string(),
        path: format!(
            "debug-artifacts/{}/{}",
            safe_debug_artifact_segment(&file.artifact_id, "artifact"),
            file_name
        ),
        size: Some(file.size()),
        content_hash: Some(file.content_hash()),
        slot_key: Some(pass_reference_file_slot_key(&file.relative_path)),
    }
}

fn pass_reference_patches_artifact_id(pass_name: &str) -> String {
    [
        "pass".to_string(),
        safe_debug_artifact_segment(pass_name, "unnamed"),
        "patch".to_string(),
        DEBUG_ARTIFACT_REFERENCE_PATCHES_SLOT.to_string(),
    ]
    .join("__")
}

fn pass_patches_artifact_id(pass_name: &str) -> String {
    [
        "pass".to_string(),
        safe_debug_artifact_segment(pass_name, "unnamed"),
        "patch".to_string(),
        DEBUG_ARTIFACT_DEFAULT_SLOT.to_string(),
    ]
    .join("__")
}

fn safe_debug_artifact_segment(value: &str, fallback: &str) -> String {
    let safe: String = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() || safe.chars().all(|ch| ch == '.') {
        fallback.to_string()
    } else {
        safe
    }
}

fn debug_artifact_content_hash(bytes: &[u8]) -> String {
    let mut hash = 0x811c9dc5u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    format!("{hash:08x}")
}

fn push_action(
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    action: PassDebugWindowAction,
) {
    if let Ok(mut pending) = pending_actions.lock() {
        pending.push(action);
    }
}

fn pass_debug_mono_font(size: f32) -> egui::FontId {
    egui::FontId::new(size, egui::FontFamily::Name("geist_mono".into()))
}

fn clean_debug_tree_row_label(label: &str) -> String {
    let Some(stripped) = strip_leading_naga_handle(label.trim_start()) else {
        return label.to_string();
    };
    stripped.trim_start().to_string()
}

fn strip_leading_naga_handle(label: &str) -> Option<&str> {
    let rest = label.strip_prefix('[')?;
    let (handle, after_handle) = rest.split_once(']')?;
    if handle.is_empty() || !handle.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    Some(after_handle.strip_prefix(':').unwrap_or(after_handle))
}

fn flatten_dependency_tree(
    root: &PassDebugDependencyNode,
    source: &PassDebugSource,
) -> Vec<PassDebugDependencyRow> {
    let mut rows = Vec::new();
    push_dependency_rows(
        root,
        source,
        0,
        None,
        &mut vec![0],
        &mut Vec::new(),
        &mut HashSet::new(),
        &mut rows,
    );
    rows
}

fn push_dependency_rows(
    node: &PassDebugDependencyNode,
    source: &PassDebugSource,
    depth: usize,
    parent_row_key: Option<String>,
    path: &mut Vec<usize>,
    relation_path: &mut Vec<String>,
    reference_stack: &mut HashSet<String>,
    rows: &mut Vec<PassDebugDependencyRow>,
) {
    if node.target_id.is_some() {
        let row_key = dependency_row_key(path);
        let relation_path_text = relation_path.join(" / ");
        let target_id = node.target_id.clone();
        let label = dependency_target_row_label(
            source,
            target_id.as_deref(),
            &node.label,
            node.display_label.as_deref(),
            node.edge_label.as_deref(),
        );
        let target_range = target_id
            .as_deref()
            .and_then(|target_id| target_source_range(source, target_id));
        let source_range = node.source_range;
        let definition_source_range = node
            .definition_source_range
            .or_else(|| source_range.is_none().then_some(target_range).flatten());
        let source_jump_range = definition_source_range
            .filter(|definition_source_range| source_range != Some(*definition_source_range));
        rows.push(PassDebugDependencyRow {
            depth,
            row_key: row_key.clone(),
            parent_row_key,
            label,
            relation_path: relation_path_text,
            target_id: target_id.clone(),
            source_range,
            source_jump_range,
            selectable: true,
        });
        let reference_children = node
            .reference
            .then(|| target_id.as_deref())
            .flatten()
            .and_then(|target_id| {
                if reference_stack.insert(target_id.to_string()) {
                    source
                        .dependency_trees
                        .get(target_id)
                        .map(|tree| (target_id.to_string(), tree.children.as_slice()))
                } else {
                    None
                }
            });
        let children = reference_children
            .as_ref()
            .map(|(_, children)| *children)
            .unwrap_or_else(|| node.children.as_slice());
        for (index, child) in children.iter().enumerate() {
            path.push(index);
            let mut child_relation_path = Vec::new();
            push_dependency_rows(
                child,
                source,
                depth + 1,
                Some(row_key.clone()),
                path,
                &mut child_relation_path,
                reference_stack,
                rows,
            );
            path.pop();
        }
        if let Some((target_id, _)) = reference_children {
            reference_stack.remove(&target_id);
        }
    } else {
        let relation_label = compact_dependency_relation_label(&node.label);
        if !relation_label.is_empty() {
            relation_path.push(relation_label);
        }
        for (index, child) in node.children.iter().enumerate() {
            path.push(index);
            push_dependency_rows(
                child,
                source,
                depth,
                parent_row_key.clone(),
                path,
                relation_path,
                reference_stack,
                rows,
            );
            path.pop();
        }
        if !relation_path.is_empty() {
            relation_path.pop();
        }
    }
}

fn dependency_target_row_label(
    source: &PassDebugSource,
    target_id: Option<&str>,
    fallback_label: &str,
    display_label: Option<&str>,
    edge_label: Option<&str>,
) -> String {
    let fallback_label = clean_debug_tree_row_label(fallback_label);
    let base_label = display_label
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            target_id
                .and_then(|target_id| {
                    source
                        .dependency_targets
                        .iter()
                        .find(|target| target.id == target_id)
                })
                .map(|target| target.name.clone())
        })
        .unwrap_or_else(|| fallback_label.clone());
    let status = ["[cycle]", "[depth limit]"]
        .into_iter()
        .find(|status| fallback_label.contains(status));
    let mut label = match edge_label.map(str::trim).filter(|edge| !edge.is_empty()) {
        Some(edge) => format!("{base_label} ({edge})"),
        None => base_label,
    };
    if let Some(status) = status {
        label.push(' ');
        label.push_str(status);
    }
    label
}

fn dependency_row_key(path: &[usize]) -> String {
    path.iter()
        .map(usize::to_string)
        .collect::<Vec<_>>()
        .join("/")
}

fn compact_dependency_relation_label(label: &str) -> String {
    let label = clean_debug_tree_row_label(label);
    let label = label.trim();
    if let Some(rest) = label.strip_prefix('[')
        && let Some((edge, after_edge)) = rest.split_once(']')
    {
        return format!("{edge}{}", after_edge).trim().to_string();
    }
    label.to_string()
}

fn target_source_range(source: &PassDebugSource, target_id: &str) -> Option<PassDebugSourceRange> {
    source
        .dependency_targets
        .iter()
        .find(|target| target.id == target_id)
        .and_then(|target| target.source_range)
}

fn dependency_path_for_row_key(rows: &[PassDebugDependencyRow], row_key: &str) -> Vec<String> {
    if !rows.iter().any(|row| row.row_key == row_key) {
        return Vec::new();
    }
    let row_parent_by_key = rows
        .iter()
        .map(|row| (row.row_key.as_str(), row.parent_row_key.as_deref()))
        .collect::<HashMap<_, _>>();
    let mut path = Vec::new();
    let mut current = Some(row_key);
    while let Some(row_key) = current {
        path.push(row_key.to_string());
        current = row_parent_by_key.get(row_key).copied().flatten();
    }
    path.reverse();
    path
}

fn identifier_at_char_index(source: &str, char_index: usize) -> Option<String> {
    let byte_index = char_index_to_byte_index(source, char_index);
    if source.is_empty() || byte_index > source.len() {
        return None;
    }

    let mut start = byte_index.min(source.len());
    while start > 0 {
        let Some((prev_index, ch)) = source[..start].char_indices().next_back() else {
            break;
        };
        if is_wgsl_identifier_char(ch) {
            start = prev_index;
        } else {
            break;
        }
    }

    let mut end = byte_index.min(source.len());
    while end < source.len() {
        let Some(ch) = source[end..].chars().next() else {
            break;
        };
        if is_wgsl_identifier_char(ch) {
            end += ch.len_utf8();
        } else {
            break;
        }
    }

    if start == end {
        return None;
    }
    let ident = &source[start..end];
    if ident
        .chars()
        .next()
        .map(is_wgsl_identifier_start)
        .unwrap_or(false)
    {
        Some(ident.to_string())
    } else {
        None
    }
}

fn char_index_to_byte_index(source: &str, char_index: usize) -> usize {
    source
        .char_indices()
        .nth(char_index)
        .map(|(index, _)| index)
        .unwrap_or(source.len())
}

fn byte_index_to_char_index(source: &str, byte_index: usize) -> usize {
    let byte_index = byte_index.min(source.len());
    source[..byte_index].chars().count()
}

fn is_wgsl_identifier_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_wgsl_identifier_char(ch: char) -> bool {
    is_wgsl_identifier_start(ch) || ch.is_ascii_digit()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use rust_wgpu_fiber::eframe::egui;

    use super::{
        PassDebugCloseCancelReason, PassDebugCloseDecision, PassDebugDependencyRow,
        PassDebugTreeClick, PassDebugViewportSnapshot, PassDebugWindowDocument,
        byte_index_to_char_index, classify_pass_debug_close_request, dependency_path_for_row_key,
        flatten_dependency_tree, is_close_request_during_large_viewport_resize,
        pass_debug_viewport_builder, shortwire_click_matches_active_row,
    };
    use crate::app::{
        RefImageAlphaMode, RefImageMode, ShortwirePastedReferenceImage, ShortwireReferenceImage,
    };
    use crate::dsl::{DebugArtifactAnchor, DebugArtifactItem, DebugArtifactRole};
    use crate::renderer::{
        PassDebugDependencyNode, PassDebugDependencyTarget, PassDebugSource, PassDebugSourceRange,
    };

    fn has_target_named(document: &PassDebugWindowDocument, name: &str) -> bool {
        document
            .analysis_source
            .as_ref()
            .map(|source| {
                source
                    .dependency_targets
                    .iter()
                    .any(|target| target.name == name)
            })
            .unwrap_or(false)
    }

    fn target_id_by_name(document: &PassDebugWindowDocument, name: &str) -> String {
        document
            .analysis_source
            .as_ref()
            .and_then(|source| {
                source
                    .dependency_targets
                    .iter()
                    .find(|target| target.name == name)
            })
            .map(|target| target.id.clone())
            .unwrap_or_else(|| panic!("missing target named {name}"))
    }

    fn dependency_root_target_name(document: &PassDebugWindowDocument) -> String {
        let source = document
            .analysis_source
            .as_ref()
            .expect("missing analysis source");
        let root_id = document
            .dependency_root_target_id
            .as_deref()
            .expect("missing dependency root target");
        source
            .dependency_targets
            .iter()
            .find(|target| target.id == root_id)
            .map(|target| target.name.clone())
            .unwrap_or_else(|| panic!("missing root target id {root_id}"))
    }

    fn root_return_shader(root_name: &str, value: f32) -> String {
        format!(
            "@fragment\nfn fs_main() -> @location(0) f32 {{ let {root_name} = {value:.1}; return {root_name}; }}\n"
        )
    }

    fn dependency_rows_contain_label_fragment(
        document: &PassDebugWindowDocument,
        fragment: &str,
    ) -> bool {
        document
            .dependency_rows
            .iter()
            .any(|row| row.label.contains(fragment))
    }

    fn source_target_id_by_name(source: &PassDebugSource, name: &str) -> String {
        source
            .dependency_targets
            .iter()
            .find(|target| target.name == name)
            .map(|target| target.id.clone())
            .unwrap_or_else(|| panic!("missing target named {name}"))
    }

    fn unique_reference_temp_dir(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "node-forge-pass-debug-{label}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn row_parent_label(rows: &[PassDebugDependencyRow], label: &str) -> Option<String> {
        let row = rows
            .iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| panic!("missing dependency row label {label}"));
        let parent_row_key = row.parent_row_key.as_deref()?;
        rows.iter()
            .find(|row| row.row_key == parent_row_key)
            .map(|row| row.label.clone())
    }

    fn assert_row_parent_label(rows: &[PassDebugDependencyRow], label: &str, parent_label: &str) {
        let found = rows.iter().any(|row| {
            row.label == label
                && row.parent_row_key.as_deref().and_then(|parent_row_key| {
                    rows.iter()
                        .find(|parent| parent.row_key == parent_row_key)
                        .map(|parent| parent.label.as_str())
                }) == Some(parent_label)
        });
        assert!(
            found,
            "missing dependency row `{label}` under `{parent_label}`\nrows:\n{}",
            rows.iter()
                .map(|row| {
                    let parent_label = row
                        .parent_row_key
                        .as_deref()
                        .and_then(|parent_row_key| {
                            rows.iter()
                                .find(|parent| parent.row_key == parent_row_key)
                                .map(|parent| parent.label.as_str())
                        })
                        .unwrap_or("<root>");
                    format!("{} <- {parent_label}", row.label)
                })
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    fn dependency_row_by_label<'a>(
        rows: &'a [PassDebugDependencyRow],
        label: &str,
    ) -> &'a PassDebugDependencyRow {
        rows.iter()
            .find(|row| row.label == label)
            .unwrap_or_else(|| {
                panic!(
                    "missing dependency row `{label}`\nrows:\n{}",
                    rows.iter()
                        .map(|row| row.label.as_str())
                        .collect::<Vec<_>>()
                        .join("\n")
                )
            })
    }

    fn seed_reference_file(
        document: &mut PassDebugWindowDocument,
        relative_path: &str,
        source: &str,
    ) {
        let file = super::ReferenceWorkspaceFile {
            relative_path: relative_path.to_string(),
            artifact_id: super::pass_reference_file_artifact_id(&document.pass_name, relative_path),
            source: source.to_string(),
            loaded_source: source.to_string(),
        };
        document.reference_workspace.replace_files(
            None,
            "Reference test".to_string(),
            vec![file],
            Some(relative_path.to_string()),
            0,
            false,
        );
    }

    fn reference_file_item(pass_name: &str, relative_path: &str) -> DebugArtifactItem {
        DebugArtifactItem {
            id: super::pass_reference_file_artifact_id(pass_name, relative_path),
            anchor: DebugArtifactAnchor::Pass {
                pass_name: pass_name.to_string(),
            },
            role: DebugArtifactRole::ReferenceCode,
            name: format!("Reference: {relative_path}"),
            mime_type: "text/plain".to_string(),
            path: format!("debug-artifacts/test/{relative_path}"),
            size: None,
            content_hash: None,
            slot_key: Some(super::pass_reference_file_slot_key(relative_path)),
        }
    }

    fn reference_snapshot(
        pass_name: &str,
        relative_path: &str,
        text: &str,
    ) -> crate::debug_artifacts::DebugArtifactTextSnapshot {
        crate::debug_artifacts::DebugArtifactTextSnapshot {
            item: reference_file_item(pass_name, relative_path),
            text: text.to_string(),
        }
    }

    fn viewport_snapshot(
        inner_rect: egui::Rect,
        outer_rect: egui::Rect,
    ) -> PassDebugViewportSnapshot {
        PassDebugViewportSnapshot {
            inner_rect: Some(inner_rect),
            outer_rect: Some(outer_rect),
            monitor_size: Some(egui::vec2(1440.0, 900.0)),
            native_pixels_per_point: Some(2.0),
            focused: Some(true),
            visible: Some(true),
        }
    }

    #[test]
    fn debug_viewport_builder_only_sets_default_size_initially() {
        let first = pass_debug_viewport_builder("Debug".to_string(), true);
        assert_eq!(
            first.inner_size,
            Some(egui::vec2(
                super::PASS_DEBUG_WINDOW_DEFAULT_WIDTH,
                super::PASS_DEBUG_WINDOW_DEFAULT_HEIGHT
            ))
        );
        assert_eq!(
            first.min_inner_size,
            Some(egui::vec2(
                super::PASS_DEBUG_WINDOW_MIN_WIDTH,
                super::PASS_DEBUG_WINDOW_MIN_HEIGHT
            ))
        );

        let subsequent = pass_debug_viewport_builder("Debug".to_string(), false);
        assert_eq!(subsequent.inner_size, None);
        assert_eq!(subsequent.title.as_deref(), Some("Debug"));
        assert_eq!(subsequent.min_inner_size, first.min_inner_size);
    }

    #[test]
    fn line_boundaries_keep_trailing_empty_line() {
        assert_eq!(
            super::line_boundaries_for_layout("a\n"),
            vec![(0, 1), (2, 2)]
        );
    }

    #[test]
    fn line_boundaries_keep_consecutive_empty_lines() {
        assert_eq!(
            super::line_boundaries_for_layout("a\n\nb"),
            vec![(0, 1), (2, 2), (3, 4)]
        );
    }

    #[test]
    fn line_boundaries_include_empty_document_line() {
        assert_eq!(super::line_boundaries_for_layout(""), vec![(0, 0)]);
    }

    #[test]
    fn line_start_char_indices_track_unicode_and_empty_lines() {
        let source = "é\n\nabc";
        let boundaries = super::line_boundaries_for_layout(source);

        assert_eq!(
            super::line_start_char_indices_for_layout(source, &boundaries),
            vec![0, 2, 3]
        );
    }

    #[test]
    fn line_index_at_char_index_treats_line_start_as_next_line() {
        let source = "a\nb";
        let boundaries = super::line_boundaries_for_layout(source);

        assert_eq!(
            super::line_index_at_char_index(source, 0, &boundaries),
            Some(0)
        );
        assert_eq!(
            super::line_index_at_char_index(source, 1, &boundaries),
            Some(0)
        );
        assert_eq!(
            super::line_index_at_char_index(source, 2, &boundaries),
            Some(1)
        );
        assert_eq!(
            super::line_index_at_char_index(source, 3, &boundaries),
            Some(1)
        );
    }

    #[test]
    fn empty_line_layout_sections_keep_default_font() {
        let theme = crate::ui::wgsl_highlight::WgslTheme::dark(egui::FontId::monospace(14.0));
        let sections = super::highlighted_line_sections_for_layout("", &theme);

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0].byte_range, 0..0);
        assert_eq!(sections[0].format.font_id, theme.font_id);
        assert_eq!(sections[0].format.color, theme.default);
    }

    #[test]
    fn close_request_resize_guard_only_matches_large_size_changes() {
        let previous = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(800.0, 600.0));
        let maximized = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1440.0, 900.0));
        let nearly_same =
            egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(804.0, 604.0));

        assert!(is_close_request_during_large_viewport_resize(
            Some(previous),
            Some(maximized),
        ));
        assert!(!is_close_request_during_large_viewport_resize(
            Some(previous),
            Some(nearly_same),
        ));
        assert!(!is_close_request_during_large_viewport_resize(
            None,
            Some(maximized),
        ));
    }

    #[test]
    fn stable_focused_close_request_is_accepted() {
        let rect = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(800.0, 600.0));
        let snapshot = viewport_snapshot(rect, rect);

        assert_eq!(
            classify_pass_debug_close_request(Some(snapshot), snapshot),
            PassDebugCloseDecision::Accept
        );
    }

    #[test]
    fn transient_close_requests_are_canceled_during_focus_or_display_changes() {
        let previous_inner =
            egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(800.0, 600.0));
        let previous_outer =
            egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(820.0, 640.0));
        let previous = viewport_snapshot(previous_inner, previous_outer);

        let mut focus_lost = previous;
        focus_lost.focused = Some(false);
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), focus_lost),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::FocusLost)
        );

        let mut hidden = previous;
        hidden.visible = Some(false);
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), hidden),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::Hidden)
        );

        let mut monitor_changed = previous;
        monitor_changed.monitor_size = Some(egui::vec2(2560.0, 1440.0));
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), monitor_changed),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::MonitorChanged)
        );

        let mut scale_changed = previous;
        scale_changed.native_pixels_per_point = Some(1.0);
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), scale_changed),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::ScaleChanged)
        );

        let mut jumped = previous;
        jumped.outer_rect = Some(egui::Rect::from_min_size(
            egui::pos2(1200.0, 10.0),
            egui::vec2(820.0, 640.0),
        ));
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), jumped),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::ViewportJumped)
        );
    }

    #[test]
    fn dirty_draft_is_not_replaced_by_source_refresh() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.draft_source = "fn edited() {}\n".to_string();
        document.dirty = true;

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(document.draft_source, "fn edited() {}\n");
        assert!(document.dirty);
    }

    #[test]
    fn clean_document_tracks_source_refresh() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 1, Some("fn patched() {}\n"));

        assert_eq!(document.draft_source, "fn patched() {}\n");
        assert_eq!(document.generated_base_source, "fn generated() {}\n");
        assert!(document.patch_active);
        assert!(!document.dirty);
    }

    #[test]
    fn same_source_revision_does_not_refresh_document() {
        let source_text = root_return_shader("a", 1.0);
        let source = PassDebugSource::from_wgsl("p", source_text.clone());
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 7, false);

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 7, None);

        assert_eq!(document.draft_source, source_text);
        assert!(!document.patch_active);
    }

    #[test]
    fn target_list_refreshes_after_source_update() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var before: f32 = 0.0; return before; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        assert!(has_target_named(&document, "before"));

        let refreshed = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var after: f32 = 1.0; return after; }\n",
        );
        document.update_source(Some(&refreshed), 1, false);

        assert!(!has_target_named(&document, "before"));
        assert!(has_target_named(&document, "after"));
        assert!(!document.dependency_rows.is_empty());
    }

    #[test]
    fn dirty_draft_does_not_replace_canonical_dependency_tree() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var loaded: f32 = 0.0; return loaded; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.draft_source =
            "fn a() -> f32 { var draft: f32 = 1.0; return draft; }\n".to_string();
        document.dirty = true;
        document.refresh_draft_analysis();
        assert!(has_target_named(&document, "loaded"));
        assert!(!has_target_named(&document, "draft"));

        let refreshed = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var generated: f32 = 2.0; return generated; }\n",
        );
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(
            document.draft_source,
            "fn a() -> f32 { var draft: f32 = 1.0; return draft; }\n"
        );
        assert!(!has_target_named(&document, "draft"));
        assert!(has_target_named(&document, "generated"));
    }

    #[test]
    fn draft_edits_do_not_schedule_dependency_analysis() {
        let ctx = egui::Context::default();
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var before: f32 = 0.0; return before; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.replace_draft_source(
            "fn a() -> f32 { var after: f32 = 1.0; return after; }\n".to_string(),
        );
        document.mark_draft_edited(10.0);

        document.maybe_refresh_pending_draft_analysis(10.10, &ctx);
        assert!(has_target_named(&document, "before"));
        assert!(!has_target_named(&document, "after"));
        assert!(document.draft_analysis_due_secs.is_none());

        document.maybe_refresh_pending_draft_analysis(10.16, &ctx);
        assert!(has_target_named(&document, "before"));
        assert!(!has_target_named(&document, "after"));
        assert_eq!(document.draft_analysis_due_secs, None);
    }

    #[test]
    fn forced_draft_analysis_keeps_canonical_dependency_source() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "fn a() -> f32 { var before: f32 = 0.0; return before; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.replace_draft_source(
            "fn a() -> f32 { var forced: f32 = 1.0; return forced; }\n".to_string(),
        );
        document.mark_draft_edited(20.0);

        document.refresh_draft_analysis();

        assert!(has_target_named(&document, "before"));
        assert!(!has_target_named(&document, "forced"));
        assert_eq!(document.draft_analysis_due_secs, None);
    }

    #[test]
    fn dependency_render_caches_invalidate_on_expansion_and_source_refresh() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var a: f32 = 0.0; let b = a + 1.0; return b; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        assert!(document.visible_dependency_row_indices_cache.is_none());

        let first_visible = document.cached_visible_dependency_row_indices().to_vec();
        assert!(!first_visible.is_empty());
        assert!(document.visible_dependency_row_indices_cache.is_some());
        let first_rows_generation = document.dependency_rows_generation;
        let first_expansion_generation = document.dependency_expansion_generation;

        document.toggle_dependency_row_expanded("0");
        assert!(document.visible_dependency_row_indices_cache.is_none());
        assert_ne!(
            document.dependency_expansion_generation,
            first_expansion_generation
        );

        let _ = document.cached_visible_dependency_row_indices();
        assert!(document.visible_dependency_row_indices_cache.is_some());
        let refreshed = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var c: f32 = 2.0; return c; }\n",
        );
        document.update_source(Some(&refreshed), 1, false);

        assert!(document.visible_dependency_row_indices_cache.is_none());
        assert!(document.dependency_expandable_row_keys_cache.is_none());
        assert!(document.dependency_tree_width_cache.is_none());
        assert_ne!(document.dependency_rows_generation, first_rows_generation);
    }

    #[test]
    fn focusing_dependency_child_does_not_replace_root_tree() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) vec4f { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return vec4f(c, c, c, 1.0); }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let root_id = document.dependency_root_target_id.clone().unwrap();
        let child_id = target_id_by_name(&document, "b");

        document.focus_target(child_id.clone(), true);

        assert_eq!(
            document.dependency_root_target_id.as_deref(),
            Some(root_id.as_str())
        );
        assert_eq!(
            document.focused_target_id.as_deref(),
            Some(child_id.as_str())
        );
        assert_eq!(
            document.dependency_rows[0].target_id.as_deref(),
            Some(root_id.as_str())
        );
    }

    #[test]
    fn dependency_root_is_fragment_return_target() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var a: f32 = 0.0; let b = a + 1.0; return b; }\n",
        );
        let document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);

        assert_eq!(
            document.dependency_root_target_id.as_deref(),
            Some("fs_main::return")
        );
        assert_eq!(
            document.dependency_rows[0].target_id.as_deref(),
            Some("fs_main::return")
        );
    }

    #[test]
    fn dependency_rows_default_to_only_root_expanded() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return c; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);

        assert_eq!(
            document.dependency_expanded_row_keys,
            HashSet::from(["0".to_string()])
        );
        let visible_labels = document
            .visible_dependency_rows()
            .iter()
            .map(|row| row.label.clone())
            .collect::<Vec<_>>();
        assert_eq!(visible_labels, vec!["return".to_string(), "c".to_string()]);

        document.toggle_dependency_row_expanded("0");
        assert_eq!(
            document
                .visible_dependency_rows()
                .iter()
                .map(|row| row.label.clone())
                .collect::<Vec<_>>(),
            vec!["return".to_string()]
        );
    }

    #[test]
    fn editor_focus_expands_only_shortest_path_from_root() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) f32 { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return c; }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let a_id = target_id_by_name(&document, "a");
        let a_row_key = document
            .shortest_dependency_row_key_for_target(&a_id)
            .unwrap();
        let path = dependency_path_for_row_key(&document.dependency_rows, &a_row_key);

        document.dependency_expanded_row_keys = document
            .dependency_expandable_row_keys()
            .into_iter()
            .collect();
        document.focus_target_from_editor(a_id);

        let expected_expanded = path
            .iter()
            .take(path.len().saturating_sub(1))
            .cloned()
            .collect::<HashSet<_>>();
        assert_eq!(document.dependency_expanded_row_keys, expected_expanded);
        assert_eq!(
            document
                .visible_dependency_rows()
                .iter()
                .map(|row| row.label.clone())
                .collect::<Vec<_>>(),
            vec![
                "return".to_string(),
                "c".to_string(),
                "b (Add)".to_string(),
                "a (Add)".to_string()
            ]
        );
    }

    #[test]
    fn dependency_tree_click_focuses_without_queueing_reveal_scroll() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) vec4f { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; return vec4f(c, c, c, 1.0); }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let row_key = document.visible_dependency_rows()[1].row_key.clone();

        document.pending_dependency_reveal_row_key = None;
        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        assert_eq!(
            document.focused_dependency_row_key.as_deref(),
            Some(row_key.as_str())
        );
        assert_eq!(document.pending_dependency_reveal_row_key, None);
    }

    #[test]
    fn focusing_target_outside_current_map_does_not_move_root() {
        let source = PassDebugSource::from_wgsl(
            "p",
            "@fragment fn fs_main() -> @location(0) vec4f { var a: f32 = 0.0; let b = a + 1.0; let c = b + 1.0; let outside = 9.0; return vec4f(c, c, c, 1.0); }\n",
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let root_id = document.dependency_root_target_id.clone().unwrap();
        let outside_id = target_id_by_name(&document, "outside");

        document.focus_target(outside_id.clone(), true);

        assert_eq!(
            document.dependency_root_target_id.as_deref(),
            Some(root_id.as_str())
        );
        assert_eq!(
            document.focused_target_id.as_deref(),
            Some(outside_id.as_str())
        );
        assert_eq!(document.focused_dependency_row_key, None);
        assert!(!document.focus_is_in_dependency_root());
    }

    #[test]
    fn dependency_rows_hide_unselectable_intermediate_nodes() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: String::new(),
            ast_tree: Vec::new(),
            dependency_targets: Vec::new(),
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let rows = flatten_dependency_tree(
            &PassDebugDependencyNode {
                label: "fs_main x (local)".to_string(),
                edge_label: None,
                display_label: None,
                source_range: None,
                definition_source_range: None,
                target_id: Some("target::x".to_string()),
                reference: false,
                children: vec![PassDebugDependencyNode {
                    label: "[rhs] Binary Add".to_string(),
                    edge_label: None,
                    display_label: None,
                    source_range: None,
                    definition_source_range: None,
                    target_id: None,
                    reference: false,
                    children: vec![
                        PassDebugDependencyNode {
                            label: "[source] function argument fs_main::0".to_string(),
                            edge_label: None,
                            display_label: None,
                            source_range: None,
                            definition_source_range: None,
                            target_id: None,
                            reference: false,
                            children: Vec::new(),
                        },
                        PassDebugDependencyNode {
                            label: "fs_main uv (argument)".to_string(),
                            edge_label: None,
                            display_label: None,
                            source_range: None,
                            definition_source_range: None,
                            target_id: Some("target::uv".to_string()),
                            reference: false,
                            children: Vec::new(),
                        },
                    ],
                }],
            },
            &source,
        );

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].label, "fs_main x (local)");
        assert_eq!(rows[0].depth, 0);
        assert_eq!(rows[0].row_key, "0");
        assert_eq!(rows[0].parent_row_key, None);
        assert_eq!(rows[0].relation_path, "");
        assert_eq!(rows[0].target_id.as_deref(), Some("target::x"));
        assert_eq!(rows[1].label, "fs_main uv (argument)");
        assert_eq!(rows[1].depth, 1);
        assert_eq!(rows[1].row_key, "0/0/1");
        assert_eq!(rows[1].parent_row_key.as_deref(), Some("0"));
        assert!(rows[1].relation_path.contains("rhs Binary Add"));
        assert_eq!(rows[1].target_id.as_deref(), Some("target::uv"));
    }

    #[test]
    fn dependency_rows_display_target_name_with_edge_label() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: "let a = input.foo.bar.x;".to_string(),
            ast_tree: Vec::new(),
            dependency_targets: vec![
                PassDebugDependencyTarget {
                    id: "target::d".to_string(),
                    name: "d".to_string(),
                    label: "debug_main let d".to_string(),
                    scope: "debug_main".to_string(),
                    kind: "let".to_string(),
                    source_range: None,
                },
                PassDebugDependencyTarget {
                    id: "target::a".to_string(),
                    name: "a".to_string(),
                    label: "debug_main let a".to_string(),
                    scope: "debug_main".to_string(),
                    kind: "let".to_string(),
                    source_range: Some(PassDebugSourceRange {
                        start_byte: 4,
                        end_byte: 5,
                        line: 1,
                        column: 5,
                    }),
                },
            ],
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let rows = flatten_dependency_tree(
            &PassDebugDependencyNode {
                label: "debug_main let d (let)".to_string(),
                edge_label: None,
                display_label: None,
                source_range: None,
                definition_source_range: None,
                target_id: Some("target::d".to_string()),
                reference: false,
                children: vec![PassDebugDependencyNode {
                    label: "debug_main let a (let)".to_string(),
                    edge_label: Some("math_multiply".to_string()),
                    display_label: Some("input.foo.bar.x".to_string()),
                    source_range: Some(PassDebugSourceRange {
                        start_byte: 8,
                        end_byte: 23,
                        line: 1,
                        column: 9,
                    }),
                    definition_source_range: None,
                    target_id: Some("target::a".to_string()),
                    reference: false,
                    children: Vec::new(),
                }],
            },
            &source,
        );

        assert_eq!(rows[0].label, "d");
        assert_eq!(rows[1].label, "input.foo.bar.x (math_multiply)");
        let row_range = rows[1]
            .source_range
            .expect("expected row source range for full access path");
        assert_eq!(
            &source.module_source[row_range.start_byte..row_range.end_byte],
            "input.foo.bar.x"
        );
    }

    #[test]
    fn function_call_dependency_rows_keep_call_site_argument_subtrees() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(b: f32, c: f32) -> f32 {
    return b - c;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let source_b = 1.0;
    let source_c = 2.0;
    let b = source_b + 10.0;
    let c = source_c + 20.0;
    let a = foo(b, c);
    let d = bar(b, c);
    return a + d;
}
"#,
        );

        let a_id = source_target_id_by_name(&source, "a");
        let a_rows = flatten_dependency_tree(
            source
                .dependency_trees
                .get(&a_id)
                .expect("a dependency tree"),
            &source,
        );
        assert_eq!(row_parent_label(&a_rows, "b (foo)").as_deref(), Some("a"));
        assert_eq!(
            row_parent_label(&a_rows, "source_b (Add)").as_deref(),
            Some("b (foo)")
        );
        assert_eq!(row_parent_label(&a_rows, "c (foo)").as_deref(), Some("a"));
        assert_eq!(
            row_parent_label(&a_rows, "source_c (Add)").as_deref(),
            Some("c (foo)")
        );

        let d_id = source_target_id_by_name(&source, "d");
        let d_rows = flatten_dependency_tree(
            source
                .dependency_trees
                .get(&d_id)
                .expect("d dependency tree"),
            &source,
        );
        assert_eq!(row_parent_label(&d_rows, "b (bar)").as_deref(), Some("d"));
        assert_eq!(
            row_parent_label(&d_rows, "source_b (Add)").as_deref(),
            Some("b (bar)")
        );
        assert_eq!(row_parent_label(&d_rows, "c (bar)").as_deref(), Some("d"));
        assert_eq!(
            row_parent_label(&d_rows, "source_c (Add)").as_deref(),
            Some("c (bar)")
        );

        let root_id = source
            .dependency_root_target_id
            .as_ref()
            .expect("dependency root target");
        let root_rows = flatten_dependency_tree(
            source
                .dependency_trees
                .get(root_id)
                .expect("root dependency tree"),
            &source,
        );
        assert_row_parent_label(&root_rows, "b (foo)", "a (Add)");
        assert_row_parent_label(&root_rows, "source_b (Add)", "b (foo)");
        assert_row_parent_label(&root_rows, "c (foo)", "a (Add)");
        assert_row_parent_label(&root_rows, "source_c (Add)", "c (foo)");
        assert_row_parent_label(&root_rows, "b (bar)", "d (Add)");
        assert_row_parent_label(&root_rows, "source_b (Add)", "b (bar)");
        assert_row_parent_label(&root_rows, "c (bar)", "d (Add)");
        assert_row_parent_label(&root_rows, "source_c (Add)", "c (bar)");
    }

    #[test]
    fn dependency_tree_click_jumps_to_reference_occurrence_range() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(a: f32, c: f32) -> f32 {
    return a + c;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let b = 1.0;
    let c = 2.0;
    let a = foo(b, c);
    let d = bar(a, c);
    return d;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let a_row = dependency_row_by_label(&document.dependency_rows, "a (bar)").clone();

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(a_row.row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .pending_editor_jump
            .expect("expected dependency click to queue editor jump");
        let expected_start = document.draft_source.find("bar(a, c)").unwrap() + "bar(".len();
        assert_eq!(jump.start_byte, expected_start);
        assert_eq!(&document.draft_source[jump.start_byte..jump.end_byte], "a");
        assert_eq!(
            document.focused_dependency_row_key.as_deref(),
            Some(a_row.row_key.as_str())
        );
    }

    #[test]
    fn reference_row_source_jump_range_jumps_to_target_source() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(a: f32, c: f32) -> f32 {
    return a + c;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let b = 1.0;
    let c = 2.0;
    let a = foo(b, c);
    let d = bar(a, c);
    return d;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let a_row = dependency_row_by_label(&document.dependency_rows, "a (bar)").clone();
        let source_jump_range = a_row
            .source_jump_range
            .expect("expected reference row to expose source jump range");

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(a_row.row_key.clone()),
                target_id: None,
                source_range: Some(source_jump_range),
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .pending_editor_jump
            .expect("expected source jump to queue editor jump");
        let expected_start = document.draft_source.find("let a = foo").unwrap() + "let ".len();
        assert_eq!(jump.start_byte, expected_start);
        assert_eq!(&document.draft_source[jump.start_byte..jump.end_byte], "a");
        assert_eq!(
            document.focused_dependency_row_key.as_deref(),
            Some(a_row.row_key.as_str())
        );
    }

    #[test]
    fn local_reference_row_clicks_occurrence_and_src_jumps_to_declaration() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
@fragment
fn fs_main() -> @location(0) f32 {
    let edge = 30.0;
    let edge_sdf = edge + 1.0;
    let aa_depth = edge * 2.0;
    var final_alpha = smoothstep(0.0, aa_depth, -edge_sdf);
    let out = 0.5 * final_alpha;
    return out;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let final_alpha_row =
            dependency_row_by_label(&document.dependency_rows, "final_alpha (Multiply)").clone();

        let occurrence_start =
            document.draft_source.find("0.5 * final_alpha").unwrap() + "0.5 * ".len();
        let row_range = final_alpha_row
            .source_range
            .expect("expected final_alpha row occurrence range");
        assert_eq!(row_range.start_byte, occurrence_start);
        assert_eq!(
            &document.draft_source[row_range.start_byte..row_range.end_byte],
            "final_alpha"
        );
        assert!(
            document.dependency_rows.iter().any(|row| {
                row.parent_row_key.as_deref() == Some(final_alpha_row.row_key.as_str())
            }),
            "final_alpha reference row should have dependency children"
        );

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(final_alpha_row.row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .pending_editor_jump
            .expect("expected final_alpha row click to queue editor jump");
        assert_eq!(jump.start_byte, occurrence_start);
        assert_eq!(
            &document.draft_source[jump.start_byte..jump.end_byte],
            "final_alpha"
        );

        let source_jump_range = final_alpha_row
            .source_jump_range
            .expect("expected final_alpha row to expose declaration jump");
        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(final_alpha_row.row_key.clone()),
                target_id: None,
                source_range: Some(source_jump_range),
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .pending_editor_jump
            .expect("expected src jump to queue editor jump");
        let declaration_start =
            document.draft_source.find("var final_alpha").unwrap() + "var ".len();
        assert_eq!(jump.start_byte, declaration_start);
        assert_eq!(
            &document.draft_source[jump.start_byte..jump.end_byte],
            "final_alpha"
        );
    }

    #[test]
    fn reassigned_local_definition_row_click_jumps_to_store_without_src_jump() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(v: f32) -> f32 {
    return v;
}

@fragment
fn fs_main() -> @location(0) f32 {
    var x: f32 = 0.0;
    x = foo(x);
    x = foo(x);
    return x;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let x_id = target_id_by_name(&document, "x");
        let latest_store_start = document.draft_source.rfind("x = foo(x);").unwrap();
        let latest_x_row = document
            .dependency_rows
            .iter()
            .find(|row| {
                row.target_id.as_deref() == Some(x_id.as_str())
                    && row
                        .source_range
                        .map(|range| range.start_byte == latest_store_start)
                        .unwrap_or(false)
            })
            .cloned()
            .expect("expected latest reassignment dependency row");

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(latest_x_row.row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .pending_editor_jump
            .expect("expected reassignment row click to jump to store");
        assert_eq!(jump.start_byte, latest_store_start);
        assert_eq!(&document.draft_source[jump.start_byte..jump.end_byte], "x");

        assert_eq!(latest_x_row.source_jump_range, None);
    }

    #[test]
    fn reassigned_reference_row_clicks_occurrence_and_src_jumps_to_reaching_definition() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn fun(v: f32) -> f32 {
    return v;
}

fn foo(v: f32) -> f32 {
    return v;
}

fn bar(v: f32) -> f32 {
    return v;
}

@fragment
fn fs_main() -> @location(0) f32 {
    var a: f32 = 1.0;
    a = fun(a);
    let b = foo(a);
    let c = bar(a);
    return b + c;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let analysis_source = document
            .analysis_source
            .as_ref()
            .expect("missing analysis source");
        let a_id = source_target_id_by_name(analysis_source, "a");
        let foo_arg_start = document.draft_source.find("foo(a)").unwrap() + "foo(".len();
        let store_start = document.draft_source.find("a = fun(a);").unwrap();
        let declaration_start = document.draft_source.find("var a").unwrap() + "var ".len();
        let a_foo_row = document
            .dependency_rows
            .iter()
            .find(|row| {
                row.target_id.as_deref() == Some(a_id.as_str())
                    && row
                        .source_range
                        .is_some_and(|range| range.start_byte == foo_arg_start)
            })
            .cloned()
            .expect("expected foo(a) dependency row");

        let row_range = a_foo_row
            .source_range
            .expect("expected foo(a) occurrence range");
        assert_eq!(row_range.start_byte, foo_arg_start);
        assert_eq!(
            &document.draft_source[row_range.start_byte..row_range.end_byte],
            "a"
        );

        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(a_foo_row.row_key.clone()),
                target_id: None,
                source_range: None,
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .pending_editor_jump
            .expect("expected row click to jump to foo(a) occurrence");
        assert_eq!(jump.start_byte, foo_arg_start);
        assert_eq!(&document.draft_source[jump.start_byte..jump.end_byte], "a");

        let source_jump_range = a_foo_row
            .source_jump_range
            .expect("expected foo(a) row to expose reaching definition jump");
        document.focus_tree_click(
            PassDebugTreeClick {
                row_key: Some(a_foo_row.row_key.clone()),
                target_id: None,
                source_range: Some(source_jump_range),
                toggle_row_key: None,
            },
            true,
        );

        let jump = document
            .pending_editor_jump
            .expect("expected src jump to go to reaching definition");
        assert_eq!(jump.start_byte, store_start);
        assert_eq!(&document.draft_source[jump.start_byte..jump.end_byte], "a");

        let fun_arg_start = store_start + "a = fun(".len();
        let a_fun_row = document
            .dependency_rows
            .iter()
            .find(|row| {
                row.target_id.as_deref() == Some(a_id.as_str())
                    && row
                        .source_range
                        .is_some_and(|range| range.start_byte == fun_arg_start)
                    && dependency_path_for_row_key(&document.dependency_rows, &row.row_key)
                        .iter()
                        .any(|row_key| row_key == &a_foo_row.row_key)
            })
            .cloned()
            .expect("expected nested fun(a) dependency row under foo(a)");
        let nested_range = a_fun_row
            .source_range
            .expect("expected fun(a) occurrence range");
        assert_eq!(nested_range.start_byte, fun_arg_start);
        assert_eq!(
            &document.draft_source[nested_range.start_byte..nested_range.end_byte],
            "a"
        );
        let nested_source_jump_range = a_fun_row
            .source_jump_range
            .expect("expected nested fun(a) src jump to previous definition");
        assert_eq!(nested_source_jump_range.start_byte, declaration_start);
        assert_eq!(
            &document.draft_source
                [nested_source_jump_range.start_byte..nested_source_jump_range.end_byte],
            "a"
        );
    }

    #[test]
    fn editor_click_on_reference_focuses_matching_dependency_row() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn foo(b: f32, c: f32) -> f32 {
    return b + c;
}

fn bar(a: f32, c: f32) -> f32 {
    return a + c;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let b = 1.0;
    let c = 2.0;
    let a = foo(b, c);
    let d = bar(a, c);
    return d;
}
"#,
        );
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let reference_start = document.draft_source.find("bar(a, c)").unwrap() + "bar(".len();
        let reference_char_index =
            byte_index_to_char_index(&document.draft_source, reference_start);

        document.focus_target_at_char_index(reference_char_index);

        let focused_row = document
            .focused_dependency_row_key
            .as_deref()
            .and_then(|row_key| {
                document
                    .dependency_rows
                    .iter()
                    .find(|row| row.row_key == row_key)
            })
            .expect("expected editor click to focus dependency row");
        assert_eq!(focused_row.label, "a (bar)");
        let focused_range = document
            .focused_source_range()
            .expect("expected focused occurrence range");
        assert_eq!(focused_range.start_byte, reference_start);
        assert_eq!(
            &document.draft_source[focused_range.start_byte..focused_range.end_byte],
            "a"
        );
    }

    #[test]
    fn duplicate_dependency_rows_share_target_id_but_keep_row_occurrences() {
        let source = PassDebugSource::from_wgsl(
            "p",
            r#"
fn bar(left: f32, right: f32) -> f32 {
    return left + right;
}

@fragment
fn fs_main() -> @location(0) f32 {
    let a = 1.0;
    let d = bar(a, a);
    return d;
}
"#,
        );
        let document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let source = document
            .analysis_source
            .as_ref()
            .expect("missing analysis source");
        let d_id = source_target_id_by_name(source, "d");
        let d_rows = flatten_dependency_tree(
            source
                .dependency_trees
                .get(&d_id)
                .expect("d dependency tree"),
            source,
        );
        let a_id = source_target_id_by_name(source, "a");
        let a_rows = d_rows
            .iter()
            .filter(|row| row.label == "a (bar)" && row.target_id.as_deref() == Some(&a_id))
            .collect::<Vec<_>>();
        assert_eq!(a_rows.len(), 2);
        assert_ne!(a_rows[0].row_key, a_rows[1].row_key);
        assert_eq!(a_rows[0].target_id, a_rows[1].target_id);

        let call_start = document.draft_source.find("bar(a, a)").unwrap();
        let first_range = a_rows[0].source_range.expect("first occurrence range");
        let second_range = a_rows[1].source_range.expect("second occurrence range");
        assert_eq!(first_range.start_byte, call_start + "bar(".len());
        assert_eq!(second_range.start_byte, call_start + "bar(a, ".len());
    }

    #[test]
    fn dependency_focus_path_returns_root_to_focus_chain() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: String::new(),
            ast_tree: Vec::new(),
            dependency_targets: Vec::new(),
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let rows = flatten_dependency_tree(
            &PassDebugDependencyNode {
                label: "root c (let)".to_string(),
                edge_label: None,
                display_label: None,
                source_range: None,
                definition_source_range: None,
                target_id: Some("target::c".to_string()),
                reference: false,
                children: vec![PassDebugDependencyNode {
                    label: "[value] named expression".to_string(),
                    edge_label: None,
                    display_label: None,
                    source_range: None,
                    definition_source_range: None,
                    target_id: None,
                    reference: false,
                    children: vec![PassDebugDependencyNode {
                        label: "mid b (let)".to_string(),
                        edge_label: None,
                        display_label: None,
                        source_range: None,
                        definition_source_range: None,
                        target_id: Some("target::b".to_string()),
                        reference: false,
                        children: vec![PassDebugDependencyNode {
                            label: "[value] named expression".to_string(),
                            edge_label: None,
                            display_label: None,
                            source_range: None,
                            definition_source_range: None,
                            target_id: None,
                            reference: false,
                            children: vec![PassDebugDependencyNode {
                                label: "leaf a (local)".to_string(),
                                edge_label: None,
                                display_label: None,
                                source_range: None,
                                definition_source_range: None,
                                target_id: Some("target::a".to_string()),
                                reference: false,
                                children: Vec::new(),
                            }],
                        }],
                    }],
                }],
            },
            &source,
        );
        let leaf_key = rows
            .iter()
            .find(|row| row.target_id.as_deref() == Some("target::a"))
            .map(|row| row.row_key.as_str())
            .unwrap();

        assert_eq!(
            dependency_path_for_row_key(&rows, leaf_key),
            vec![
                "0".to_string(),
                "0/0/0".to_string(),
                "0/0/0/0/0".to_string()
            ]
        );
    }

    #[test]
    fn duplicate_target_matches_focus_specific_row_key() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: String::new(),
            ast_tree: Vec::new(),
            dependency_targets: Vec::new(),
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.dependency_rows = vec![
            PassDebugDependencyRow {
                depth: 0,
                row_key: "0".to_string(),
                parent_row_key: None,
                label: "root c (let)".to_string(),
                relation_path: String::new(),
                target_id: Some("target::c".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 1,
                row_key: "0/0".to_string(),
                parent_row_key: Some("0".to_string()),
                label: "shared a (local)".to_string(),
                relation_path: "left".to_string(),
                target_id: Some("target::a".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 1,
                row_key: "0/1".to_string(),
                parent_row_key: Some("0".to_string()),
                label: "shared a (local)".to_string(),
                relation_path: "right".to_string(),
                target_id: Some("target::a".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            },
        ];
        document.focus_dependency_row_key("0/1", true, false, false);

        assert_eq!(document.focused_target_id.as_deref(), Some("target::a"));
        assert_eq!(document.focused_dependency_row_key.as_deref(), Some("0/1"));
    }

    #[test]
    fn editor_focus_prefers_dependency_access_path_range() {
        let source = PassDebugSource {
            pass_name: "p".to_string(),
            module_source: "let a = input.foo.bar.x;".to_string(),
            ast_tree: Vec::new(),
            dependency_targets: vec![PassDebugDependencyTarget {
                id: "target::input".to_string(),
                name: "input".to_string(),
                label: "debug_main argument input".to_string(),
                scope: "debug_main".to_string(),
                kind: "argument".to_string(),
                source_range: Some(PassDebugSourceRange {
                    start_byte: 8,
                    end_byte: 13,
                    line: 1,
                    column: 9,
                }),
            }],
            dependency_trees: HashMap::new(),
            dependency_root_target_id: None,
            dependency_error: None,
            parse_error: None,
        };
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.dependency_rows = vec![
            PassDebugDependencyRow {
                depth: 0,
                row_key: "0".to_string(),
                parent_row_key: None,
                label: "input".to_string(),
                relation_path: String::new(),
                target_id: Some("target::input".to_string()),
                source_range: Some(PassDebugSourceRange {
                    start_byte: 8,
                    end_byte: 13,
                    line: 1,
                    column: 9,
                }),
                source_jump_range: None,
                selectable: true,
            },
            PassDebugDependencyRow {
                depth: 1,
                row_key: "0/0".to_string(),
                parent_row_key: Some("0".to_string()),
                label: "input.foo.bar.x".to_string(),
                relation_path: "use_value".to_string(),
                target_id: Some("target::input".to_string()),
                source_range: Some(PassDebugSourceRange {
                    start_byte: 8,
                    end_byte: 23,
                    line: 1,
                    column: 9,
                }),
                source_jump_range: None,
                selectable: true,
            },
        ];

        document.focus_target_at_char_index(18);

        assert_eq!(document.focused_dependency_row_key.as_deref(), Some("0/0"));
        let focused_range = document
            .focused_source_range()
            .expect("expected focused access path range");
        assert_eq!(
            &document.draft_source[focused_range.start_byte..focused_range.end_byte],
            "input.foo.bar.x"
        );
    }

    #[test]
    fn draft_analysis_does_not_replace_canonical_dependency_source() {
        let source = PassDebugSource::from_wgsl("p", "fn a() -> f32 { return 1.0; }\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        document.draft_source = "fn nope() -> { return vec4f(1.0); }\n".to_string();
        document.dirty = true;
        document.refresh_draft_analysis();

        assert_eq!(
            document.draft_source,
            "fn nope() -> { return vec4f(1.0); }\n"
        );
        assert!(
            document
                .analysis_source
                .as_ref()
                .and_then(|source| source.parse_error.as_ref())
                .is_none()
        );
        assert!(document.analysis_source_text.contains("return 1.0"));
        assert_eq!(document.loaded_source, "fn a() -> f32 { return 1.0; }\n");
    }

    // --- Shortwire tests ---

    fn test_diff_result(max_ae: f32) -> super::ShortwireDiffResult {
        super::ShortwireDiffResult {
            metric: "AE".to_string(),
            max_ae,
            min: 0.0,
            avg: 0.5,
            rms: 0.75,
            p95_abs: 1.0,
            sample_count: 16,
            non_finite_count: 0,
            render_size: [4, 4],
            reference_size: [4, 4],
            reference_offset: [0, 0],
        }
    }

    fn test_patch_with_diff(max_ae: Option<f32>) -> super::ShortwireNodePatch {
        super::ShortwireNodePatch {
            hunks: Vec::new(),
            base_source_hash: 1,
            reference_image: None,
            diff_result: max_ae.map(test_diff_result),
        }
    }

    fn test_reference_image() -> ShortwireReferenceImage {
        ShortwireReferenceImage {
            artifact_id: "pass__p__shortwire-reference-image__k".to_string(),
            name: "clip.png".to_string(),
            width: 2,
            height: 1,
            alpha_mode: RefImageAlphaMode::Premultiplied,
            mode: RefImageMode::Overlay,
            opacity: 0.5,
            offset: [1.0, -2.0],
        }
    }

    fn test_pasted_reference_image() -> ShortwirePastedReferenceImage {
        ShortwirePastedReferenceImage {
            name: "clip.png".to_string(),
            png_bytes: vec![137, 80, 78, 71, 13, 10, 26, 10],
            width: 2,
            height: 1,
            alpha_mode: RefImageAlphaMode::Premultiplied,
            mode: RefImageMode::Overlay,
            opacity: 0.5,
            offset: [0.0, 0.0],
        }
    }

    #[test]
    fn compact_diff_view_shows_only_changed_line_and_three_context_lines() {
        let base = (1..=10)
            .map(|line| format!("line{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let edited = (1..=10)
            .map(|line| {
                if line == 5 {
                    "line5 edited".to_string()
                } else {
                    format!("line{line}")
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        let view = super::build_shortwire_diff_view(&base, &edited);

        assert_eq!(view.rows.first().unwrap().old_line, Some(2));
        assert_eq!(view.rows.first().unwrap().new_line, Some(2));
        assert_eq!(view.rows.last().unwrap().old_line, Some(8));
        assert_eq!(view.rows.last().unwrap().new_line, Some(8));
        assert!(view.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Removed
                && row.old_line == Some(5)
                && row.new_line.is_none()
                && row.text == "line5"
        }));
        assert!(view.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Added
                && row.old_line.is_none()
                && row.new_line == Some(5)
                && row.text == "line5 edited"
        }));
        assert!(!view.rows.iter().any(|row| row.old_line == Some(1)));
        assert!(!view.rows.iter().any(|row| row.old_line == Some(9)));
    }

    #[test]
    fn compact_diff_view_keeps_distant_hunks_separated() {
        let base = (1..=18)
            .map(|line| format!("line{line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let edited = (1..=18)
            .map(|line| match line {
                2 => "line2 edited".to_string(),
                17 => "line17 edited".to_string(),
                _ => format!("line{line}"),
            })
            .collect::<Vec<_>>()
            .join("\n");

        let view = super::build_shortwire_diff_view(&base, &edited);

        assert!(
            view.rows
                .iter()
                .any(|row| row.kind == super::ShortwireDiffRowKind::Separator)
        );
        assert!(view.rows.iter().any(|row| row.text == "line2 edited"));
        assert!(view.rows.iter().any(|row| row.text == "line17 edited"));
    }

    #[test]
    fn compact_diff_view_records_insert_delete_and_replace_line_numbers() {
        let replace = super::build_shortwire_diff_view("a\nold\nc\n", "a\nnew\nc\n");
        assert!(replace.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Removed
                && row.old_line == Some(2)
                && row.new_line.is_none()
                && row.text == "old"
        }));
        assert!(replace.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Added
                && row.old_line.is_none()
                && row.new_line == Some(2)
                && row.text == "new"
        }));

        let insert = super::build_shortwire_diff_view("a\nb\n", "a\nx\nb\n");
        assert!(insert.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Added
                && row.old_line.is_none()
                && row.new_line == Some(2)
                && row.text == "x"
        }));

        let delete = super::build_shortwire_diff_view("a\nb\nc\n", "a\nc\n");
        assert!(delete.rows.iter().any(|row| {
            row.kind == super::ShortwireDiffRowKind::Removed
                && row.old_line == Some(2)
                && row.new_line.is_none()
                && row.text == "b"
        }));
    }

    #[test]
    fn legacy_shortwire_patch_json_restores_without_diff_result() {
        let text = r#"{"version":1,"patches":{"k":{"hunks":[],"base_source_hash":42}}}"#;
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.restore_shortwire_patches_from_text(text);

        let patch = document.shortwire_patches.get("k").unwrap();
        assert_eq!(patch.base_source_hash, 42);
        assert!(patch.diff_result.is_none());
    }

    #[test]
    fn shortwire_diff_result_round_trips_in_patch_artifact() {
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        document
            .shortwire_patches
            .insert("k".to_string(), test_patch_with_diff(Some(1.25)));
        document.shortwire_patches_dirty = true;

        let (_item, content) = document.take_patches_dirty_artifact().unwrap();
        assert!(content.contains("diffResult"));

        let mut restored = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        restored.restore_shortwire_patches_from_text(&content);

        assert_eq!(
            restored
                .shortwire_patches
                .get("k")
                .and_then(|patch| patch.diff_result.as_ref())
                .map(|result| result.max_ae),
            Some(1.25)
        );
    }

    #[test]
    fn shortwire_reference_image_round_trips_in_patch_artifact() {
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        let reference_image = test_reference_image();
        document.shortwire_patches.insert(
            "k".to_string(),
            super::ShortwireNodePatch {
                hunks: Vec::new(),
                base_source_hash: 1,
                reference_image: Some(reference_image.clone()),
                diff_result: None,
            },
        );
        document.shortwire_patches_dirty = true;

        let (_item, content) = document.take_patches_dirty_artifact().unwrap();
        assert!(content.contains("referenceImage"));
        assert!(content.contains(reference_image.artifact_id.as_str()));

        let mut restored = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        restored.restore_shortwire_patches_from_text(&content);

        assert_eq!(
            restored
                .shortwire_patches
                .get("k")
                .and_then(|patch| patch.reference_image.as_ref())
                .map(|image| image.artifact_id.as_str()),
            Some(reference_image.artifact_id.as_str())
        );
    }

    #[test]
    fn shortwire_dot_status_uses_diff_result_threshold() {
        assert_eq!(
            super::shortwire_dot_info_for_patch(&test_patch_with_diff(None)).status,
            super::ShortwireDotStatus::PendingDiff
        );
        assert_eq!(
            super::shortwire_dot_info_for_patch(&test_patch_with_diff(Some(
                super::SHORTWIRE_DIFF_PASS_MAX_AE * 0.99
            )))
            .status,
            super::ShortwireDotStatus::Passing
        );
        assert_eq!(
            super::shortwire_dot_info_for_patch(&test_patch_with_diff(Some(
                super::SHORTWIRE_DIFF_PASS_MAX_AE
            )))
            .status,
            super::ShortwireDotStatus::Failing
        );
        assert_eq!(
            super::shortwire_dot_info_for_patch(&test_patch_with_diff(Some(0.992458))).status,
            super::ShortwireDotStatus::Failing
        );
    }

    #[test]
    fn closing_shortwire_preserves_matching_diff_result() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };

        document.enter_shortwire(&row, &pending_actions);
        document.draft_source = "fn edited() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        let patched_source = PassDebugSource::from_wgsl("p", "fn edited() {}\n");
        document.mark_applied(
            Some(&patched_source),
            1,
            "fn edited() {}\n".to_string(),
            "Applied".to_string(),
        );

        let patch_key = super::shortwire_patch_key(&row);
        document
            .shortwire_patches
            .get_mut(&patch_key)
            .unwrap()
            .diff_result = Some(test_diff_result(0.25));

        document.exit_shortwire_navigate(&pending_actions);

        assert_eq!(
            document
                .shortwire_patches
                .get(&patch_key)
                .and_then(|patch| patch.diff_result.as_ref())
                .map(|result| result.max_ae),
            Some(0.25)
        );
    }

    #[test]
    fn active_shortwire_diff_capture_clears_stale_result() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut windows = super::PassDebugWindowMap::new();
        windows.insert(
            "p".to_string(),
            super::PassDebugWindowState::new("p".to_string(), Some(source), 0, None),
        );

        let state = windows.get("p").unwrap();
        let pending_actions = state.pending_actions.clone();
        let patch_key = {
            let mut document = state.document.lock().unwrap();
            let row = document.dependency_rows.first().cloned().unwrap();
            let patch_key = super::shortwire_patch_key(&row);
            document.enter_shortwire(&row, &pending_actions);
            document
                .shortwire_patches
                .insert(patch_key.clone(), test_patch_with_diff(Some(1.25)));
            patch_key
        };

        let result = super::request_active_shortwire_diff_capture(&mut windows, None);

        assert_eq!(
            result.diff_capture,
            Some(super::ShortwireDiffCaptureRequest {
                pass_name: "p".to_string(),
                patch_key: patch_key.clone(),
            })
        );
        assert_eq!(result.artifacts.len(), 1);
        let state = windows.get("p").unwrap();
        let document = state.document.lock().unwrap();
        assert!(
            document
                .shortwire_patches
                .get(&patch_key)
                .unwrap()
                .diff_result
                .is_none()
        );
        assert!(!document.shortwire_patches_dirty);
    }

    #[test]
    fn pasted_shortwire_reference_creates_image_only_patch() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut windows = super::PassDebugWindowMap::new();
        windows.insert(
            "p".to_string(),
            super::PassDebugWindowState::new("p".to_string(), Some(source), 0, None),
        );

        let state = windows.get("p").unwrap();
        let pending_actions = state.pending_actions.clone();
        let patch_key = {
            let mut document = state.document.lock().unwrap();
            let row = document.dependency_rows.first().cloned().unwrap();
            let patch_key = super::shortwire_patch_key(&row);
            document.enter_shortwire(&row, &pending_actions);
            patch_key
        };

        let result = super::request_active_shortwire_diff_capture(
            &mut windows,
            Some(test_pasted_reference_image()),
        );

        assert_eq!(
            result.diff_capture,
            Some(super::ShortwireDiffCaptureRequest {
                pass_name: "p".to_string(),
                patch_key: patch_key.clone(),
            })
        );
        assert_eq!(result.artifacts.len(), 1);
        assert_eq!(result.binary_artifacts.len(), 1);
        let (image_item, image_bytes) = &result.binary_artifacts[0];
        assert_eq!(image_item.role, DebugArtifactRole::Image);
        assert_eq!(image_item.mime_type, "image/png");
        assert!(image_item.path.starts_with("debug-artifacts/"));
        assert!(
            image_item
                .slot_key
                .as_deref()
                .is_some_and(|slot| slot.starts_with("shortwire-reference:"))
        );
        assert_eq!(image_bytes.as_slice(), &[137, 80, 78, 71, 13, 10, 26, 10]);
        assert!(result.artifacts[0].1.contains(image_item.id.as_str()));

        let state = windows.get("p").unwrap();
        let document = state.document.lock().unwrap();
        let patch = document.shortwire_patches.get(&patch_key).unwrap();
        assert!(patch.hunks.is_empty());
        assert_eq!(
            patch
                .reference_image
                .as_ref()
                .map(|image| image.artifact_id.as_str()),
            Some(image_item.id.as_str())
        );
        assert!(patch.diff_result.is_none());
        assert!(!document.shortwire_patches_dirty);
    }

    #[test]
    fn window_state_restores_shortwire_patches_when_artifact_arrives_late() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut state = super::PassDebugWindowState::new("p".to_string(), Some(source), 0, None);
        let payload = super::ShortwirePatchesPayload {
            version: 1,
            patches: HashMap::from([(
                "k".to_string(),
                test_patch_with_diff(Some(super::SHORTWIRE_DIFF_PASS_MAX_AE * 0.5)),
            )]),
        };
        let content = serde_json::to_string(&payload).unwrap();

        state.sync_shortwire_patches_from_artifact(None);
        assert!(state.document.lock().unwrap().shortwire_patches.is_empty());

        state.sync_shortwire_patches_from_artifact(Some(&content));

        let document = state.document.lock().unwrap();
        assert_eq!(document.shortwire_patches.len(), 1);
        assert_eq!(
            super::shortwire_dot_info_for_patch(document.shortwire_patches.get("k").unwrap())
                .status,
            super::ShortwireDotStatus::Passing
        );
    }

    #[test]
    fn legacy_default_reference_artifact_migrates_to_workspace() {
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(None, &[], Some("legacy reference\n"), None);

        assert_eq!(
            document.reference_workspace.selected_file.as_deref(),
            Some("reference.txt")
        );
        assert_eq!(
            document.reference_workspace.editor_source,
            "legacy reference\n"
        );
        assert!(document.reference_workspace.manifest_dirty);
        assert!(document.reference_workspace.selected_file_dirty());

        let artifacts = document.take_reference_workspace_dirty_artifacts();
        assert_eq!(artifacts.len(), 2);
        assert!(artifacts.iter().any(|(item, _)| {
            item.role == DebugArtifactRole::Attachment
                && item.slot_key.as_deref() == Some(super::DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT)
        }));
        assert!(artifacts.iter().any(|(item, content)| {
            item.role == DebugArtifactRole::ReferenceCode
                && item
                    .slot_key
                    .as_deref()
                    .is_some_and(|slot| slot.starts_with("file:"))
                && content == "legacy reference\n"
        }));
    }

    #[test]
    fn reference_workspace_restores_multiple_files_from_artifacts() {
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": null,
            "rootLabel": "reference",
            "selectedFile": "metal/pass.metal",
            "files": [
                {
                    "relativePath": "glsl/pass.frag",
                    "artifactId": super::pass_reference_file_artifact_id("p", "glsl/pass.frag"),
                    "contentHash": "aaa",
                    "size": 12
                },
                {
                    "relativePath": "metal/pass.metal",
                    "artifactId": super::pass_reference_file_artifact_id("p", "metal/pass.metal"),
                    "contentHash": "bbb",
                    "size": 13
                }
            ],
            "skippedFiles": 2
        })
        .to_string();
        let files = vec![
            reference_snapshot("p", "glsl/pass.frag", "glsl source\n"),
            reference_snapshot("p", "metal/pass.metal", "metal source\n"),
        ];
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(Some(&manifest), &files, None, None);

        assert_eq!(document.reference_workspace.files.len(), 2);
        assert_eq!(
            document.reference_workspace.selected_file.as_deref(),
            Some("metal/pass.metal")
        );
        assert_eq!(document.reference_workspace.editor_source, "metal source\n");
        assert_eq!(document.reference_workspace.skipped_files, 2);
        assert!(!document.reference_workspace.has_dirty_files());
    }

    #[test]
    fn reference_workspace_with_root_path_loads_local_file_instead_of_artifact_text() {
        let temp_dir = unique_reference_temp_dir("root-loads-local");
        fs::create_dir_all(temp_dir.join("metal")).unwrap();
        fs::write(temp_dir.join("metal/pass.metal"), "local source\n").unwrap();
        let relative_path = "metal/pass.metal";
        let artifact_id = super::pass_reference_file_artifact_id("p", relative_path);
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": temp_dir.to_string_lossy(),
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        let files = vec![reference_snapshot("p", relative_path, "archive source\n")];
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(Some(&manifest), &files, None, None);

        assert_eq!(document.reference_workspace.editor_source, "local source\n");
        assert_eq!(
            document.reference_workspace.last_status.as_deref(),
            Some("Loaded local reference")
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn reference_workspace_with_missing_root_path_file_does_not_fallback_to_artifact_text() {
        let temp_dir = unique_reference_temp_dir("root-missing-no-fallback");
        fs::create_dir_all(&temp_dir).unwrap();
        let relative_path = "metal/pass.metal";
        let artifact_id = super::pass_reference_file_artifact_id("p", relative_path);
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": temp_dir.to_string_lossy(),
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        let files = vec![reference_snapshot("p", relative_path, "archive source\n")];
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(Some(&manifest), &files, None, None);

        assert!(document.reference_workspace.files.is_empty());
        assert!(document.reference_workspace.editor_source.is_empty());
        assert_eq!(
            document.reference_workspace.last_status.as_deref(),
            Some("Local reference missing (1 missing)")
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn pathless_reference_workspace_still_restores_from_artifact_text() {
        let relative_path = "metal/pass.metal";
        let artifact_id = super::pass_reference_file_artifact_id("p", relative_path);
        let manifest = serde_json::json!({
            "version": 1,
            "rootPath": null,
            "rootLabel": "reference",
            "selectedFile": relative_path,
            "files": [
                {
                    "relativePath": relative_path,
                    "artifactId": artifact_id,
                    "contentHash": "archive",
                    "size": 15
                }
            ],
            "skippedFiles": 0
        })
        .to_string();
        let files = vec![reference_snapshot("p", relative_path, "archive source\n")];
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);

        document.update_reference_workspace(Some(&manifest), &files, None, None);

        assert_eq!(
            document.reference_workspace.editor_source,
            "archive source\n"
        );
        assert_eq!(
            document.reference_workspace.last_status.as_deref(),
            Some("Loaded archived reference")
        );
    }

    #[test]
    fn rooted_reference_sync_writes_local_file_and_only_emits_manifest() {
        let temp_dir = unique_reference_temp_dir("root-sync-writes-local");
        fs::create_dir_all(&temp_dir).unwrap();
        let reference_path = temp_dir.join("pass.metal");
        fs::write(&reference_path, "fn original() {}\n").unwrap();
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        document.import_reference_file_from_path(&reference_path, 0.0);
        document.reference_workspace.editor_source = "fn edited() {}\n".to_string();

        let artifacts = document.take_reference_workspace_dirty_artifacts();

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn edited() {}\n"
        );
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].0.role, DebugArtifactRole::Attachment);
        assert_eq!(
            artifacts[0].0.slot_key.as_deref(),
            Some(super::DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT)
        );
        assert!(
            !artifacts
                .iter()
                .any(|(item, _)| item.role == DebugArtifactRole::ReferenceCode)
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn reference_shortwire_patch_key_isolated_by_file() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.dependency_rows.first().cloned().unwrap();
        let base = "fn ref() {}\n";
        let file_a = super::ReferenceWorkspaceFile {
            relative_path: "a.metal".to_string(),
            artifact_id: super::pass_reference_file_artifact_id("p", "a.metal"),
            source: base.to_string(),
            loaded_source: base.to_string(),
        };
        let file_b = super::ReferenceWorkspaceFile {
            relative_path: "b.metal".to_string(),
            artifact_id: super::pass_reference_file_artifact_id("p", "b.metal"),
            source: base.to_string(),
            loaded_source: base.to_string(),
        };
        document.reference_workspace.replace_files(
            None,
            "Reference test".to_string(),
            vec![file_a, file_b],
            Some("a.metal".to_string()),
            0,
            false,
        );
        let row_patch_key = super::shortwire_patch_key(&row);
        document.reference_patches.insert(
            super::reference_shortwire_patch_key("a.metal", &row_patch_key),
            super::ShortwireNodePatch {
                hunks: super::compute_hunks(base, "fn ref_a() {}\n"),
                base_source_hash: super::hash_source(base),
                reference_image: None,
                diff_result: None,
            },
        );
        document.reference_patches.insert(
            super::reference_shortwire_patch_key("b.metal", &row_patch_key),
            super::ShortwireNodePatch {
                hunks: super::compute_hunks(base, "fn ref_b() {}\n"),
                base_source_hash: super::hash_source(base),
                reference_image: None,
                diff_result: None,
            },
        );

        document.enter_shortwire(&row, &pending_actions);
        assert_eq!(
            document.reference_workspace.editor_source,
            "fn ref_a() {}\n"
        );
        document.exit_shortwire_cancel();
        assert!(document.reference_workspace.select_file("b.metal"));
        document.enter_shortwire(&row, &pending_actions);
        assert_eq!(
            document.reference_workspace.editor_source,
            "fn ref_b() {}\n"
        );
    }

    #[test]
    fn reference_shortwire_save_commits_after_left_apply_and_stays_active() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.dependency_rows.first().cloned().unwrap();
        seed_reference_file(&mut document, "metal/pass.metal", "fn ref() {}\n");

        document.enter_shortwire(&row, &pending_actions);
        document.reference_workspace.editor_source = "fn ref_patched() {}\n".to_string();
        document.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);

        assert!(
            document
                .reference_workspace
                .pending_shortwire_patch
                .is_some()
        );
        document.mark_applied(
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );

        assert_eq!(
            document.reference_workspace.editor_source,
            "fn ref_patched() {}\n"
        );
        assert!(document.reference_workspace.shortwire_active_key.is_some());
        assert_eq!(document.reference_patches.len(), 1);
        assert!(document.reference_patches_dirty);
        let patch = document.reference_patches.values().next().unwrap();
        assert_eq!(
            super::apply_hunks("fn ref() {}\n", &patch.hunks).unwrap(),
            "fn ref_patched() {}\n"
        );

        document.exit_shortwire_navigate(&pending_actions);

        assert_eq!(document.reference_workspace.editor_source, "fn ref() {}\n");
        assert!(document.reference_workspace.shortwire_active_key.is_none());
    }

    #[test]
    fn reference_shortwire_save_writes_local_file_and_exit_restores_original() {
        let temp_dir = unique_reference_temp_dir("save-restore");
        fs::create_dir_all(&temp_dir).unwrap();
        let reference_path = temp_dir.join("pass.metal");
        fs::write(&reference_path, "fn original() {}\n").unwrap();

        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        document.import_reference_file_from_path(&reference_path, 0.0);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.dependency_rows.first().cloned().unwrap();

        document.enter_shortwire(&row, &pending_actions);
        document.reference_workspace.editor_source = "fn patched_ref() {}\n".to_string();
        document.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );

        document.mark_applied(
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn patched_ref() {}\n"
        );

        document.exit_shortwire_navigate(&pending_actions);

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn reference_shortwire_reenter_writes_stored_patch_to_local_file() {
        let temp_dir = unique_reference_temp_dir("reenter-write");
        fs::create_dir_all(&temp_dir).unwrap();
        let reference_path = temp_dir.join("pass.metal");
        fs::write(&reference_path, "fn original() {}\n").unwrap();

        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        document.import_reference_file_from_path(&reference_path, 0.0);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.dependency_rows.first().cloned().unwrap();

        document.enter_shortwire(&row, &pending_actions);
        document.reference_workspace.editor_source = "fn patched_ref() {}\n".to_string();
        document.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        document.mark_applied(
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );
        document.exit_shortwire_navigate(&pending_actions);
        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );

        document.mark_reset(Some(&source), 2, "Reset".to_string());
        pending_actions.lock().unwrap().clear();
        document.enter_shortwire(&row, &pending_actions);

        assert_eq!(
            document.reference_workspace.editor_source,
            "fn patched_ref() {}\n"
        );
        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn patched_ref() {}\n"
        );

        document.exit_shortwire_navigate(&pending_actions);
        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn reference_shortwire_close_restores_local_file() {
        let temp_dir = unique_reference_temp_dir("close-restore");
        fs::create_dir_all(&temp_dir).unwrap();
        let reference_path = temp_dir.join("pass.metal");
        fs::write(&reference_path, "fn original() {}\n").unwrap();

        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        document.import_reference_file_from_path(&reference_path, 0.0);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.dependency_rows.first().cloned().unwrap();

        document.enter_shortwire(&row, &pending_actions);
        document.reference_workspace.editor_source = "fn patched_ref() {}\n".to_string();
        document.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        document.mark_applied(
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );
        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn patched_ref() {}\n"
        );

        pending_actions.lock().unwrap().clear();
        document.prepare_debug_window_close(&pending_actions);

        assert_eq!(
            fs::read_to_string(&reference_path).unwrap(),
            "fn original() {}\n"
        );
        assert!(document.shortwire_active.is_none());
        let actions = pending_actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        assert!(matches!(
            actions[0],
            super::PassDebugWindowAction::ResetPatch { .. }
        ));
        drop(actions);
        let _ = fs::remove_dir_all(temp_dir);
    }

    #[test]
    fn mark_patch_applied_returns_shortwire_artifacts_immediately() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut windows = super::PassDebugWindowMap::new();
        windows.insert(
            "p".to_string(),
            super::PassDebugWindowState::new("p".to_string(), Some(source.clone()), 0, None),
        );

        let state = windows.get("p").unwrap();
        let pending_actions = state.pending_actions.clone();
        {
            let mut document = state.document.lock().unwrap();
            let row = document.dependency_rows.first().cloned().unwrap();
            seed_reference_file(&mut document, "metal/pass.metal", "fn ref() {}\n");
            document.enter_shortwire(&row, &pending_actions);
            document.reference_workspace.editor_source = "fn ref_patched() {}\n".to_string();
            document.draft_source = "fn patched_left() {}\n".to_string();
            document.exit_shortwire_done(&pending_actions);
        }

        let result = super::mark_patch_applied(
            &mut windows,
            "p",
            Some(&source),
            1,
            "fn patched_left() {}\n".to_string(),
            "Applied".to_string(),
        );
        let artifacts = result.artifacts;

        assert_eq!(artifacts.len(), 2);
        assert!(result.diff_capture.is_some());
        assert!(artifacts.iter().any(|(item, content)| {
            item.role == DebugArtifactRole::Patch
                && item.slot_key.as_deref() == Some(super::DEBUG_ARTIFACT_DEFAULT_SLOT)
                && content.contains("patched_left")
        }));
        assert!(artifacts.iter().any(|(item, content)| {
            item.role == DebugArtifactRole::Patch
                && item.slot_key.as_deref() == Some(super::DEBUG_ARTIFACT_REFERENCE_PATCHES_SLOT)
                && content.contains("ref_patched")
        }));

        let state = windows.get("p").unwrap();
        let document = state.document.lock().unwrap();
        assert!(!document.shortwire_patches_dirty);
        assert!(!document.reference_patches_dirty);
    }

    #[test]
    fn reference_shortwire_apply_error_keeps_patch_draft_uncommitted() {
        let source = PassDebugSource::from_wgsl("p", root_return_shader("a", 1.0));
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document.dependency_rows.first().cloned().unwrap();
        seed_reference_file(&mut document, "metal/pass.metal", "fn ref() {}\n");

        document.enter_shortwire(&row, &pending_actions);
        document.reference_workspace.editor_source = "fn ref_patched() {}\n".to_string();
        document.draft_source = "fn patched_left() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        document.record_error("left apply failed".to_string());

        assert_eq!(
            document.reference_workspace.editor_source,
            "fn ref_patched() {}\n"
        );
        assert!(
            document
                .reference_workspace
                .pending_shortwire_patch
                .is_some()
        );
        assert!(document.reference_patches.is_empty());
        assert!(!document.reference_patches_dirty);
        assert!(document.shortwire_is_editor_interactive());
    }

    #[test]
    fn reference_reload_missing_path_keeps_archive_snapshot() {
        let mut document = PassDebugWindowDocument::new("p".to_string(), None, 0, false);
        seed_reference_file(&mut document, "metal/pass.metal", "fn ref() {}\n");
        document.reference_workspace.root_path =
            Some("/tmp/node-forge-missing-reference-root-for-test".to_string());

        document.reload_reference_workspace(0.0);

        assert_eq!(document.reference_workspace.editor_source, "fn ref() {}\n");
        assert!(
            document
                .reference_workspace
                .last_status
                .as_deref()
                .unwrap_or_default()
                .contains("missing")
        );
    }

    #[test]
    fn patch_source_updates_editor_but_dependency_tree_stays_canonical() {
        let canonical = root_return_shader("canonical_root", 1.0);
        let patched = root_return_shader("shortwire_root", 2.0);
        let source = PassDebugSource::from_wgsl("p", canonical.clone());
        let document =
            PassDebugWindowDocument::new("p".to_string(), Some(source), 0, Some(patched.as_str()));

        assert_eq!(document.draft_source, patched);
        assert_eq!(document.loaded_source, patched);
        assert_eq!(document.generated_base_source, canonical);
        assert_eq!(dependency_root_target_name(&document), "return");
        assert!(has_target_named(&document, "canonical_root"));
        assert!(!has_target_named(&document, "shortwire_root"));
        assert!(dependency_rows_contain_label_fragment(
            &document,
            "canonical_root"
        ));
        assert!(!dependency_rows_contain_label_fragment(
            &document,
            "shortwire_root"
        ));
    }

    #[test]
    fn applying_shortwire_does_not_change_dependency_root() {
        let canonical = root_return_shader("canonical_root", 1.0);
        let patched = root_return_shader("shortwire_root", 2.0);
        let source = PassDebugSource::from_wgsl("p", canonical.clone());
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let root_before = document.dependency_root_target_id.clone();
        let row = document
            .dependency_rows
            .first()
            .cloned()
            .expect("dependency root row");

        document.enter_shortwire(&row, &pending_actions);
        document.draft_source = patched.clone();
        document.exit_shortwire_done(&pending_actions);
        document.mark_applied(Some(&source), 1, patched.clone(), "Applied".to_string());

        assert_eq!(document.draft_source, patched);
        assert_eq!(document.generated_base_source, canonical);
        assert_eq!(document.dependency_root_target_id, root_before);
        assert_eq!(dependency_root_target_name(&document), "return");
        assert!(has_target_named(&document, "canonical_root"));
        assert!(!has_target_named(&document, "shortwire_root"));
        assert!(dependency_rows_contain_label_fragment(
            &document,
            "canonical_root"
        ));
        assert!(!dependency_rows_contain_label_fragment(
            &document,
            "shortwire_root"
        ));
    }

    #[test]
    fn canonical_source_refresh_updates_deps_tree_while_patch_exists() {
        let canonical_before = root_return_shader("before_root", 1.0);
        let canonical_after = root_return_shader("after_root", 3.0);
        let patched = root_return_shader("shortwire_root", 2.0);
        let source_before = PassDebugSource::from_wgsl("p", canonical_before);
        let source_after = PassDebugSource::from_wgsl("p", canonical_after.clone());
        let mut document = PassDebugWindowDocument::new(
            "p".to_string(),
            Some(source_before),
            0,
            Some(patched.as_str()),
        );
        assert_eq!(dependency_root_target_name(&document), "return");
        assert!(dependency_rows_contain_label_fragment(
            &document,
            "before_root"
        ));

        document.update_source(Some(&source_after), 1, Some(patched.as_str()));

        assert_eq!(document.draft_source, patched);
        assert_eq!(document.generated_base_source, canonical_after);
        assert_eq!(dependency_root_target_name(&document), "return");
        assert!(has_target_named(&document, "after_root"));
        assert!(!has_target_named(&document, "shortwire_root"));
        assert!(dependency_rows_contain_label_fragment(
            &document,
            "after_root"
        ));
        assert!(!dependency_rows_contain_label_fragment(
            &document,
            "shortwire_root"
        ));
    }

    #[test]
    fn navigating_to_other_dependency_exits_shortwire_without_auto_apply() {
        let canonical = root_return_shader("canonical_root", 1.0);
        let patched = root_return_shader("shortwire_root", 2.0);
        let source = PassDebugSource::from_wgsl("p", canonical.clone());
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document
            .dependency_rows
            .first()
            .cloned()
            .expect("dependency root row");

        document.enter_shortwire(&row, &pending_actions);
        document.draft_source = patched.clone();
        document.mark_draft_edited(0.0);

        document.exit_shortwire_navigate(&pending_actions);

        assert!(document.shortwire_active.is_none());
        assert_eq!(document.draft_source, canonical);
        assert_eq!(document.loaded_source, canonical);
        assert!(!document.patch_active);
        assert!(!document.dirty);
        assert!(!document.shortwire_patches.is_empty());
        let actions = pending_actions.lock().unwrap();
        assert!(actions.is_empty());
    }

    #[test]
    fn shortwire_active_row_click_is_noop_but_other_row_exits() {
        let active_click = PassDebugTreeClick {
            row_key: Some("0".to_string()),
            target_id: None,
            source_range: None,
            toggle_row_key: None,
        };
        let active_toggle = PassDebugTreeClick {
            row_key: None,
            target_id: None,
            source_range: None,
            toggle_row_key: Some("0".to_string()),
        };
        let other_click = PassDebugTreeClick {
            row_key: Some("1".to_string()),
            target_id: None,
            source_range: None,
            toggle_row_key: None,
        };

        assert!(shortwire_click_matches_active_row("0", &active_click));
        assert!(shortwire_click_matches_active_row("0", &active_toggle));
        assert!(!shortwire_click_matches_active_row("0", &other_click));
    }

    #[test]
    fn generated_base_source_tracks_canonical_when_patch_active() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        assert_eq!(document.generated_base_source, "fn a() {}\n");

        let refreshed = PassDebugSource::from_wgsl("p", "fn b() {}\n");
        document.update_source(Some(&refreshed), 1, Some("fn patched() {}\n"));

        assert_eq!(document.generated_base_source, "fn b() {}\n");
        assert_eq!(document.draft_source, "fn patched() {}\n");
        assert!(document.patch_active);
    }

    #[test]
    fn canonical_change_with_applied_patch_auto_merges_cleanly() {
        let base = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
        let local = "fn a() {\n    let x = 99;\n    let y = 2;\n}\n";
        let incoming = "fn a() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n";
        let expected = "fn a() {\n    let x = 99;\n    let y = 2;\n    let z = 3;\n}\n";

        let source = PassDebugSource::from_wgsl("p", base);
        let incoming_source = PassDebugSource::from_wgsl("p", incoming);
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source), 0, Some(local));
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        document.update_source_with_actions(
            Some(&incoming_source),
            1,
            Some(local),
            &pending_actions,
        );

        assert!(document.merge_conflict.is_none());
        assert_eq!(document.generated_base_source, incoming);
        assert!(document.last_status.as_ref().unwrap().contains("rebasing"));
        let actions = pending_actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            super::PassDebugWindowAction::ApplyPatch { source, .. } => {
                assert_eq!(source, expected);
            }
            _ => panic!("expected ApplyPatch action"),
        }
    }

    #[test]
    fn auto_merge_rebases_matching_shortwire_patch_after_apply_success() {
        let base = "fn a() {\n    let x = 1;\n    let y = 2;\n}\n";
        let local = "fn a() {\n    let x = 99;\n    let y = 2;\n}\n";
        let incoming = "fn a() {\n    let x = 1;\n    let y = 2;\n    let z = 3;\n}\n";
        let expected = "fn a() {\n    let x = 99;\n    let y = 2;\n    let z = 3;\n}\n";

        let source = PassDebugSource::from_wgsl("p", base);
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = document
            .dependency_rows
            .first()
            .cloned()
            .expect("dependency root row");

        document.enter_shortwire(&row, &pending_actions);
        document.draft_source = local.to_string();
        document.exit_shortwire_done(&pending_actions);
        document.mark_applied(Some(&source), 1, local.to_string(), "Applied".to_string());
        assert_eq!(document.shortwire_patches.len(), 1);

        pending_actions.lock().unwrap().clear();
        let incoming_source = PassDebugSource::from_wgsl("p", incoming);
        document.update_source_with_actions(
            Some(&incoming_source),
            2,
            Some(local),
            &pending_actions,
        );
        assert!(
            document
                .shortwire_active
                .as_ref()
                .unwrap()
                .base_source_stale
        );
        assert!(pending_actions.lock().unwrap().is_empty());

        document.exit_shortwire_done(&pending_actions);
        let actions = pending_actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            super::PassDebugWindowAction::ApplyPatch { source, .. } => {
                assert_eq!(source, expected);
            }
            _ => panic!("expected ApplyPatch action"),
        }
        drop(actions);
        document.mark_applied(
            Some(&incoming_source),
            3,
            expected.to_string(),
            "Applied".to_string(),
        );

        let patch = document
            .shortwire_patches
            .values()
            .next()
            .expect("rebased patch");
        assert_eq!(patch.base_source_hash, super::hash_source(incoming));
        assert_eq!(
            super::apply_hunks(incoming, &patch.hunks).unwrap(),
            expected
        );
    }

    #[test]
    fn canonical_change_with_applied_patch_enters_merge_conflict() {
        let base = "fn a() {\n    let x = 1;\n}\n";
        let local = "fn a() {\n    let x = 99;\n}\n";
        let incoming = "fn a() {\n    let x = 2;\n}\n";

        let source = PassDebugSource::from_wgsl("p", base);
        let incoming_source = PassDebugSource::from_wgsl("p", incoming);
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source), 0, Some(local));
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        document.update_source_with_actions(
            Some(&incoming_source),
            1,
            Some(local),
            &pending_actions,
        );

        assert!(pending_actions.lock().unwrap().is_empty());
        let conflict = document.merge_conflict.as_ref().expect("merge conflict");
        assert_eq!(conflict.base_source, base);
        assert_eq!(conflict.incoming_source, incoming);
        assert_eq!(conflict.local_source, local);
        assert_eq!(conflict.resolved_source, local);
        assert!(conflict.choice_popup_open);
        assert!(!conflict.resolver_window_open);
        assert_eq!(document.generated_base_source, incoming);
        assert!(document.last_error.as_ref().unwrap().contains("conflicts"));

        document.open_merge_resolver();
        let conflict = document.merge_conflict.as_ref().expect("merge conflict");
        assert!(!conflict.choice_popup_open);
        assert!(conflict.resolver_window_open);
    }

    #[test]
    fn generated_base_source_updated_when_not_patch_active() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);

        let refreshed = PassDebugSource::from_wgsl("p", "fn b() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(document.generated_base_source, "fn b() {}\n");
    }

    #[test]
    fn update_source_during_active_shortwire_does_not_overwrite_draft() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = document
            .dependency_rows
            .first()
            .cloned()
            .unwrap_or(PassDebugDependencyRow {
                depth: 0,
                row_key: "0".to_string(),
                parent_row_key: None,
                label: "test".to_string(),
                relation_path: String::new(),
                target_id: Some("t".to_string()),
                source_range: None,
                source_jump_range: None,
                selectable: true,
            });
        document.enter_shortwire(&row, &pending_actions);
        assert!(document.shortwire_active.is_some());

        document.draft_source = "fn user_edit() {}\n".to_string();

        let refreshed = PassDebugSource::from_wgsl("p", "fn generated() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert_eq!(document.draft_source, "fn user_edit() {}\n");
    }

    #[test]
    fn update_source_during_active_shortwire_sets_base_source_stale() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);

        let refreshed = PassDebugSource::from_wgsl("p", "fn new_base() {}\n");
        document.update_source(Some(&refreshed), 1, false);

        assert!(
            document
                .shortwire_active
                .as_ref()
                .unwrap()
                .base_source_stale
        );
    }

    #[test]
    fn mark_reset_triggers_pending_reset_then_enter_transition() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new(
            "p".to_string(),
            Some(source.clone()),
            0,
            Some("fn patched() {}\n"),
        );

        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);
        assert!(matches!(
            document.shortwire_active.as_ref().unwrap().phase,
            super::ShortwirePhase::PendingResetThenEnter { .. }
        ));

        let fresh_source = PassDebugSource::from_wgsl("p", "fn fresh() {}\n");
        document.mark_reset(Some(&fresh_source), 2, "Reset".to_string());

        assert!(matches!(
            document.shortwire_active.as_ref().unwrap().phase,
            super::ShortwirePhase::Editing
        ));
        assert!(
            !document
                .shortwire_active
                .as_ref()
                .unwrap()
                .base_source_stale
        );
        assert_eq!(document.generated_base_source, "fn fresh() {}\n");
    }

    #[test]
    fn record_error_during_pending_reset_clears_shortwire() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new(
            "p".to_string(),
            Some(source),
            0,
            Some("fn patched() {}\n"),
        );

        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);

        document.record_error("reset failed".to_string());

        assert!(document.shortwire_active.is_none());
        assert!(
            document
                .last_error
                .as_ref()
                .unwrap()
                .contains("Failed to reset patch")
        );
    }

    #[test]
    fn record_error_during_pending_apply_reverts_to_editing() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);
        if let Some(ref mut active) = document.shortwire_active {
            active.diff_view_enabled = true;
        }
        document.draft_source = "fn edited() {}\n".to_string();
        document.mark_draft_edited(1.0);
        document.exit_shortwire_done(&pending_actions);

        assert!(matches!(
            document.shortwire_active.as_ref().unwrap().phase,
            super::ShortwirePhase::PendingApply { .. }
        ));

        document.record_error("apply failed".to_string());

        assert!(matches!(
            document.shortwire_active.as_ref().unwrap().phase,
            super::ShortwirePhase::Editing
        ));
        assert!(document.shortwire_is_editor_interactive());
        assert!(!document.shortwire_exit_on_apply);
        assert!(document.dirty);
        assert!(
            !document
                .shortwire_active
                .as_ref()
                .unwrap()
                .diff_view_enabled
        );
        assert_eq!(document.last_error.as_deref(), Some("apply failed"));
    }

    #[test]
    fn patch_error_summary_stays_single_line() {
        let error = "\n\nerror: shader failed to compile because a very long generated WGSL line could not be parsed and would otherwise cover the editor with many details about bindings, functions, expressions, and source spans\n  --> generated.wgsl:12:5\n  |\n";
        let summary = super::compact_patch_error(error);

        assert!(!summary.contains('\n'));
        assert!(summary.ends_with("..."));
        assert!(summary.chars().count() <= super::PASS_DEBUG_PATCH_ERROR_SUMMARY_CHARS);
    }

    #[test]
    fn cancel_during_editing_no_patch_stored() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);
        document.draft_source = "fn edited() {}\n".to_string();

        document.exit_shortwire_cancel();

        assert!(document.shortwire_active.is_none());
        assert!(document.shortwire_patches.is_empty());
        assert_eq!(document.draft_source, "fn a() {}\n");
    }

    #[test]
    fn apply_hunks_reverse_order() {
        let base = "line1\nline2\nline3\nline4\n";
        let hunks = vec![
            super::ShortwireHunk {
                old_start: 0,
                old_lines: vec!["line1".to_string()],
                new_lines: vec!["LINE1".to_string()],
                context_before: vec![],
                context_after: vec!["line2".to_string()],
            },
            super::ShortwireHunk {
                old_start: 2,
                old_lines: vec!["line3".to_string()],
                new_lines: vec!["LINE3".to_string()],
                context_before: vec!["line2".to_string()],
                context_after: vec!["line4".to_string()],
            },
        ];
        let result = super::apply_hunks(base, &hunks).unwrap();
        assert_eq!(result, "LINE1\nline2\nLINE3\nline4\n");
    }

    #[test]
    fn fuzzy_hunk_application_at_shifted_offset() {
        let hunks = vec![super::ShortwireHunk {
            old_start: 1,
            old_lines: vec!["line2".to_string()],
            new_lines: vec!["LINE2".to_string()],
            context_before: vec!["line1".to_string()],
            context_after: vec!["line3".to_string()],
        }];

        let shifted_base = "extra\nheader\nline1\nline2\nline3\n";
        let result = super::apply_hunks(shifted_base, &hunks).unwrap();
        assert_eq!(result, "extra\nheader\nline1\nLINE2\nline3\n");
    }

    #[test]
    fn insert_only_hunks_use_context_for_positioning() {
        let base = "a\nb\nc\n";
        let hunks = vec![super::ShortwireHunk {
            old_start: 1,
            old_lines: vec![],
            new_lines: vec!["INSERTED".to_string()],
            context_before: vec!["a".to_string()],
            context_after: vec!["b".to_string()],
        }];
        let result = super::apply_hunks(base, &hunks).unwrap();
        assert_eq!(result, "a\nINSERTED\nb\nc\n");
    }

    #[test]
    fn failed_hunk_application_returns_error() {
        let base = "line1\nline2\nline3\n";
        let hunks = vec![super::ShortwireHunk {
            old_start: 0,
            old_lines: vec!["nonexistent".to_string()],
            new_lines: vec!["replaced".to_string()],
            context_before: vec![],
            context_after: vec![],
        }];
        let result = super::apply_hunks(base, &hunks);
        assert!(result.is_err());
    }

    #[test]
    fn patch_key_stability_different_relation_paths() {
        let row_a = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "x".to_string(),
            relation_path: "path_a".to_string(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        let row_b = PassDebugDependencyRow {
            depth: 0,
            row_key: "1".to_string(),
            parent_row_key: None,
            label: "x".to_string(),
            relation_path: "path_b".to_string(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        assert_ne!(
            super::shortwire_patch_key(&row_a),
            super::shortwire_patch_key(&row_b)
        );
    }

    #[test]
    fn patch_key_with_source_range_fingerprint() {
        let row_a = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "x".to_string(),
            relation_path: "path".to_string(),
            target_id: Some("t".to_string()),
            source_range: Some(PassDebugSourceRange {
                start_byte: 10,
                end_byte: 20,
                line: 1,
                column: 1,
            }),
            source_jump_range: None,
            selectable: true,
        };
        let row_b = PassDebugDependencyRow {
            depth: 0,
            row_key: "1".to_string(),
            parent_row_key: None,
            label: "x".to_string(),
            relation_path: "path".to_string(),
            target_id: Some("t".to_string()),
            source_range: Some(PassDebugSourceRange {
                start_byte: 30,
                end_byte: 40,
                line: 2,
                column: 1,
            }),
            source_jump_range: None,
            selectable: true,
        };
        assert_ne!(
            super::shortwire_patch_key(&row_a),
            super::shortwire_patch_key(&row_b)
        );
    }

    #[test]
    fn document_opened_with_patch_active_keeps_canonical_base() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let document = PassDebugWindowDocument::new(
            "p".to_string(),
            Some(source),
            0,
            Some("fn patched() {}\n"),
        );

        assert_eq!(document.generated_base_source, "fn a() {}\n");
        assert_eq!(document.draft_source, "fn patched() {}\n");
        assert!(document.patch_active);
    }

    #[test]
    fn compute_and_apply_hunks_roundtrip() {
        let base = "fn main() {\n    let x = 1;\n    let y = 2;\n    return x + y;\n}\n";
        let edited = "fn main() {\n    let x = 10;\n    let y = 2;\n    let z = 3;\n    return x + y + z;\n}\n";

        let hunks = super::compute_hunks(base, edited);
        assert!(!hunks.is_empty());

        let result = super::apply_hunks(base, &hunks).unwrap();
        assert_eq!(result, edited);
    }

    #[test]
    fn re_entering_same_node_after_apply_restores_patch() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);
        document.draft_source = "fn edited() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);

        let patched_source = PassDebugSource::from_wgsl("p", "fn edited() {}\n");
        document.mark_applied(
            Some(&patched_source),
            1,
            "fn edited() {}\n".to_string(),
            "Applied".to_string(),
        );

        assert!(document.shortwire_active.is_some());
        assert!(document.shortwire_is_editor_interactive());
        assert!(document.patch_active);
        assert!(!document.shortwire_patches.is_empty());

        pending_actions.lock().unwrap().clear();
        document.exit_shortwire_navigate(&pending_actions);
        assert!(document.shortwire_active.is_none());
        assert!(!document.patch_active);
        {
            let actions = pending_actions.lock().unwrap();
            assert_eq!(actions.len(), 1);
            assert!(matches!(
                actions[0],
                super::PassDebugWindowAction::ResetPatch { .. }
            ));
        }

        let reset_source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        document.mark_reset(Some(&reset_source), 2, "Reset".to_string());

        document.enter_shortwire(&row, &pending_actions);

        assert!(document.shortwire_active.is_some());
        assert_eq!(document.draft_source, "fn edited() {}\n");
        assert!(
            document
                .shortwire_active
                .as_ref()
                .unwrap()
                .diff_view_enabled
        );
    }

    #[test]
    fn enter_shortwire_and_apply_auto_applies_stored_patch() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };

        document.enter_shortwire(&row, &pending_actions);
        document.draft_source = "fn edited() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        let patched_source = PassDebugSource::from_wgsl("p", "fn edited() {}\n");
        document.mark_applied(
            Some(&patched_source),
            1,
            "fn edited() {}\n".to_string(),
            "Applied".to_string(),
        );
        let patch_key = super::shortwire_patch_key(&row);
        document
            .shortwire_patches
            .get_mut(&patch_key)
            .unwrap()
            .reference_image = Some(test_reference_image());
        pending_actions.lock().unwrap().clear();
        document.exit_shortwire_navigate(&pending_actions);
        let reset_source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        document.mark_reset(Some(&reset_source), 2, "Reset".to_string());

        pending_actions.lock().unwrap().clear();
        document.enter_shortwire_and_apply(&row, &pending_actions);

        assert!(matches!(
            document.shortwire_active.as_ref().unwrap().phase,
            super::ShortwirePhase::PendingApply { .. }
        ));
        let actions = pending_actions.lock().unwrap();
        assert_eq!(actions.len(), 1);
        match &actions[0] {
            super::PassDebugWindowAction::ApplyPatch {
                source,
                reference_image,
                ..
            } => {
                assert_eq!(source, "fn edited() {}\n");
                assert_eq!(
                    reference_image
                        .as_ref()
                        .map(|image| image.artifact_id.as_str()),
                    Some("pass__p__shortwire-reference-image__k")
                );
            }
            _ => panic!("expected ApplyPatch action"),
        }
    }

    #[test]
    fn enter_shortwire_and_apply_falls_back_to_edit_on_conflict() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {}\n");
        let mut document =
            PassDebugWindowDocument::new("p".to_string(), Some(source.clone()), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };

        document.enter_shortwire(&row, &pending_actions);
        document.draft_source = "fn edited() {}\n".to_string();
        document.exit_shortwire_done(&pending_actions);
        let patched_source = PassDebugSource::from_wgsl("p", "fn edited() {}\n");
        document.mark_applied(
            Some(&patched_source),
            1,
            "fn edited() {}\n".to_string(),
            "Applied".to_string(),
        );
        pending_actions.lock().unwrap().clear();
        document.exit_shortwire_navigate(&pending_actions);

        let completely_different = PassDebugSource::from_wgsl(
            "p",
            "struct X { v: f32 }\nfn totally_different() -> X { return X(0.0); }\n",
        );
        document.mark_reset(Some(&completely_different), 2, "Reset".to_string());

        pending_actions.lock().unwrap().clear();
        document.enter_shortwire_and_apply(&row, &pending_actions);

        assert!(matches!(
            document.shortwire_active.as_ref().unwrap().phase,
            super::ShortwirePhase::Editing
        ));
        assert!(document.shortwire_patches.is_empty());
        assert!(document.last_error.as_ref().unwrap().contains("outdated"));
    }

    #[test]
    fn done_with_stale_base_rebases_edits() {
        let source = PassDebugSource::from_wgsl("p", "fn a() {\n    let x = 1;\n}\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);

        document.draft_source = "fn a() {\n    let x = 99;\n}\n".to_string();

        let new_base =
            PassDebugSource::from_wgsl("p", "fn a() {\n    let x = 1;\n    let y = 2;\n}\n");
        document.update_source(Some(&new_base), 1, false);
        assert!(
            document
                .shortwire_active
                .as_ref()
                .unwrap()
                .base_source_stale
        );

        document.exit_shortwire_done(&pending_actions);

        assert!(matches!(
            document.shortwire_active.as_ref().unwrap().phase,
            super::ShortwirePhase::PendingApply { .. }
        ));
        assert_eq!(
            document.draft_source,
            "fn a() {\n    let x = 99;\n    let y = 2;\n}\n"
        );
    }

    #[test]
    fn pending_analysis_discarded_on_shortwire_entry() {
        let ctx = egui::Context::default();
        let source = PassDebugSource::from_wgsl("p", "fn a() -> f32 { return 1.0; }\n");
        let mut document = PassDebugWindowDocument::new("p".to_string(), Some(source), 0, false);
        let pending_actions = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));

        document.draft_source = "fn edited() -> f32 { return 2.0; }\n".to_string();
        document.mark_draft_edited(10.0);
        document.maybe_refresh_pending_draft_analysis(10.2, &ctx);

        let row = PassDebugDependencyRow {
            depth: 0,
            row_key: "0".to_string(),
            parent_row_key: None,
            label: "test".to_string(),
            relation_path: String::new(),
            target_id: Some("t".to_string()),
            source_range: None,
            source_jump_range: None,
            selectable: true,
        };
        document.enter_shortwire(&row, &pending_actions);

        assert_eq!(document.draft_analysis_due_secs, None);
    }
}
