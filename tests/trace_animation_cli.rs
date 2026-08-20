//! Integration coverage for the shared `--trace-animation` library path.

use std::{path::PathBuf, sync::Mutex};

use node_forge_render_server::animation::{TraceScenario, format_summary, run_trace};
use node_forge_render_server::asset_store;
use node_forge_render_server::state_machine::types::{GraphInnerNodeType, StateValueSource};

const POSITIONS_OVERRIDE: &str = "Vector2ArrayInput_IntelligentLightPositions:value";
static FUNCTION_REGISTRY_TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_function_registry() -> std::sync::MutexGuard<'static, ()> {
    FUNCTION_REGISTRY_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

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
    let _function_registry_guard = lock_function_registry();
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
    let _function_registry_guard = lock_function_registry();
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
fn lalaland_wave_radius_is_motion_and_effect_cycle_is_derived() {
    let _function_registry_guard = lock_function_registry();
    let scene = lalaland_scene();
    let state_machine = scene
        .state_machine
        .as_ref()
        .expect("Lalaland state machine");
    let state_param_ids = state_machine
        .state_params
        .iter()
        .map(|param| param.id.as_str())
        .collect::<Vec<_>>();
    assert!(
        state_param_ids.contains(&"sp_effect_wave_radius_dp"),
        "wave radius must be an Animation Engine State Param"
    );
    assert!(
        state_param_ids.contains(&"sp_effect_pulse_gain"),
        "pulse gain must be an Animation Engine State Param"
    );
    for id in [
        "sp_effect_wave_width_dp",
        "sp_effect_wave_alpha",
        "sp_effect_wave_dispersion",
        "sp_effect_outer_mid_occlusion",
    ] {
        assert!(
            !state_param_ids.contains(&id),
            "derived or constant render property {id} must not allocate a State Param"
        );
    }
    for (state_id, node_id) in [
        ("island", "effect_cycle_island"),
        ("supercharge", "mutation_fn_mszxci1f_4"),
        ("st_msybtf2o_m", "effect_cycle_fastcharge"),
    ] {
        let state = state_machine
            .states
            .iter()
            .find(|state| state.id == state_id)
            .unwrap_or_else(|| panic!("missing state {state_id}"));
        let graph = state
            .mutation_graph
            .as_ref()
            .unwrap_or_else(|| panic!("missing mutation graph for {state_id}"));
        let node = graph
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .unwrap_or_else(|| panic!("missing wave radius Mutation Function in {state_id}"));
        assert_eq!(node.node_type, GraphInnerNodeType::MutationFunction);
        assert_eq!(
            node.outputs
                .iter()
                .filter(|port| port.motion == Some(true))
                .map(|port| port.id.as_str())
                .collect::<Vec<_>>(),
            vec!["pulseGain", "waveRadiusDp"]
        );
        assert!(graph.input_bindings.iter().any(|binding| {
            matches!(
                &binding.source,
                StateValueSource::FrameInput { frame_input_id }
                    if frame_input_id == "localElapsedTime"
            ) && binding.to.node_id == node_id
                && binding.to.port_id == "localElapsedTime"
        }));
        assert!(graph.input_bindings.iter().any(|binding| {
            binding.source.state_param_id() == Some("sp_effect_wave_travel_dp")
                && binding.to.node_id == node_id
                && binding.to.port_id == "waveTravelDp"
        }));
        assert!(graph.output_bindings.iter().any(|binding| {
            binding.state_param_id == "sp_effect_wave_radius_dp"
                && binding.from.node_id == node_id
                && binding.from.port_id == "waveRadiusDp"
        }));
        assert!(graph.output_bindings.iter().any(|binding| {
            binding.state_param_id == "sp_effect_pulse_gain"
                && binding.from.node_id == node_id
                && binding.from.port_id == "pulseGain"
        }));
    }
    let island_layout = state_machine
        .derivations
        .iter()
        .find(|derivation| derivation.id == "island_layout")
        .expect("Island Layout Derivation");
    assert!(island_layout.input_bindings.iter().any(|binding| {
        binding.source.state_param_id() == Some("sp_effect_wave_radius_dp")
            && binding.to.node_id == "derive_island_layout"
            && binding.to.port_id == "waveRadiusDp"
    }));
    assert!(
        island_layout.input_bindings.iter().all(|binding| {
            !matches!(
                &binding.source,
                StateValueSource::FrameInput { frame_input_id }
                    if frame_input_id == "sceneElapsedTime"
            )
        }),
        "Effect Cycle Derivation must derive phase from physical wave radius, not frame time"
    );
    for (port_id, uniform_node_id) in [
        ("waveRadiusDp", "EffectWaveRadiusDp"),
        ("waveWidthDp", "EffectWaveWidthDp"),
        ("waveAlpha", "EffectWaveAlpha"),
        ("waveDispersion", "EffectWaveDispersion"),
    ] {
        assert!(
            island_layout.output_bindings.iter().any(|binding| {
                binding.from.node_id == "derive_island_layout"
                    && binding.from.port_id == port_id
                    && binding.uniform.node_id == uniform_node_id
                    && binding.uniform.param_id == "value"
            }),
            "Island Layout Derivation output {port_id} is not bound to {uniform_node_id}.value"
        );
    }
    assert!(island_layout.passthrough_bindings.iter().any(|binding| {
        binding.source.state_param_id() == Some("sp_effect_pulse_gain")
            && binding.uniform.node_id == "EffectPulseGain"
            && binding.uniform.param_id == "value"
    }));
    let outer_mid_occlusion = scene
        .nodes
        .iter()
        .find(|node| node.id == "EffectOuterMidOcclusion")
        .expect("constant Effect Outer by Mid input");
    assert_eq!(
        outer_mid_occlusion.params["value"],
        serde_json::json!(1),
        "constant outer-by-mid occlusion should remain a root render parameter"
    );
}

