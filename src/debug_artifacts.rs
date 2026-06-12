use std::collections::HashMap;

use crate::dsl::{DebugArtifactAnchor, DebugArtifactItem, DebugArtifactRole, DebugArtifacts};

const DEFAULT_SLOT_KEY: &str = "default";
const REFERENCE_WORKSPACE_SLOT_KEY: &str = "reference-workspace";
const REFERENCE_PATCHES_SLOT_KEY: &str = "reference-patches";
const REFERENCE_FILE_SLOT_PREFIX: &str = "file:";

#[derive(Clone, Debug)]
pub struct DebugArtifactTextSnapshot {
    pub item: DebugArtifactItem,
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct DebugArtifactStore {
    manifest: DebugArtifacts,
    text_contents: HashMap<String, String>,
    binary_contents: HashMap<String, Vec<u8>>,
}

impl DebugArtifactStore {
    pub fn is_empty(&self) -> bool {
        self.manifest.items.is_empty()
    }

    pub fn export_manifest(&self) -> Option<DebugArtifacts> {
        if self.manifest.items.is_empty() {
            None
        } else {
            Some(self.manifest.clone())
        }
    }

    pub fn sync_manifest(&mut self, manifest: Option<DebugArtifacts>) -> Vec<String> {
        self.manifest = manifest.unwrap_or_default();
        self.text_contents
            .retain(|artifact_id, _| self.manifest.items.contains_key(artifact_id));
        self.binary_contents
            .retain(|artifact_id, _| self.manifest.items.contains_key(artifact_id));
        self.missing_content_artifact_ids()
    }

    pub fn upsert(&mut self, item: DebugArtifactItem, content_text: Option<String>) {
        let artifact_id = item.id.clone();
        self.manifest.version = 1;
        self.manifest.items.insert(artifact_id.clone(), item);
        if let Some(content) = content_text {
            self.binary_contents.remove(artifact_id.as_str());
            self.text_contents.insert(artifact_id, content);
        }
    }

    pub fn upsert_bytes(&mut self, item: DebugArtifactItem, bytes: Vec<u8>) {
        let artifact_id = item.id.clone();
        self.manifest.version = 1;
        self.manifest.items.insert(artifact_id.clone(), item);
        self.text_contents.remove(artifact_id.as_str());
        self.binary_contents.insert(artifact_id, bytes);
    }

    pub fn delete(&mut self, artifact_id: &str) {
        self.manifest.items.remove(artifact_id);
        self.text_contents.remove(artifact_id);
        self.binary_contents.remove(artifact_id);
    }

    pub fn text(&self, artifact_id: &str) -> Option<&str> {
        self.text_contents.get(artifact_id).map(String::as_str)
    }

    pub fn bytes(&self, artifact_id: &str) -> Option<&[u8]> {
        self.binary_contents
            .get(artifact_id)
            .map(Vec::as_slice)
            .or_else(|| {
                self.text_contents
                    .get(artifact_id)
                    .map(|text| text.as_bytes())
            })
    }

    pub fn find_pass_reference_item(&self, pass_name: &str) -> Option<&DebugArtifactItem> {
        self.manifest.items.values().find(|item| {
            item.role == DebugArtifactRole::ReferenceCode
                && item.slot_key.as_deref().unwrap_or(DEFAULT_SLOT_KEY) == DEFAULT_SLOT_KEY
                && matches!(
                    &item.anchor,
                    DebugArtifactAnchor::Pass { pass_name: anchor_pass_name }
                        if anchor_pass_name == pass_name
                )
        })
    }

    pub fn pass_reference_text(&self, pass_name: &str) -> Option<&str> {
        let item = self.find_pass_reference_item(pass_name)?;
        self.text(item.id.as_str())
    }

    pub fn find_pass_reference_workspace_item(
        &self,
        pass_name: &str,
    ) -> Option<&DebugArtifactItem> {
        self.manifest.items.values().find(|item| {
            item.role == DebugArtifactRole::Attachment
                && item.slot_key.as_deref() == Some(REFERENCE_WORKSPACE_SLOT_KEY)
                && matches!(
                    &item.anchor,
                    DebugArtifactAnchor::Pass { pass_name: anchor_pass_name }
                        if anchor_pass_name == pass_name
                )
        })
    }

    pub fn pass_reference_workspace_text(&self, pass_name: &str) -> Option<&str> {
        let item = self.find_pass_reference_workspace_item(pass_name)?;
        self.text(item.id.as_str())
    }

    pub fn pass_reference_file_texts(&self, pass_name: &str) -> Vec<DebugArtifactTextSnapshot> {
        let mut snapshots = self
            .manifest
            .items
            .values()
            .filter(|item| {
                item.role == DebugArtifactRole::ReferenceCode
                    && item
                        .slot_key
                        .as_deref()
                        .is_some_and(|slot| slot.starts_with(REFERENCE_FILE_SLOT_PREFIX))
                    && matches!(
                        &item.anchor,
                        DebugArtifactAnchor::Pass { pass_name: anchor_pass_name }
                            if anchor_pass_name == pass_name
                    )
            })
            .filter_map(|item| {
                self.text(item.id.as_str())
                    .map(|text| DebugArtifactTextSnapshot {
                        item: item.clone(),
                        text: text.to_string(),
                    })
            })
            .collect::<Vec<_>>();
        snapshots.sort_by(|a, b| a.item.id.cmp(&b.item.id));
        snapshots
    }

    pub fn find_pass_patches_item(&self, pass_name: &str) -> Option<&DebugArtifactItem> {
        self.manifest.items.values().find(|item| {
            item.role == DebugArtifactRole::Patch
                && item.slot_key.as_deref().unwrap_or(DEFAULT_SLOT_KEY) == DEFAULT_SLOT_KEY
                && matches!(
                    &item.anchor,
                    DebugArtifactAnchor::Pass { pass_name: anchor_pass_name }
                        if anchor_pass_name == pass_name
                )
        })
    }

    pub fn pass_patches_text(&self, pass_name: &str) -> Option<&str> {
        let item = self.find_pass_patches_item(pass_name)?;
        self.text(item.id.as_str())
    }

    pub fn find_pass_reference_patches_item(&self, pass_name: &str) -> Option<&DebugArtifactItem> {
        self.manifest.items.values().find(|item| {
            item.role == DebugArtifactRole::Patch
                && item.slot_key.as_deref() == Some(REFERENCE_PATCHES_SLOT_KEY)
                && matches!(
                    &item.anchor,
                    DebugArtifactAnchor::Pass { pass_name: anchor_pass_name }
                        if anchor_pass_name == pass_name
                )
        })
    }

    pub fn pass_reference_patches_text(&self, pass_name: &str) -> Option<&str> {
        let item = self.find_pass_reference_patches_item(pass_name)?;
        self.text(item.id.as_str())
    }

    fn missing_content_artifact_ids(&self) -> Vec<String> {
        self.manifest
            .items
            .values()
            .filter(|item| {
                if item.mime_type.starts_with("text/") {
                    !self.text_contents.contains_key(item.id.as_str())
                } else {
                    !self.binary_contents.contains_key(item.id.as_str())
                }
            })
            .map(|item| item.id.clone())
            .collect()
    }
}
