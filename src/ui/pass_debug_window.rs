use std::collections::HashMap;

use rust_wgpu_fiber::eframe::egui;

use crate::renderer::PassDebugSource;
use crate::ui::pass_debug::registry::{
    PassDebugWindowRenderHooks, show_pass_debug_windows_with_render_hooks,
};
use crate::ui::pass_debug::render::root::{
    render_pass_debug_embedded_content, render_pass_debug_viewport,
};

pub use crate::ui::pass_debug::artifacts::ShortwireDiffResult;
pub use crate::ui::pass_debug::document::PassDebugWindowDocument;
pub(crate) use crate::ui::pass_debug::document::ShortwireDiffCaptureAttempt;
pub use crate::ui::pass_debug::event::{PassDebugPatchApplyResult, PassDebugWindowAction};
pub use crate::ui::pass_debug::registry::{
    PassDebugWindowMap, PassDebugWindowState, has_active_shortwire, mark_all_patches_reset,
    mark_patch_applied, mark_patch_reset, open_pass_debug_window, record_all_patch_error,
    record_patch_error, record_shortwire_diff_result, request_active_shortwire_diff_capture,
};
pub use crate::ui::pass_debug::shortwire::ShortwireDiffCaptureRequest;

pub fn show_pass_debug_windows(
    ctx: &egui::Context,
    windows: &mut PassDebugWindowMap,
    pass_sources: &HashMap<String, PassDebugSource>,
    pass_sources_revision: u64,
    pass_shader_overrides: &HashMap<String, String>,
    debug_artifacts: &crate::debug_artifacts::DebugArtifactStore,
) -> Vec<PassDebugWindowAction> {
    show_pass_debug_windows_with_render_hooks(
        ctx,
        windows,
        pass_sources,
        pass_sources_revision,
        pass_shader_overrides,
        debug_artifacts,
        PassDebugWindowRenderHooks {
            render_embedded_content: render_pass_debug_embedded_content,
            render_viewport: render_pass_debug_viewport,
        },
    )
}