#[test]
fn lalaland_v2_phase_holds_then_repeats_in_motion_engine() {
    let _function_registry_guard = lock_function_registry();
    let scenario_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("scenarios/doubao-lalaland-v2-phase.json");
    let scenario = TraceScenario::from_path(&scenario_path).expect("parse v2 phase scenario");
    let result = run_trace(
        &lalaland_scene(),
        &scenario.into_run_config(Some("doubao-lala-land-v2-phase".into())),
    )
    .expect("run v2 phase trace");

    assert_eq!(result.report.summary.identity_violations, 0);
    for state_id in ["st_mt1ajfjb_p", "st_mt1ajfjb_r", "st_mt1ajfjb_s"] {
        let samples = result
            .report
            .frames
            .iter()
            .filter(|frame| frame.current_state_id == state_id)
            .filter_map(|frame| {
                let local_time = frame.state_local_times.get(state_id).copied()?;
                let channel = frame
                    .motion_channels
                    .iter()
                    .find(|channel| channel.key == "sp_effect_cycle_phase")?;
                Some((local_time, channel))
            })
            .collect::<Vec<_>>();
        assert!(!samples.is_empty(), "missing phase samples for {state_id}");
        assert!(
            samples
                .iter()
                .filter(|(time, _)| *time < 1.0 - 1.0e-6)
                .all(|(_, channel)| channel.value[0].abs() <= 1.0e-6),
            "{state_id} did not hold phase=0 for its first second"
        );

        let (sample_time, quarter_cycle) = samples
            .iter()
            .min_by(|(left, _), (right, _)| (left - 1.25).abs().total_cmp(&(right - 1.25).abs()))
            .copied()
            .expect("quarter-cycle sample");
        assert!((sample_time - 1.25).abs() <= 1.0 / 60.0);
        assert!(
            (quarter_cycle.value[0] - 0.1).abs() <= 1.0e-5,
            "{state_id} phase at {sample_time}s was {}, expected 0.1",
            quarter_cycle.value[0]
        );
        assert_eq!(quarter_cycle.mutation_repeat_count, Some(-1));
        assert_eq!(quarter_cycle.mutation_plan_completed, Some(false));
    }

    let charging_second_cycle = result
        .report
        .frames
        .iter()
        .filter(|frame| frame.current_state_id == "st_mt1ajfjb_p")
        .filter_map(|frame| {
            let local_time = frame.state_local_times.get("st_mt1ajfjb_p").copied()?;
            let channel = frame
                .motion_channels
                .iter()
                .find(|channel| channel.key == "sp_effect_cycle_phase")?;
            Some((local_time, channel))
        })
        .min_by(|(left, _), (right, _)| (left - 3.5).abs().total_cmp(&(right - 3.5).abs()))
        .expect("charging repeat seam sample");
    assert!((charging_second_cycle.0 - 3.5).abs() <= 1.0 / 60.0);
    assert!(charging_second_cycle.1.value[0].abs() <= 1.0e-5);
    assert_eq!(charging_second_cycle.1.mutation_repeat_iteration, Some(2));
}

