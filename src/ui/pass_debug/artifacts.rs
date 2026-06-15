use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::app::{DiffStats, ShortwirePastedReferenceImage, ShortwireReferenceImage};
use crate::dsl::{DebugArtifactAnchor, DebugArtifactItem, DebugArtifactRole};
use crate::ui::pass_debug::patch::ShortwireHunk;

pub(crate) const DEBUG_ARTIFACT_DEFAULT_SLOT: &str = "default";
pub(crate) const DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT: &str = "reference-workspace";
pub(crate) const DEBUG_ARTIFACT_REFERENCE_PATCHES_SLOT: &str = "reference-patches";
const DEBUG_ARTIFACT_REFERENCE_FILE_SLOT_PREFIX: &str = "file:";
pub(crate) const REFERENCE_WORKSPACE_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ShortwirePatchesPayload<Patch> {
    pub(crate) version: u32,
    pub(crate) patches: HashMap<String, Patch>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ShortwireNodePatch {
    pub(crate) hunks: Vec<ShortwireHunk>,
    pub(crate) base_source_hash: u64,
    #[serde(
        rename = "referenceImage",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) reference_image: Option<ShortwireReferenceImage>,
    #[serde(
        rename = "diffResult",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) diff_result: Option<ShortwireDiffResult>,
}

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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferenceWorkspaceManifest {
    pub(crate) version: u32,
    pub(crate) root_path: Option<String>,
    pub(crate) root_label: String,
    pub(crate) selected_file: Option<String>,
    pub(crate) files: Vec<ReferenceWorkspaceManifestFile>,
    #[serde(default)]
    pub(crate) skipped_files: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReferenceWorkspaceManifestFile {
    pub(crate) relative_path: String,
    pub(crate) artifact_id: String,
    pub(crate) content_hash: String,
    pub(crate) size: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ReferencePatchesPayload<Patch> {
    pub(crate) version: u32,
    pub(crate) patches: HashMap<String, Patch>,
}

pub(crate) fn pass_reference_workspace_artifact_id(pass_name: &str) -> String {
    [
        "pass".to_string(),
        safe_debug_artifact_segment(pass_name, "unnamed"),
        "reference-workspace".to_string(),
        DEBUG_ARTIFACT_REFERENCE_WORKSPACE_SLOT.to_string(),
    ]
    .join("__")
}

pub(crate) fn pass_reference_file_artifact_id(pass_name: &str, relative_path: &str) -> String {
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

pub(crate) fn pass_reference_file_slot_key(relative_path: &str) -> String {
    format!(
        "{}{}",
        DEBUG_ARTIFACT_REFERENCE_FILE_SLOT_PREFIX,
        debug_artifact_content_hash(relative_path.as_bytes())
    )
}

pub(crate) fn pass_reference_file_artifact_item(
    pass_name: &str,
    relative_path: &str,
    artifact_id: &str,
    size: u64,
    content_hash: String,
) -> DebugArtifactItem {
    let file_name = safe_debug_artifact_segment(&relative_path.replace('/', "__"), "reference");
    DebugArtifactItem {
        id: artifact_id.to_string(),
        anchor: DebugArtifactAnchor::Pass {
            pass_name: pass_name.to_string(),
        },
        role: DebugArtifactRole::ReferenceCode,
        name: format!("Reference: {relative_path}"),
        mime_type: "text/plain".to_string(),
        path: format!(
            "debug-artifacts/{}/{}",
            safe_debug_artifact_segment(artifact_id, "artifact"),
            file_name
        ),
        size: Some(size),
        content_hash: Some(content_hash),
        slot_key: Some(pass_reference_file_slot_key(relative_path)),
    }
}

pub(crate) fn pass_reference_patches_artifact_id(pass_name: &str) -> String {
    [
        "pass".to_string(),
        safe_debug_artifact_segment(pass_name, "unnamed"),
        "patch".to_string(),
        DEBUG_ARTIFACT_REFERENCE_PATCHES_SLOT.to_string(),
    ]
    .join("__")
}

pub(crate) fn pass_patches_artifact_id(pass_name: &str) -> String {
    [
        "pass".to_string(),
        safe_debug_artifact_segment(pass_name, "unnamed"),
        "patch".to_string(),
        DEBUG_ARTIFACT_DEFAULT_SLOT.to_string(),
    ]
    .join("__")
}

pub(crate) fn reference_workspace_artifact_item(
    pass_name: &str,
    content_text: &str,
) -> DebugArtifactItem {
    let artifact_id = pass_reference_workspace_artifact_id(pass_name);
    let file_name = format!(
        "{}.reference-workspace.json",
        safe_debug_artifact_segment(pass_name, "pass")
    );
    DebugArtifactItem {
        id: artifact_id.clone(),
        anchor: DebugArtifactAnchor::Pass {
            pass_name: pass_name.to_string(),
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
    }
}

pub(crate) fn reference_patches_artifact_item(
    pass_name: &str,
    content_text: &str,
) -> DebugArtifactItem {
    let artifact_id = pass_reference_patches_artifact_id(pass_name);
    let file_name = format!(
        "{}.reference-patches.json",
        safe_debug_artifact_segment(pass_name, "pass")
    );
    DebugArtifactItem {
        id: artifact_id.clone(),
        anchor: DebugArtifactAnchor::Pass {
            pass_name: pass_name.to_string(),
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
    }
}

pub(crate) fn shortwire_patches_artifact_item(
    pass_name: &str,
    content_text: &str,
) -> DebugArtifactItem {
    let artifact_id = pass_patches_artifact_id(pass_name);
    let file_name = format!(
        "{}.patches.json",
        safe_debug_artifact_segment(pass_name, "pass")
    );
    DebugArtifactItem {
        id: artifact_id.clone(),
        anchor: DebugArtifactAnchor::Pass {
            pass_name: pass_name.to_string(),
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
    }
}

pub(crate) fn shortwire_reference_image_artifact_id(pass_name: &str, patch_key: &str) -> String {
    format!(
        "pass__{}__shortwire-reference-image__{}",
        safe_debug_artifact_segment(pass_name, "pass"),
        debug_artifact_content_hash(patch_key.as_bytes()),
    )
}

pub(crate) fn shortwire_reference_image_artifact(
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

pub(crate) fn safe_debug_artifact_segment(value: &str, fallback: &str) -> String {
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

pub(crate) fn debug_artifact_content_hash(bytes: &[u8]) -> String {
    let mut hash = 0x811c9dc5u32;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    format!("{hash:08x}")
}
