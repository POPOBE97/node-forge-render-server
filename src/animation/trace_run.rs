//! Deterministic animation tracing on the app `AnimationSession` path.
//!
//! Powers `--trace-animation` and the `animation_values` golden harness so
//! product diagnostics and tests never diverge.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};

use crate::animation::session::{AnimationSession, AnimationStep};
use crate::dsl::SceneDSL;
use crate::state_machine::{
    EventModifiers, EventSchedule, FiredEvent, MotionChannelDebug, MousePosition, ScheduledEvent,
    TickSchedule, build_initial_values, canonicalize_json_value, round_f64, tracked_override_keys,
};
use crate::state_machine::{AnimationTraceFrame, AnimationTraceLog};

const TRACE_REPORT_SCHEMA_VERSION: u32 = 2;
const ANIMATION_TRACE_LOG_SCHEMA_VERSION: u32 = 1;
const IDENTITY_EPSILON: f64 = 1.0e-6;
const HANDOFF_EPSILON: f64 = 1.0e-8;
const DEFAULT_JUMP_THRESHOLD_CHANNEL: f64 = 0.05;
const DEFAULT_JUMP_THRESHOLD_OVERRIDE: f64 = 5.0;

// ---------------------------------------------------------------------------
// Scenario / config
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceScenario {
    #[serde(default = "default_scenario_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default = "default_fps")]
    pub fps: u32,
    #[serde(default)]
    pub track: TraceTrack,
    #[serde(default)]
    pub analyze: TraceAnalyzeConfig,
    #[serde(default)]
    pub actions: Vec<ScenarioAction>,
}

fn default_scenario_schema_version() -> u32 {
    1
}

