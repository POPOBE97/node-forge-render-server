use node_forge_render_server::camera_studio;

fn main() -> anyhow::Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!(
            "Camera Studio\n\nUsage: camera-studio [--review photo.jpg | --check-connection [--release-occupied]]\n\nIndependent camera window; also available in the Render Server sidebar.\nStarting the window does not access a camera.\n--ui-preview photo.jpg --state live|review|recording|stopping|downloading|error|disconnected|paused renders isolated UI fixtures without USB.\n--check-connection opens and closes a camera session without taking pictures.\n--release-occupied allows that diagnostic to stop competing camera clients.\nRequires libgphoto2 (macOS: brew install libgphoto2 gphoto2).\nNODE_FORGE_CAPTURE_DIR overrides the default Pictures/Node Forge Captures folder.\nNODE_FORGE_GPHOTO2_LIBRARY overrides the runtime library path."
        );
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--check-connection") {
        anyhow::ensure!(
            args.len() == 1 || (args.len() == 2 && args[1] == "--release-occupied"),
            "Usage: camera-studio --check-connection [--release-occupied]"
        );
        let info =
            camera_studio::check_connection(args.len() == 2, |message| eprintln!("{message}"))?;
        println!(
            "Connection verified: {} at {}; {} parameters. Session released.",
            info.model,
            info.port,
            info.parameters.len()
        );
        return Ok(());
    }
    if let [flag, path, state_flag, state] = args.as_slice() {
        if flag == "--ui-preview" && state_flag == "--state" {
            return camera_studio::run_ui_preview(path.into(), state.clone());
        }
    }
    let review = match args.as_slice() {
        [] => None,
        [flag, path] if flag == "--review" => Some(path.into()),
        _ => anyhow::bail!("Usage: camera-studio [--review photo.jpg]"),
    };
    camera_studio::run_standalone(review)
}
