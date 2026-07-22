use std::{
    collections::{BTreeMap, HashMap},
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, TransactionBehavior, params};
use serde_json::{Map, Value, json};

use crate::{
    asset_store::{AssetData, AssetStore, LoadedNforge},
    debug_artifacts::DebugArtifactStore,
    dsl::{DebugArtifactItem, SceneDSL},
    renderer::node_compiler::template_loader,
};

const APPLICATION_ID: i64 = 1_313_232_455;
const FORMAT_VERSION: i64 = 2;
const CHANGE_LOG_RETENTION: i64 = 10_000;

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn configure(connection: &Connection) -> Result<()> {
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

fn open(path: &Path) -> Result<Connection> {
    let prefix = std::fs::read(path)
        .with_context(|| format!("failed to read .nforge at {}", path.display()))?;
    if prefix.starts_with(b"PK") {
        bail!("legacy ZIP .nforge unsupported; expected the SQLite .nforge format");
    }
    if !prefix.starts_with(b"SQLite format 3\0") {
        bail!("unsupported .nforge file; expected a SQLite document");
    }
    let connection = Connection::open(path)
        .with_context(|| format!("failed to open .nforge at {}", path.display()))?;
    configure(&connection)?;
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
    Ok(connection)
}

fn parse_json(text: String, label: &str) -> Result<Value> {
    serde_json::from_str(&text).with_context(|| format!("invalid {label} JSON in .nforge"))
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
    let mut mutations = BTreeMap::<String, Value>::new();
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
            if kind == "group" {
                groups.push(definition);
            } else if let Some(owner_id) = owner_id {
                mutations.insert(owner_id, definition);
            }
        }
    }

    let asset_store = AssetStore::new();
    let mut asset_manifest = Map::<String, Value>::new();
    {
        let mut statement = connection
            .prepare("SELECT asset_id, metadata_json, content FROM assets ORDER BY asset_id")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        for row in rows {
            let (asset_id, metadata_text, bytes) = row?;
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
                serde_json::from_str::<crate::state_machine::mutation_function::FunctionResource>(
                    &row?,
                )
                .context("invalid Mutation Function resource in .nforge")?,
            );
        }
    }
    crate::state_machine::mutation_function::install_document_functions(functions)?;

    let mut debug_store = DebugArtifactStore::default();
    let mut debug_items = HashMap::<String, DebugArtifactItem>::new();
    let mut debug_contents = Vec::<(DebugArtifactItem, Vec<u8>)>::new();
    {
        let mut statement = connection.prepare(
            "SELECT artifact_id, item_json, content
               FROM debug_artifacts ORDER BY artifact_id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;
        for row in rows {
            let (artifact_id, item_text, content) = row?;
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
            .remove("mutationOrder")
            .and_then(|value| value.as_array().cloned())
            .unwrap_or_default();
        let mut ordered = Vec::new();
        for id in order.iter().filter_map(Value::as_str) {
            if let Some(mutation) = mutations.remove(id) {
                ordered.push(mutation);
            }
        }
        ordered.extend(mutations.into_values());
        header.insert("mutations".to_string(), Value::Array(ordered));
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

    let mut parsed: SceneDSL = serde_json::from_value(Value::Object(scene))
        .context("failed to parse SceneDSL from .nforge")?;
    crate::dsl::normalize_scene_defaults(&mut parsed)?;
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
    let connection = open(path)?;
    let (scene, asset_store, debug_artifacts) = read_scene(&connection)?;
    Ok(LoadedNforge {
        scene,
        asset_store,
        debug_artifacts,
    })
}

pub fn save_debug_artifacts(path: &Path, debug_artifacts: &DebugArtifactStore) -> Result<()> {
    let mut connection = open(path)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .context("failed to begin .nforge debug artifact transaction")?;
    let (document_id, current_revision): (String, i64) = transaction
        .query_row(
            "SELECT document_id, revision FROM document WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .context("invalid .nforge document row")?;
    let revision = current_revision + 1;
    let timestamp = now_millis().to_string();
    let transaction_id = format!("render-{}-{}", std::process::id(), now_millis());
    transaction.execute(
        "UPDATE document SET revision = ?, updated_at = ? WHERE singleton = 1",
        params![revision, timestamp],
    )?;
    transaction.execute("DELETE FROM debug_artifacts", [])?;
    if let Some(manifest) = debug_artifacts.export_manifest() {
        for item in manifest.items.values() {
            let content = debug_artifacts.bytes(item.id.as_str()).unwrap_or_default();
            transaction.execute(
                "INSERT INTO debug_artifacts(
                   artifact_id, item_json, content, updated_revision
                 ) VALUES (?, ?, ?, ?)",
                params![item.id, serde_json::to_string(item)?, content, revision],
            )?;
        }
    }
    transaction.execute(
        "INSERT INTO change_log(
           revision, transaction_id, actor_id, entity_kind, scope_id,
           entity_id, operation, patch_json, committed_at
         ) VALUES (?, ?, 'render-server', 'debug_artifacts', NULL, ?, 'replace', ?, ?)",
        params![
            revision,
            transaction_id,
            document_id,
            json!({"count": debug_artifacts.export_manifest().map_or(0, |m| m.items.len())})
                .to_string(),
            timestamp
        ],
    )?;
    transaction.execute(
        "DELETE FROM change_log WHERE revision < ?",
        [std::cmp::max(0, revision - CHANGE_LOG_RETENTION)],
    )?;
    transaction
        .commit()
        .context("failed to commit debug artifacts")?;
    Ok(())
}

#[cfg(test)]
pub fn initialize_test_document(path: &Path, scene: &SceneDSL) -> Result<()> {
    let schema = include_str!("../../node-forge-editor/packages/document/sql/001_init.sql");
    let connection = Connection::open(path)?;
    configure(&connection)?;
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
