use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use crate::app::ShortwireReferenceImage;
use crate::dsl::DebugArtifactItem;
use crate::ui::pass_debug::artifacts::ReferenceWorkspaceManifest;
use crate::ui::pass_debug::dependency_tree::PassDebugTreeClick;
use crate::ui::pass_debug::reference_workspace::ReferenceSyncPlan;
use crate::ui::pass_debug::shortwire::ShortwireDiffCaptureRequest;

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

pub struct PassDebugPatchApplyResult {
    pub artifacts: Vec<(DebugArtifactItem, String)>,
    pub binary_artifacts: Vec<(DebugArtifactItem, Vec<u8>)>,
    pub diff_capture: Option<ShortwireDiffCaptureRequest>,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum PassDebugEvent {
    Tick {
        now_secs: f64,
    },
    EmitEffect(PassDebugEffect),
    SaveRequested,
    CloseRequested,
    ToggleShortwireDiff,
    ReferenceReloadRequested {
        now_secs: f64,
    },
    ReferenceOpenFolderRequested {
        now_secs: f64,
    },
    ReferenceFileSelected {
        relative_path: String,
        now_secs: f64,
    },
    ReferenceSyncTick {
        now_secs: f64,
    },
    ShaderDraftEdited {
        now_secs: f64,
    },
    ShaderDraftReplaced {
        source: String,
        now_secs: f64,
    },
    ReferenceDraftEdited {
        now_secs: f64,
    },
    ReferenceEditorReplaced {
        source: String,
        now_secs: f64,
    },
    ShaderEditorClicked {
        char_index: usize,
    },
    DependencyTreeClicked {
        click: PassDebugTreeClick,
    },
    DependencyFilterEdited {
        text: String,
    },
    DependencyShortwireRequested {
        row_index: usize,
    },
    MergeOpenResolver,
    MergeCloseConflictWindows,
    MergeReopenChoicePopup,
    MergeResolvedEdited {
        source: String,
    },
    MergeCancelResolution,
    MergeUseIncoming,
    MergeKeepLocal,
    MergeApplyResolved,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) struct ReferenceFileWrite {
    pub(crate) relative_path: String,
    pub(crate) content: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub(crate) enum PassDebugEffect {
    ApplyPatch {
        pass_name: String,
        source: String,
        reference_image: Option<ShortwireReferenceImage>,
    },
    ResetPatch {
        pass_name: String,
    },
    ResetAllPatches,
    UpsertTextArtifact {
        item: DebugArtifactItem,
        content_text: String,
    },
    UpsertBinaryArtifact {
        item: DebugArtifactItem,
        bytes: Vec<u8>,
    },
    ReadReferenceManifestFiles {
        root: PathBuf,
        manifest: ReferenceWorkspaceManifest,
    },
    ReadReferenceFolder {
        path: PathBuf,
        now_secs: f64,
    },
    ReloadReferenceWorkspace {
        root: PathBuf,
        root_label: String,
        selected_file: Option<String>,
        single_file: bool,
        now_secs: f64,
    },
    WriteReferenceFiles {
        root: PathBuf,
        files: Vec<ReferenceFileWrite>,
    },
    RunReferenceSyncPlan {
        plan: ReferenceSyncPlan,
    },
    ReadReferenceShortwireFile {
        path: PathBuf,
        write_after_read: bool,
    },
    WriteReferenceShortwireFile {
        path: PathBuf,
        content: String,
    },
    RestoreReferenceShortwireFile {
        path: PathBuf,
        content: String,
    },
    PickReferenceFolder {
        now_secs: f64,
    },
    RequestDiffCapture(ShortwireDiffCaptureRequest),
    CloseViewport,
    FocusViewport,
}

impl PassDebugEffect {
    pub(crate) fn into_window_action(self) -> Result<PassDebugWindowAction, Self> {
        match self {
            Self::ApplyPatch {
                pass_name,
                source,
                reference_image,
            } => Ok(PassDebugWindowAction::ApplyPatch {
                pass_name,
                source,
                reference_image,
            }),
            Self::ResetPatch { pass_name } => Ok(PassDebugWindowAction::ResetPatch { pass_name }),
            Self::ResetAllPatches => Ok(PassDebugWindowAction::ResetAllPatches),
            Self::UpsertTextArtifact { item, content_text } => {
                Ok(PassDebugWindowAction::UpsertDebugArtifact { item, content_text })
            }
            effect => Err(effect),
        }
    }
}

pub(crate) fn push_window_action(
    pending_actions: &Arc<Mutex<Vec<PassDebugWindowAction>>>,
    action: PassDebugWindowAction,
) {
    if let Ok(mut pending) = pending_actions.lock() {
        pending.push(action);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_patch_effect_converts_to_public_window_action() {
        let action = PassDebugEffect::ApplyPatch {
            pass_name: "main".to_string(),
            source: "shader".to_string(),
            reference_image: None,
        }
        .into_window_action()
        .expect("apply patch maps to a public action");

        match action {
            PassDebugWindowAction::ApplyPatch {
                pass_name,
                source,
                reference_image,
            } => {
                assert_eq!(pass_name, "main");
                assert_eq!(source, "shader");
                assert!(reference_image.is_none());
            }
            _ => panic!("expected apply patch action"),
        }
    }

    #[test]
    fn non_public_effect_stays_internal() {
        let effect = PassDebugEffect::FocusViewport;

        assert!(effect.into_window_action().is_err());
    }
}
