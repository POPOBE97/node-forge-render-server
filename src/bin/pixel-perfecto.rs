fn main() -> anyhow::Result<()> {
    let paths: Vec<_> = std::env::args_os()
        .skip(1)
        .map(std::path::PathBuf::from)
        .collect();
    if paths
        .iter()
        .any(|p| p == std::path::Path::new("--help") || p == std::path::Path::new("-h"))
    {
        println!(
            "Pixel Perfecto\n\nUsage: pixel-perfecto [project.pixelperfecto | image.png image.jpg ...]\n\nMiddle drag: pan view. Left drag: move image by RGB grid cells. Wheel: zoom.\nDrag from top/left ruler: create guide. Alt-drag: duplicate guide. Delete: remove guide.\nShift+R: toggle rulers. Calibrate: drag grid points on the accumulated corrected image.\nM: measure. Click A then B or drag; drag endpoints to adjust. Shift: horizontal/vertical.\nMeasure independently counts crossed image/grid subpixels and shows their difference.\nCmd/Ctrl+O: open project. Cmd/Ctrl+I: import images. Cmd/Ctrl+S: save.\nCmd/Ctrl+Z / Shift+Z: undo / redo. Tab: switch A/B. Esc: cancel drag."
        );
        return Ok(());
    }
    node_forge_render_server::pixel_perfecto::run_standalone(paths)
}
