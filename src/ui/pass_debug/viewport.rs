use rust_wgpu_fiber::eframe::egui;

pub(crate) const PASS_DEBUG_CLOSE_RESIZE_DELTA_THRESHOLD: f32 = 48.0;
const PASS_DEBUG_WINDOW_DEFAULT_WIDTH: f32 = 1480.0;
const PASS_DEBUG_WINDOW_DEFAULT_HEIGHT: f32 = 760.0;
const PASS_DEBUG_WINDOW_MIN_WIDTH: f32 = 960.0;
const PASS_DEBUG_WINDOW_MIN_HEIGHT: f32 = 360.0;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PassDebugViewportSnapshot {
    pub(crate) inner_rect: Option<egui::Rect>,
    pub(crate) outer_rect: Option<egui::Rect>,
    pub(crate) monitor_size: Option<egui::Vec2>,
    pub(crate) native_pixels_per_point: Option<f32>,
    pub(crate) focused: Option<bool>,
    pub(crate) visible: Option<bool>,
}

impl PassDebugViewportSnapshot {
    pub(crate) fn from_info(info: &egui::ViewportInfo) -> Self {
        Self {
            inner_rect: info.inner_rect,
            outer_rect: info.outer_rect,
            monitor_size: info.monitor_size,
            native_pixels_per_point: info.native_pixels_per_point,
            focused: info.focused,
            visible: info.visible(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassDebugCloseDecision {
    Accept,
    Cancel(PassDebugCloseCancelReason),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassDebugCloseCancelReason {
    FocusLost,
    Hidden,
    MonitorChanged,
    ScaleChanged,
    ViewportJumped,
}

pub(crate) fn pass_debug_default_window_size() -> egui::Vec2 {
    egui::vec2(
        PASS_DEBUG_WINDOW_DEFAULT_WIDTH,
        PASS_DEBUG_WINDOW_DEFAULT_HEIGHT,
    )
}

fn pass_debug_min_window_size() -> egui::Vec2 {
    egui::vec2(PASS_DEBUG_WINDOW_MIN_WIDTH, PASS_DEBUG_WINDOW_MIN_HEIGHT)
}

pub(crate) fn pass_debug_viewport_builder(
    title: String,
    include_initial_size: bool,
) -> egui::ViewportBuilder {
    let builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_min_inner_size(pass_debug_min_window_size());

    if include_initial_size {
        builder.with_inner_size(pass_debug_default_window_size())
    } else {
        builder
    }
}

#[cfg(test)]
pub(crate) fn is_close_request_during_large_viewport_resize(
    previous: Option<egui::Rect>,
    current: Option<egui::Rect>,
) -> bool {
    let (Some(previous), Some(current)) = (previous, current) else {
        return false;
    };
    let width_delta = (previous.width() - current.width()).abs();
    let height_delta = (previous.height() - current.height()).abs();
    width_delta.max(height_delta) >= PASS_DEBUG_CLOSE_RESIZE_DELTA_THRESHOLD
}

pub(crate) fn classify_pass_debug_close_request(
    previous: Option<PassDebugViewportSnapshot>,
    current: PassDebugViewportSnapshot,
) -> PassDebugCloseDecision {
    if current.focused == Some(false) {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::FocusLost);
    }

    if current.visible == Some(false) {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::Hidden);
    }

    let Some(previous) = previous else {
        return PassDebugCloseDecision::Accept;
    };

    if previous.monitor_size != current.monitor_size {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::MonitorChanged);
    }

    if viewport_scale_changed(
        previous.native_pixels_per_point,
        current.native_pixels_per_point,
    ) {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::ScaleChanged);
    }

    if viewport_rect_jumped(previous.inner_rect, current.inner_rect)
        || viewport_rect_jumped(previous.outer_rect, current.outer_rect)
    {
        return PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::ViewportJumped);
    }

    PassDebugCloseDecision::Accept
}

fn viewport_scale_changed(previous: Option<f32>, current: Option<f32>) -> bool {
    match (previous, current) {
        (Some(previous), Some(current)) => {
            (previous - current).abs() >= f32::EPSILON && current.is_finite()
        }
        _ => false,
    }
}

