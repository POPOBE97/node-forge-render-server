use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{
    asset_store::{AssetData, AssetStore, LoadedNforge},
    debug_artifacts::DebugArtifactStore,
    dsl::{DebugArtifactItem, SceneDSL},
    renderer::node_compiler::template_loader,
};

const APPLICATION_ID: i64 = 1_313_232_455;
const FORMAT_VERSION: i64 = 5;
const SCENE_DSL_VERSION: &str = "5.0";
const SYNC_LOG_RETENTION: i64 = 10_000;
const CONTENT_HISTORY_RETENTION: i64 = 256;
const HISTORY_ENTITY_KIND: &str = "document_history";

fn sha256_hex(content: &[u8]) -> String {
    let digest = Sha256::digest(content);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn verify_blob(blob_hash: &str, byte_length: i64, content: &[u8], label: &str) -> Result<()> {
    if byte_length < 0 || content.len() != byte_length as usize {
        bail!("invalid blob byte length for {label} ('{blob_hash}')");
    }
    let actual = sha256_hex(content);
    if actual != blob_hash {
        bail!("blob hash mismatch for {label}: expected '{blob_hash}', got '{actual}'");
    }
    Ok(())
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn strip_graph_routing(value: &mut Value) {
    if let Some(object) = value.as_object_mut() {
        object.remove("routing");
    }
}

/// Gate routing is persisted editor layout metadata and is not part of the renderer DSL.
fn strip_editor_only_routing(scene: &mut Map<String, Value>) {
    scene.remove("routing");
    if let Some(groups) = scene.get_mut("groups").and_then(Value::as_array_mut) {
        groups.iter_mut().for_each(strip_graph_routing);
    }
    let Some(state_machine) = scene.get_mut("stateMachine").and_then(Value::as_object_mut) else {
        return;
    };
    if let Some(states) = state_machine
        .get_mut("states")
        .and_then(Value::as_array_mut)
    {
        for state in states {
            if let Some(graph) = state.get_mut("mutationGraph") {
                strip_graph_routing(graph);
            }
        }
    }
    if let Some(derivations) = state_machine
        .get_mut("derivations")
        .and_then(Value::as_array_mut)
    {
        derivations.iter_mut().for_each(strip_graph_routing);
    }
    if let Some(motion_graphs) = state_machine
        .get_mut("motionGraphs")
        .and_then(Value::as_array_mut)
    {
        motion_graphs.iter_mut().for_each(strip_graph_routing);
    }
}

fn configure_writable(connection: &Connection) -> Result<()> {
    connection
        .busy_timeout(Duration::from_secs(5))
        .context("failed to configure SQLite busy timeout")?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .context("failed to enable SQLite foreign keys")?;
    connection
        .pragma_update(None, "journal_mode", "WAL")
        .context("failed to enable SQLite WAL mode")?;
    connection
        .pragma_update(None, "synchronous", "FULL")
        .context("failed to configure SQLite durability")?;
    Ok(())
}

fn validate_header(path: &Path) -> Result<()> {
    let prefix = std::fs::read(path)
        .with_context(|| format!("failed to read .nforge at {}", path.display()))?;
    if prefix.starts_with(b"PK") {
        bail!("legacy ZIP .nforge unsupported; expected the SQLite .nforge format");
    }
    if !prefix.starts_with(b"SQLite format 3\0") {
        bail!("unsupported .nforge file; expected a SQLite document");
    }
    Ok(())
}

fn validate_format(connection: &Connection) -> Result<()> {
    let application_id: i64 = connection
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .context("failed to read .nforge application id")?;
    let format_version: i64 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .context("failed to read .nforge format version")?;
    if application_id != APPLICATION_ID || format_version != FORMAT_VERSION {
        bail!(
            "unsupported .nforge SQLite format (application_id={application_id}, version={format_version})"
        );
    }
    Ok(())
}

fn open_writable(path: &Path) -> Result<Connection> {
    validate_header(path)?;
    let connection = Connection::open(path)
        .with_context(|| format!("failed to open .nforge at {}", path.display()))?;
    configure_writable(&connection)?;
    validate_format(&connection)?;
    Ok(connection)
}

fn readonly_sqlite_uri(path: &Path) -> Result<String> {
    let absolute = path
        .canonicalize()
        .with_context(|| format!("failed to resolve .nforge at {}", path.display()))?;
    let text = absolute
        .to_str()
        .ok_or_else(|| anyhow::anyhow!(".nforge path is not valid UTF-8: {}", path.display()))?;
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~' | b':') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write;
            write!(&mut encoded, "%{byte:02X}").expect("writing to String cannot fail");
        }
    }
    if std::path::PathBuf::from(format!("{}-wal", path.display())).exists() {
        Ok(format!("file:{encoded}?mode=ro"))
    } else {
        Ok(format!("file:{encoded}?mode=ro&immutable=1"))
    }
}

