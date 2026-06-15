use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

use rust_wgpu_fiber::eframe::egui;

use crate::app::ShortwirePastedReferenceImage;
use crate::dsl::DebugArtifactItem;
use crate::metric_log;
use crate::renderer::PassDebugSource;
use crate::ui::pass_debug::artifacts::ShortwireDiffResult;
use crate::ui::pass_debug::event::{
    PassDebugEffect, PassDebugPatchApplyResult, PassDebugWindowAction, push_window_action,
};
use crate::ui::pass_debug::file_io::{
    read_manifest_reference_file, read_reference_file, read_reference_folder,
    read_reference_shortwire_local_file, write_reference_shortwire_local_file,
    write_reference_workspace_file,
};
use crate::ui::pass_debug::reference_workspace::{
    ReferenceSyncCompletion, ReferenceSyncPlan, ReferenceSyncedFile, ReferenceWorkspaceFile,
    ReferenceWorkspaceState, reference_file_artifact_from_sync_file,
    reference_workspace_artifact_from_sync_plan,
};
use crate::ui::pass_debug::shader_document::hash_source;
use crate::ui::pass_debug::shortwire::ShortwireDiffCaptureRequest;
use crate::ui::pass_debug::viewport::{
    PassDebugCloseDecision, PassDebugViewportSnapshot, classify_pass_debug_close_request,
    pass_debug_default_window_size, pass_debug_viewport_builder,
};
use crate::ui::pass_debug_window::{PassDebugWindowDocument, ShortwireDiffCaptureAttempt};

pub struct PassDebugWindowState {
    pub(crate) pass_name: String,
    pub(crate) viewport_id: egui::ViewportId,
    pub(crate) document: Arc<Mutex<PassDebugWindowDocument>>,
    pub(crate) close_requested: Arc<AtomicBool>,
    pub(crate) pending_actions: Arc<Mutex<Vec<PassDebugWindowAction>>>,
    pub(crate) last_viewport_snapshot: Arc<Mutex<Option<PassDebugViewportSnapshot>>>,
    pub(crate) loaded_shortwire_patches_artifact_hash: Option<u64>,
    pub(crate) viewport_initialized: bool,
    pub(crate) focus_requested: bool,
}