fn viewport_rect_jumped(previous: Option<egui::Rect>, current: Option<egui::Rect>) -> bool {
    let (Some(previous), Some(current)) = (previous, current) else {
        return false;
    };
    let position_delta = previous.min.distance(current.min);
    let size_delta = (previous.width() - current.width())
        .abs()
        .max((previous.height() - current.height()).abs());
    position_delta.max(size_delta) >= PASS_DEBUG_CLOSE_RESIZE_DELTA_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    fn viewport_snapshot(
        inner_rect: egui::Rect,
        outer_rect: egui::Rect,
    ) -> PassDebugViewportSnapshot {
        PassDebugViewportSnapshot {
            inner_rect: Some(inner_rect),
            outer_rect: Some(outer_rect),
            monitor_size: Some(egui::vec2(1440.0, 900.0)),
            native_pixels_per_point: Some(2.0),
            focused: Some(true),
            visible: Some(true),
        }
    }

    #[test]
    fn debug_viewport_builder_only_sets_default_size_initially() {
        let first = pass_debug_viewport_builder("Debug".to_string(), true);
        assert_eq!(
            first.inner_size,
            Some(egui::vec2(
                PASS_DEBUG_WINDOW_DEFAULT_WIDTH,
                PASS_DEBUG_WINDOW_DEFAULT_HEIGHT
            ))
        );
        assert_eq!(
            first.min_inner_size,
            Some(egui::vec2(
                PASS_DEBUG_WINDOW_MIN_WIDTH,
                PASS_DEBUG_WINDOW_MIN_HEIGHT
            ))
        );

        let subsequent = pass_debug_viewport_builder("Debug".to_string(), false);
        assert_eq!(subsequent.inner_size, None);
        assert_eq!(subsequent.title.as_deref(), Some("Debug"));
        assert_eq!(subsequent.min_inner_size, first.min_inner_size);
    }

    #[test]
    fn close_request_resize_guard_only_matches_large_size_changes() {
        let previous = egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(800.0, 600.0));
        let maximized = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1440.0, 900.0));
        let nearly_same =
            egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(804.0, 604.0));

        assert!(is_close_request_during_large_viewport_resize(
            Some(previous),
            Some(maximized),
        ));
        assert!(!is_close_request_during_large_viewport_resize(
            Some(previous),
            Some(nearly_same),
        ));
        assert!(!is_close_request_during_large_viewport_resize(
            None,
            Some(maximized),
        ));
    }

    #[test]
    fn stable_focused_close_request_is_accepted() {
        let rect = egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(800.0, 600.0));
        let snapshot = viewport_snapshot(rect, rect);

        assert_eq!(
            classify_pass_debug_close_request(Some(snapshot), snapshot),
            PassDebugCloseDecision::Accept
        );
    }

    #[test]
    fn transient_close_requests_are_canceled_during_focus_or_display_changes() {
        let previous_inner =
            egui::Rect::from_min_size(egui::pos2(20.0, 20.0), egui::vec2(800.0, 600.0));
        let previous_outer =
            egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(820.0, 640.0));
        let previous = viewport_snapshot(previous_inner, previous_outer);

        let mut focus_lost = previous;
        focus_lost.focused = Some(false);
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), focus_lost),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::FocusLost)
        );

        let mut hidden = previous;
        hidden.visible = Some(false);
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), hidden),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::Hidden)
        );

        let mut monitor_changed = previous;
        monitor_changed.monitor_size = Some(egui::vec2(2560.0, 1440.0));
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), monitor_changed),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::MonitorChanged)
        );

        let mut scale_changed = previous;
        scale_changed.native_pixels_per_point = Some(1.0);
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), scale_changed),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::ScaleChanged)
        );

        let mut jumped = previous;
        jumped.outer_rect = Some(egui::Rect::from_min_size(
            egui::pos2(1200.0, 10.0),
            egui::vec2(820.0, 640.0),
        ));
        assert_eq!(
            classify_pass_debug_close_request(Some(previous), jumped),
            PassDebugCloseDecision::Cancel(PassDebugCloseCancelReason::ViewportJumped)
        );
    }
}
