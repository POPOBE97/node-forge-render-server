use anyhow::Result;
use node_forge_render_server::screen_test::{
    device::DeviceConnection, geometry, measurement, model::*, pattern::Pattern, raw::RawFrame,
};
use std::path::PathBuf;
fn mode(s: &str) -> Result<DisplayMode> {
    match s {
        "srgb" => Ok(DisplayMode::Srgb),
        "p3" => Ok(DisplayMode::DisplayP3),
        "hdr" | "hdr-linear" => Ok(DisplayMode::LinearHdr),
        _ => anyhow::bail!("Mode must be srgb, p3 or hdr-linear"),
    }
}
fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [command, capture] if command == "recalibrate-reference" => {
            let root = node_forge_render_server::screen_test::repository::default_root();
            let result =
                node_forge_render_server::screen_test::reprocess::reference(&root, capture)?;
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        [command] | [command, _] if command == "calibrate-sweep" => {
            let root = node_forge_render_server::screen_test::repository::default_root();
            let run = node_forge_render_server::screen_test::batch::run(
                &root,
                args.get(1).map(String::as_str).unwrap_or(""),
            )?;
            anyhow::ensure!(
                run.saved() == 33 && run.issue.is_none(),
                "Sweep incomplete: {}",
                run.issue
                    .as_deref()
                    .unwrap_or("some display conditions failed; see the saved run for details")
            );
        }
        [command, path] if command == "raw" => {
            let raw = RawFrame::decode(&PathBuf::from(path))?;
            println!("{}", serde_json::to_string_pretty(&raw.metadata)?);
        }
        [command, raw_path, out] if command == "detect" => {
            let raw = RawFrame::decode(&PathBuf::from(raw_path))?;
            let out = PathBuf::from(out);
            std::fs::create_dir_all(&out)?;
            raw.detection_image().save(out.join("detection.png"))?;
            let detected = geometry::detect(&raw, &out)?;
            std::fs::write(
                out.join("markers.json"),
                serde_json::to_vec_pretty(&detected)?,
            )?;
            std::fs::write(
                out.join("raw.json"),
                serde_json::to_vec_pretty(&raw.metadata)?,
            )?;
            println!(
                "{} markers detected; identity requires the original presentation manifest",
                detected.len()
            );
        }
        [command, width, height, m, out] if command == "generate" => {
            let size = [width.parse()?, height.parse()?];
            let device = DisplayDevice {
                id: "generated".into(),
                platform: Platform::Android,
                name: "Generated chart".into(),
                size,
                drawable: size,
                insets: [16, 32, 16, 96],
                modes: vec![mode(m)?],
                brightness: 0.5,
                headroom: 4.,
                active: false,
                generation: String::new(),
                brightness_guard: None,
            };
            Pattern::generate(&device, mode(m)?, 2.)?.save(&PathBuf::from(out))?;
        }
        [command, platform, id, m, brightness, peak, out] if command == "show" => {
            let platform = match platform.as_str() {
                "android" => Platform::Android,
                "ios" => Platform::Ios,
                _ => anyhow::bail!("Platform must be android or ios"),
            };
            let mut connection = DeviceConnection::connect(platform, id)?;
            let pattern = Pattern::generate(&connection.device, mode(m)?, peak.parse()?)?;
            pattern.save(&PathBuf::from(out))?;
            let status = connection.present(&pattern, brightness.parse()?)?;
            println!("{}", serde_json::to_string_pretty(&status)?);
            if platform == Platform::Ios {
                eprintln!(
                    "Brightness is held. Press Enter to release the display and restore brightness."
                );
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
            }
        }
        [command, manifest_path, raw_path, out] if command == "geometry" => {
            let manifest: PatternManifest = serde_json::from_slice(&std::fs::read(manifest_path)?)?;
            let raw = RawFrame::decode(&PathBuf::from(raw_path))?;
            let out = PathBuf::from(out);
            std::fs::create_dir_all(&out)?;
            let detected = geometry::detect(&raw, &out)?;
            let profile = geometry::fit(&raw, &manifest, &detected, "offline-explicit-geometry")?;
            std::fs::write(
                out.join("geometry.json"),
                serde_json::to_vec_pretty(&profile)?,
            )?;
            measurement::export(&raw, &profile, &manifest, None, &out)?;
            println!("Geometry holdout: {:.4} px", profile.holdout_rms);
        }
        [command, manifest_path, raw_path, geometry_path, out] if command == "measure" => {
            let manifest: PatternManifest = serde_json::from_slice(&std::fs::read(manifest_path)?)?;
            let raw = RawFrame::decode(&PathBuf::from(raw_path))?;
            let profile: GeometryProfile = serde_json::from_slice(&std::fs::read(geometry_path)?)?;
            let out = PathBuf::from(out);
            std::fs::create_dir_all(&out)?;
            let corrected = geometry::rectify_for_color(&raw, &profile, &manifest, &out)?;
            let measured = measurement::measure(&corrected)?;
            std::fs::write(
                out.join("measurements.json"),
                serde_json::to_vec_pretty(&measured)?,
            )?;
        }
        _ => {
            println!(
                "Screen Test\n  calibrate-sweep [IPHONE_USB_ID]  Physical camera capture: 3 modes × 11 brightness settings\nDiagnostics (no camera):\n  recalibrate-reference CAPTURE_ID  Refit a saved RAW and append calibration results\n  raw original.CR2\n  detect RAW OUT_DIR\n  generate WIDTH HEIGHT srgb|p3|hdr-linear OUT_DIR\n  show android|ios USB_ID srgb|p3|hdr-linear BRIGHTNESS_0_1 PEAK OUT_DIR\n  geometry MANIFEST RAW OUT_DIR\n  measure MANIFEST RAW GEOMETRY_JSON OUT_DIR"
            );
            anyhow::ensure!(args.is_empty() || args == ["--help"], "Invalid arguments");
        }
    }
    Ok(())
}
