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

fn lalaland_scene() -> node_forge_render_server::dsl::SceneDSL {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/render/editor-examples/doubao-lala-land/scene.nforge");
    asset_store::load_from_nforge(&path)
        .expect("load Lalaland fixture")
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
fn lalaland_outer_resolve_uses_safe_area_and_state_driven_shared_effect() {
    let scenario_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios/doubao-lalaland-blur-waypoint.json");
    let scenario = TraceScenario::from_path(&scenario_path).expect("parse Lalaland scenario");
    let scene = lalaland_scene();
    let state_param_ids = scene
        .state_machine
        .as_ref()
        .expect("Lalaland state machine")
        .state_params
        .iter()
        .map(|param| param.id.as_str())
        .collect::<Vec<_>>();
    assert!(state_param_ids.contains(&"sp_content_scale"));
    for id in [
        "sp_effect_mid_color",
        "sp_effect_outer_color",
        "sp_effect_medium_gain",
        "sp_effect_outer_sigma_dp",
        "sp_effect_outer_gain",
        "sp_effect_mid_sigma_dp",
        "sp_effect_mid_gain",
        "sp_effect_glow_gain",
        "sp_effect_icon_softness",
        "sp_effect_icon_gain",
        "sp_effect_absorption_gain",
        "sp_effect_medium_outer_occlusion",
        "sp_effect_medium_mid_occlusion",
        "sp_effect_outer_mid_occlusion",
        "sp_effect_outer_medium_occlusion",
        "sp_effect_mid_medium_occlusion",
    ] {
        assert!(
            state_param_ids.contains(&id),
            "Shared Effect material property {id} is not owned by the State Machine"
        );
    }
    assert!(!state_param_ids.contains(&"param_msrh5zs8_4"));
    assert!(!state_param_ids.contains(&"sp_supercharge_scale"));
    let effect_material = scene
        .nodes
        .iter()
        .find(|node| node.id == "ChargingEffectMaterial")
        .expect("Shared Effect ShaderMaterial");
    for (port_id, source_node_id) in [
        ("param:mid_color", "EffectMidColor"),
        ("param:outer_color", "EffectOuterColor"),
        ("param:medium_gain", "EffectMediumGain"),
        ("param:outer_sigma_dp", "EffectOuterSigmaDp"),
        ("param:outer_gain", "EffectOuterGain"),
        ("param:mid_sigma_dp", "EffectMidSigmaDp"),
        ("param:mid_gain", "EffectMidGain"),
        ("param:glow_gain", "EffectGlowGain"),
        ("param:icon_softness", "EffectIconSoftness"),
        ("param:icon_gain", "EffectIconGain"),
        ("param:absorption_gain", "EffectAbsorptionGain"),
        ("param:medium_outer_occlusion", "EffectMediumOuterOcclusion"),
        ("param:medium_mid_occlusion", "EffectMediumMidOcclusion"),
        ("param:outer_mid_occlusion", "EffectOuterMidOcclusion"),
        ("param:outer_medium_occlusion", "EffectOuterMediumOcclusion"),
        ("param:mid_medium_occlusion", "EffectMidMediumOcclusion"),
    ] {
        let binding = effect_material
            .input_bindings
            .iter()
            .find(|binding| binding.port_id == port_id)
            .unwrap_or_else(|| panic!("Shared Effect input {port_id} is not bound"));
        assert_eq!(
            binding
                .source_binding
                .as_ref()
                .unwrap_or_else(|| panic!("Shared Effect input {port_id} has no source"))
                .node_id,
            source_node_id
        );
    }
    for id in [
        "RenderTexture_8e46d6bc_9",
        "ChargingIslandRT",
        "SuperchargeIslandRT",
    ] {
        let target = scene
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("missing safe-area render target {id}"));
        assert_eq!(target.params["width"], serde_json::json!(618));
        assert_eq!(target.params["height"], serde_json::json!(184));
    }
    let result = run_trace(
        &scene,
        &scenario.into_run_config(Some("doubao-lala-land".into())),
    )
    .expect("run Lalaland trace");

    assert_eq!(result.report.summary.identity_violations, 0);
    assert_eq!(
        result.report.summary.transitions_seen,
        vec![
            "tr_msx1vaks_p".to_string(),
            "tr_msx1vbpw_s".to_string(),
            "tr_msx1vd4o_v".to_string(),
        ]
    );
    for (label, waypoint, final_target) in [
        ("to-collapsed", 64.0, 64.0),
        ("to-charging", 32.0, 0.0),
        ("to-supercharge", 32.0, 0.0),
        ("back-to-charging", 32.0, 0.0),
        ("back-to-collapsed", 64.0, 64.0),
    ] {
        let values = result
            .report
            .frames
            .iter()
            .filter(|frame| frame.label.as_deref() == Some(label))
            .filter_map(|frame| {
                frame
                    .motion_channels
                    .iter()
                    .find(|channel| channel.key == "param_msrhaqn8_6")
                    .and_then(|channel| channel.value.first().copied())
            })
            .collect::<Vec<_>>();
        assert!(!values.is_empty(), "missing Blur trace frames for {label}");
        let waypoint_distance = values
            .iter()
            .map(|value| (value - waypoint).abs())
            .fold(f64::INFINITY, f64::min);
        assert!(
            waypoint_distance <= 1.0,
            "{label} did not pass through authored waypoint {waypoint}; closest distance was {waypoint_distance}"
        );
        assert!(
            (values.last().copied().unwrap_or(f64::NAN) - final_target).abs() <= 1.0e-6,
            "{label} did not settle at {final_target}"
        );
    }

    for (label, final_target) in [
        ("to-collapsed", 0.5),
        ("to-charging", 1.0),
        ("to-supercharge", 1.0),
        ("back-to-charging", 1.0),
        ("back-to-collapsed", 0.5),
    ] {
        let values = result
            .report
            .frames
            .iter()
            .filter(|frame| frame.label.as_deref() == Some(label))
            .filter_map(|frame| {
                frame
                    .motion_channels
                    .iter()
                    .find(|channel| channel.key == "sp_content_scale")
                    .and_then(|channel| channel.value.first().copied())
            })
            .collect::<Vec<_>>();
        assert!(
            !values.is_empty(),
            "missing Content Scale trace frames for {label}"
        );
        assert!(
            (values.last().copied().unwrap_or(f64::NAN) - final_target).abs() <= 1.0e-6,
            "{label} Content Scale did not settle at {final_target}"
        );
        if matches!(label, "to-supercharge" | "back-to-charging") {
            assert!(
                values.iter().all(|value| (value - 1.0).abs() <= 1.0e-9),
                "{label} unexpectedly animated the shared Content Scale"
            );
        }
    }

    for frame in &result.report.frames {
        if frame.label.is_some() {
            let channel_value = |key: &str| {
                frame
                    .motion_channels
                    .iter()
                    .find(|channel| channel.key == key)
                    .and_then(|channel| channel.value.first().copied())
                    .unwrap_or_else(|| {
                        panic!("missing channel {key} at frame {}", frame.frame_index)
                    })
            };
            let override_value = |key: &str| {
                frame
                    .values
                    .get(key)
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or_else(|| panic!("missing {key} at frame {}", frame.frame_index))
            };
            let leading_center_x = 8.0 + channel_value("sp_leading_center_x_dp") * 3.5;
            let trailing_center_x = 8.0 + channel_value("sp_trailing_center_x_dp") * 3.5;
            let effect_center_x = 309.0 + channel_value("sp_effect_offset_dp") * 3.5;
            for key in ["LeftContentCenterPx:x", "SuperchargeLeftContentCenterPx:x"] {
                assert!(
                    (override_value(key) - leading_center_x).abs() <= 1.0e-5,
                    "{key} was not derived from its independently interpolated State Param at frame {}",
                    frame.frame_index
                );
            }
            for key in [
                "RightContentCenterPx:x",
                "SuperchargeRightContentCenterPx:x",
            ] {
                assert!(
                    (override_value(key) - trailing_center_x).abs() <= 1.0e-5,
                    "{key} was not derived from its independently interpolated State Param at frame {}",
                    frame.frame_index
                );
            }
            assert!(
                (override_value("EffectPlacementCenterPx:x") - effect_center_x).abs() <= 1.0e-5,
                "Shared Effect position was not derived from its independently interpolated State Param at frame {}",
                frame.frame_index
            );
            assert!(
                (override_value("EffectPlacementCenterPx:y") - 92.0).abs() <= 1.0e-9,
                "safe-area inset did not preserve the Shared Effect's centered LocalPx position at frame {}",
                frame.frame_index
            );

            for (channel_key, uniform_key) in [
                ("param_charging_glow_color", "ChargingGlowColor:value"),
                ("sp_effect_mid_color", "EffectMidColor:value"),
                ("sp_effect_outer_sigma_dp", "EffectOuterSigmaDp:value"),
                ("sp_effect_glow_gain", "EffectGlowGain:value"),
                (
                    "sp_effect_medium_outer_occlusion",
                    "EffectMediumOuterOcclusion:value",
                ),
                (
                    "sp_effect_outer_medium_occlusion",
                    "EffectOuterMediumOcclusion:value",
                ),
            ] {
                let channel = frame
                    .motion_channels
                    .iter()
                    .find(|channel| channel.key == channel_key)
                    .unwrap_or_else(|| {
                        panic!(
                            "missing channel {channel_key} at frame {}",
                            frame.frame_index
                        )
                    });
                let uniform = frame.values.get(uniform_key).unwrap_or_else(|| {
                    panic!("missing {uniform_key} at frame {}", frame.frame_index)
                });
                let uniform_values = match uniform {
                    serde_json::Value::Array(values) => values
                        .iter()
                        .map(|value| value.as_f64().expect("numeric color component"))
                        .collect::<Vec<_>>(),
                    serde_json::Value::Number(value) => {
                        vec![value.as_f64().expect("numeric material value")]
                    }
                    _ => panic!(
                        "unexpected {uniform_key} value at frame {}: {uniform}",
                        frame.frame_index
                    ),
                };
                assert_eq!(channel.value.len(), uniform_values.len());
                for (actual, expected) in channel.value.iter().zip(uniform_values) {
                    assert!(
                        (actual - expected).abs() <= 1.0e-5,
                        "{uniform_key} bypassed independently interpolated {channel_key} at frame {}",
                        frame.frame_index
                    );
                }
            }
        }

        for (key, expected) in [
            ("SideTextureSizePx:x", 224.0),
            ("SideTextureSizePx:y", 112.0),
            ("SuperchargeSideTextureSizePx:x", 266.0),
            ("SuperchargeSideTextureSizePx:y", 112.0),
        ] {
            let actual = frame
                .values
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .unwrap_or_else(|| panic!("missing {key} at frame {}", frame.frame_index));
            assert!(
                (actual - expected).abs() <= 1.0e-9,
                "{key} was scaled before Outer Island Resolve at frame {}: {actual}",
                frame.frame_index
            );
        }
    }

    let max_transition_width_px = result
        .report
        .frames
        .iter()
        .filter(|frame| frame.label.is_some())
        .filter_map(|frame| frame.values.get("IslandSizePx:x"))
        .filter_map(serde_json::Value::as_f64)
        .fold(f64::NEG_INFINITY, f64::max);
    assert!(
        max_transition_width_px > 602.0,
        "the fixture no longer exercises the spring overshoot that the safe area protects"
    );
    assert!(
        max_transition_width_px <= 618.0,
        "spring overshoot escaped the 16px-expanded state-island RT: {max_transition_width_px}px"
    );

    for (label, expected_center_x) in [
        ("to-collapsed", 505.0),
        ("to-charging", 505.0),
        ("to-supercharge", 547.0),
        ("back-to-charging", 505.0),
        ("back-to-collapsed", 505.0),
    ] {
        let last = result
            .report
            .frames
            .iter()
            .filter(|frame| frame.label.as_deref() == Some(label))
            .last()
            .unwrap_or_else(|| panic!("missing Shared Effect frames for {label}"));
        let actual = last.values["EffectPlacementCenterPx:x"]
            .as_f64()
            .expect("Shared Effect center x");
        assert!(
            (actual - expected_center_x).abs() <= 1.0e-6,
            "{label} Shared Effect center did not settle at {expected_center_x}: {actual}"
        );
    }

    for (label, expected_left, expected_right) in [
        ("to-supercharge", 141.0, 477.0),
        ("back-to-charging", 162.0, 456.0),
    ] {
        let frames = result
            .report
            .frames
            .iter()
            .filter(|frame| frame.label.as_deref() == Some(label))
            .collect::<Vec<_>>();
        assert!(
            !frames.is_empty(),
            "missing content-center frames for {label}"
        );
        for frame in &frames {
            let value = |key: &str| {
                frame
                    .values
                    .get(key)
                    .and_then(serde_json::Value::as_f64)
                    .unwrap_or_else(|| panic!("missing {key} at frame {}", frame.frame_index))
            };
            let charging_left = value("LeftContentCenterPx:x");
            let charging_right = value("RightContentCenterPx:x");
            let supercharge_left = value("SuperchargeLeftContentCenterPx:x");
            let supercharge_right = value("SuperchargeRightContentCenterPx:x");
            assert!(
                (charging_left - supercharge_left).abs() <= 1.0e-9,
                "{label} left centers diverged at frame {}: {charging_left} vs {supercharge_left}",
                frame.frame_index
            );
            assert!(
                (charging_right - supercharge_right).abs() <= 1.0e-9,
                "{label} right centers diverged at frame {}: {charging_right} vs {supercharge_right}",
                frame.frame_index
            );
        }
        let last = frames.last().expect("checked non-empty frames");
        assert!(
            (last.values["LeftContentCenterPx:x"].as_f64().unwrap() - expected_left).abs()
                <= 1.0e-6,
            "{label} left center did not settle at {expected_left}"
        );
        assert!(
            (last.values["RightContentCenterPx:x"].as_f64().unwrap() - expected_right).abs()
                <= 1.0e-6,
            "{label} right center did not settle at {expected_right}"
        );
    }
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
