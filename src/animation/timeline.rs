//! Timeline recorder for state-machine animation frames.
//!
//! The recorder owns a monotonic logical clock that advances only while the
//! state-machine control is active. This keeps the timeline independent from
//! State-local scene time resets and from wall-clock time spent paused.

use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    sync::atomic::{AtomicU64, Ordering},
};

use crate::state_machine::{MotionChannelDebug, OverrideKey};

static NEXT_TIMELINE_BUFFER_ID: AtomicU64 = AtomicU64::new(1);

/// Stable identity for one frame while it remains in a timeline buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TimelineFrameId(pub(crate) u64);

/// Full render-side values needed to display a recorded or live frame.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineRenderSnapshot {
    pub scene_time_secs: f64,
    pub active_overrides: HashMap<OverrideKey, serde_json::Value>,
}

/// A single recorded frame of state-machine data.
#[derive(Debug, Clone)]
pub struct TimelineFrame {
    pub id: TimelineFrameId,
    /// Monotonic logical seconds since the current recording began.
    pub presentation_time_secs: f64,
    /// State-machine scene time after this step. This may reset on State changes.
    pub scene_time_secs: f64,
    pub current_state_id: String,
    pub active_transition_id: Option<String>,
    pub motion_channels: Vec<MotionChannelDebug>,
    pub transition_source_name: Option<String>,
    pub transition_target_name: Option<String>,
    pub state_local_times: BTreeMap<String, f64>,
    pub diagnostics: Vec<String>,
    /// Complete presentation snapshot for every tracked override key.
    pub active_overrides: HashMap<OverrideKey, serde_json::Value>,
}

impl TimelineFrame {
    pub fn render_snapshot(&self) -> TimelineRenderSnapshot {
        TimelineRenderSnapshot {
            scene_time_secs: self.scene_time_secs,
            active_overrides: self.active_overrides.clone(),
        }
    }
}

/// Frame data supplied by the recorder. `TimelineBuffer` assigns the stable id
/// and current presentation time when this value is pushed.
#[derive(Debug, Clone)]
pub struct TimelineFrameRecord {
    pub scene_time_secs: f64,
    pub current_state_id: String,
    pub active_transition_id: Option<String>,
    pub motion_channels: Vec<MotionChannelDebug>,
    pub transition_source_name: Option<String>,
    pub transition_target_name: Option<String>,
    pub state_local_times: BTreeMap<String, f64>,
    pub diagnostics: Vec<String>,
    pub active_overrides: HashMap<OverrideKey, serde_json::Value>,
}

/// Rolling-window buffer of recorded timeline frames.
#[derive(Debug, Clone)]
pub struct TimelineBuffer {
    recording_id: u64,
    frames: VecDeque<TimelineFrame>,
    presentation_time_secs: f64,
    max_duration_secs: f64,
    next_frame_id: u64,
    /// Sorted list of tracked override key strings (discovered at creation).
    pub tracked_keys: Vec<String>,
}

impl TimelineBuffer {
    pub fn new(max_duration_secs: f64, tracked_keys: Vec<String>) -> Self {
        Self {
            recording_id: NEXT_TIMELINE_BUFFER_ID.fetch_add(1, Ordering::Relaxed),
            frames: VecDeque::new(),
            presentation_time_secs: 0.0,
            max_duration_secs,
            next_frame_id: 0,
            tracked_keys,
        }
    }

    /// Identity of this recording lifecycle. A fresh scene creates a fresh id;
    /// clearing for replay keeps the id while frame ids continue monotonically.
    pub(crate) fn recording_id(&self) -> u64 {
        self.recording_id
    }

    /// Advance the logical recording clock by accepted animation time.
    pub fn advance_time(&mut self, dt: f64) {
        if dt.is_finite() && dt > 0.0 {
            self.presentation_time_secs += dt;
        }
    }

    pub fn elapsed_secs(&self) -> f64 {
        self.presentation_time_secs
    }

    /// Append a frame at the current logical time and trim the rolling window.
    pub fn push(&mut self, record: TimelineFrameRecord) -> TimelineFrameId {
        let id = TimelineFrameId(self.next_frame_id);
        self.next_frame_id = self.next_frame_id.saturating_add(1);
        let frame = TimelineFrame {
            id,
            presentation_time_secs: self.presentation_time_secs,
            scene_time_secs: record.scene_time_secs,
            current_state_id: record.current_state_id,
            active_transition_id: record.active_transition_id,
            motion_channels: record.motion_channels,
            transition_source_name: record.transition_source_name,
            transition_target_name: record.transition_target_name,
            state_local_times: record.state_local_times,
            diagnostics: record.diagnostics,
            active_overrides: record.active_overrides,
        };
        let cutoff = frame.presentation_time_secs - self.max_duration_secs;
        self.frames.push_back(frame);
        while self
            .frames
            .front()
            .is_some_and(|front| front.presentation_time_secs < cutoff)
        {
            self.frames.pop_front();
        }
        id
    }

    /// Drop all frames and rewind logical time. Frame ids are not reused.
    pub fn clear(&mut self) {
        self.frames.clear();
        self.presentation_time_secs = 0.0;
    }

