use std::{fs, path::Path};

use crate::ui::pass_debug::artifacts::{
    ReferenceWorkspaceManifestFile, pass_reference_file_artifact_id,
};

const PASS_DEBUG_REFERENCE_MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;
const PASS_DEBUG_REFERENCE_MAX_FOLDER_FILES: usize = 512;

#[derive(Clone, Debug)]
pub(crate) struct ReferenceFileRead {
    pub(crate) relative_path: String,
    pub(crate) artifact_id: String,
    pub(crate) source: String,
    pub(crate) loaded_source: String,
}

pub(crate) fn read_manifest_reference_file(
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

pub(crate) fn write_reference_workspace_file(
    root: &Path,
    relative_path: &str,
    source: &str,
) -> Result<(), String> {
    let path = root.join(relative_path);
    fs::write(&path, source.as_bytes())
        .map_err(|error| format!("Failed to write {}: {error}", path.display()))
}

pub(crate) fn read_reference_shortwire_local_file(path: &Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map_err(|error| format!("Reference local restore unavailable: {error}"))
}

pub(crate) fn write_reference_shortwire_local_file(
    path: &Path,
    content: &str,
) -> Result<(), String> {
    fs::write(path, content).map_err(|error| format!("Reference file write failed: {error}"))
}

pub(crate) fn read_reference_file(
    path: &Path,
    root: &Path,
    pass_name: &str,
    mark_dirty: bool,
) -> Result<ReferenceFileRead, String> {
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
    Ok(ReferenceFileRead {
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

pub(crate) fn read_reference_folder(
    root: &Path,
    pass_name: &str,
    mark_dirty: bool,
) -> Result<(Vec<ReferenceFileRead>, usize), String> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "node-forge-pass-debug-file-io-{label}-{nonce}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn read_reference_file_returns_relative_path_and_dirty_state() {
        let root = temp_root("single");
        let nested = root.join("metal");
        fs::create_dir_all(&nested).unwrap();
        let path = nested.join("pass.metal");
        fs::write(&path, "fn ref() {}\n").unwrap();

        let file = read_reference_file(&path, &root, "pass", true).unwrap();

        assert_eq!(file.relative_path, "metal/pass.metal");
        assert_eq!(file.source, "fn ref() {}\n");
        assert_eq!(file.loaded_source, "");
        assert_eq!(
            file.artifact_id,
            pass_reference_file_artifact_id("pass", "metal/pass.metal")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn read_reference_folder_recurses_sorts_and_counts_skips() {
        let root = temp_root("folder");
        fs::create_dir_all(root.join("nested")).unwrap();
        fs::write(root.join("b.wgsl"), "b").unwrap();
        fs::write(root.join("nested/a.wgsl"), "a").unwrap();
        fs::write(root.join("bad.bin"), [0xff, 0xfe]).unwrap();

        let (files, skipped) = read_reference_folder(&root, "pass", false).unwrap();

        assert_eq!(skipped, 1);
        assert_eq!(
            files
                .iter()
                .map(|file| file.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["b.wgsl", "nested/a.wgsl"]
        );
        assert_eq!(files[0].loaded_source, files[0].source);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn write_and_shortwire_helpers_round_trip_text() {
        let root = temp_root("write");
        fs::create_dir_all(root.join("metal")).unwrap();

        write_reference_workspace_file(&root, "metal/pass.metal", "workspace").unwrap();
        let path = root.join("metal/pass.metal");
        assert_eq!(fs::read_to_string(&path).unwrap(), "workspace");

        write_reference_shortwire_local_file(&path, "shortwire").unwrap();
        assert_eq!(
            read_reference_shortwire_local_file(&path).unwrap(),
            "shortwire"
        );

        let manifest_file = ReferenceWorkspaceManifestFile {
            relative_path: "metal/pass.metal".to_string(),
            artifact_id: "artifact".to_string(),
            content_hash: "hash".to_string(),
            size: 9,
        };
        assert_eq!(
            read_manifest_reference_file(&root, &manifest_file).unwrap(),
            "shortwire"
        );

        let _ = fs::remove_dir_all(root);
    }
}