fn open_readonly(path: &Path) -> Result<Connection> {
    validate_header(path)?;
    let uri = readonly_sqlite_uri(path)?;
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open .nforge read-only at {}", path.display()))?;
    connection
        .busy_timeout(Duration::from_secs(5))
        .context("failed to configure SQLite busy timeout")?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .context("failed to enable SQLite foreign keys")?;
    connection
        .pragma_update(None, "query_only", true)
        .context("failed to enforce read-only SQLite queries")?;
    validate_format(&connection)?;
    Ok(connection)
}

fn parse_json(text: String, label: &str) -> Result<Value> {
    serde_json::from_str(&text).with_context(|| format!("invalid {label} JSON in .nforge"))
}

fn validate_container(connection: &Connection) -> Result<()> {
    let quick_check: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .context("failed to run SQLite quick_check")?;
    if quick_check != "ok" {
        bail!("SQLite quick_check failed: {quick_check}");
    }
    let foreign_key_violations: i64 = connection
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
            row.get(0)
        })
        .context("failed to run SQLite foreign_key_check")?;
    if foreign_key_violations != 0 {
        bail!("SQLite foreign_key_check found {foreign_key_violations} violation(s)");
    }

    let mut statement = connection
        .prepare("SELECT blob_hash, byte_length, content FROM document_blobs ORDER BY blob_hash")?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, i64>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;
    for row in rows {
        let (blob_hash, byte_length, content) = row?;
        verify_blob(
            &blob_hash,
            byte_length,
            &content,
            &format!("document blob '{blob_hash}'"),
        )?;
    }
    let orphan: Option<String> = connection
        .query_row(
            "SELECT b.blob_hash
               FROM document_blobs b
              WHERE NOT EXISTS (SELECT 1 FROM assets a WHERE a.blob_hash = b.blob_hash)
                AND NOT EXISTS (
                  SELECT 1 FROM debug_artifacts d WHERE d.blob_hash = b.blob_hash
                )
                AND NOT EXISTS (
                  SELECT 1 FROM history_blob_refs h WHERE h.blob_hash = b.blob_hash
                )
              LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(orphan) = orphan {
        bail!("orphan document blob '{orphan}'");
    }

    let mut history = connection.prepare(
        "SELECT sequence, patch_json FROM change_log
          WHERE entity_kind = ? ORDER BY sequence",
    )?;
    let rows = history.query_map([HISTORY_ENTITY_KIND], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (sequence, patch) = row?;
        let entry: Value = serde_json::from_str(&patch)
            .with_context(|| format!("invalid history JSON at change {sequence}"))?;
        if entry.get("version").and_then(Value::as_i64) != Some(2) {
            bail!("unsupported history entry version at change {sequence}");
        }
        let record_kind = entry.get("recordKind").and_then(Value::as_str);
        let before_value = entry
            .get("before")
            .filter(|before| !before.is_null())
            .and_then(|before| before.get("value"));
        if matches!(record_kind, Some("asset" | "debug_artifact")) && before_value.is_some() {
            let before_value = before_value.expect("checked above");
            if before_value.get("content").is_some() {
                bail!("history change {sequence} contains inline binary content");
            }
            let blob_hash = before_value
                .get("blobHash")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("history change {sequence} is missing blobHash"))?;
            let byte_length = before_value
                .get("byteLength")
                .and_then(Value::as_i64)
                .ok_or_else(|| {
                    anyhow::anyhow!("history change {sequence} is missing byteLength")
                })?;
            let referenced_length: Option<i64> = connection
                .query_row(
                    "SELECT b.byte_length
                       FROM history_blob_refs h
                       JOIN document_blobs b ON b.blob_hash = h.blob_hash
                      WHERE h.change_sequence = ? AND h.slot = 'content'
                        AND h.blob_hash = ?",
                    params![sequence, blob_hash],
                    |row| row.get(0),
                )
                .optional()?;
            if referenced_length != Some(byte_length) {
                bail!("history change {sequence} has an invalid blob reference");
            }
        } else {
            let unexpected_refs: i64 = connection.query_row(
                "SELECT COUNT(*) FROM history_blob_refs WHERE change_sequence = ?",
                [sequence],
                |row| row.get(0),
            )?;
            if unexpected_refs != 0 {
                bail!("history change {sequence} has an unexpected blob reference");
            }
        }
    }
    Ok(())
}