    pub fn len(&self) -> usize {
        self.frames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    pub fn frames(&self) -> &VecDeque<TimelineFrame> {
        &self.frames
    }

    pub fn time_range(&self) -> Option<(f64, f64)> {
        Some((
            self.frames.front()?.presentation_time_secs,
            self.frames.back()?.presentation_time_secs,
        ))
    }

    /// Find the nearest frame by presentation time. Exact duplicate timestamps
    /// and distance ties resolve to the newest frame.
    pub fn nearest_frame_id(&self, time_secs: f64) -> Option<TimelineFrameId> {
        if self.frames.is_empty() {
            return None;
        }
        let after_equal = self
            .frames
            .partition_point(|frame| frame.presentation_time_secs <= time_secs);
        if after_equal == 0 {
            return self.frames.front().map(|frame| frame.id);
        }
        if after_equal >= self.frames.len() {
            return self.frames.back().map(|frame| frame.id);
        }
        let before = &self.frames[after_equal - 1];
        let after = &self.frames[after_equal];
        let before_distance = (before.presentation_time_secs - time_secs).abs();
        let after_distance = (after.presentation_time_secs - time_secs).abs();
        Some(if before_distance < after_distance {
            before.id
        } else {
            after.id
        })
    }

    pub fn frame_by_id(&self, id: TimelineFrameId) -> Option<&TimelineFrame> {
        self.frames.iter().find(|frame| frame.id == id)
    }

    pub fn latest_frame_id(&self) -> Option<TimelineFrameId> {
        self.frames.back().map(|frame| frame.id)
    }

    pub fn latest_frame(&self) -> Option<&TimelineFrame> {
        self.frames.back()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(scene_time_secs: f64) -> TimelineFrameRecord {
        TimelineFrameRecord {
            scene_time_secs,
            current_state_id: "idle".into(),
            active_transition_id: None,
            motion_channels: Vec::new(),
            transition_source_name: None,
            transition_target_name: None,
            state_local_times: BTreeMap::new(),
            diagnostics: Vec::new(),
            active_overrides: HashMap::new(),
        }
    }

    fn push_at(buffer: &mut TimelineBuffer, time_secs: f64) -> TimelineFrameId {
        let dt = time_secs - buffer.elapsed_secs();
        buffer.advance_time(dt);
        buffer.push(record(time_secs))
    }

    #[test]
    fn logical_time_advances_only_for_positive_finite_deltas() {
        let mut buffer = TimelineBuffer::new(10.0, vec![]);
        buffer.advance_time(0.25);
        buffer.advance_time(0.0);
        buffer.advance_time(-1.0);
        buffer.advance_time(f64::NAN);
        assert_eq!(buffer.elapsed_secs(), 0.25);
    }

    #[test]
    fn trims_by_logical_duration() {
        let mut buffer = TimelineBuffer::new(1.0, vec![]);
        for index in 0..=20 {
            push_at(&mut buffer, index as f64 * 0.1);
        }
        assert!(buffer.frames.front().unwrap().presentation_time_secs >= 1.0);
        assert_eq!(buffer.frames.back().unwrap().presentation_time_secs, 2.0);
    }

    #[test]
    fn clear_rewinds_time_without_reusing_frame_ids() {
        let mut buffer = TimelineBuffer::new(10.0, vec!["key".into()]);
        let first = push_at(&mut buffer, 1.0);
        buffer.clear();
        assert!(buffer.is_empty());
        assert_eq!(buffer.elapsed_secs(), 0.0);
        let second = buffer.push(record(0.0));
        assert!(second > first);
    }

    #[test]
    fn nearest_frame_prefers_newest_duplicate_and_distance_tie() {
        let mut buffer = TimelineBuffer::new(10.0, vec![]);
        let first = push_at(&mut buffer, 1.0);
        let duplicate = buffer.push(record(0.0));
        let later = push_at(&mut buffer, 3.0);

        assert_eq!(buffer.nearest_frame_id(1.0), Some(duplicate));
        assert_eq!(buffer.nearest_frame_id(2.0), Some(later));
        assert_ne!(first, duplicate);
    }

    #[test]
    fn scene_time_can_reset_without_affecting_presentation_time() {
        let mut buffer = TimelineBuffer::new(10.0, vec![]);
        buffer.advance_time(1.0);
        buffer.push(record(5.0));
        buffer.advance_time(0.5);
        buffer.push(record(0.0));

        let frames = buffer.frames();
        assert_eq!(frames[0].presentation_time_secs, 1.0);
        assert_eq!(frames[1].presentation_time_secs, 1.5);
        assert_eq!(frames[1].scene_time_secs, 0.0);
    }

    #[test]
    fn time_range_and_id_lookup_follow_retained_frames() {
        let mut buffer = TimelineBuffer::new(10.0, vec![]);
        let first = push_at(&mut buffer, 0.5);
        let second = push_at(&mut buffer, 1.5);
        assert_eq!(buffer.time_range(), Some((0.5, 1.5)));
        assert_eq!(buffer.frame_by_id(first).map(|frame| frame.id), Some(first));
        assert_eq!(buffer.latest_frame_id(), Some(second));
    }
}
