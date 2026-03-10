//! Timeline recorder for state-machine animation frames.
//!
//! Captures a rolling window of per-frame state-machine snapshots during
//! playback. The buffer is trimmed by wall-clock duration (not tick count)
//! so that variable frame rates produce a consistent 10-second window.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::Instant;

use crate::state_machine::OverrideKey;

/// A single recorded frame of state-machine data.
#[derive(Debug, Clone)]
pub struct TimelineFrame {
    /// Wall-clock seconds since the recording started (latest Play).
    pub presentation_time_secs: f64,
    /// Scene time reported by the animation clock after this step.
    pub scene_time_secs: f64,
    /// Active state id after this step.
    pub current_state_id: String,
    /// Active transition id, if transitioning.
    pub active_transition_id: Option<String>,
    /// Blend factor during transition (0.0 → 1.0).
    pub transition_blend: Option<f64>,
    /// Transition source state name (resolved at record time).
    pub transition_source_name: Option<String>,
    /// Transition target state name (resolved at record time).
    pub transition_target_name: Option<String>,
    /// Per-state local elapsed times.
    pub state_local_times: BTreeMap<String, f64>,
    /// Runtime diagnostics for this frame.
    pub diagnostics: Vec<String>,
    /// Full set of active parameter overrides (state-machine tracked keys).
    pub active_overrides: HashMap<OverrideKey, serde_json::Value>,
}

/// Rolling-window buffer of recorded timeline frames.
#[derive(Debug, Clone)]
pub struct TimelineBuffer {
    frames: VecDeque<TimelineFrame>,
    /// Wall-clock anchor for computing presentation_time_secs.
    recording_start: Instant,
    /// Cumulative wall-clock seconds spent paused since recording started.
    /// Subtracted from `recording_start.elapsed()` to produce a pausable
    /// presentation clock that doesn't jump after a pause.
    pause_accumulated_secs: f64,
    /// `Some(instant)` while the recording clock is paused.
    pause_start: Option<Instant>,
    /// Maximum duration (in seconds) to retain. Older frames are trimmed.
    max_duration_secs: f64,
    /// Sorted list of tracked override key strings (discovered at creation).
    pub tracked_keys: Vec<String>,
}

impl TimelineBuffer {
    /// Create a new empty buffer with the given duration limit.
    pub fn new(max_duration_secs: f64, tracked_keys: Vec<String>) -> Self {
        Self {
            frames: VecDeque::new(),
            recording_start: Instant::now(),
            pause_accumulated_secs: 0.0,
            pause_start: None,
            max_duration_secs,
            tracked_keys,
        }
    }

    /// Seconds elapsed since recording started, excluding paused time.
    pub fn elapsed_secs(&self) -> f64 {
        let raw = self.recording_start.elapsed().as_secs_f64();
        let current_pause = self
            .pause_start
            .map(|s| s.elapsed().as_secs_f64())
            .unwrap_or(0.0);
        raw - self.pause_accumulated_secs - current_pause
    }

    /// Pause the presentation clock.  While paused, `elapsed_secs` freezes.
    pub fn pause(&mut self) {
        if self.pause_start.is_none() {
            self.pause_start = Some(Instant::now());
        }
    }

    /// Resume the presentation clock after a pause.
    pub fn resume(&mut self) {
        if let Some(start) = self.pause_start.take() {
            self.pause_accumulated_secs += start.elapsed().as_secs_f64();
        }
    }

    /// Append a frame and trim anything older than `max_duration_secs`.
    pub fn push(&mut self, frame: TimelineFrame) {
        let cutoff = frame.presentation_time_secs - self.max_duration_secs;
        self.frames.push_back(frame);
        while let Some(front) = self.frames.front() {
            if front.presentation_time_secs < cutoff {
                self.frames.pop_front();
            } else {
                break;
            }
        }
    }

