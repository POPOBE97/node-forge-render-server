use anyhow::{anyhow, Result};
use deno_core::ascii_str;
use deno_core::{JsRuntime, RuntimeOptions};

pub struct TsRuntime {
    rt: JsRuntime,
}

impl TsRuntime {
    pub fn new() -> Self {
        let rt = JsRuntime::new(RuntimeOptions::default());
        Self { rt }
    }

    pub fn eval_script<T: serde::de::DeserializeOwned>(&mut self, src: &str) -> Result<T> {
        let value = self
            .rt
            .execute_script(ascii_str!("<dataparse>"), src.to_string())
            .map_err(|e| anyhow!("ts runtime execute_script failed: {e:?}\n--- source ---\n{src}\n--- end source ---"))?;

        deno_core::scope!(scope, self.rt);
        let local = deno_core::v8::Local::new(scope, value);
        deno_core::serde_v8::from_v8(scope, local)
            .map_err(|e| anyhow!("ts runtime deserialize failed: {e:?}"))
    }
}