fn default_fps() -> u32 {
    60
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceTrack {
    /// MotionEngine channel keys (`StateParam` ids). Empty / missing means all.
    #[serde(default)]
    pub channels: Option<Vec<String>>,
    /// Derivation override keys (`nodeId:param`). Empty vec means none; missing means all tracked.
    #[serde(default)]
    pub overrides: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceAnalyzeConfig {
    #[serde(default = "default_jump_threshold_channel")]
    pub jump_threshold_channel: f64,
    #[serde(default = "default_jump_threshold_override")]
    pub jump_threshold_override: f64,
    #[serde(default = "default_true")]
    pub check_physic_identity: bool,
    #[serde(default)]
    pub fail_on_jump: bool,
}

impl Default for TraceAnalyzeConfig {
    fn default() -> Self {
        Self {
            jump_threshold_channel: DEFAULT_JUMP_THRESHOLD_CHANNEL,
            jump_threshold_override: DEFAULT_JUMP_THRESHOLD_OVERRIDE,
            check_physic_identity: true,
            fail_on_jump: false,
        }
    }
}

fn default_jump_threshold_channel() -> f64 {
    DEFAULT_JUMP_THRESHOLD_CHANNEL
}

fn default_jump_threshold_override() -> f64 {
    DEFAULT_JUMP_THRESHOLD_OVERRIDE
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ScenarioAction {
    #[serde(rename = "step")]
    Step {
        #[serde(default)]
        frames: Option<u64>,
        #[serde(default)]
        seconds: Option<f64>,
    },
    #[serde(rename = "settle")]
    Settle {
        #[serde(rename = "maxFrames")]
        max_frames: u64,
    },
    #[serde(rename = "event")]
    Event {
        #[serde(rename = "eventType", alias = "eventName")]
        event_type: String,
        #[serde(default)]
        key: Option<String>,
        #[serde(default)]
        button: Option<String>,
        #[serde(default)]
        repeat: Option<bool>,
        #[serde(default)]
        modifiers: Option<EventModifiers>,
    },
    #[serde(rename = "mouse")]
    Mouse { x: f64, y: f64 },
    #[serde(rename = "forceState")]
    ForceState {
        #[serde(rename = "stateId")]
        state_id: String,
    },
    #[serde(rename = "assertState")]
    AssertState {
        #[serde(rename = "stateId")]
        state_id: String,
    },
    #[serde(rename = "assertTransition")]
    AssertTransition {
        #[serde(default, alias = "id")]
        transition_id: Option<String>,
    },
    #[serde(rename = "label")]
    Label { name: String },
}

#[derive(Debug, Clone)]
pub struct TraceRunConfig {
    pub fps: u32,
    pub scenario_name: Option<String>,
    pub scene_source: Option<String>,
    pub initial_state: Option<String>,
    pub actions: Vec<ScenarioAction>,
    pub channel_filter: KeyFilter,
    pub override_filter: KeyFilter,
    pub include_values: bool,
    pub analyze: TraceAnalyzeConfig,
    /// When true, `force_state` was used (routing disabled while forced).
    pub routing_forced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyFilter {
    All,
    None,
    Only(BTreeSet<String>),
}

impl KeyFilter {
    pub fn from_cli_csv(raw: Option<&str>) -> Result<Self> {
        match raw {
            None | Some("*") | Some("") => Ok(Self::All),
            Some("none") => Ok(Self::None),
            Some(list) => {
                let set = list
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect::<BTreeSet<_>>();
                if set.is_empty() {
                    bail!("channel/override filter list is empty");
                }
                Ok(Self::Only(set))
            }
        }
    }

    pub fn from_optional_list(list: Option<Vec<String>>) -> Self {
        match list {
            None => Self::All,
            Some(items) if items.is_empty() => Self::None,
            Some(items) if items.iter().all(|item| item == "*") => Self::All,
            Some(items) => Self::Only(items.into_iter().filter(|item| item != "*").collect()),
        }
    }

    fn resolved_keys(&self, available: &BTreeSet<String>) -> Result<Vec<String>> {
        match self {
            Self::All => Ok(available.iter().cloned().collect()),
            Self::None => Ok(Vec::new()),
            Self::Only(wanted) => {
                let missing: Vec<_> = wanted.difference(available).cloned().collect();
                if !missing.is_empty() {
                    bail!(
                        "unknown filter key(s): {}\navailable: {}",
                        missing.join(", "),
                        available.iter().cloned().collect::<Vec<_>>().join(", ")
                    );
                }
                Ok(wanted.iter().cloned().collect())
            }
        }
    }

}

// ---------------------------------------------------------------------------
// Report (schema v2)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceReport {
    pub schema_version: u32,
    pub tool: String,
    pub scene: TraceSceneInfo,
    pub config: TraceReportConfig,
    pub summary: TraceSummary,
    pub frames: Vec<TraceReportFrame>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceSceneInfo {
    pub source: Option<String>,
    pub state_machine_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceReportConfig {
    pub fps: u32,
    pub scenario_name: Option<String>,
    pub routing_mode: String,
    pub filters: TraceReportFilters,
    pub analyze: TraceAnalyzeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceReportFilters {
    pub channels: Vec<String>,
    pub overrides: Vec<String>,
    pub include_values: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TraceSummary {
    pub frame_count: usize,
    pub final_state_id: String,
    pub final_transition_id: Option<String>,
    pub transitions_seen: Vec<String>,
    pub identity_violations: usize,
    pub jumps: Vec<TraceJump>,
    pub max_abs_channel_delta: BTreeMap<String, f64>,
    pub max_abs_override_delta: BTreeMap<String, f64>,
    pub handoffs: Vec<TraceHandoff>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceJump {
    pub frame_index: usize,
    pub kind: String,
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub component: Option<usize>,
    pub delta: f64,
    pub from: f64,
    pub to: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceHandoff {
    pub frame_index: usize,
    pub from_state: String,
    pub to_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_id: Option<String>,
    pub channel_continuity: Vec<TraceChannelContinuity>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceChannelContinuity {
    pub key: String,
    pub outgoing_p: Vec<f64>,
    pub incoming_p: Vec<f64>,
    pub ok: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TraceReportFrame {
    pub frame_index: usize,
    pub time_secs: f64,
    pub dt_secs: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub current_state_id: String,
    pub active_transition_id: Option<String>,
    pub scene_time_secs: f64,
    pub state_local_times: BTreeMap<String, f64>,
    pub finished: bool,
    pub diagnostics: Vec<String>,
    pub motion_channels: Vec<MotionChannelDebug>,
    pub values: BTreeMap<String, serde_json::Value>,
    pub analysis: TraceFrameAnalysis,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TraceFrameAnalysis {
    pub identity_ok: bool,
    pub identity_violations: Vec<String>,
    pub jumps: Vec<TraceJump>,
}

#[derive(Debug, Clone)]
pub struct TraceRunResult {
    pub report: TraceReport,
    /// Nonzero when scenario assert failed or fail_on_jump triggered.
    pub exit_code: i32,
    pub assert_error: Option<String>,
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

impl TraceScenario {
    pub fn from_path(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("failed to read scenario {}: {e}", path.display()))?;
        let scenario: Self = serde_json::from_str(&text)
            .map_err(|e| anyhow!("failed to parse scenario {}: {e}", path.display()))?;
        if scenario.schema_version != 1 {
            bail!(
                "unsupported scenario schemaVersion {} (expected 1)",
                scenario.schema_version
            );
        }
        if scenario.fps == 0 {
            bail!("scenario fps must be > 0");
        }
        if scenario.actions.is_empty() {
            bail!("scenario actions must not be empty");
        }
        Ok(scenario)
    }

    pub fn into_run_config(self, scene_source: Option<String>) -> TraceRunConfig {
        TraceRunConfig {
            fps: self.fps,
            scenario_name: self.name,
            scene_source,
            initial_state: None,
            actions: self.actions,
            channel_filter: KeyFilter::from_optional_list(self.track.channels),
            override_filter: KeyFilter::from_optional_list(self.track.overrides),
            include_values: true,
            analyze: self.analyze,
            routing_forced: false,
        }
    }
}

impl TraceRunConfig {
    /// Free-run convenience config: fixed schedule from t=0 (no scenario actions).
    ///
    /// Prefer [`run_schedule_trace`] for free-run execution; this builder exists
    /// so CLI can share filter/analyze fields with scenario runs.
    pub fn free_run_meta(
        fps: u32,
        scene_source: Option<String>,
        channel_filter: KeyFilter,
        override_filter: KeyFilter,
        include_values: bool,
        analyze: TraceAnalyzeConfig,
        initial_state: Option<String>,
    ) -> Self {
        Self {
            fps,
            scenario_name: None,
            scene_source,
            initial_state,
            actions: Vec::new(),
            channel_filter,
            override_filter,
            include_values,
            analyze,
            routing_forced: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

pub fn run_trace(scene: &SceneDSL, config: &TraceRunConfig) -> Result<TraceRunResult> {
    let mut session = AnimationSession::from_scene(scene)?
        .ok_or_else(|| anyhow!("scene has no stateMachine"))?;

    let mut routing_forced = config.routing_forced;
    if let Some(state_id) = &config.initial_state {
        session.force_state(state_id)?;
        routing_forced = true;
    }

    let tracked_keys: BTreeSet<String> = tracked_override_keys(session.runtime().definition());
    let mut current_values = build_initial_values(scene, &tracked_keys.iter().cloned().collect::<Vec<_>>());

    // Channel availability is discovered after the first recorded step.
    let mut available_channels: BTreeSet<String> = BTreeSet::new();
    let mut resolved_channel_keys: Option<BTreeSet<String>> = None;
    let resolved_override_keys = config.override_filter.resolved_keys(&tracked_keys)?;
    let resolved_override_set: BTreeSet<String> = resolved_override_keys.iter().cloned().collect();

    let mut frames: Vec<TraceReportFrame> = Vec::new();
    let mut current_label: Option<String> = None;
    let mut cumulative_time = 0.0f64;
    let mut assert_error: Option<String> = None;
    let dt_step = 1.0 / f64::from(config.fps);

    for action in &config.actions {
        if assert_error.is_some() {
            break;
        }
        match action {
            ScenarioAction::Label { name } => {
                current_label = Some(name.clone());
            }
            ScenarioAction::Mouse { x, y } => {
                session.update_mouse_position(MousePosition { x: *x, y: *y });
            }
            ScenarioAction::ForceState { state_id } => {
                let step = session.force_state(state_id)?;
                routing_forced = true;
                // force_state already performed a dt=0 step; record it.
                record_step(
                    &step,
                    0.0,
                    &mut cumulative_time,
                    &current_label,
                    &mut current_values,
                    &tracked_keys,
                    &resolved_override_set,
                    config.include_values,
                    &config.channel_filter,
                    &mut available_channels,
                    &mut resolved_channel_keys,
                    &mut frames,
                )?;
            }
            ScenarioAction::Event {
                event_type,
                key,
                button,
                repeat,
                modifiers,
            } => {
                session.fire_event(FiredEvent {
                    event_type: event_type.clone(),
                    key: key.clone(),
                    button: button.clone(),
                    repeat: repeat.unwrap_or(false),
                    modifiers: modifiers.clone().unwrap_or_default(),
                });
                let step = session.step(0.0);
                record_step(
                    &step,
                    0.0,
                    &mut cumulative_time,
                    &current_label,
                    &mut current_values,
                    &tracked_keys,
                    &resolved_override_set,
                    config.include_values,
                    &config.channel_filter,
                    &mut available_channels,
                    &mut resolved_channel_keys,
                    &mut frames,
                )?;
            }
            ScenarioAction::Step { frames: f, seconds } => {
                match resolve_step_plan(*f, *seconds, config.fps)? {
                    StepPlan::Zero => {
                        let step = session.step(0.0);
                        record_step(
                            &step,
                            0.0,
                            &mut cumulative_time,
                            &current_label,
                            &mut current_values,
                            &tracked_keys,
                            &resolved_override_set,
                            config.include_values,
                            &config.channel_filter,
                            &mut available_channels,
                            &mut resolved_channel_keys,
                            &mut frames,
                        )?;
                    }
                    StepPlan::FixedFrames { count, dt } => {
                        for _ in 0..count {
                            let step = session.step(dt);
                            record_step(
                                &step,
                                dt,
                                &mut cumulative_time,
                                &current_label,
                                &mut current_values,
                                &tracked_keys,
                                &resolved_override_set,
                                config.include_values,
                                &config.channel_filter,
                                &mut available_channels,
                                &mut resolved_channel_keys,
                                &mut frames,
                            )?;
                        }
                    }
                    StepPlan::SingleDt { dt } => {
                        let step = session.step(dt);
                        record_step(
                            &step,
                            dt,
                            &mut cumulative_time,
                            &current_label,
                            &mut current_values,
                            &tracked_keys,
                            &resolved_override_set,
                            config.include_values,
                            &config.channel_filter,
                            &mut available_channels,
                            &mut resolved_channel_keys,
                            &mut frames,
                        )?;
                    }
                }
            }
            ScenarioAction::Settle { max_frames } => {
                // Always take at least one sample at current time, then advance.
                let mut steps_taken = 0u64;
                loop {
                    if session.runtime().active_transition_id().is_none() && steps_taken > 0 {
                        break;
                    }
                    if steps_taken >= *max_frames {
                        break;
                    }
                    let step = session.step(dt_step);
                    record_step(
                        &step,
                        dt_step,
                        &mut cumulative_time,
                        &current_label,
                        &mut current_values,
                        &tracked_keys,
                        &resolved_override_set,
                        config.include_values,
                        &config.channel_filter,
                        &mut available_channels,
                        &mut resolved_channel_keys,
                        &mut frames,
                    )?;
                    steps_taken += 1;
                    if step.active_transition_id.is_none() {
                        break;
                    }
                }
            }
            ScenarioAction::AssertState { state_id } => {
                let current = session
                    .runtime()
                    .current_state_id()
                    .to_string();
                if current != *state_id {
                    assert_error = Some(format!(
                        "assertState failed: expected '{state_id}', got '{current}'"
                    ));
                }
            }
            ScenarioAction::AssertTransition { transition_id } => {
                let current = session.runtime().active_transition_id().map(str::to_string);
                if current.as_deref() != transition_id.as_deref() {
                    assert_error = Some(format!(
                        "assertTransition failed: expected {:?}, got {:?}",
                        transition_id, current
                    ));
                }
            }
        }
    }

    let mut result = finalize_report(
        scene,
        config,
        routing_forced,
        frames,
        available_channels,
        resolved_channel_keys,
        resolved_override_keys,
    )?;
    if assert_error.is_some() {
        result.exit_code = 2;
        result.assert_error = assert_error;
    }
    Ok(result)
}

fn finalize_report(
    scene: &SceneDSL,
    config: &TraceRunConfig,
    routing_forced: bool,
    frames: Vec<TraceReportFrame>,
    available_channels: BTreeSet<String>,
    resolved_channel_keys: Option<BTreeSet<String>>,
    resolved_override_keys: Vec<String>,
) -> Result<TraceRunResult> {
    let channel_keys = match &resolved_channel_keys {
        Some(set) => set.iter().cloned().collect(),
        None => config
            .channel_filter
            .resolved_keys(&available_channels)
            .unwrap_or_default(),
    };

    let mut report_frames = frames;
    apply_analysis(&mut report_frames, &config.analyze);
    let summary = build_summary(&report_frames, &config.analyze);
    let sm_id = scene.state_machine.as_ref().map(|sm| sm.id.clone());

    let report = TraceReport {
        schema_version: TRACE_REPORT_SCHEMA_VERSION,
        tool: "trace-animation".into(),
        scene: TraceSceneInfo {
            source: config.scene_source.clone(),
            state_machine_id: sm_id,
        },
        config: TraceReportConfig {
            fps: config.fps,
            scenario_name: config.scenario_name.clone(),
            routing_mode: if routing_forced {
                "forced".into()
            } else {
                "natural".into()
            },
            filters: TraceReportFilters {
                channels: if matches!(config.channel_filter, KeyFilter::All) {
                    vec!["*".into()]
                } else {
                    channel_keys
                },
                overrides: if matches!(config.override_filter, KeyFilter::All) {
                    vec!["*".into()]
                } else {
                    resolved_override_keys
                },
                include_values: config.include_values,
            },
            analyze: config.analyze.clone(),
        },
        summary,
        frames: report_frames,
    };

    let exit_code = if config.analyze.fail_on_jump && !report.summary.jumps.is_empty() {
        2
    } else {
        0
    };

    Ok(TraceRunResult {
        report,
        exit_code,
        assert_error: None,
    })
}

enum StepPlan {
    /// `frames: 0` — record one `dt=0` sample.
    Zero,
    /// Multi-frame advance at fixed `1/fps` (or an aligned `seconds` value).
    FixedFrames { count: u64, dt: f64 },
    /// One physical step with an arbitrary `dt` (used when `seconds` is not
    /// frame-aligned, matching hand-authored diagnostics like `step(0.21)`).
    SingleDt { dt: f64 },
}

fn resolve_step_plan(frames: Option<u64>, seconds: Option<f64>, fps: u32) -> Result<StepPlan> {
    let dt_step = 1.0 / f64::from(fps);
    match (frames, seconds) {
        (Some(0), None) => Ok(StepPlan::Zero),
        (Some(count), None) => Ok(StepPlan::FixedFrames {
            count,
            dt: dt_step,
        }),
        (None, Some(s)) => {
            if !s.is_finite() || s < 0.0 {
                bail!("step.seconds must be finite and >= 0");
            }
            if s == 0.0 {
                return Ok(StepPlan::Zero);
            }
            let steps = s * f64::from(fps);
            let rounded = steps.round();
            if (steps - rounded).abs() <= 1e-9 {
                Ok(StepPlan::FixedFrames {
                    count: rounded as u64,
                    dt: dt_step,
                })
            } else {
                Ok(StepPlan::SingleDt { dt: s })
            }
        }
        (None, None) => bail!("step action requires frames or seconds"),
        (Some(_), Some(_)) => bail!("step action cannot set both frames and seconds"),
    }
}

fn record_step(
    step: &AnimationStep,
    dt: f64,
    cumulative_time: &mut f64,
    label: &Option<String>,
    current_values: &mut BTreeMap<String, serde_json::Value>,
    tracked_keys: &BTreeSet<String>,
    resolved_override_set: &BTreeSet<String>,
    include_values: bool,
    channel_filter: &KeyFilter,
    available_channels: &mut BTreeSet<String>,
    resolved_channel_keys: &mut Option<BTreeSet<String>>,
    frames: &mut Vec<TraceReportFrame>,
) -> Result<()> {
    for channel in &step.motion_channels {
        available_channels.insert(channel.key.clone());
    }
    if resolved_channel_keys.is_none() && !available_channels.is_empty() {
        let keys = channel_filter.resolved_keys(available_channels)?;
        *resolved_channel_keys = Some(keys.into_iter().collect());
    }

    for (key, value) in &step.active_overrides {
        let trace_key = format!("{}:{}", key.node_id, key.param_name);
        current_values.insert(trace_key, canonicalize_json_value(value));
    }

    let time_secs = round_f64(*cumulative_time);
    *cumulative_time += dt;

    let motion_channels = filter_channels(
        &step.motion_channels,
        resolved_channel_keys.as_ref(),
        channel_filter,
    );

    let values = if include_values {
        let mut frame_values = BTreeMap::new();
        for key in tracked_keys {
            if resolved_override_set.contains(key) {
                let value = current_values
                    .get(key)
                    .cloned()
                    .unwrap_or(serde_json::Value::Null);
                frame_values.insert(key.clone(), value);
            }
        }
        frame_values
    } else {
        BTreeMap::new()
    };

    let state_local_times: BTreeMap<String, f64> = step
        .state_local_times
        .iter()
        .map(|(k, v)| (k.clone(), round_f64(*v)))
        .collect();

    frames.push(TraceReportFrame {
        frame_index: frames.len(),
        time_secs,
        dt_secs: round_f64(dt),
        label: label.clone(),
        current_state_id: step.current_state_id.clone(),
        active_transition_id: step.active_transition_id.clone(),
        scene_time_secs: round_f64(step.scene_time_secs),
        state_local_times,
        finished: step.finished,
        diagnostics: step.diagnostics.clone(),
        motion_channels,
        values,
        analysis: TraceFrameAnalysis::default(),
    });
    Ok(())
}

fn filter_channels(
    channels: &[MotionChannelDebug],
    resolved: Option<&BTreeSet<String>>,
    filter: &KeyFilter,
) -> Vec<MotionChannelDebug> {
    match filter {
        KeyFilter::None => Vec::new(),
        KeyFilter::All if resolved.is_none() => channels.to_vec(),
        KeyFilter::All => channels.to_vec(),
        KeyFilter::Only(_) => {
            let Some(set) = resolved else {
                return Vec::new();
            };
            channels
                .iter()
                .filter(|c| set.contains(&c.key))
                .cloned()
                .collect()
        }
    }
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

fn apply_analysis(frames: &mut [TraceReportFrame], analyze: &TraceAnalyzeConfig) {
    for i in 0..frames.len() {
        let mut identity_ok = true;
        let mut identity_violations = Vec::new();
        let mut jumps = Vec::new();

        if analyze.check_physic_identity {
            for channel in &frames[i].motion_channels {
                for (comp, (((p, q), e), _)) in channel
                    .value
                    .iter()
                    .zip(channel.target_value.iter())
                    .zip(channel.transition_error.iter())
                    .zip(std::iter::repeat(()))
                    .enumerate()
                {
                    let expected = q - e;
                    let delta = (p - expected).abs();
                    if delta > IDENTITY_EPSILON {
                        identity_ok = false;
                        identity_violations.push(format!(
                            "{}[{comp}]: P={p:.8} Q={q:.8} E={e:.8} |P-(Q-E)|={delta:.2e}",
                            channel.key
                        ));
                    }
                }
            }
        }

        if i > 0 {
            let prev_channels = frames[i - 1].motion_channels.clone();
            let prev_values = frames[i - 1].values.clone();
            let label = frames[i].label.clone();
            let frame_index = frames[i].frame_index;

            for channel in &frames[i].motion_channels {
                if let Some(prev) = prev_channels.iter().find(|c| c.key == channel.key) {
                    for (comp, (cur, old)) in channel.value.iter().zip(prev.value.iter()).enumerate()
                    {
                        let delta = (cur - old).abs();
                        if delta > analyze.jump_threshold_channel {
                            jumps.push(TraceJump {
                                frame_index,
                                kind: "channel".into(),
                                key: channel.key.clone(),
                                component: Some(comp),
                                delta,
                                from: *old,
                                to: *cur,
                                label: label.clone(),
                            });
                        }
                    }
                }
            }

            for (key, value) in &frames[i].values {
                if let Some(prev) = prev_values.get(key) {
                    collect_value_jumps(
                        key,
                        prev,
                        value,
                        frame_index,
                        label.as_deref(),
                        analyze.jump_threshold_override,
                        &mut jumps,
                    );
                }
            }
        }

        frames[i].analysis = TraceFrameAnalysis {
            identity_ok,
            identity_violations,
            jumps,
        };
    }
}

fn collect_value_jumps(
    key: &str,
    prev: &serde_json::Value,
    cur: &serde_json::Value,
    frame_index: usize,
    label: Option<&str>,
    threshold: f64,
    out: &mut Vec<TraceJump>,
) {
    let mut prev_nums = Vec::new();
    let mut cur_nums = Vec::new();
    flatten_numbers(prev, &mut prev_nums);
    flatten_numbers(cur, &mut cur_nums);
    let n = prev_nums.len().min(cur_nums.len());
    for i in 0..n {
        let delta = (cur_nums[i] - prev_nums[i]).abs();
        if delta > threshold {
            out.push(TraceJump {
                frame_index,
                kind: "override".into(),
                key: key.to_string(),
                component: Some(i),
                delta,
                from: prev_nums[i],
                to: cur_nums[i],
                label: label.map(str::to_string),
            });
        }
    }
}

fn flatten_numbers(value: &serde_json::Value, out: &mut Vec<f64>) {
    match value {
        serde_json::Value::Number(n) => {
            if let Some(v) = n.as_f64() {
                out.push(v);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                flatten_numbers(item, out);
            }
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<_> = map.keys().collect();
            keys.sort();
            for k in keys {
                if let Some(v) = map.get(k) {
                    flatten_numbers(v, out);
                }
            }
        }
        _ => {}
    }
}

fn build_summary(frames: &[TraceReportFrame], _analyze: &TraceAnalyzeConfig) -> TraceSummary {
    let mut transitions_seen = BTreeSet::new();
    let mut jumps = Vec::new();
    let mut identity_violations = 0usize;
    let mut max_abs_channel_delta: BTreeMap<String, f64> = BTreeMap::new();
    let mut max_abs_override_delta: BTreeMap<String, f64> = BTreeMap::new();
    let mut handoffs = Vec::new();

    for frame in frames {
        if let Some(id) = &frame.active_transition_id {
            transitions_seen.insert(id.clone());
        }
        if !frame.analysis.identity_ok {
            identity_violations += frame.analysis.identity_violations.len().max(1);
        }
        jumps.extend(frame.analysis.jumps.clone());
    }

    for i in 1..frames.len() {
        let prev = &frames[i - 1];
        let cur = &frames[i];
        for channel in &cur.motion_channels {
            if let Some(p) = prev.motion_channels.iter().find(|c| c.key == channel.key) {
                for (a, b) in channel.value.iter().zip(p.value.iter()) {
                    let d = (a - b).abs();
                    max_abs_channel_delta
                        .entry(channel.key.clone())
                        .and_modify(|m| *m = (*m).max(d))
                        .or_insert(d);
                }
            }
        }
        for (key, value) in &cur.values {
            if let Some(prev_v) = prev.values.get(key) {
                let mut a = Vec::new();
                let mut b = Vec::new();
                flatten_numbers(value, &mut a);
                flatten_numbers(prev_v, &mut b);
                for (x, y) in a.iter().zip(b.iter()) {
                    let d = (x - y).abs();
                    max_abs_override_delta
                        .entry(key.clone())
                        .and_modify(|m| *m = (*m).max(d))
                        .or_insert(d);
                }
            }
        }

        let state_changed = prev.current_state_id != cur.current_state_id;
        let transition_changed = prev.active_transition_id != cur.active_transition_id;
        if state_changed || (transition_changed && cur.active_transition_id.is_some()) {
            let mut continuity = Vec::new();
            for channel in &cur.motion_channels {
                if let Some(p) = prev.motion_channels.iter().find(|c| c.key == channel.key) {
                    let ok = channel
                        .value
                        .iter()
                        .zip(p.value.iter())
                        .all(|(a, b)| (a - b).abs() <= HANDOFF_EPSILON);
                    continuity.push(TraceChannelContinuity {
                        key: channel.key.clone(),
                        outgoing_p: p.value.clone(),
                        incoming_p: channel.value.clone(),
                        ok,
                    });
                }
            }
            if state_changed {
                handoffs.push(TraceHandoff {
                    frame_index: cur.frame_index,
                    from_state: prev.current_state_id.clone(),
                    to_state: cur.current_state_id.clone(),
                    transition_id: cur.active_transition_id.clone(),
                    channel_continuity: continuity,
                });
                // Also record state jump markers in summary jumps for visibility.
                jumps.push(TraceJump {
                    frame_index: cur.frame_index,
                    kind: "state".into(),
                    key: "currentStateId".into(),
                    component: None,
                    delta: 0.0,
                    from: 0.0,
                    to: 0.0,
                    label: cur.label.clone(),
                });
            }
            if transition_changed {
                jumps.push(TraceJump {
                    frame_index: cur.frame_index,
                    kind: "transition".into(),
                    key: "activeTransitionId".into(),
                    component: None,
                    delta: 0.0,
                    from: 0.0,
                    to: 0.0,
                    label: cur.label.clone(),
                });
            }
        }
    }

    let (final_state_id, final_transition_id) = frames
        .last()
        .map(|f| (f.current_state_id.clone(), f.active_transition_id.clone()))
        .unwrap_or_else(|| (String::new(), None));

    TraceSummary {
        frame_count: frames.len(),
        final_state_id,
        final_transition_id,
        transitions_seen: transitions_seen.into_iter().collect(),
        identity_violations,
        jumps,
        max_abs_channel_delta,
        max_abs_override_delta,
        handoffs,
    }
}

// ---------------------------------------------------------------------------
// Golden / AnimationTraceLog compatibility
// ---------------------------------------------------------------------------

/// Session-path generator used by `animation_values` goldens and CLI free-run.
///
/// One recorded frame per schedule sample. Events scheduled for a sample are
/// queued before `session.step(dt)` so edges share that sample's tick, matching
/// the app session semantics used by the previous test helper.
pub fn generate_animation_trace_log(
    scene: &SceneDSL,
    schedule: &TickSchedule,
    event_schedule: &[ScheduledEvent],
) -> Result<AnimationTraceLog> {
    let config = TraceRunConfig::free_run_meta(
        schedule.fps,
        None,
        KeyFilter::All,
        KeyFilter::All,
        true,
        TraceAnalyzeConfig::default(),
        None,
    );
    let result = run_schedule_trace(scene, schedule, event_schedule, &config)?;
    Ok(trace_report_to_animation_log(
        &result.report,
        schedule.start_secs,
        schedule.end_secs,
        schedule.fps,
        schedule.include_end,
    ))
}

/// Free-run / fixed-schedule execution with full v2 report + analysis.
pub fn run_schedule_trace(
    scene: &SceneDSL,
    schedule: &TickSchedule,
    event_schedule: &[ScheduledEvent],
    config: &TraceRunConfig,
) -> Result<TraceRunResult> {
    let mut session = AnimationSession::from_scene(scene)?
        .ok_or_else(|| anyhow!("scene has no stateMachine"))?;

    let mut routing_forced = config.routing_forced;
    if let Some(state_id) = &config.initial_state {
        session.force_state(state_id)?;
        routing_forced = true;
    }

    let tracked_keys: BTreeSet<String> = tracked_override_keys(session.runtime().definition());
    let mut current_values =
        build_initial_values(scene, &tracked_keys.iter().cloned().collect::<Vec<_>>());
    let resolved_override_keys = config.override_filter.resolved_keys(&tracked_keys)?;
    let resolved_override_set: BTreeSet<String> = resolved_override_keys.iter().cloned().collect();

    let mut available_channels: BTreeSet<String> = BTreeSet::new();
    let mut resolved_channel_keys: Option<BTreeSet<String>> = None;
    let mut frames: Vec<TraceReportFrame> = Vec::with_capacity(schedule.frame_count());
    let current_label: Option<String> = None;
    // Schedule owns absolute times; cumulative is only for record_step API.
    let mut _unused_cumulative = 0.0f64;

    for sample in schedule.samples() {
        for ev in event_schedule
            .iter()
            .filter(|e| e.frame_index == sample.frame_index)
        {
            session.fire_event(ev.to_fired_event());
        }
        let step = session.step(sample.dt_secs);
        // Override cumulative/time from schedule for golden-stable timestamps.
        let frame_index_before = frames.len();
        record_step(
            &step,
            sample.dt_secs,
            &mut _unused_cumulative,
            &current_label,
            &mut current_values,
            &tracked_keys,
            &resolved_override_set,
            config.include_values,
            &config.channel_filter,
            &mut available_channels,
            &mut resolved_channel_keys,
            &mut frames,
        )?;
        if let Some(frame) = frames.get_mut(frame_index_before) {
            frame.frame_index = sample.frame_index;
            frame.time_secs = round_f64(sample.time_secs);
            frame.dt_secs = round_f64(sample.dt_secs);
        }
    }

    finalize_report(
        scene,
        config,
        routing_forced,
        frames,
        available_channels,
        resolved_channel_keys,
        resolved_override_keys,
    )
}

fn trace_report_to_animation_log(
    report: &TraceReport,
    start_secs: f64,
    end_secs: f64,
    fps: u32,
    include_end: bool,
) -> AnimationTraceLog {
    let tracked_keys: Vec<String> = if report.frames.is_empty() {
        Vec::new()
    } else {
        report.frames[0].values.keys().cloned().collect()
    };
    let frames: Vec<AnimationTraceFrame> = report
        .frames
        .iter()
        .map(|f| AnimationTraceFrame {
            frame_index: f.frame_index,
            time_secs: f.time_secs,
            dt_secs: f.dt_secs,
            current_state_id: f.current_state_id.clone(),
            state_local_times: f.state_local_times.clone(),
            scene_time_secs: f.scene_time_secs,
            active_transition_id: f.active_transition_id.clone(),
            motion_channels: f.motion_channels.clone(),
            finished: f.finished,
            diagnostics: f.diagnostics.clone(),
            values: f.values.clone(),
        })
        .collect();
    AnimationTraceLog {
        schema_version: ANIMATION_TRACE_LOG_SCHEMA_VERSION,
        start_secs: round_f64(start_secs),
        end_secs: round_f64(end_secs),
        fps,
        include_end,
        frame_count: frames.len(),
        tracked_keys,
        frames,
    }
}

// ---------------------------------------------------------------------------
// Human output
// ---------------------------------------------------------------------------

pub fn format_summary(report: &TraceReport) -> String {
    let mut lines = Vec::new();
    lines.push(format!(
        "trace-animation: frames={} final_state={} transition={:?} routing={}",
        report.summary.frame_count,
        report.summary.final_state_id,
        report.summary.final_transition_id,
        report.config.routing_mode
    ));
    lines.push(format!(
        "  transitions_seen: {}",
        if report.summary.transitions_seen.is_empty() {
            "(none)".into()
        } else {
            report.summary.transitions_seen.join(", ")
        }
    ));
    lines.push(format!(
        "  identity_violations: {}",
        report.summary.identity_violations
    ));
    lines.push(format!("  jumps: {}", report.summary.jumps.len()));
    for handoff in &report.summary.handoffs {
        let cont: &str = if handoff.channel_continuity.is_empty() {
            "no-channels"
        } else if handoff.channel_continuity.iter().all(|c| c.ok) {
            "continuous"
        } else {
            "DISCONTINUOUS"
        };
        lines.push(format!(
            "  handoff frame {} {} -> {} ({:?}) [{cont}]",
            handoff.frame_index, handoff.from_state, handoff.to_state, handoff.transition_id
        ));
        for c in &handoff.channel_continuity {
            if !c.ok {
                lines.push(format!(
                    "    ! {} P {:?} -> {:?}",
                    c.key, c.outgoing_p, c.incoming_p
                ));
            }
        }
    }
    let channel_jumps: Vec<_> = report
        .summary
        .jumps
        .iter()
        .filter(|j| j.kind == "channel" || j.kind == "override")
        .take(20)
        .collect();
    for jump in channel_jumps {
        lines.push(format!(
            "  jump frame {} {} {}[{:?}] Δ={:.6} ({:.6} -> {:.6})",
            jump.frame_index,
            jump.kind,
            jump.key,
            jump.component,
            jump.delta,
            jump.from,
            jump.to
        ));
    }
    if report.summary.jumps.iter().filter(|j| j.kind == "channel" || j.kind == "override").count()
        > 20
    {
        lines.push("  ... (more jumps truncated)".into());
    }
    lines.join("\n")
}

pub fn format_table(report: &TraceReport, max_rows: usize) -> String {
    let channel_keys: Vec<String> = report
        .frames
        .iter()
        .flat_map(|f| f.motion_channels.iter().map(|c| c.key.clone()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    if channel_keys.is_empty() {
        return format_table_routing_only(report, max_rows);
    }

    let mut out = String::new();
    for key in channel_keys {
        out.push_str(&format!(
            "### channel {key}\n| frame | t_ms | state | transition | label | S | Q | E | P | V | mut | tr |\n|---:|---:|---|---|---|---:|---:|---:|---:|---:|---|---|\n"
        ));
        let mut rows = 0usize;
        for frame in &report.frames {
            if rows >= max_rows {
                out.push_str(&format!("| ... | | | | | | | | | | | | ({max_rows} row cap)\n"));
                break;
            }
            let Some(ch) = frame.motion_channels.iter().find(|c| c.key == key) else {
                continue;
            };
            let s = ch.state_value.first().copied().unwrap_or(f64::NAN);
            let q = ch.target_value.first().copied().unwrap_or(f64::NAN);
            let e = ch.transition_error.first().copied().unwrap_or(f64::NAN);
            let p = ch.value.first().copied().unwrap_or(f64::NAN);
            let v = ch.velocity.first().copied().unwrap_or(f64::NAN);
            out.push_str(&format!(
                "| {} | {:.0} | {} | {} | {} | {:.6} | {:.6} | {:.6} | {:.6} | {:.6} | {} | {} |\n",
                frame.frame_index,
                frame.time_secs * 1000.0,
                frame.current_state_id,
                frame
                    .active_transition_id
                    .as_deref()
                    .unwrap_or("none"),
                frame.label.as_deref().unwrap_or(""),
                s,
                q,
                e,
                p,
                v,
                ch.mutation_driver,
                ch.transition_driver,
            ));
            rows += 1;
        }
        out.push('\n');
    }
    out
}

fn format_table_routing_only(report: &TraceReport, max_rows: usize) -> String {
    let mut out = String::from(
        "| frame | t_ms | state | transition | label |\n|---:|---:|---|---|---|\n",
    );
    for (i, frame) in report.frames.iter().enumerate() {
        if i >= max_rows {
            out.push_str(&format!("| ... | | | | | ({max_rows} row cap)\n"));
            break;
        }
        out.push_str(&format!(
            "| {} | {:.0} | {} | {} | {} |\n",
            frame.frame_index,
            frame.time_secs * 1000.0,
            frame.current_state_id,
            frame
                .active_transition_id
                .as_deref()
                .unwrap_or("none"),
            frame.label.as_deref().unwrap_or(""),
        ));
    }
    out
}

pub fn write_report_json(report: &TraceReport, path: &Path, pretty: bool) -> Result<()> {
    let text = if pretty {
        serde_json::to_string_pretty(report)?
    } else {
        serde_json::to_string(report)?
    };
    if path == Path::new("-") {
        println!("{text}");
    } else {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        std::fs::write(path, text)?;
    }
    Ok(())
}

pub fn load_event_schedule(path: &Path) -> Result<Vec<ScheduledEvent>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("failed to read events {}: {e}", path.display()))?;
    let schedule: EventSchedule = serde_json::from_str(&text)
        .map_err(|e| anyhow!("failed to parse events {}: {e}", path.display()))?;
    Ok(schedule.events)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl;
    use crate::state_machine::types::{
        AnimationState, AnimationStateType, AnimationTransition, EventModifiers, GraphEndpoint,
        GraphPort, Position, StateMachine, StateMutationGraph, StateMutationGraphLayout,
        StateParamDeclaration, StateParamGraph, TransitionConditionBinding, TransitionMotionGraph,
        TransitionMotionNode,
    };
    use std::collections::HashMap;

    fn empty_mutation() -> StateMutationGraph {
        let target = GraphPort {
            id: "target_value".into(),
            name: Some("Target".into()),
            port_type: Some("float".into()),
            array_length: None,
            motion: None,
        };
        StateMutationGraph {
            inputs: vec![target.clone()],
            outputs: vec![target],
            nodes: vec![],
            connections: vec![],
            input_bindings: vec![],
            output_bindings: vec![],
            layout: StateMutationGraphLayout {
                parameter_positions: HashMap::new(),
                node_widths: HashMap::new(),
                runtime_input_position: Position::default(),
                output_position: Position::default(),
                runtime_input_collapsed: false,
                output_collapsed: false,
            },
            viewport: None,
        }
    }

    fn event_graph(id: &str, event_type: &str) -> TransitionMotionGraph {
        let mut graph = TransitionMotionGraph::instant(id);
        graph.nodes.push(TransitionMotionNode::EventTrigger {
            id: "trigger".into(),
            position: Position::default(),
            label: None,
            event_type: event_type.into(),
            key: None,
            modifiers: EventModifiers::default(),
            ignore_repeat: true,
        });
        graph.condition_binding = Some(TransitionConditionBinding::Node {
            from: GraphEndpoint {
                node_id: "trigger".into(),
                port_id: "fired".into(),
            },
        });
        graph
    }

    fn simple_scene() -> dsl::SceneDSL {
        dsl::SceneDSL {
            version: "5.0".into(),
            metadata: dsl::Metadata {
                name: "trace-run".into(),
                created: None,
                modified: None,
            },
            nodes: vec![dsl::Node {
                id: "Target".into(),
                node_type: "FloatInput".into(),
                params: [("value".into(), serde_json::json!(0.0))]
                    .into_iter()
                    .collect(),
                inputs: vec![],
                outputs: vec![],
                input_bindings: vec![],
                wgsl_override: None,
            }],
            connections: vec![],
            outputs: None,
            groups: vec![],
            assets: HashMap::new(),
            state_machine: Some(StateMachine {
                id: "sm".into(),
                name: "sm".into(),
                state_params: vec![StateParamDeclaration {
                    id: "target_value".into(),
                    name: "Target".into(),
                    param_type: "float".into(),
                    default_value: serde_json::json!(0.0),
                    array_length: None,
                }],
                state_param_graph: StateParamGraph {
                    declaration_positions: [("target_value".into(), Position::default())]
                        .into_iter()
                        .collect(),
                    ..Default::default()
                },
                node_widths: HashMap::new(),
                states: vec![
                    AnimationState {
                        id: "entry".into(),
                        name: "Entry".into(),
                        state_type: AnimationStateType::EntryState,
                        position: None,
                        state_param_overrides: HashMap::new(),
                        mutation_graph: None,
                        derivation_id: None,
                    },
                    AnimationState {
                        id: "any".into(),
                        name: "Any".into(),
                        state_type: AnimationStateType::AnyState,
                        position: None,
                        state_param_overrides: HashMap::new(),
                        mutation_graph: None,
                        derivation_id: None,
                    },
                    AnimationState {
                        id: "exit".into(),
                        name: "Exit".into(),
                        state_type: AnimationStateType::ExitState,
                        position: None,
                        state_param_overrides: HashMap::new(),
                        mutation_graph: None,
                        derivation_id: None,
                    },
                    AnimationState {
                        id: "a".into(),
                        name: "A".into(),
                        state_type: AnimationStateType::AnimationState,
                        position: None,
                        state_param_overrides: [("target_value".into(), serde_json::json!(1.0))]
                            .into_iter()
                            .collect(),
                        mutation_graph: Some(empty_mutation()),
                        derivation_id: None,
                    },
                    AnimationState {
                        id: "b".into(),
                        name: "B".into(),
                        state_type: AnimationStateType::AnimationState,
                        position: None,
                        state_param_overrides: [("target_value".into(), serde_json::json!(2.0))]
                            .into_iter()
                            .collect(),
                        mutation_graph: Some(empty_mutation()),
                        derivation_id: None,
                    },
                ],
                transitions: vec![
                    AnimationTransition {
                        id: "entry_to_a".into(),
                        source: "entry".into(),
                        target: "a".into(),
                        motion_graph_id: "mg_entry_a".into(),
                    },
                    AnimationTransition {
                        id: "a_to_b".into(),
                        source: "a".into(),
                        target: "b".into(),
                        motion_graph_id: "mg_a_b".into(),
                    },
                ],
                derivation_bindings: vec![],
                derivations: vec![],
                motion_graphs: vec![
                    TransitionMotionGraph::instant("mg_entry_a"),
                    event_graph("mg_a_b", "click"),
                ],
                initial_state_id: Some("entry".into()),
                viewport: None,
            }),
            debug_artifacts: None,
        }
    }

    #[test]
    fn scenario_step_and_event_runs() {
        let scene = simple_scene();
        let config = TraceRunConfig {
            fps: 60,
            scenario_name: Some("test".into()),
            scene_source: None,
            initial_state: None,
            actions: vec![
                ScenarioAction::Step {
                    frames: Some(0),
                    seconds: None,
                },
                ScenarioAction::Settle { max_frames: 30 },
                ScenarioAction::AssertState {
                    state_id: "a".into(),
                },
                ScenarioAction::Event {
                    event_type: "click".into(),
                    key: None,
                    button: None,
                    repeat: None,
                    modifiers: None,
                },
                ScenarioAction::Step {
                    frames: Some(1),
                    seconds: None,
                },
                ScenarioAction::AssertState {
                    state_id: "b".into(),
                },
            ],
            channel_filter: KeyFilter::All,
            override_filter: KeyFilter::None,
            include_values: false,
            analyze: TraceAnalyzeConfig::default(),
            routing_forced: false,
        };
        let result = run_trace(&scene, &config).expect("trace");
        assert!(
            result.assert_error.is_none(),
            "{:?}",
            result.assert_error
        );
        assert_eq!(result.report.summary.final_state_id, "b");
        assert!(result.report.summary.frame_count > 0);
    }

    #[test]
    fn identity_and_jump_analysis_on_synthetic_frames() {
        let mut frames = vec![
            TraceReportFrame {
                frame_index: 0,
                time_secs: 0.0,
                dt_secs: 0.0,
                label: None,
                current_state_id: "a".into(),
                active_transition_id: None,
                scene_time_secs: 0.0,
                state_local_times: BTreeMap::new(),
                finished: false,
                diagnostics: vec![],
                motion_channels: vec![MotionChannelDebug {
                    key: "x".into(),
                    driver: "spring".into(),
                    state_value: vec![1.0],
                    value: vec![0.5],
                    velocity: vec![0.0],
                    target_value: vec![1.0],
                    target_velocity: vec![0.0],
                    transition_error: vec![0.5],
                    transition_error_velocity: vec![0.0],
                    mutation_driver: "spring".into(),
                    transition_driver: "spring".into(),
                    timeline_progress: None,
                    blending_progress: None,
                    completed: false,
                }],
                values: BTreeMap::new(),
                analysis: TraceFrameAnalysis::default(),
            },
            TraceReportFrame {
                frame_index: 1,
                time_secs: 0.016,
                dt_secs: 0.016,
                label: None,
                current_state_id: "a".into(),
                active_transition_id: None,
                scene_time_secs: 0.016,
                state_local_times: BTreeMap::new(),
                finished: false,
                diagnostics: vec![],
                motion_channels: vec![MotionChannelDebug {
                    key: "x".into(),
                    driver: "spring".into(),
                    state_value: vec![1.0],
                    value: vec![0.9],
                    velocity: vec![0.0],
                    target_value: vec![1.0],
                    target_velocity: vec![0.0],
                    transition_error: vec![0.1],
                    transition_error_velocity: vec![0.0],
                    mutation_driver: "spring".into(),
                    transition_driver: "spring".into(),
                    timeline_progress: None,
                    blending_progress: None,
                    completed: false,
                }],
                values: BTreeMap::new(),
                analysis: TraceFrameAnalysis::default(),
            },
        ];
        let analyze = TraceAnalyzeConfig {
            jump_threshold_channel: 0.2,
            ..TraceAnalyzeConfig::default()
        };
        apply_analysis(&mut frames, &analyze);
        assert!(frames[0].analysis.identity_ok);
        assert!(frames[1].analysis.identity_ok);
        assert_eq!(frames[1].analysis.jumps.len(), 1);
        assert_eq!(frames[1].analysis.jumps[0].kind, "channel");
    }

    #[test]
    fn key_filter_csv() {
        assert_eq!(KeyFilter::from_cli_csv(Some("*")).unwrap(), KeyFilter::All);
        assert_eq!(
            KeyFilter::from_cli_csv(Some("none")).unwrap(),
            KeyFilter::None
        );
        let only = KeyFilter::from_cli_csv(Some("a, b")).unwrap();
        assert!(matches!(only, KeyFilter::Only(_)));
    }

    #[test]
    fn scenario_json_roundtrip_actions() {
        let json = r#"{
          "schemaVersion": 1,
          "name": "demo",
          "fps": 60,
          "actions": [
            { "type": "step", "frames": 0 },
            { "type": "event", "eventType": "keyup", "key": " " },
            { "type": "settle", "maxFrames": 10 },
            { "type": "assertState", "stateId": "st_thinking" }
          ]
        }"#;
        let scenario: TraceScenario = serde_json::from_str(json).unwrap();
        assert_eq!(scenario.actions.len(), 4);
    }
}
