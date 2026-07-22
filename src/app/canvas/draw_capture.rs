use rust_wgpu_fiber::shader_space::{
    PASS_CAPTURE_OUTPUT_TEXTURE_NAME, PassCaptureRequest, RenderProfile,
};

use crate::app::types::App;

/// Render the live scene and refresh the selected draw-call capture, when active.
pub(crate) fn render_profiled(app: &mut App, wait_for_gpu: bool) -> RenderProfile {
    let request = app
        .canvas
        .display
        .pass_capture
        .as_ref()
        .map(|capture| PassCaptureRequest::new(capture.pass_name.as_str(), capture.mode));

    let Some(request) = request else {
        return app.core.shader_space.render_profiled(wait_for_gpu);
    };

    match app.core.shader_space.prepare_pass_capture(&request) {
        Ok(_) => app
            .core
            .shader_space
            .render_profiled_with_pass_capture(wait_for_gpu, &request),
        Err(err) => {
            eprintln!("[draw-capture] disabled: {err}");
            app.canvas.display.pass_capture = None;
            if app
                .canvas
                .display
                .preview_texture_name
                .as_ref()
                .is_some_and(|name| name.as_str() == PASS_CAPTURE_OUTPUT_TEXTURE_NAME)
            {
                app.canvas.display.preview_texture_name = None;
            }
            app.core.shader_space.render_profiled(wait_for_gpu)
        }
    }
}
