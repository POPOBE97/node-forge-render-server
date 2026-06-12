use std::collections::HashMap;

use crate::dsl::{DebugArtifactAnchor, DebugArtifactItem, DebugArtifactRole, DebugArtifacts};

const DEFAULT_SLOT_KEY: &str = "default";

#[derive(Clone, Debug, Default)]
pub struct DebugArtifactStore {
    manifest: DebugArtifacts,
    text_contents: HashMap<String, String>,
}

impl DebugArtifactStore {
    pub fn sync_manifest(&mut self, manifest: Option<DebugArtifacts>) -> Vec<String> {
        self.manifest = manifest.unwrap_or_default();
        self.text_contents
            .retain(|artifact_id, _| self.manifest.items.contains_key(artifact_id));
        self.missing_text_artifact_ids()
    }

    pub fn upsert(&mut self, item: DebugArtifactItem, content_text: Option<String>) {
        let artifact_id = item.id.clone();
        self.manifest.version = 1;
        self.manifest.items.insert(artifact_id.clone(), item);
        if let Some(content) = content_text {
            self.text_contents.insert(artifact_id, content);
        }
    }

    pub fn delete(&mut self, artifact_id: &str) {
        self.manifest.items.remove(artifact_id);
        self.text_contents.remove(artifact_id);
    }

    pub fn text(&self, artifact_id: &str) -> Option<&str> {
        self.text_contents.get(artifact_id).map(String::as_str)
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

    fn missing_text_artifact_ids(&self) -> Vec<String> {
        self.manifest
            .items
            .values()
            .filter(|item| {
                item.mime_type.starts_with("text/")
                    && !self.text_contents.contains_key(item.id.as_str())
            })
            .map(|item| item.id.clone())
            .collect()
    }
}
