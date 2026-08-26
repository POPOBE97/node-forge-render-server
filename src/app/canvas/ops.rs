use std::{
    borrow::Cow,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
};

use crate::android_screencap::{
    PublicEncoding, ScreencapReady, capture_screencap_with_cancel, write_system_clipboard,
};
use crate::app::{canvas::state::CanvasAsyncOps, types::ViewportOperationIndicatorVisual};

pub const VIEWPORT_OPERATION_TIMEOUT_SECS: f64 = 5.0;
pub const ANDROID_SCREENCAP_TIMEOUT_SECS: f64 = 45.0;

pub enum ClipboardWorkerMessage {
    Simple(bool),
    AndroidScreencap(Result<ScreencapReady, String>),
}

pub enum ClipboardCopyState {
    Idle,
    Running {
        request_id: u64,
        started_at: f64,
        timeout_secs: f64,
        rx: mpsc::Receiver<(u64, ClipboardWorkerMessage)>,
        cancel: Arc<AtomicBool>,
    },
    Cancelling {
        request_id: u64,
        rx: mpsc::Receiver<(u64, ClipboardWorkerMessage)>,
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
    begin_worker(async_ops, now, VIEWPORT_OPERATION_TIMEOUT_SECS, move |_| {
        ClipboardWorkerMessage::Simple(
            arboard::Clipboard::new()
                .and_then(|mut clipboard| {
                    clipboard.set_image(arboard::ImageData {
                        width,
                        height,
                        bytes: Cow::Owned(bytes),
                    })
                })
                .is_ok(),
        )
    });
}

pub fn begin_android_screencap_clipboard_copy(async_ops: &mut CanvasAsyncOps, now: f64) {
    begin_android_screencap(async_ops, now, PublicEncoding::Source);
}

pub fn begin_android_screencap_srgb_clipboard_copy(async_ops: &mut CanvasAsyncOps, now: f64) {
    begin_android_screencap(async_ops, now, PublicEncoding::FigmaSrgb);
}

fn begin_android_screencap(async_ops: &mut CanvasAsyncOps, now: f64, encoding: PublicEncoding) {
    if clipboard_worker_in_flight(&async_ops.clipboard_copy) {
        return;
    }
    begin_worker(
        async_ops,
        now,
        ANDROID_SCREENCAP_TIMEOUT_SECS,
        move |cancel| match capture_screencap_with_cancel(encoding, cancel) {
            Ok(ready) => ClipboardWorkerMessage::AndroidScreencap(Ok(ready)),
            Err(error) => {
                eprintln!("[android-screencap] failed: {error:#}");
                ClipboardWorkerMessage::AndroidScreencap(Err(format!("{error:#}")))
            }
        },
    );
}

fn begin_worker(
    async_ops: &mut CanvasAsyncOps,
    now: f64,
    timeout_secs: f64,
    work: impl FnOnce(Arc<AtomicBool>) -> ClipboardWorkerMessage + Send + 'static,
) {
    if clipboard_worker_in_flight(&async_ops.clipboard_copy) {
        return;
    }
    async_ops.next_request_id = async_ops.next_request_id.wrapping_add(1);
    let request_id = async_ops.next_request_id;
    let (tx, rx) = mpsc::channel::<(u64, ClipboardWorkerMessage)>();
    let cancel = Arc::new(AtomicBool::new(false));
    async_ops.clipboard_copy = ClipboardCopyState::Running {
        request_id,
        started_at: now,
        timeout_secs,
        rx,
        cancel: Arc::clone(&cancel),
    };
    async_ops.last_visual = Some(ViewportOperationIndicatorVisual::InProgress);

    thread::spawn(move || {
        let message = work(cancel);
        let _ = tx.send((request_id, message));
    });
}

fn clipboard_worker_in_flight(state: &ClipboardCopyState) -> bool {
    matches!(
        state,
        ClipboardCopyState::Running { .. } | ClipboardCopyState::Cancelling { .. }
    )
}

pub fn poll(async_ops: &mut CanvasAsyncOps, now: f64) {
    let state = std::mem::replace(&mut async_ops.clipboard_copy, ClipboardCopyState::Idle);
    async_ops.clipboard_copy = match state {
        ClipboardCopyState::Running {
            request_id,
            started_at,
            timeout_secs,
            rx,
            cancel,
        } => match rx.try_recv() {
            Ok((completed_request_id, message)) if completed_request_id == request_id => {
                if now - started_at >= timeout_secs {
                    async_ops.last_visual = Some(ViewportOperationIndicatorVisual::Failure);
                    ClipboardCopyState::Failed { hide_at: now + 1.0 }
                } else {
                    complete_worker_message(async_ops, now, message)
                }
            }
            Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                async_ops.last_visual = Some(ViewportOperationIndicatorVisual::Failure);
                ClipboardCopyState::Failed { hide_at: now + 1.0 }
            }
            Err(mpsc::TryRecvError::Empty) if now - started_at >= timeout_secs => {
                cancel.store(true, Ordering::Release);
                async_ops.last_visual = Some(ViewportOperationIndicatorVisual::Failure);
                ClipboardCopyState::Cancelling { request_id, rx }
            }
            Err(mpsc::TryRecvError::Empty) => ClipboardCopyState::Running {
                request_id,
                started_at,
                timeout_secs,
                rx,
                cancel,
            },
        },
        ClipboardCopyState::Cancelling { request_id, rx } => match rx.try_recv() {
            Ok((completed_request_id, _)) if completed_request_id == request_id => {
                ClipboardCopyState::Failed { hide_at: now + 1.0 }
            }
            Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                ClipboardCopyState::Failed { hide_at: now + 1.0 }
            }
            Err(mpsc::TryRecvError::Empty) => ClipboardCopyState::Cancelling { request_id, rx },
        },
        ClipboardCopyState::Succeeded { hide_at } if now >= hide_at => ClipboardCopyState::Idle,
        ClipboardCopyState::Failed { hide_at } if now >= hide_at => ClipboardCopyState::Idle,
        other => other,
    };
}

