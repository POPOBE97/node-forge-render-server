use std::{borrow::Cow, sync::mpsc, thread};

use crate::app::{canvas::state::CanvasAsyncOps, types::ViewportOperationIndicatorVisual};

pub const VIEWPORT_OPERATION_TIMEOUT_SECS: f64 = 5.0;
const ANDROID_SCREENCAP_TIMEOUT_SECS: f64 = 15.0;

pub enum ClipboardCopyState {
    Idle,
    Running {
        request_id: u64,
        started_at: f64,
        timeout_secs: f64,
        rx: mpsc::Receiver<(u64, bool)>,
    },
    Succeeded {
        hide_at: f64,
    },
    Failed {
        hide_at: f64,
    },
}

impl Default for ClipboardCopyState {
    fn default() -> Self {
        Self::Idle
    }
}

pub fn begin_clipboard_copy(
    async_ops: &mut CanvasAsyncOps,
    now: f64,
    width: usize,
    height: usize,
    bytes: Vec<u8>,
) {
    begin_async_clipboard_copy(async_ops, now, VIEWPORT_OPERATION_TIMEOUT_SECS, move || {
        arboard::Clipboard::new()
            .and_then(|mut clipboard| {
                clipboard.set_image(arboard::ImageData {
                    width,
                    height,
                    bytes: Cow::Owned(bytes),
                })
            })
            .is_ok()
    });
}

pub fn begin_android_screencap_clipboard_copy(async_ops: &mut CanvasAsyncOps, now: f64) {
    begin_async_clipboard_copy(async_ops, now, ANDROID_SCREENCAP_TIMEOUT_SECS, move || {
        match crate::android_reference::copy_screencap_png_to_clipboard() {
            Ok(result) => {
                eprintln!(
                    "[android-screencap] copied {}x{} PNG from {} to clipboard ({} bytes)",
                    result.width, result.height, result.serial, result.png_byte_len
                );
                true
            }
            Err(error) => {
                eprintln!("[android-screencap] failed: {error:#}");
                false
            }
        }
    });
}

fn begin_async_clipboard_copy(
    async_ops: &mut CanvasAsyncOps,
    now: f64,
    timeout_secs: f64,
    copy: impl FnOnce() -> bool + Send + 'static,
) {
    async_ops.next_request_id = async_ops.next_request_id.wrapping_add(1);
    let request_id = async_ops.next_request_id;
    let (tx, rx) = mpsc::channel::<(u64, bool)>();
    async_ops.clipboard_copy = ClipboardCopyState::Running {
        request_id,
        started_at: now,
        timeout_secs,
        rx,
    };
    async_ops.last_visual = Some(ViewportOperationIndicatorVisual::InProgress);

    thread::spawn(move || {
        let copied = copy();
        let _ = tx.send((request_id, copied));
    });
}

pub fn poll(async_ops: &mut CanvasAsyncOps, now: f64) {
    match &async_ops.clipboard_copy {
        ClipboardCopyState::Running {
            request_id,
            started_at,
            timeout_secs,
            rx,
        } => match rx.try_recv() {
            Ok((completed_request_id, success)) if completed_request_id == *request_id => {
                if success {
                    async_ops.clipboard_copy = ClipboardCopyState::Succeeded { hide_at: now + 1.0 };
                    async_ops.last_visual = Some(ViewportOperationIndicatorVisual::Success);
                } else {
                    async_ops.clipboard_copy = ClipboardCopyState::Failed { hide_at: now + 1.0 };
                    async_ops.last_visual = Some(ViewportOperationIndicatorVisual::Failure);
                }
            }
            Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                async_ops.clipboard_copy = ClipboardCopyState::Failed { hide_at: now + 1.0 };
                async_ops.last_visual = Some(ViewportOperationIndicatorVisual::Failure);
            }
            Err(mpsc::TryRecvError::Empty) if now - started_at >= *timeout_secs => {
                async_ops.clipboard_copy = ClipboardCopyState::Failed { hide_at: now + 1.0 };
                async_ops.last_visual = Some(ViewportOperationIndicatorVisual::Failure);
            }
            Err(mpsc::TryRecvError::Empty) => {}
        },
        ClipboardCopyState::Succeeded { hide_at } | ClipboardCopyState::Failed { hide_at }
            if now >= *hide_at =>
        {
            async_ops.clipboard_copy = ClipboardCopyState::Idle;
        }
        ClipboardCopyState::Idle
        | ClipboardCopyState::Succeeded { .. }
        | ClipboardCopyState::Failed { .. } => {}
    }
}

