use anyhow::Result;
use crate::portable_js::PortableJs;

pub struct TsRuntime {
    runtime: Result<PortableJs>,
}

impl TsRuntime {
    pub fn new() -> Self { Self { runtime: PortableJs::new() } }

    pub fn eval_script<T: serde::de::DeserializeOwned>(&mut self, source: &str) -> Result<T> {
        self.runtime.as_ref().map_err(|error| anyhow::anyhow!("{error:#}"))?.eval_json(source)
    }
}