fn complete_worker_message(
    async_ops: &mut CanvasAsyncOps,
    now: f64,
    message: ClipboardWorkerMessage,
) -> ClipboardCopyState {
    let success = match message {
        ClipboardWorkerMessage::Simple(success) => success,
        ClipboardWorkerMessage::AndroidScreencap(Ok(ready)) => {
            match write_system_clipboard(&ready.private_envelope, &ready.public_png) {
                Ok(()) => {
                    eprintln!("[android-screencap] {}", ready.log_line());
                    true
                }
                Err(error) => {
                    eprintln!("[android-screencap] pasteboard write failed: {error:#}");
                    false
                }
            }
        }
        ClipboardWorkerMessage::AndroidScreencap(Err(error)) => {
            eprintln!("[android-screencap] failed: {error}");
            false
        }
    };
    if success {
        async_ops.last_visual = Some(ViewportOperationIndicatorVisual::Success);
        ClipboardCopyState::Succeeded { hide_at: now + 1.0 }
    } else {
        async_ops.last_visual = Some(ViewportOperationIndicatorVisual::Failure);
        ClipboardCopyState::Failed { hide_at: now + 1.0 }
    }
}

pub fn current_visual(async_ops: &CanvasAsyncOps) -> Option<ViewportOperationIndicatorVisual> {
    match async_ops.clipboard_copy {
        ClipboardCopyState::Running { .. } => Some(ViewportOperationIndicatorVisual::InProgress),
        ClipboardCopyState::Cancelling { .. } => Some(ViewportOperationIndicatorVisual::Failure),
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
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };

    fn cancellation() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    use super::{
        ClipboardCopyState, ClipboardWorkerMessage, VIEWPORT_OPERATION_TIMEOUT_SECS,
        current_visual, is_visible, poll,
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
                cancel: cancellation(),
            },
            last_visual: None,
            next_request_id: 7,
        };

        tx.send((7, ClipboardWorkerMessage::Simple(true))).unwrap();
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
                cancel: cancellation(),
            },
            last_visual: None,
            next_request_id: 7,
        };

        tx.send((6, ClipboardWorkerMessage::Simple(true))).unwrap();
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
        let (tx, rx) = mpsc::channel();
        let cancel = cancellation();
        let mut async_ops = CanvasAsyncOps {
            clipboard_copy: ClipboardCopyState::Running {
                request_id: 3,
                started_at: 10.0,
                timeout_secs: VIEWPORT_OPERATION_TIMEOUT_SECS,
                rx,
                cancel: Arc::clone(&cancel),
            },
            last_visual: None,
            next_request_id: 3,
        };

        poll(&mut async_ops, 10.0 + VIEWPORT_OPERATION_TIMEOUT_SECS);

        assert!(matches!(
            async_ops.clipboard_copy,
            ClipboardCopyState::Cancelling { request_id: 3, .. }
        ));
        assert!(cancel.load(Ordering::Acquire));

        super::begin_android_screencap_clipboard_copy(&mut async_ops, 16.0);
        assert!(matches!(
            async_ops.clipboard_copy,
            ClipboardCopyState::Cancelling { request_id: 3, .. }
        ));

        drop(tx);
        poll(&mut async_ops, 16.1);
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

    #[test]
    fn in_flight_android_screencap_ignores_a_second_shortcut() {
        let (_tx, rx) = mpsc::channel();
        let mut async_ops = CanvasAsyncOps {
            clipboard_copy: ClipboardCopyState::Running {
                request_id: 9,
                started_at: 1.0,
                timeout_secs: super::ANDROID_SCREENCAP_TIMEOUT_SECS,
                rx,
                cancel: cancellation(),
            },
            last_visual: Some(ViewportOperationIndicatorVisual::InProgress),
            next_request_id: 9,
        };
        super::begin_android_screencap_clipboard_copy(&mut async_ops, 1.1);
        assert!(matches!(
            async_ops.clipboard_copy,
            ClipboardCopyState::Running { request_id: 9, .. }
        ));
    }

    #[test]
    fn expired_android_result_does_not_count_as_success() {
        let (tx, rx) = mpsc::channel();
        let mut async_ops = CanvasAsyncOps {
            clipboard_copy: ClipboardCopyState::Running {
                request_id: 4,
                started_at: 0.0,
                timeout_secs: super::ANDROID_SCREENCAP_TIMEOUT_SECS,
                rx,
                cancel: cancellation(),
            },
            last_visual: None,
            next_request_id: 4,
        };
        tx.send((
            4,
            ClipboardWorkerMessage::AndroidScreencap(Err("late".into())),
        ))
        .unwrap();
        poll(&mut async_ops, super::ANDROID_SCREENCAP_TIMEOUT_SECS);
        assert!(matches!(
            async_ops.clipboard_copy,
            ClipboardCopyState::Failed { .. }
        ));
    }
}