pub fn current_visual(async_ops: &CanvasAsyncOps) -> Option<ViewportOperationIndicatorVisual> {
    match async_ops.clipboard_copy {
        ClipboardCopyState::Running { .. } => Some(ViewportOperationIndicatorVisual::InProgress),
        ClipboardCopyState::Succeeded { .. } => Some(ViewportOperationIndicatorVisual::Success),
        ClipboardCopyState::Failed { .. } => Some(ViewportOperationIndicatorVisual::Failure),
        ClipboardCopyState::Idle => async_ops.last_visual,
    }
}

pub fn is_visible(async_ops: &CanvasAsyncOps) -> bool {
    !matches!(async_ops.clipboard_copy, ClipboardCopyState::Idle)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;

    use super::{
        ClipboardCopyState, VIEWPORT_OPERATION_TIMEOUT_SECS, current_visual, is_visible, poll,
    };
    use crate::app::{canvas::state::CanvasAsyncOps, types::ViewportOperationIndicatorVisual};

    #[test]
    fn poll_marks_success_for_matching_request_id() {
        let (tx, rx) = mpsc::channel();
        let mut async_ops = CanvasAsyncOps {
            clipboard_copy: ClipboardCopyState::Running {
                request_id: 7,
                started_at: 10.0,
                timeout_secs: VIEWPORT_OPERATION_TIMEOUT_SECS,
                rx,
            },
            last_visual: None,
            next_request_id: 7,
        };

        tx.send((7, true)).unwrap();
        poll(&mut async_ops, 10.5);

        assert!(matches!(
            async_ops.clipboard_copy,
            ClipboardCopyState::Succeeded { hide_at } if (hide_at - 11.5).abs() < f64::EPSILON
        ));
        assert!(matches!(
            current_visual(&async_ops),
            Some(ViewportOperationIndicatorVisual::Success)
        ));
        assert!(is_visible(&async_ops));
    }

    #[test]
    fn poll_marks_failure_for_stale_request_id() {
        let (tx, rx) = mpsc::channel();
        let mut async_ops = CanvasAsyncOps {
            clipboard_copy: ClipboardCopyState::Running {
                request_id: 7,
                started_at: 10.0,
                timeout_secs: VIEWPORT_OPERATION_TIMEOUT_SECS,
                rx,
            },
            last_visual: None,
            next_request_id: 7,
        };

        tx.send((6, true)).unwrap();
        poll(&mut async_ops, 10.5);

        assert!(matches!(
            async_ops.clipboard_copy,
            ClipboardCopyState::Failed { hide_at } if (hide_at - 11.5).abs() < f64::EPSILON
        ));
        assert!(matches!(
            current_visual(&async_ops),
            Some(ViewportOperationIndicatorVisual::Failure)
        ));
    }

    #[test]
    fn poll_times_out_running_clipboard_copy() {
        let (_tx, rx) = mpsc::channel();
        let mut async_ops = CanvasAsyncOps {
            clipboard_copy: ClipboardCopyState::Running {
                request_id: 3,
                started_at: 10.0,
                timeout_secs: VIEWPORT_OPERATION_TIMEOUT_SECS,
                rx,
            },
            last_visual: None,
            next_request_id: 3,
        };

        poll(&mut async_ops, 10.0 + VIEWPORT_OPERATION_TIMEOUT_SECS);

        assert!(matches!(
            async_ops.clipboard_copy,
            ClipboardCopyState::Failed { .. }
        ));
    }

    #[test]
    fn poll_hides_completed_indicator_after_deadline() {
        let mut async_ops = CanvasAsyncOps {
            clipboard_copy: ClipboardCopyState::Succeeded { hide_at: 3.0 },
            last_visual: Some(ViewportOperationIndicatorVisual::Success),
            next_request_id: 0,
        };

        poll(&mut async_ops, 3.0);

        assert!(matches!(async_ops.clipboard_copy, ClipboardCopyState::Idle));
        assert!(!is_visible(&async_ops));
        assert!(matches!(
            current_visual(&async_ops),
            Some(ViewportOperationIndicatorVisual::Success)
        ));
    }
}