fn json_rows(connection: &Connection, sql: &str, parameter: &str) -> Result<Vec<Value>> {
    let mut statement = connection.prepare(sql)?;
    let rows = statement.query_map([parameter], |row| row.get::<_, String>(0))?;
    rows.map(|row| parse_json(row?, "entity")).collect()
}

fn read_scene(connection: &Connection) -> Result<(SceneDSL, AssetStore, DebugArtifactStore)> {
    let scene_version: String = connection
        .query_row(
            "SELECT scene_version FROM document WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .context("invalid .nforge document row")?;
    if scene_version != SCENE_DSL_VERSION {
        bail!("unsupported SceneDSL version '{scene_version}' (expected {SCENE_DSL_VERSION})");
    }

    let mut sections = BTreeMap::<String, Value>::new();
    {
        let mut statement =
            connection.prepare("SELECT section_key, value_json FROM document_sections")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (key, text) = row?;
            sections.insert(key, parse_json(text, "document section")?);
        }
    }

    let mut root_nodes = Vec::new();
    let mut root_connections = Vec::new();
    let mut groups = Vec::new();
    let mut state_graphs = BTreeMap::<String, Value>::new();
    let mut derivations = BTreeMap::<String, Value>::new();
    {
        let mut statement = connection.prepare(
            "SELECT scope_id, scope_kind, owner_id, definition_json
               FROM graph_scopes
              ORDER BY CASE scope_kind WHEN 'root' THEN 0 WHEN 'group' THEN 1 ELSE 2 END,
                       order_index, scope_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;
        for row in rows {
            let (scope_id, kind, owner_id, definition_text) = row?;
            let nodes = json_rows(
                connection,
                "SELECT node_json FROM nodes
                  WHERE scope_id = ? ORDER BY order_index, node_id",
                &scope_id,
            )?;
            let connections = json_rows(
                connection,
                "SELECT connection_json FROM connections
                  WHERE scope_id = ? ORDER BY order_index, connection_id",
                &scope_id,
            )?;
            if kind == "root" {
                root_nodes = nodes;
                root_connections = connections;
                continue;
            }
            let mut definition = parse_json(definition_text, "scope definition")?;
            if let Some(object) = definition.as_object_mut() {
                object.insert("nodes".to_string(), Value::Array(nodes));
                object.insert("connections".to_string(), Value::Array(connections));
            }
            match (kind.as_str(), owner_id) {
                ("group", _) => groups.push(definition),
                ("state", Some(owner_id)) => {
                    state_graphs.insert(owner_id, definition);
                }
                ("derivation", Some(owner_id)) => {
                    derivations.insert(owner_id, definition);
                }
                (unsupported, _) => {
                    bail!("unsupported graph scope kind '{unsupported}' in v5 .nforge");
                }
            }
        }
    }

    let asset_store = AssetStore::new();
    let mut asset_manifest = Map::<String, Value>::new();
    {
        let mut statement = connection.prepare(
            "SELECT a.asset_id, a.metadata_json, a.blob_hash, b.byte_length, b.content
               FROM assets a JOIN document_blobs b ON b.blob_hash = a.blob_hash
              ORDER BY a.asset_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })?;
        for row in rows {
            let (asset_id, metadata_text, blob_hash, byte_length, bytes) = row?;
            verify_blob(
                &blob_hash,
                byte_length,
                &bytes,
                &format!("asset '{asset_id}'"),
            )?;
            let metadata = parse_json(metadata_text, "asset metadata")?;
            let mime_type = metadata
                .get("mimeType")
                .and_then(Value::as_str)
                .unwrap_or("application/octet-stream")
                .to_string();
            let original_name = metadata
                .get("originalName")
                .and_then(Value::as_str)
                .unwrap_or(asset_id.as_str())
                .to_string();
            asset_manifest.insert(asset_id.clone(), metadata);
            asset_store.insert(
                asset_id,
                AssetData {
                    bytes,
                    mime_type,
                    original_name,
                },
            );
        }
    }

    let mut materials = BTreeMap::<String, String>::new();
    {
        let mut statement =
            connection.prepare("SELECT node_id, content FROM materials ORDER BY node_id")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (node_id, content) = row?;
            materials.insert(node_id, content);
        }
    }
    template_loader::install_document_overrides(materials);

    let mut functions = Vec::new();
    {
        let mut statement =
            connection.prepare("SELECT resource_json FROM functions ORDER BY scope_id, node_id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        for row in rows {
            functions.push(
                serde_json::from_str::<crate::state_machine::graph_function::FunctionResource>(
                    &row?,
                )
                .context("invalid Graph Function resource in .nforge")?,
            );
        }
    }
    let mut debug_store = DebugArtifactStore::default();
    let mut debug_items = HashMap::<String, DebugArtifactItem>::new();
    let mut debug_contents = Vec::<(DebugArtifactItem, Vec<u8>)>::new();
    {
        let mut statement = connection.prepare(
            "SELECT d.artifact_id, d.item_json, d.blob_hash, b.byte_length, b.content
               FROM debug_artifacts d JOIN document_blobs b ON b.blob_hash = d.blob_hash
              ORDER BY d.artifact_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Vec<u8>>(4)?,
            ))
        })?;
        for row in rows {
            let (artifact_id, item_text, blob_hash, byte_length, content) = row?;
            verify_blob(
                &blob_hash,
                byte_length,
                &content,
                &format!("debug artifact '{artifact_id}'"),
            )?;
            let item: DebugArtifactItem = serde_json::from_str(&item_text)
                .context("invalid debug artifact JSON in .nforge")?;
            debug_items.insert(artifact_id, item.clone());
            debug_contents.push((item, content));
        }
    }

    let mut state_machine = sections
        .remove("stateMachine")
        .filter(|value| !value.is_null());
    if let Some(header) = state_machine.as_mut().and_then(Value::as_object_mut) {
        let order = header
            .remove("derivationOrder")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        let mut ordered = Vec::new();
        for id in order.iter().filter_map(Value::as_str) {
            if let Some(derivation) = derivations.remove(id) {
                ordered.push(derivation);
            }
        }
        ordered.extend(derivations.into_values());
        header.insert("derivations".to_string(), Value::Array(ordered));

        let states = header
            .get_mut("states")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| anyhow::anyhow!("stateMachine.states is missing in v5 .nforge"))?;
        for state in states {
            let Some(object) = state.as_object_mut() else {
                bail!("invalid State definition in v5 .nforge");
            };
            if object.get("type").and_then(Value::as_str) != Some("animationState") {
                continue;
            }
            let state_id = object
                .get("id")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("animationState id is missing"))?;
            let graph = state_graphs
                .remove(state_id)
                .ok_or_else(|| anyhow::anyhow!("missing state graph scope 'state:{state_id}'"))?;
            object.insert("mutationGraph".to_string(), graph);
        }
        if let Some(extra) = state_graphs.keys().next() {
            bail!("orphan state graph scope 'state:{extra}'");
        }
    }

    let mut scene = sections
        .remove("extras")
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    scene.insert("version".to_string(), Value::String(scene_version));
    scene.insert(
        "metadata".to_string(),
        sections
            .remove("metadata")
            .unwrap_or_else(|| json!({"name": "Untitled"})),
    );
    scene.insert("nodes".to_string(), Value::Array(root_nodes));
    scene.insert("connections".to_string(), Value::Array(root_connections));
    if !groups.is_empty() {
        scene.insert("groups".to_string(), Value::Array(groups));
    }
    if let Some(outputs) = sections.remove("outputs").filter(|value| !value.is_null()) {
        scene.insert("outputs".to_string(), outputs);
    }
    scene.insert("assets".to_string(), Value::Object(asset_manifest));
    if let Some(state_machine) = state_machine {
        scene.insert("stateMachine".to_string(), state_machine);
    }
    if !debug_items.is_empty() {
        scene.insert(
            "debugArtifacts".to_string(),
            serde_json::to_value(crate::dsl::DebugArtifacts {
                version: 1,
                items: debug_items,
            })?,
        );
    }
    if let Some(pass_sizes) = sections
        .remove("passTargetSizes")
        .filter(|value| !value.is_null())
    {
        scene.insert("passTargetSizes".to_string(), pass_sizes);
    }

    strip_editor_only_routing(&mut scene);
    let mut parsed: SceneDSL = serde_json::from_value(Value::Object(scene))
        .context("failed to parse SceneDSL from .nforge")?;
    crate::dsl::normalize_scene_defaults(&mut parsed)?;
    crate::state_machine::graph_function::validate_function_resources(&parsed, &functions)?;
    crate::state_machine::graph_function::install_document_functions(functions)?;
    debug_store.sync_manifest(parsed.debug_artifacts.clone());
    for (item, content) in debug_contents {
        if item.mime_type.starts_with("text/") {
            debug_store.upsert(item, Some(String::from_utf8_lossy(&content).into_owned()));
        } else {
            debug_store.upsert_bytes(item, content);
        }
    }
    Ok((parsed, asset_store, debug_store))
}

