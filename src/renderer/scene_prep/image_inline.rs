use std::collections::HashMap;

use anyhow::{Result, bail};

use crate::dsl::{Node, SceneDSL};

pub(crate) fn copy_image_file_params_into_image_texture(
    dst: &mut Node,
    asset_id: Option<serde_json::Value>,
    data_url: Option<serde_json::Value>,
    path: Option<serde_json::Value>,
) {
    // Prefer assetId (new protocol). Legacy: {dataUrl, path}.
    if let Some(v) = asset_id {
        if v.as_str().is_some_and(|s| !s.trim().is_empty()) {
            let already = dst
                .params
                .get("assetId")
                .and_then(|x| x.as_str())
                .is_some_and(|s| !s.trim().is_empty());
            if !already {
                dst.params.insert("assetId".to_string(), v);
            }
            return; // assetId is authoritative; skip legacy params.
        }
    }

    // Legacy fallbacks.
    if let Some(v) = data_url {
        if v.as_str().is_some_and(|s| !s.trim().is_empty()) {
            let already = dst
                .params
                .get("dataUrl")
                .and_then(|x| x.as_str())
                .is_some_and(|s| !s.trim().is_empty());
            if !already {
                dst.params.insert("dataUrl".to_string(), v);
            }
        }
    }
    if let Some(v) = path {
        if v.as_str().is_some_and(|s| !s.trim().is_empty()) {
            let already = dst
                .params
                .get("path")
                .and_then(|x| x.as_str())
                .is_some_and(|s| !s.trim().is_empty());
            if !already {
                dst.params.insert("path".to_string(), v);
            }
        }
    }
}

pub(crate) fn inline_image_file_connections_into_image_textures(
    scene: &mut SceneDSL,
) -> Result<usize> {
    // ImageTexture currently loads its image from params.{assetId,dataUrl,path} at runtime.
    // But the node scheme models image flow as a connection: ImageFile.image -> ImageTexture.image.
    // Inline that connection by copying the ImageFile params into the connected ImageTexture.
    //
    // This keeps authoring in the graph model while satisfying runtime expectations.
    let by_id: HashMap<String, Node> = scene
        .nodes
        .iter()
        .cloned()
        .map(|n| (n.id.clone(), n))
        .collect();

    // Collect destinations we need to patch without holding overlapping borrows.
    let mut patches: Vec<(
        String,
        Option<serde_json::Value>,
        Option<serde_json::Value>,
        Option<serde_json::Value>,
    )> = Vec::new();
    for c in &scene.connections {
        if c.to.port_id != "image" {
            continue;
        }
        let Some(dst) = by_id.get(&c.to.node_id) else {
            continue;
        };
        if dst.node_type != "ImageTexture" {
            continue;
        }

        let Some(src) = by_id.get(&c.from.node_id) else {
            bail!(
                "ImageTexture '{}' has image input from missing node '{}'",
                c.to.node_id,
                c.from.node_id
            );
        };
        if src.node_type != "ImageFile" {
            bail!(
                "ImageTexture '{}' image input must come from ImageFile, got {} (node {})",
                c.to.node_id,
                src.node_type,
                src.id
            );
        }

        let asset_id = src.params.get("assetId").cloned();
        let data_url = src.params.get("dataUrl").cloned();
        let path = src.params.get("path").cloned();
        patches.push((dst.id.clone(), asset_id, data_url, path));
    }

    // Apply patches to the real scene.
    for (dst_id, asset_id, data_url, path) in &patches {
        let Some(dst) = scene.nodes.iter_mut().find(|n| n.id == *dst_id) else {
            bail!(
                "missing ImageTexture node '{}' when inlining ImageFile",
                dst_id
            );
        };
        if dst.node_type != "ImageTexture" {
            bail!(
                "expected ImageTexture node '{}' when inlining ImageFile, got {}",
                dst_id,
                dst.node_type
            );
        }
        copy_image_file_params_into_image_texture(
            dst,
            asset_id.clone(),
            data_url.clone(),
            path.clone(),
        );
    }

    Ok(patches.len())
}