impl PassDebugWindowState {
    pub(crate) fn new(
        pass_name: String,
        source: Option<PassDebugSource>,
        source_revision: u64,
        patch_source: Option<&str>,
    ) -> Self {
        let viewport_id = egui::ViewportId::from_hash_of(("pass-debug", pass_name.as_str()));
        Self {
            document: Arc::new(Mutex::new(PassDebugWindowDocument::new_from_runtime_patch(
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

    pub(crate) fn update_source(
        &self,
        source: Option<&PassDebugSource>,
        source_revision: u64,
        patch_source: Option<&str>,
    ) {
        if let Ok(mut document) = self.document.lock() {
            document.update_source_with_runtime_patch(
                source,
                source_revision,
                patch_source,
                &self.pending_actions,
            );
        }
    }

    pub(crate) fn update_reference_workspace(
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

    pub(crate) fn sync_shortwire_patches_from_artifact(&mut self, text: Option<&str>) {
        let Some(text) = text else {
            return;
        };
        let artifact_hash = hash_source(text);
        if self.loaded_shortwire_patches_artifact_hash == Some(artifact_hash) {
            return;
        }
        if let Ok(mut document) = self.document.lock() {
            if !document.can_restore_shortwire_patches_from_artifact() {
                return;
            }
            if document.restore_shortwire_patches_from_text(text) {
                eprintln!(
                    "[shortwire-diff] restore_patches_from_artifact pass={} artifact_hash={} patches={}",
                    self.pass_name,
                    artifact_hash,
                    document.shortwire_patch_count(),
                );
                self.loaded_shortwire_patches_artifact_hash = Some(artifact_hash);
            }
        }
    }

    pub(crate) fn drain_actions(&self, out: &mut Vec<PassDebugWindowAction>) {
        if let Ok(mut pending) = self.pending_actions.lock() {
            out.extend(pending.drain(..));
        }
    }
}

pub type PassDebugWindowMap = HashMap<String, PassDebugWindowState>;

#[derive(Clone, Copy)]
pub(crate) struct PassDebugWindowRenderHooks {
    pub(crate) render_embedded_content: fn(
        &mut egui::Ui,
        &Arc<Mutex<PassDebugWindowDocument>>,
        &Arc<Mutex<Vec<PassDebugWindowAction>>>,
        &AtomicBool,
    ),
    pub(crate) render_viewport: fn(
        &mut egui::Ui,
        &Arc<Mutex<PassDebugWindowDocument>>,
        &Arc<Mutex<Vec<PassDebugWindowAction>>>,
        &AtomicBool,
    ),
}

pub(crate) fn show_pass_debug_windows_with_render_hooks(
    ctx: &egui::Context,
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
    pass_sources_revision: u64,
    pass_shader_overrides: &HashMap<String, String>,
    debug_artifacts: &crate::debug_artifacts::DebugArtifactStore,
    hooks: PassDebugWindowRenderHooks,
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
                            (hooks.render_embedded_content)(
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
                    (hooks.render_viewport)(
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

        run_document_effects(&state.document, &state.pending_actions);
        state.drain_actions(&mut actions);
        if let Ok(mut document) = state.document.lock() {
            if let Some((item, content_text)) = document.take_patches_dirty_artifact() {
                actions.push(
                    PassDebugEffect::UpsertTextArtifact { item, content_text }
                        .into_window_action()
                        .expect("shortwire patch artifact effect maps to window action"),
                );
            }
            if let Some((item, content_text)) = document.take_reference_patches_dirty_artifact() {
                actions.push(
                    PassDebugEffect::UpsertTextArtifact { item, content_text }
                        .into_window_action()
                        .expect("reference patch artifact effect maps to window action"),
                );
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

fn run_document_effects(
    document: &Arc<Mutex<PassDebugWindowDocument>>,
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
) {
    let mut effects = document
        .lock()
        .map(|mut document| document.drain_effects())
        .unwrap_or_default();

    while !effects.is_empty() {
        let effect = effects.remove(0);
        match effect.into_window_action() {
            Ok(action) => push_window_action(pending_actions, action),
            Err(PassDebugEffect::PickReferenceFolder { now_secs }) => {
                let picked_path = rfd::FileDialog::new().pick_folder();
                if let Some(path) = picked_path {
                    effects.push(PassDebugEffect::ReadReferenceFolder { path, now_secs });
                }
            }
            Err(PassDebugEffect::ReadReferenceFolder { path, now_secs }) => {
                let pass_name = document
                    .lock()
                    .map(|document| document.pass_name.clone())
                    .unwrap_or_else(|_| "Reference".to_string());
                let result = read_reference_folder(&path, &pass_name, true);
                if let Ok(mut document) = document.lock() {
                    document.apply_reference_folder_import_result(&path, now_secs, result);
                    effects.extend(document.drain_effects());
                }
            }
            Err(PassDebugEffect::ReloadReferenceWorkspace {
                root,
                root_label,
                selected_file,
                single_file,
                now_secs,
            }) => {
                if !root.exists() {
                    if let Ok(mut document) = document.lock() {
                        document.mark_reference_reload_missing_path();
                        effects.extend(document.drain_effects());
                    }
                    continue;
                }

                let pass_name = document
                    .lock()
                    .map(|document| document.pass_name.clone())
                    .unwrap_or_else(|_| "Reference".to_string());
                if single_file {
                    let Some(relative_path) = selected_file else {
                        if let Ok(mut document) = document.lock() {
                            document.store.reference_workspace.last_status =
                                Some("No selected file to reload".to_string());
                            effects.extend(document.drain_effects());
                        }
                        continue;
                    };
                    let path = root.join(&relative_path);
                    let result = read_reference_file(&path, &root, &pass_name, true);
                    if let Ok(mut document) = document.lock() {
                        document.apply_reference_file_reload_result(
                            &root,
                            root_label,
                            relative_path,
                            now_secs,
                            result,
                        );
                        effects.extend(document.drain_effects());
                    }
                } else {
                    let result = read_reference_folder(&root, &pass_name, true);
                    if let Ok(mut document) = document.lock() {
                        document.apply_reference_folder_reload_result(
                            &root,
                            root_label,
                            selected_file,
                            now_secs,
                            result,
                        );
                        effects.extend(document.drain_effects());
                    }
                }
            }
            Err(PassDebugEffect::ReadReferenceManifestFiles { root, manifest }) => {
                let incoming = run_reference_manifest_local_read_effect(root, manifest);
                if let Ok(mut document) = document.lock() {
                    document.apply_reference_manifest_local_read_result(incoming);
                    effects.extend(document.drain_effects());
                }
            }
            Err(PassDebugEffect::RunReferenceSyncPlan { plan }) => {
                let pass_name = document
                    .lock()
                    .map(|document| document.pass_name.clone())
                    .unwrap_or_else(|_| "Reference".to_string());
                let completion = run_reference_sync_plan_effect(&pass_name, plan);
                if let Ok(mut document) = document.lock() {
                    let artifacts = document.apply_reference_sync_completion(completion);
                    for (item, content_text) in artifacts {
                        push_window_action(
                            pending_actions,
                            PassDebugEffect::UpsertTextArtifact { item, content_text }
                                .into_window_action()
                                .expect("reference sync artifact effect maps to window action"),
                        );
                    }
                    effects.extend(document.drain_effects());
                }
            }
            Err(PassDebugEffect::ReadReferenceShortwireFile {
                path,
                write_after_read,
            }) => {
                let result = read_reference_shortwire_local_file(&path);
                if let Ok(mut document) = document.lock() {
                    document.apply_reference_shortwire_local_snapshot(
                        path,
                        result,
                        write_after_read,
                    );
                    effects.extend(document.drain_effects());
                }
            }
            Err(PassDebugEffect::WriteReferenceShortwireFile { path, content }) => {
                let result = write_reference_shortwire_local_file(&path, &content);
                if let Ok(mut document) = document.lock() {
                    document.apply_reference_shortwire_local_write_result(path, result);
                    effects.extend(document.drain_effects());
                }
            }
            Err(PassDebugEffect::RestoreReferenceShortwireFile { path, content }) => {
                let result = write_reference_shortwire_local_file(&path, &content);
                if let Ok(mut document) = document.lock() {
                    document.apply_reference_shortwire_local_restore_result(path, result);
                    effects.extend(document.drain_effects());
                }
            }
            Err(effect) => {
                eprintln!("[pass-debug] unhandled internal effect: {effect:?}");
            }
        }
    }
}

fn run_reference_manifest_local_read_effect(
    root: std::path::PathBuf,
    manifest: crate::ui::pass_debug::artifacts::ReferenceWorkspaceManifest,
) -> ReferenceWorkspaceState {
    let mut files = Vec::new();
    let mut local_loaded_count = 0usize;
    let mut missing_count = 0usize;

    for manifest_file in manifest.files {
        match read_manifest_reference_file(&root, &manifest_file) {
            Ok(source) => {
                local_loaded_count += 1;
                files.push(ReferenceWorkspaceFile {
                    relative_path: manifest_file.relative_path,
                    artifact_id: manifest_file.artifact_id,
                    source: source.clone(),
                    loaded_source: source,
                });
            }
            Err(_) => {
                missing_count += 1;
            }
        }
    }

    let mut state = ReferenceWorkspaceState::default();
    state.replace_files(
        Some(root.to_string_lossy().to_string()),
        manifest.root_label,
        files,
        manifest.selected_file,
        manifest.skipped_files + missing_count,
        false,
    );
    state.last_status = if missing_count > 0 {
        Some(if local_loaded_count > 0 {
            format!("Loaded local reference ({missing_count} missing)")
        } else {
            format!("Local reference missing ({missing_count} missing)")
        })
    } else {
        Some("Loaded local reference".to_string())
    };
    state
}

fn run_reference_sync_plan_effect(
    pass_name: &str,
    plan: ReferenceSyncPlan,
) -> ReferenceSyncCompletion {
    let mut artifacts = Vec::new();
    let mut synced_files = Vec::new();
    let mut write_errors = Vec::new();
    let mut wrote_any_file = false;

    if let Some(root_path) = plan.root_path.as_deref() {
        let root = std::path::PathBuf::from(root_path);
        for file in plan.files.iter().filter(|file| file.is_dirty()) {
            match write_reference_workspace_file(&root, &file.relative_path, &file.source) {
                Ok(()) => {
                    wrote_any_file = true;
                    synced_files.push(ReferenceSyncedFile {
                        relative_path: file.relative_path.clone(),
                        source: file.source.clone(),
                    });
                }
                Err(error) => write_errors.push(error),
            }
        }

        let should_upsert_manifest = plan.manifest_dirty || wrote_any_file;
        let mut emitted_manifest = false;
        if should_upsert_manifest
            && let Some(artifact) = reference_workspace_artifact_from_sync_plan(pass_name, &plan)
        {
            emitted_manifest = true;
            artifacts.push(artifact);
        }
        return ReferenceSyncCompletion {
            plan,
            artifacts,
            synced_files,
            write_errors,
            emitted_manifest,
        };
    }

    let mut emitted_manifest = false;
    if plan.manifest_dirty
        && let Some(artifact) = reference_workspace_artifact_from_sync_plan(pass_name, &plan)
    {
        emitted_manifest = true;
        artifacts.push(artifact);
    }

    for file in plan.files.iter().filter(|file| file.is_dirty()) {
        artifacts.push(reference_file_artifact_from_sync_file(pass_name, file));
        synced_files.push(ReferenceSyncedFile {
            relative_path: file.relative_path.clone(),
            source: file.source.clone(),
        });
    }

    ReferenceSyncCompletion {
        plan,
        artifacts,
        synced_files,
        write_errors,
        emitted_manifest,
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
        existing
            .close_requested
            .store(false, std::sync::atomic::Ordering::Relaxed);
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

pub fn has_active_shortwire(windows: &PassDebugWindowMap) -> bool {
    windows.values().any(|state| {
        state
            .document
            .lock()
            .map(|document| document.has_active_shortwire())
            .unwrap_or(false)
    })
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
    document.record_shortwire_diff_result(request, diff_result)
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
        match document.request_shortwire_diff_capture(&mut pending_pasted_reference) {
            ShortwireDiffCaptureAttempt::Inactive => {}
            ShortwireDiffCaptureAttempt::MissingPatch => {
                active_count += 1;
                missing_patch_count += 1;
            }
            ShortwireDiffCaptureAttempt::Captured(result) => {
                return result;
            }
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_window_state_initializes_viewport_and_focus_request() {
        let state = PassDebugWindowState::new("main".to_string(), None, 0, None);

        assert_eq!(state.pass_name, "main");
        assert!(state.focus_requested);
        assert!(!state.viewport_initialized);
        assert_eq!(
            state.viewport_id,
            egui::ViewportId::from_hash_of(("pass-debug", "main"))
        );
    }
}