#[test]
fn lalaland_island_passes_fit_width_before_outer_resolve() {
    let _function_registry_guard = lock_function_registry();
    let scenario_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("scenarios/doubao-lalaland-blur-waypoint.json");
    let scenario = TraceScenario::from_path(&scenario_path).expect("parse Lalaland scenario");
    let scene = lalaland_scene();
    let state_machine = scene
        .state_machine
        .as_ref()
        .expect("Lalaland state machine");
    let state_param_ids = state_machine
        .state_params
        .iter()
        .map(|param| param.id.as_str())
        .collect::<Vec<_>>();
    for removed_id in [
        "sp_content_scale",
        "sp_leading_center_x_dp",
        "sp_trailing_center_x_dp",
    ] {
        assert!(
            !state_param_ids.contains(&removed_id),
            "obsolete island layout State Param {removed_id} must be removed"
        );
    }
    for id in [
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
    let expected_group_ids = [
        "island.charging",
        "island.fast-charge",
        "island.supercharge",
        "effect.shared-charging",
        "Outer Composite",
    ];
    for group_id in expected_group_ids {
        assert!(
            scene.groups.iter().any(|group| group.id == group_id),
            "missing Lalaland render group {group_id}"
        );
    }
    let island_layout = state_machine
        .derivations
        .iter()
        .find(|derivation| derivation.id == "island_layout")
        .expect("Island Layout Derivation");
    for uniform_node_id in island_layout
        .output_bindings
        .iter()
        .map(|binding| binding.uniform.node_id.as_str())
    {
        assert!(
            scene.nodes.iter().any(|node| node.id == uniform_node_id),
            "Derivation output uniform {uniform_node_id} must remain in root"
        );
    }
    for uniform_node_id in island_layout
        .passthrough_bindings
        .iter()
        .map(|binding| binding.uniform.node_id.as_str())
    {
        assert!(
            scene.nodes.iter().any(|node| node.id == uniform_node_id),
            "Derivation passthrough uniform {uniform_node_id} must remain in root"
        );
    }
    let effect_group = scene
        .groups
        .iter()
        .find(|group| group.id == "effect.shared-charging")
        .expect("Shared Charging Effect group");
    let effect_material = effect_group
        .nodes
        .iter()
        .find(|node| node.id == "ChargingEffectMaterial")
        .expect("Shared Effect ShaderMaterial");
    for (port_id, source_node_id) in [
        ("param:icon_variant", "EffectIconVariant"),
        ("param:px_per_dp", "EffectPxPerDp"),
        ("param:glow_color", "ChargingGlowColor"),
        ("param:wave_radius_dp_animated", "EffectWaveRadiusDp"),
        ("param:wave_width_dp_animated", "EffectWaveWidthDp"),
        ("param:wave_alpha_animated", "EffectWaveAlpha"),
        ("param:wave_opacity", "EffectWaveOpacity"),
        ("param:wave_dispersion_animated", "EffectWaveDispersion"),
        ("param:pulse_gain_animated", "EffectPulseGain"),
        ("param:content_alpha", "FloatInput_e581097c_28"),
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
        let group_binding = effect_group
            .input_bindings
            .iter()
            .find(|binding| {
                binding.to.node_id == "ChargingEffectMaterial" && binding.to.port_id == port_id
            })
            .unwrap_or_else(|| panic!("Shared Effect group input {port_id} is not bound"));
        let boundary_connection = scene
            .connections
            .iter()
            .find(|connection| {
                connection.to.node_id == "GroupInstance_SharedChargingEffect"
                    && connection.to.port_id == group_binding.group_port_id
            })
            .unwrap_or_else(|| panic!("Shared Effect group input {port_id} has no root uniform"));
        assert_eq!(boundary_connection.from.node_id, source_node_id);
    }
    assert_eq!(
        effect_material
            .inputs
            .iter()
            .filter(|input| input.id.ends_with("_animated"))
            .map(|input| input.id.as_str())
            .collect::<Vec<_>>(),
        vec![
            "param:wave_radius_dp_animated",
            "param:wave_width_dp_animated",
            "param:wave_alpha_animated",
            "param:wave_dispersion_animated",
            "param:pulse_gain_animated",
        ],
        "only Mutation/Derivation outputs may use the animated suffix"
    );
    for static_port_id in [
        "param:px_per_dp",
        "param:wave_center_gate_width",
        "param:outer_mid_occlusion",
    ] {
        assert!(
            effect_material
                .inputs
                .iter()
                .any(|input| input.id == static_port_id),
            "non-animated Shared Effect input {static_port_id} changed naming"
        );
    }
    assert!(
        effect_material
            .inputs
            .iter()
            .all(|input| input.id != "param:animated_pulse_gain"),
        "CPU-animated uniforms use a suffix, never the animated_ prefix"
    );
    let outer_resolve = scene
        .groups
        .iter()
        .flat_map(|group| group.nodes.iter())
        .find(|node| node.id == "ShaderMaterial_faaaafde_35")
        .expect("Outer Island Resolve ShaderMaterial");
    for removed_input in ["param:content_scale", "param:content_size_px"] {
        assert!(
            outer_resolve
                .inputs
                .iter()
                .all(|input| input.id != removed_input),
            "Outer Resolve must only crop the already-fitted island passes"
        );
    }
    for id in [
        "RenderTexture_8e46d6bc_9",
        "ChargingIslandRT",
        "SuperchargeIslandRT",
        "FastChargeIslandRT",
    ] {
        let target = scene
            .nodes
            .iter()
            .chain(scene.groups.iter().flat_map(|group| group.nodes.iter()))
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
            "tr_msx1vbpw_s".to_string(),
            "tr_mszxk3wu_q".to_string(),
            "tr_mt1aax6i_25".to_string(),
            "tr_mt1abve2_2e".to_string(),
        ]
    );
    let wave_radius_values = result
        .report
        .frames
        .iter()
        .filter(|frame| frame.label.as_deref() == Some("to-charging"))
        .filter_map(|frame| {
            frame
                .values
                .get("EffectWaveRadiusDp:value")
                .and_then(serde_json::Value::as_f64)
        })
        .collect::<Vec<_>>();
    assert!(
        wave_radius_values.iter().copied().fold(0.0, f64::max) >= 39.0,
        "wave radius never reached its State-driven travel distance"
    );
    assert!(
        wave_radius_values
            .windows(2)
            .any(|values| values[1] - values[0] > 0.1),
        "wave radius did not travel outward"
    );
    assert!(
        wave_radius_values
            .windows(2)
            .any(|values| values[0] - values[1] > 0.1),
        "wave radius cycle did not restart after reaching its travel distance"
    );
    for (label, waypoint, final_target) in [
        ("to-collapsed", None, 24.0),
        ("to-charging", None, 0.0),
        ("to-supercharge", Some(18.0), 0.0),
        ("back-to-charging", Some(18.0), 0.0),
        ("back-to-collapsed", None, 24.0),
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
        if let Some(waypoint) = waypoint {
            let waypoint_distance = values
                .iter()
                .map(|value| (value - waypoint).abs())
                .fold(f64::INFINITY, f64::min);
            assert!(
                waypoint_distance <= 1.5,
                "{label} did not pass through authored waypoint {waypoint}; closest distance was {waypoint_distance}"
            );
        }
        assert!(
            (values.last().copied().unwrap_or(f64::NAN) - final_target).abs() <= 1.0e-6,
            "{label} did not settle at {final_target}"
        );
    }

    for frame in &result.report.frames {
        let channel_value = |key: &str| {
            frame
                .motion_channels
                .iter()
                .find(|channel| channel.key == key)
                .and_then(|channel| channel.value.first().copied())
                .unwrap_or_else(|| panic!("missing channel {key} at frame {}", frame.frame_index))
        };
        let override_value = |key: &str| {
            frame
                .values
                .get(key)
                .and_then(serde_json::Value::as_f64)
                .unwrap_or_else(|| panic!("missing {key} at frame {}", frame.frame_index))
        };
        let assert_override = |key: &str, expected: f64| {
            let actual = override_value(key);
            assert!(
                (actual - expected).abs() <= 1.0e-5,
                "{key} did not match the pass-local width fit at frame {}: expected {expected}, got {actual}",
                frame.frame_index
            );
        };

        let island_width_px = override_value("IslandSizePx:x");
        let charging_scale = island_width_px / 518.0;
        let charging_side_width = 224.0 * charging_scale;
        let charging_side_height = 112.0 * charging_scale;
        let charging_center_offset = 147.0 * charging_scale;
        let charging_left = 309.0 - charging_center_offset;
        let charging_right = 309.0 + charging_center_offset;
        assert_override("SideTextureSizePx:x", charging_side_width);
        assert_override("SideTextureSizePx:y", charging_side_height);
        assert_override("LeftContentCenterPx:x", charging_left);
        assert_override("RightContentCenterPx:x", charging_right);
        assert!(
            ((charging_right + charging_side_width * 0.5)
                - (charging_left - charging_side_width * 0.5)
                - island_width_px)
                .abs()
                <= 1.0e-5,
            "Charging pass did not fill the island width at frame {}",
            frame.frame_index
        );

        let wide_scale = island_width_px / 602.0;
        let wide_side_width = 266.0 * wide_scale;
        let wide_side_height = 112.0 * wide_scale;
        let wide_center_offset = 168.0 * wide_scale;
        let wide_left = 309.0 - wide_center_offset;
        let wide_right = 309.0 + wide_center_offset;
        assert_override("SuperchargeSideTextureSizePx:x", wide_side_width);
        assert_override("SuperchargeSideTextureSizePx:y", wide_side_height);
        assert_override("SuperchargeLeftContentCenterPx:x", wide_left);
        assert_override("SuperchargeRightContentCenterPx:x", wide_right);
        assert!(
            ((wide_right + wide_side_width * 0.5)
                - (wide_left - wide_side_width * 0.5)
                - island_width_px)
                .abs()
                <= 1.0e-5,
            "wide island pass did not fill the island width at frame {}",
            frame.frame_index
        );

        let (icon_variant, effect_offset_dp) = if frame.current_state_id == "entry" {
            (0.0, 56.0)
        } else {
            (
                channel_value("sp_icon_variant").clamp(0.0, 1.0),
                channel_value("sp_effect_offset_dp"),
            )
        };
        let effect_native_width = 518.0 + (602.0 - 518.0) * icon_variant;
        let effect_scale = island_width_px / effect_native_width;
        let effect_center_x = 309.0 + effect_offset_dp * 3.5 * effect_scale;
        assert_override("EffectPlacementCenterPx:x", effect_center_x);
        assert_override("EffectPlacementCenterPx:y", 92.0);
        assert_override("EffectSizePx:x", 308.0 * effect_scale);
        assert_override("EffectSizePx:y", 308.0 * effect_scale);
        assert_override("EffectPxPerDp:value", 3.5 * effect_scale);

        if frame.label.is_some() {
            for (channel_key, uniform_key) in [
                ("param_charging_glow_color", "ChargingGlowColor:value"),
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
}

#[test]
fn doubao_scale_four_trace_uses_the_shader_graph_scene_size() {
    let _function_registry_guard = lock_function_registry();
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
