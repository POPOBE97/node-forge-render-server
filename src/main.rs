mod command;

fn main() -> anyhow::Result<()> {
    if std::env::var_os("NODE_FORGE_EDITOR_STDIN").is_some() {
        std::thread::Builder::new()
            .name("editor-lifetime".into())
            .spawn(|| {
                // The editor owns the pipe writer; EOF survives even a forced dev restart.
                if let Err(error) = std::io::copy(&mut std::io::stdin().lock(), &mut std::io::sink()) {
                    eprintln!("[editor-lifetime] stdin closed with error: {error}");
                }
                std::process::exit(0);
            })?;
    }
    command::run()
}
