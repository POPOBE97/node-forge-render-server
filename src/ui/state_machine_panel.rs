use rust_wgpu_fiber::eframe::egui;

use super::components::two_column_section;
use crate::animation::AnimationSession;

/// 状态机面板所需的快照数据，每帧从 AnimationSession 提取。
/// 使用独立结构体避免在 UI 层直接持有 runtime 引用。
#[derive(Debug, Clone, Default)]
pub struct StateMachineSnapshot {
    /// 状态机名称 (来自 StateMachine.name)
    pub name: String,
    /// 状态机 id
    pub id: String,
    /// 当前活跃状态 id
    pub current_state_id: String,
    /// 当前活跃状态名称（用于显示）
    pub current_state_name: String,
    /// 是否已结束（到达 exit state）
    pub finished: bool,
    /// 场景累计时间（秒）
    pub scene_time_secs: f64,
    /// 活跃转场 id（如果正在转场）
    pub active_transition_id: Option<String>,
    /// 转场混合因子 0.0 → 1.0
    pub transition_blend: Option<f64>,
    /// 转场源状态名称
    pub transition_source_name: Option<String>,
    /// 转场目标状态名称
    pub transition_target_name: Option<String>,
    /// 所有状态的定义信息（id, name, type）
    pub states: Vec<StateInfo>,
    /// 各状态本地经过时间
    pub state_local_times: Vec<(String, f64)>,
    /// 当前活跃的动画 override 值
    pub override_values: Vec<(String, String)>,
}

/// 单个状态的摘要信息
#[derive(Debug, Clone)]
pub struct StateInfo {
    pub id: String,
    pub name: String,
    pub state_type: String,
    pub is_current: bool,
}

/// 将 f64 值格式化为两位小数字符串。
/// NaN 返回 `"NaN"`，无穷大返回 `"Inf"`。
pub fn format_f64_2dp(v: f64) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return "Inf".to_string();
    }
    format!("{:.2}", v)
}

/// 将 serde_json::Value 格式化为两位小数的显示字符串。
/// 数值类型格式化为两位小数，数组逐元素格式化，布尔/字符串直接输出。
pub fn format_json_value_2dp(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Number(n) => format_f64_2dp(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::Array(arr) => {
            let parts: Vec<String> = arr
                .iter()
                .map(|v| {
                    v.as_f64()
                        .map(format_f64_2dp)
                        .unwrap_or_else(|| v.to_string())
                })
                .collect();
            format!("[{}]", parts.join(", "))
        }
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Build a `StateMachineSnapshot` from a live `AnimationSession`.
///
/// Extracts the current runtime state (current state, transitions, etc.)
/// from the session's `StateMachineRuntime` and its definition.
///
/// Fields that come from the per-frame `AnimationStep` (state_local_times,
/// transition_blend, override_values) are left at their defaults — the
/// caller is expected to fill those in from the most recent step result.
pub fn snapshot_from_session(session: &AnimationSession) -> StateMachineSnapshot {
    let runtime = session.runtime();
    let def = runtime.definition();
    let current_id = runtime.current_state_id();

    // Build state info list from the definition.
    let states: Vec<StateInfo> = def
        .states
        .iter()
        .map(|s| StateInfo {
            id: s.id.clone(),
            name: s.name.clone(),
            state_type: format!("{:?}", s.resolved_type()),
            is_current: s.id == current_id,
        })
        .collect();

    // Resolve current state name (fall back to raw id).
    let current_state_name = def
        .states
        .iter()
        .find(|s| s.id == current_id)
        .map(|s| s.name.clone())
        .unwrap_or_else(|| current_id.to_string());

    // Resolve transition source/target names when a transition is active.
    let (transition_source_name, transition_target_name) = runtime
        .active_transition_id()
        .and_then(|tid| def.transitions.iter().find(|t| t.id == tid))
        .map(|t| {
            let src = def
                .states
                .iter()
                .find(|s| s.id == t.source)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| t.source.clone());
            let tgt = def
                .states
                .iter()
                .find(|s| s.id == t.target)
                .map(|s| s.name.clone())
                .unwrap_or_else(|| t.target.clone());
            (Some(src), Some(tgt))
        })
        .unwrap_or((None, None));

    StateMachineSnapshot {
        name: def.name.clone(),
        id: def.id.clone(),
        current_state_id: current_id.to_string(),
        current_state_name,
        finished: runtime.finished,
        scene_time_secs: session.scene_time(),
        active_transition_id: runtime.active_transition_id().map(str::to_string),
        transition_blend: None,
        transition_source_name,
        transition_target_name,
        states,
        state_local_times: Vec::new(),
        override_values: Vec::new(),
    }
}

/// Render the State Machine debug section.
///
/// Pure display function — reads from `snapshot` and draws four collapsible
/// sub-trees (Status, Transition, States, Values) without side effects.
pub fn show_state_machine_section(ui: &mut egui::Ui, snapshot: &StateMachineSnapshot) {
    two_column_section::section(ui, "State Machine", |ui| {
        // ── Status sub-tree ──
        egui::CollapsingHeader::new("Status")
            .default_open(true)
            .show(ui, |ui| {
                label_value(ui, "Name", &snapshot.name);
                label_value(ui, "Current State", &snapshot.current_state_name);
                label_value(ui, "Scene Time", &format_f64_2dp(snapshot.scene_time_secs));
                label_value(ui, "Finished", &snapshot.finished.to_string());
            });

        // ── Transition sub-tree (only when a transition is active) ──
        if snapshot.active_transition_id.is_some() {
            egui::CollapsingHeader::new("Transition")
                .default_open(true)
                .show(ui, |ui| {
                    if let Some(ref src) = snapshot.transition_source_name {
                        label_value(ui, "From", src);
                    }
                    if let Some(ref tgt) = snapshot.transition_target_name {
                        label_value(ui, "To", tgt);
                    }
                    if let Some(blend) = snapshot.transition_blend {
                        label_value(ui, "Blend", &format_f64_2dp(blend));
                    }
                });
        }

        // ── States sub-tree ──
        egui::CollapsingHeader::new("States")
            .default_open(false)
            .show(ui, |ui| {
                for (state_id, local_time) in &snapshot.state_local_times {
                    let info = snapshot.states.iter().find(|s| s.id == *state_id);
                    let label = info.map(|s| s.name.as_str()).unwrap_or(state_id.as_str());
                    let marker = if info.map(|s| s.is_current).unwrap_or(false) {
                        " ●"
                    } else {
                        ""
                    };
                    label_value(
                        ui,
                        &format!("{label}{marker}"),
                        &format_f64_2dp(*local_time),
                    );
                }
            });

        // ── Values sub-tree ──
        egui::CollapsingHeader::new("Values")
            .default_open(true)
            .show(ui, |ui| {
                if snapshot.override_values.is_empty() {
                    ui.label("(no active overrides)");
                } else {
                    for (key, value) in &snapshot.override_values {
                        label_value(ui, key, value);
                    }
                }
            });
    });
}

/// Horizontal row: left-aligned label, right-aligned monospace value.
fn label_value(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.monospace(value);
        });
    });
}
