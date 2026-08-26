use anyhow::{Result, anyhow};

/// Browser-native JavaScript backend for DataParse.
///
/// Archives are trusted input. The host page must permit `unsafe-eval` in its CSP.
pub struct TsRuntime;

impl TsRuntime {
    pub fn new() -> Self {
        Self
    }

    pub fn eval_script<T: serde::de::DeserializeOwned>(&mut self, src: &str) -> Result<T> {
        let value = js_sys::eval(src)
            .map_err(|error| anyhow!("DataParse browser execution failed: {error:?}"))?;
        serde_wasm_bindgen::from_value(value)
            .map_err(|error| anyhow!("DataParse result decode failed: {error}"))
    }
}
