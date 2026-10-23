//! Reproducible captures through the real AnimationSession and headless GPU renderer.
//! cargo run --example doubao_capture -- <archive> <state-name|events> <seconds> <output.exr>
use std::{collections::BTreeMap, path::PathBuf};
use anyhow::{Context, Result, ensure};
use node_forge_render_server::{animation::AnimationSession, asset_store, renderer};
use node_forge_render_server::state_machine::{EventModifiers, FiredEvent, MousePosition};

fn main() -> Result<()> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    ensure!(args.len() == 4, "expected archive, state-name|events, seconds, output.exr");
    let (scene, assets) = asset_store::load_from_nforge(&PathBuf::from(&args[0]))?;
    let mut session = AnimationSession::from_scene(&scene)?.context("missing state machine")?;
    let event_scenario = ["events", "gesture", "exit", "reopen"].contains(&args[1].as_str());
    let mut frame = if event_scenario {
        session.step(0.0)
    } else {
        // A selectable state can inherit an independent axis (e.g. the reply panel).
        // Accept an explicit setup path so captures preserve that authored history.
        let mut frame = None;
        for selector in args[1].split('>') {
            let state = scene.state_machine.as_ref().unwrap().states.iter()
                .find(|s| s.name == selector || s.id == selector).context("unknown state")?;
            frame = Some(session.force_state(&state.id)?);
        }
        frame.unwrap()
    };
    ensure!(frame.diagnostics.is_empty(), "activation: {:?}", frame.diagnostics);
    let frames = (args[2].parse::<f64>()? * 60.0).round() as usize;
    let mut diagnostics = Vec::new();
    for i in 0..frames {
        if event_scenario && (i == 0 || i == 6) {
            session.fire_event(FiredEvent {
                event_type: if i == 0 { "keydown" } else { "keyup" }.into(),
                key: Some(" ".into()), button: None, repeat: false,
                modifiers: EventModifiers::default(),
            });
        }
        if args[1] == "gesture" && [156, 186, 210, 228].contains(&i) {
            session.update_mouse_position(MousePosition {
                x: 630.0, y: if i == 186 { 650.0 } else { 217.0 },
            });
            session.fire_event(FiredEvent {
                event_type: match i { 156 => "mousedown", 228 => "mouseup", _ => "mousemove" }.into(),
                key: None, button: Some("primary".into()), repeat: false,
                modifiers: EventModifiers::default(),
            });
        }
        if ["exit", "reopen"].contains(&args[1].as_str())
            && ([156, 157].contains(&i) || (args[1] == "reopen" && [216, 222].contains(&i))) {
            session.fire_event(FiredEvent {
                event_type: if i == 157 || i == 222 { "keyup" } else { "keydown" }.into(),
                key: Some(if i <= 157 { "Escape" } else { " " }.into()),
                button: None, repeat: false, modifiers: EventModifiers::default(),
            });
        }
        frame = session.step(1.0 / 60.0);
        diagnostics.extend(frame.diagnostics.clone());
    }
    // Keep transient runtime rollbacks in the evidence; never accept a stale final frame.
    ensure!(diagnostics.is_empty(), "frame diagnostics: {:?}", diagnostics);
    let overrides = frame.active_overrides.iter()
        .map(|(k,v)| (format!("{}:{}", k.node_id, k.param_name), v))
        .collect::<BTreeMap<_,_>>();
    let output = PathBuf::from(&args[3]);
    std::fs::write(output.with_extension("json"), serde_json::to_string_pretty(&serde_json::json!({
        "stateId": frame.current_state_id, "sceneTimeSeconds": frame.scene_time_secs,
        "activeTransitionId": frame.active_transition_id, "overrides": overrides,
        "stateLocalTimes": frame.state_local_times,
        "diagnostics": diagnostics,
    }))?)?;
    renderer::render_animation_frame_to_file_headless(
        &scene, &output, Some(&assets), &frame.active_overrides, frame.scene_time_secs as f32,
    )
}
