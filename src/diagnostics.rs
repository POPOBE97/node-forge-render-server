//! Low-frequency desktop lifecycle diagnostics, retained across process exits.
use std::{fs::OpenOptions, io::Write, path::PathBuf, sync::OnceLock};

pub fn log_path() -> &'static PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        if let Some(path) = std::env::var_os("NODE_FORGE_RENDER_SERVER_LOG_PATH") {
            return PathBuf::from(path);
        }
        let base = if cfg!(target_os = "macos") {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Logs/NodeForge"))
        } else {
            std::env::var_os("LOCALAPPDATA")
                .or_else(|| std::env::var_os("XDG_STATE_HOME"))
                .map(|base| PathBuf::from(base).join("NodeForge/logs"))
        }
        .unwrap_or_else(|| std::env::temp_dir().join("node-forge/logs"));
        base.join(format!(
            "render-server-{}-{}.log",
            crate::protocol::now_millis(),
            std::process::id()
        ))
    })
}

pub fn record(stage: &str, message: &str) {
    let line = format!("{} [{stage}] {message}", crate::protocol::now_millis());
    eprintln!("{line}");
    let path = log_path();
    let result = (|| -> std::io::Result<()> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = OpenOptions::new().create(true).append(true).open(path)?;
        writeln!(file, "{line}")?;
        file.flush()
    })();
    if let Err(error) = result {
        eprintln!("[diagnostics] could not write {}: {error}", path.display());
    }
}
