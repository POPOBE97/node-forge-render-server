//! WebSocket/streaming integration goes here.
//!
//! Intentionally minimal placeholder for now. The goal is to keep a clean seam between
//! scene acquisition (local file, websocket stream, etc.) and the renderer/vm runtime.

use anyhow::Result;

use crate::dsl::SceneDSL;

pub trait SceneSource {
    fn next_scene(&mut self) -> Result<Option<SceneDSL>>;
}
