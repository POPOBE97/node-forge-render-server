//! Integration coverage for the shared `--trace-animation` library path.

use std::path::PathBuf;

use node_forge_render_server::animation::{TraceScenario, format_summary, run_trace};
use node_forge_render_server::asset_store;

const POSITIONS_OVERRIDE: &str = "Vector2ArrayInput_IntelligentLightPositions:value";

fn doubao_scene() -> node_forge_render_server::dsl::SceneDSL {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/render/editor-examples/doubao-voice-interaction/scene.nforge");
    asset_store::load_from_nforge(&path)
        .expect("load doubao fixture")
        .0
}

fn max_positions_frame_delta(
    frames: &[node_forge_render_server::animation::TraceReportFrame],
) -> f64 {
    let mut max_delta: f64 = 0.0;
    let mut previous: Option<&serde_json::Value> = None;
    for frame in frames {
        let Some(positions) = frame.values.get(POSITIONS_OVERRIDE) else {
            continue;
        };
        if let Some(prev) = previous {
            let prev_rows = prev.as_array().expect("positions array");
            let next_rows = positions.as_array().expect("positions array");
            for (prev_row, next_row) in prev_rows.iter().zip(next_rows.iter()) {
                let prev_xy = prev_row.as_array().expect("vec2");
                let next_xy = next_row.as_array().expect("vec2");
                for (a, b) in prev_xy.iter().zip(next_xy.iter()) {
                    let delta = (a.as_f64().unwrap_or(0.0) - b.as_f64().unwrap_or(0.0)).abs();
                    max_delta = max_delta.max(delta);
                }
            }
        }
        previous = Some(positions);
    }
    max_delta
}

#[test]
fn doubao_listening_to_thinking_scenario_handoff_is_continuous() {
    let scenario_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios/doubao-listening-to-thinking.json");
    let scenario = TraceScenario::from_path(&scenario_path).expect("parse scenario");
    let config = scenario.into_run_config(Some("doubao-voice-interaction".into()));
    let result = run_trace(&doubao_scene(), &config).expect("run trace");

    assert!(
        result.assert_error.is_none(),
        "assert failed: {:?}",
        result.assert_error
    );
    assert_eq!(result.report.summary.final_state_id, "st_thinking");
    assert!(
        result
            .report
            .summary
            .transitions_seen
            .iter()
            .any(|id| id == "tr_listening_to_thinking"),
        "expected listening→thinking transition, summary={}",
        format_summary(&result.report)
    );

    let thinking_handoffs: Vec<_> = result
        .report
        .summary
        .handoffs
        .iter()
        .filter(|h| h.to_state == "st_thinking")
        .collect();
    assert!(
        !thinking_handoffs.is_empty(),
        "expected a handoff into st_thinking\n{}",
        format_summary(&result.report)
    );
    for handoff in thinking_handoffs {
        for channel in &handoff.channel_continuity {
            assert!(
                channel.ok,
                "handoff discontinuity on {}: {:?} -> {:?}\n{}",
                channel.key,
                channel.outgoing_p,
                channel.incoming_p,
                format_summary(&result.report)
            );
        }
    }

    assert_eq!(
        result.report.summary.identity_violations,
        0,
        "P=Q-E violations\n{}",
        format_summary(&result.report)
    );
}

/// Pin thinking: IntelligentLightPositions must not period-flip (~4–11px) from
/// discrete snap convergence or unwrapNear against a moving orbit center.
/// Mild ~1px deltas from rounded-rect corners while orbiting are allowed.
#[test]
fn doubao_pinned_thinking_positions_have_no_snap_period_jumps() {
    let scenario_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenarios/doubao-thinking-free-run.json");
    let scenario = TraceScenario::from_path(&scenario_path).expect("parse scenario");
    let config = scenario.into_run_config(Some("doubao-voice-interaction".into()));
    let result = run_trace(&doubao_scene(), &config).expect("run trace");

    assert!(
        result.assert_error.is_none(),
        "assert failed: {:?}",
        result.assert_error
    );
    assert_eq!(result.report.summary.final_state_id, "st_thinking");
    assert_eq!(result.report.config.routing_mode, "forced");

    let thinking_frames: Vec<_> = result
        .report
        .frames
        .iter()
        .filter(|frame| frame.current_state_id == "st_thinking")
        .cloned()
        .collect();
    assert!(
        thinking_frames.len() > 30,
        "expected a multi-frame pin thinking run"
    );

    let max_delta = max_positions_frame_delta(&thinking_frames);
    assert!(
        max_delta < 2.0,
        "pin thinking IntelligentLightPositions jumped {max_delta:.3}px between frames \
         (period-flip / step-converge regression); summary:\n{}",
        format_summary(&result.report)
    );
}

#[test]
fn doubao_scale_four_trace_uses_the_shader_graph_scene_size() {
    let mut scene = doubao_scene();
    scene
        .nodes
        .iter_mut()
        .find(|node| node.id == "FloatInput_RenderPxPerDp")
        .expect("RenderPxPerDp root input")
        .params
        .insert("value".into(), serde_json::json!(4.0));
    let scenario: TraceScenario = serde_json::from_value(serde_json::json!({
        "schemaVersion": 1,
        "name": "doubao-scale-four-ptt",
        "fps": 60,
        "track": {
            "channels": ["sp_ptt_object_scene_px"],
            "overrides": []
        },
        "actions": [
            { "type": "forceState", "stateId": "st_push_to_talk" },
            { "type": "mouse", "x": 720.0, "y": 3200.0 / 24.0 },
            { "type": "settle", "maxFrames": 360 }
        ]
    }))
    .expect("scale-four trace scenario");
    let result = run_trace(
        &scene,
        &scenario.into_run_config(Some("doubao-scale-four".into())),
    )
    .expect("run scale-four trace");
    let channel = result
        .report
        .frames
        .last()
        .and_then(|frame| {
            frame
                .motion_channels
                .iter()
                .find(|channel| channel.key == "sp_ptt_object_scene_px")
        })
        .expect("final PTT Motion channel");
    assert!((channel.target_value[0] - 720.0).abs() <= 1.0e-8);
    assert!((channel.target_value[1] - 3200.0 / 24.0).abs() <= 1.0e-8);
}