pub fn load(path: &Path) -> Result<LoadedNforge> {
    let connection = open_readonly(path)?;
    validate_container(&connection)?;
    let (scene, asset_store, debug_artifacts) = read_scene(&connection)?;
    Ok(LoadedNforge {
        scene,
        asset_store,
        debug_artifacts,
    })
}

pub fn save_debug_artifacts(path: &Path, debug_artifacts: &DebugArtifactStore) -> Result<()> {
    let mut connection = open_writable(path)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to begin .nforge debug artifact transaction")?;
    let current_revision: i64 = transaction
        .query_row(
            "SELECT revision FROM document WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .context("invalid .nforge document row")?;
    let mut existing = BTreeMap::<String, (Value, String, i64)>::new();
    {
        let mut statement = transaction.prepare(
            "SELECT d.artifact_id, d.item_json, d.blob_hash, b.byte_length
               FROM debug_artifacts d JOIN document_blobs b ON b.blob_hash = d.blob_hash",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
            ))
        })?;
        for row in rows {
            let (id, item_json, blob_hash, byte_length) = row?;
            existing.insert(
                id,
                (
                    parse_json(item_json, "debug artifact")?,
                    blob_hash,
                    byte_length,
                ),
            );
        }
    }

    let mut desired = BTreeMap::<String, (Value, Vec<u8>, String, i64)>::new();
    if let Some(manifest) = debug_artifacts.export_manifest() {
        for item in manifest.items.values() {
            let content = debug_artifacts.bytes(item.id.as_str()).unwrap_or_default();
            let blob_hash = sha256_hex(&content);
            let byte_length = i64::try_from(content.len())
                .map_err(|_| anyhow::anyhow!("debug artifact is too large"))?;
            desired.insert(
                item.id.clone(),
                (
                    serde_json::to_value(item)?,
                    content.to_vec(),
                    blob_hash,
                    byte_length,
                ),
            );
        }
    }

    let changed_ids = existing
        .keys()
        .chain(desired.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|id| {
            existing
                .get(id)
                .map(|(item, hash, length)| (item, hash, length))
                != desired
                    .get(id)
                    .map(|(item, _, hash, length)| (item, hash, length))
        })
        .collect::<Vec<_>>();
    if changed_ids.is_empty() {
        transaction.rollback()?;
        return Ok(());
    }

    let revision = current_revision + 1;
    let timestamp = now_millis().to_string();
    let transaction_id = format!("render-{}-{}", std::process::id(), now_millis());
    transaction.execute(
        "UPDATE document SET revision = ?, updated_at = ? WHERE singleton = 1",
        params![revision, timestamp],
    )?;

    for artifact_id in changed_ids {
        let before = existing.get(&artifact_id);
        let after = desired.get(&artifact_id);
        if let Some((item, content, blob_hash, byte_length)) = after {
            transaction.execute(
                "INSERT INTO document_blobs(blob_hash, byte_length, content)
                 VALUES (?, ?, ?)
                 ON CONFLICT(blob_hash) DO NOTHING",
                params![blob_hash, byte_length, content],
            )?;
            let (stored_length, stored_content): (i64, Vec<u8>) = transaction.query_row(
                "SELECT byte_length, content FROM document_blobs WHERE blob_hash = ?",
                [blob_hash],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            verify_blob(
                blob_hash,
                stored_length,
                &stored_content,
                "debug artifact write",
            )?;
            if stored_length != *byte_length {
                bail!("debug artifact blob '{blob_hash}' has inconsistent byte length");
            }
            transaction.execute(
                "INSERT INTO debug_artifacts(
                   artifact_id, item_json, blob_hash, updated_revision
                 ) VALUES (?, ?, ?, ?)
                 ON CONFLICT(artifact_id) DO UPDATE SET
                   item_json = excluded.item_json,
                   blob_hash = excluded.blob_hash,
                   updated_revision = excluded.updated_revision",
                params![artifact_id, item.to_string(), blob_hash, revision],
            )?;
        } else {
            transaction.execute(
                "DELETE FROM debug_artifacts WHERE artifact_id = ?",
                [&artifact_id],
            )?;
        }

        let descriptor = |value: Option<&(Value, String, i64)>| {
            value.map(|(item, blob_hash, byte_length)| {
                json!({"item": item, "blobHash": blob_hash, "byteLength": byte_length})
            })
        };
        let before_descriptor = descriptor(before);
        let after_descriptor = after.map(|(item, _, blob_hash, byte_length)| {
            json!({"item": item, "blobHash": blob_hash, "byteLength": byte_length})
        });
        let operation = match (before, after) {
            (None, Some(_)) => "add",
            (Some(_), None) => "delete",
            (Some(_), Some(_)) => "patch",
            (None, None) => unreachable!("changed artifact must exist before or after"),
        };
        transaction.execute(
            "INSERT INTO change_log(
               revision, transaction_id, actor_id, entity_kind, scope_id,
               entity_id, operation, patch_json, committed_at
             ) VALUES (?, ?, 'render-server', 'debug_artifact', NULL, ?, ?, ?, ?)",
            params![
                revision,
                transaction_id,
                artifact_id,
                operation,
                json!({"before": before_descriptor.clone(), "after": after_descriptor}).to_string(),
                timestamp
            ],
        )?;
        transaction.execute(
            "INSERT INTO change_log(
               revision, transaction_id, actor_id, entity_kind, scope_id,
               entity_id, operation, patch_json, committed_at
             ) VALUES (?, ?, 'render-server', ?, NULL, ?, 'before', ?, ?)",
            params![
                revision,
                transaction_id,
                HISTORY_ENTITY_KIND,
                artifact_id,
                json!({
                    "version": 2,
                    "recordKind": "debug_artifact",
                    "key": {"artifactId": artifact_id},
                    "before": before_descriptor.map(|value| json!({"value": value}))
                })
                .to_string(),
                timestamp
            ],
        )?;
        if let Some((_, blob_hash, _)) = before {
            transaction.execute(
                "INSERT INTO history_blob_refs(change_sequence, slot, blob_hash)
                 VALUES (?, 'content', ?)",
                params![transaction.last_insert_rowid(), blob_hash],
            )?;
        }
    }

    transaction.execute(
        "DELETE FROM change_log WHERE revision < ? AND entity_kind <> ?",
        params![
            std::cmp::max(1, revision - SYNC_LOG_RETENTION + 1),
            HISTORY_ENTITY_KIND
        ],
    )?;
    let history_floor = transaction
        .query_row(
            "SELECT revision FROM change_log
              WHERE entity_kind = ?
              GROUP BY revision ORDER BY revision DESC
              LIMIT 1 OFFSET ?",
            params![HISTORY_ENTITY_KIND, CONTENT_HISTORY_RETENTION - 1],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    if let Some(history_floor) = history_floor {
        transaction.execute(
            "DELETE FROM change_log WHERE entity_kind = ? AND revision < ?",
            params![HISTORY_ENTITY_KIND, history_floor],
        )?;
    }
    transaction.execute(
        "DELETE FROM document_blobs
          WHERE NOT EXISTS (SELECT 1 FROM assets a WHERE a.blob_hash = document_blobs.blob_hash)
            AND NOT EXISTS (
              SELECT 1 FROM debug_artifacts d WHERE d.blob_hash = document_blobs.blob_hash
            )
            AND NOT EXISTS (
              SELECT 1 FROM history_blob_refs h WHERE h.blob_hash = document_blobs.blob_hash
            )",
        [],
    )?;
    validate_container(&transaction)?;
    transaction
        .commit()
        .context("failed to commit debug artifacts")?;
    Ok(())
}