    /// Drop all frames and reset the wall-clock anchor.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.recording_start = Instant::now();
        self.pause_accumulated_secs = 0.0;
        self.pause_start = None;
    }

    /// Number of recorded frames currently in the buffer.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the buffer contains no frames.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Access all recorded frames as a slice.
    pub fn frames(&self) -> &VecDeque<TimelineFrame> {
        &self.frames
    }

    /// First and last presentation times, or `None` if empty.
    pub fn time_range(&self) -> Option<(f64, f64)> {
        let first = self.frames.front()?.presentation_time_secs;
        let last = self.frames.back()?.presentation_time_secs;
        Some((first, last))
    }

    /// Find the frame nearest to the given presentation time.
    /// Returns the index into the internal deque.
    pub fn nearest_frame_index(&self, t: f64) -> Option<usize> {
        if self.frames.is_empty() {
            return None;
        }
        // Binary search for the insertion point, then compare neighbours.
        let idx = self
            .frames
            .partition_point(|f| f.presentation_time_secs < t);
        if idx == 0 {
            return Some(0);
        }
        if idx >= self.frames.len() {
            return Some(self.frames.len() - 1);
        }
        let before = (self.frames[idx - 1].presentation_time_secs - t).abs();
        let after = (self.frames[idx].presentation_time_secs - t).abs();
        if before <= after {
            Some(idx - 1)
        } else {
            Some(idx)
        }
    }

    /// Get a frame by index.
    pub fn frame_at(&self, index: usize) -> Option<&TimelineFrame> {
        self.frames.get(index)
    }

    /// Find the frame nearest to `t` and return a reference, or `None`.
    pub fn frame_at_time(&self, t: f64) -> Option<&TimelineFrame> {
        self.nearest_frame_index(t)
            .and_then(|i| self.frames.get(i))
    }

    /// The wall-clock `Instant` when this recording started.
    pub fn recording_start(&self) -> Instant {
        self.recording_start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_frame(t: f64) -> TimelineFrame {
        TimelineFrame {
            presentation_time_secs: t,
            scene_time_secs: t,
            current_state_id: "idle".into(),
            active_transition_id: None,
            transition_blend: None,
            transition_source_name: None,
            transition_target_name: None,
            state_local_times: BTreeMap::new(),
            diagnostics: Vec::new(),
            active_overrides: HashMap::new(),
        }
    }

    #[test]
    fn trim_by_wall_clock_duration() {
        let mut buf = TimelineBuffer::new(1.0, vec![]);
        // Push frames spanning 2 seconds.
        for i in 0..=20 {
            buf.push(make_frame(i as f64 * 0.1));
        }
        // Last frame is at t=2.0, so cutoff = 2.0 - 1.0 = 1.0.
        // Frames with t < 1.0 should be trimmed.
        assert!(buf.frames.front().unwrap().presentation_time_secs >= 1.0);
        assert_eq!(
            buf.frames.back().unwrap().presentation_time_secs,
            2.0
        );
    }

    #[test]
    fn trim_at_varying_intervals() {
        let mut buf = TimelineBuffer::new(0.1, vec![]);
        // Varying intervals: 16ms, 16ms, 32ms, 16ms, 16ms, 32ms …
        let times = [0.0, 0.016, 0.032, 0.064, 0.080, 0.096, 0.128, 0.144, 0.160];
        for &t in &times {
            buf.push(make_frame(t));
        }
        // Last = 0.160, cutoff = 0.160 - 0.1 = 0.060
        let front_t = buf.frames.front().unwrap().presentation_time_secs;
        assert!(
            front_t >= 0.060,
            "front {front_t} should be >= 0.060"
        );
    }

    #[test]
    fn clear_resets_buffer() {
        let mut buf = TimelineBuffer::new(10.0, vec!["key".into()]);
        buf.push(make_frame(1.0));
        buf.push(make_frame(2.0));
        assert_eq!(buf.len(), 2);
        buf.clear();
        assert!(buf.is_empty());
        assert!(buf.time_range().is_none());
    }

    #[test]
    fn lifecycle_clear_on_play_preserve_on_stop() {
        let mut buf = TimelineBuffer::new(10.0, vec![]);
        // Simulate Play session: push some frames.
        buf.push(make_frame(0.0));
        buf.push(make_frame(0.5));
        buf.push(make_frame(1.0));
        assert_eq!(buf.len(), 3);

        // Stop: buffer preserved.
        let preserved_len = buf.len();
        assert_eq!(preserved_len, 3);

        // New Play: clear and start fresh.
        buf.clear();
        assert!(buf.is_empty());
        buf.push(make_frame(0.0));
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn nearest_frame_lookup() {
        let mut buf = TimelineBuffer::new(10.0, vec![]);
        buf.push(make_frame(1.0));
        buf.push(make_frame(2.0));
        buf.push(make_frame(3.0));

        // Exact match
        assert_eq!(buf.nearest_frame_index(2.0), Some(1));
        // Closer to 2.0 than 3.0
        assert_eq!(buf.nearest_frame_index(2.3), Some(1));
        // Closer to 3.0
        assert_eq!(buf.nearest_frame_index(2.7), Some(2));
        // Before all frames
        assert_eq!(buf.nearest_frame_index(0.0), Some(0));
        // After all frames
        assert_eq!(buf.nearest_frame_index(99.0), Some(2));
    }

    #[test]
    fn nearest_frame_empty() {
        let buf = TimelineBuffer::new(10.0, vec![]);
        assert_eq!(buf.nearest_frame_index(1.0), None);
        assert!(buf.frame_at_time(1.0).is_none());
    }

    #[test]
    fn time_range() {
        let mut buf = TimelineBuffer::new(10.0, vec![]);
        assert!(buf.time_range().is_none());
        buf.push(make_frame(0.5));
        buf.push(make_frame(1.5));
        assert_eq!(buf.time_range(), Some((0.5, 1.5)));
    }
}