#[cfg(test)]
pub fn initialize_test_document(path: &Path, scene: &SceneDSL) -> Result<()> {
    let schema = include_str!("../../node-forge-editor/packages/document/sql/001_init.sql");
    let connection = Connection::open(path)?;
    configure_writable(&connection)?;
    connection.execute_batch(schema)?;
    let now = now_millis().to_string();
    connection.execute(
        "INSERT INTO document(
           singleton, document_id, format_version, scene_version,
           revision, created_at, updated_at
         ) VALUES (1, 'test-document', ?, ?, 1, ?, ?)",
        params![FORMAT_VERSION, scene.version, now, now],
    )?;
    let value = serde_json::to_value(scene)?;
    let object = value
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("test SceneDSL must serialize to an object"))?;
    connection.execute(
        "INSERT INTO document_sections(section_key, value_json, updated_revision)
         VALUES ('metadata', ?, 1)",
        [object
            .get("metadata")
            .cloned()
            .unwrap_or(Value::Null)
            .to_string()],
    )?;
    for (key, value) in [
        (
            "outputs",
            object.get("outputs").cloned().unwrap_or(Value::Null),
        ),
        ("stateMachine", Value::Null),
        (
            "passTargetSizes",
            object
                .get("passTargetSizes")
                .cloned()
                .unwrap_or(Value::Null),
        ),
        ("extras", json!({})),
    ] {
        connection.execute(
            "INSERT INTO document_sections(section_key, value_json, updated_revision)
             VALUES (?, ?, 1)",
            params![key, value.to_string()],
        )?;
    }
    let nodes = object
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let connections = object
        .get("connections")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (index, node) in nodes.iter().enumerate() {
        connection.execute(
            "INSERT INTO nodes(scope_id, node_id, order_index, node_json, updated_revision)
             VALUES ('root', ?, ?, ?, 1)",
            params![
                node.get("id").and_then(Value::as_str).unwrap_or_default(),
                index as i64,
                node.to_string()
            ],
        )?;
    }
    for (index, edge) in connections.iter().enumerate() {
        connection.execute(
            "INSERT INTO connections(
               scope_id, connection_id, order_index, connection_json, updated_revision
             ) VALUES ('root', ?, ?, ?, 1)",
            params![
                edge.get("id").and_then(Value::as_str).unwrap_or_default(),
                index as i64,
                edge.to_string()
            ],
        )?;
    }
    Ok(())
}
