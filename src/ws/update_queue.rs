//! Bounded, coalescing handoff from producers to the render thread.
//!
//! Full snapshots supersede pending scene data, but never cross control-message barriers.
//! Uniform patches merge by node and field; a full snapshot followed by patches remains ordered.
//! Payloads move into the queue without cloning scenes or binary artifacts.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Condvar, Mutex, MutexGuard},
};

use crossbeam_channel::{RecvError, SendError, TryRecvError};

use super::{Node, SceneUpdate};

const MAX_PENDING_ENTRIES: usize = 64;

struct UniformBatch {
    nodes: Vec<Node>,
    indexes: HashMap<String, usize>,
    request_id: Option<String>,
    perf_trace: Option<super::ScenePerfTrace>,
}

impl UniformBatch {
    fn extend(
        &mut self,
        nodes: Vec<Node>,
        request_id: Option<String>,
        perf_trace: Option<super::ScenePerfTrace>,
    ) {
        for node in nodes {
            if let Some(&index) = self.indexes.get(&node.id) {
                // Uniform classification guarantees stable node identity/type and ports.
                // Partial patches must retain earlier fields, not replace the whole node.
                self.nodes[index].params.extend(node.params);
            } else {
                self.indexes.insert(node.id.clone(), self.nodes.len());
                self.nodes.push(node);
            }
        }
        self.request_id = request_id;
        self.perf_trace = perf_trace;
    }

    fn into_update(self) -> SceneUpdate {
        SceneUpdate::UniformDelta {
            updated_nodes: self.nodes,
            request_id: self.request_id,
            perf_trace: self.perf_trace,
        }
    }
}

enum PendingEntry {
    Snapshot(SceneUpdate),
    Uniforms(UniformBatch),
    Control(SceneUpdate),
}

impl PendingEntry {
    fn into_update(self) -> SceneUpdate {
        match self {
            Self::Snapshot(update) | Self::Control(update) => update,
            Self::Uniforms(batch) => batch.into_update(),
        }
    }
}

struct PendingState {
    entries: VecDeque<PendingEntry>,
    senders: usize,
    receiver_alive: bool,
}

struct Shared {
    state: Mutex<PendingState>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, PendingState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }
}

pub struct SceneUpdateSender {
    shared: Arc<Shared>,
}

pub struct SceneUpdateReceiver {
    shared: Arc<Shared>,
}

pub fn scene_update_channel() -> (SceneUpdateSender, SceneUpdateReceiver) {
    let shared = Arc::new(Shared {
        state: Mutex::new(PendingState {
            entries: VecDeque::new(),
            senders: 1,
            receiver_alive: true,
        }),
        changed: Condvar::new(),
    });
    (
        SceneUpdateSender {
            shared: shared.clone(),
        },
        SceneUpdateReceiver { shared },
    )
}

impl Clone for SceneUpdateSender {
    fn clone(&self) -> Self {
        self.shared.lock().senders += 1;
        Self {
            shared: self.shared.clone(),
        }
    }
}

impl Drop for SceneUpdateSender {
    fn drop(&mut self) {
        self.shared.lock().senders -= 1;
        self.shared.changed.notify_all();
    }
}

impl SceneUpdateSender {
    pub fn send(&self, update: SceneUpdate) -> Result<(), SendError<SceneUpdate>> {
        let mut state = self.shared.lock();
        loop {
            if !state.receiver_alive {
                return Err(SendError(update));
            }
            // A new full scene is authoritative for pending scene data only. Never remove controls.
            if matches!(&update, SceneUpdate::Parsed { .. }) {
                while state
                    .entries
                    .back()
                    .is_some_and(|entry| !matches!(entry, PendingEntry::Control(_)))
                {
                    state.entries.pop_back();
                }
            }
            if state.entries.len() < MAX_PENDING_ENTRIES
                || (matches!(&update, SceneUpdate::UniformDelta { .. })
                    && matches!(state.entries.back(), Some(PendingEntry::Uniforms(_))))
            {
                break;
            }
            // Only a backlog of non-coalescible control messages causes backpressure.
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }

        match update {
            SceneUpdate::UniformDelta {
                updated_nodes,
                request_id,
                perf_trace,
            } => {
                if let Some(PendingEntry::Uniforms(batch)) = state.entries.back_mut() {
                    batch.extend(updated_nodes, request_id, perf_trace);
                } else {
                    let mut batch = UniformBatch {
                        nodes: Vec::new(),
                        indexes: HashMap::new(),
                        request_id: None,
                        perf_trace: None,
                    };
                    batch.extend(updated_nodes, request_id, perf_trace);
                    state.entries.push_back(PendingEntry::Uniforms(batch));
                }
            }
            update @ SceneUpdate::Parsed { .. } => {
                state.entries.push_back(PendingEntry::Snapshot(update));
            }
            update => state.entries.push_back(PendingEntry::Control(update)),
        }
        drop(state);
        self.shared.changed.notify_all();
        Ok(())
    }
}

impl SceneUpdateReceiver {
    pub fn try_recv(&self) -> Result<SceneUpdate, TryRecvError> {
        let mut state = self.shared.lock();
        let update = state.entries.pop_front();
        let senders = state.senders;
        drop(state);
        if let Some(update) = update {
            self.shared.changed.notify_all();
            Ok(update.into_update())
        } else if senders == 0 {
            Err(TryRecvError::Disconnected)
        } else {
            Err(TryRecvError::Empty)
        }
    }

    pub fn recv(&self) -> Result<SceneUpdate, RecvError> {
        let mut state = self.shared.lock();
        loop {
            if let Some(update) = state.entries.pop_front() {
                drop(state);
                self.shared.changed.notify_all();
                return Ok(update.into_update());
            }
            if state.senders == 0 {
                return Err(RecvError);
            }
            state = self
                .shared
                .changed
                .wait(state)
                .unwrap_or_else(|error| error.into_inner());
        }
    }

    pub fn is_empty(&self) -> bool {
        self.shared.lock().entries.is_empty()
    }
}

impl Drop for SceneUpdateReceiver {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.receiver_alive = false;
        state.entries.clear();
        drop(state);
        self.shared.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsl::{Metadata, SceneDSL};
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn node(id: &str, fields: &[(&str, i64)]) -> Node {
        Node {
            id: id.into(),
            node_type: "FloatInput".into(),
            params: fields
                .iter()
                .map(|(key, value)| ((*key).into(), (*value).into()))
                .collect(),
            inputs: Vec::new(),
            outputs: Vec::new(),
            input_bindings: Vec::new(),
            wgsl_override: None,
        }
    }

    fn delta(nodes: Vec<Node>) -> SceneUpdate {
        SceneUpdate::UniformDelta {
            updated_nodes: nodes,
            request_id: None,
            perf_trace: None,
        }
    }

    fn snapshot(nodes: Vec<Node>) -> SceneUpdate {
        SceneUpdate::Parsed {
            scene: SceneDSL {
                version: "6.1".into(),
            working_color_space: crate::color::WorkingColorSpace::Srgb,
            dpi: 3.0,
                metadata: Metadata {
                    name: "queue-test".into(),
                    created: None,
                    modified: None,
                },
                nodes,
                connections: Vec::new(),
                outputs: None,
                groups: Vec::new(),
                assets: HashMap::new(),
                state_machine: None,
                debug_artifacts: None,
            },
            request_id: None,
            source: super::super::ParsedSceneSource::SceneUpdate,
            perf_trace: None,
        }
    }

    #[test]
    fn partial_fields_merge_and_latest_write_wins() {
        let (tx, rx) = scene_update_channel();
        tx.send(delta(vec![node("gain", &[("x", 1), ("y", 2)])]))
            .unwrap();
        tx.send(delta(vec![node("gain", &[("x", 3)])])).unwrap();
        let SceneUpdate::UniformDelta { updated_nodes, .. } = rx.recv().unwrap() else {
            panic!("expected uniform batch");
        };
        assert_eq!(updated_nodes.len(), 1);
        assert_eq!(updated_nodes[0].params["x"], serde_json::json!(3));
        assert_eq!(updated_nodes[0].params["y"], serde_json::json!(2));
        assert!(rx.is_empty());
    }

    #[test]
    fn snapshot_and_following_patch_remain_ordered() {
        let (tx, rx) = scene_update_channel();
        tx.send(snapshot(vec![node("gain", &[("value", 0)])]))
            .unwrap();
        tx.send(delta(vec![node("gain", &[("value", 9)])])).unwrap();
        assert!(matches!(rx.recv().unwrap(), SceneUpdate::Parsed { .. }));
        assert!(matches!(
            rx.recv().unwrap(),
            SceneUpdate::UniformDelta { .. }
        ));
    }

    #[test]
    fn authoritative_snapshot_supersedes_only_pending_scene_data() {
        let (tx, rx) = scene_update_channel();
        tx.send(delta(vec![node("removed", &[("value", 1)])]))
            .unwrap();
        tx.send(snapshot(vec![node("current", &[("value", 2)])]))
            .unwrap();
        let SceneUpdate::Parsed { scene, .. } = rx.recv().unwrap() else {
            panic!("expected latest snapshot");
        };
        assert_eq!(scene.nodes[0].id, "current");
        assert!(rx.is_empty());
    }

    #[test]
    fn control_barrier_is_not_dropped_or_crossed_by_a_new_snapshot() {
        let (tx, rx) = scene_update_channel();
        tx.send(snapshot(vec![node("before", &[])])).unwrap();
        tx.send(SceneUpdate::DebugArtifactDelete {
            artifact_id: "artifact".into(),
        })
        .unwrap();
        tx.send(delta(vec![node("old", &[])])).unwrap();
        tx.send(snapshot(vec![node("after", &[])])).unwrap();
        let SceneUpdate::Parsed { scene, .. } = rx.recv().unwrap() else {
            panic!("expected preceding snapshot");
        };
        assert_eq!(scene.nodes[0].id, "before");
        assert!(matches!(
            rx.recv().unwrap(),
            SceneUpdate::DebugArtifactDelete { .. }
        ));
        let SceneUpdate::Parsed { scene, .. } = rx.recv().unwrap() else {
            panic!("expected following snapshot");
        };
        assert_eq!(scene.nodes[0].id, "after");
        assert!(rx.is_empty());
    }

    #[test]
    fn errors_and_freeze_controls_survive_scene_replacement() {
        let (tx, rx) = scene_update_channel();
        tx.send(SceneUpdate::ParseError {
            message: "invalid scene".into(),
            request_id: None,
        })
        .unwrap();
        tx.send(SceneUpdate::FrameFreezesSync { cases: Vec::new() })
            .unwrap();
        tx.send(SceneUpdate::FrameFreezePersisted {
            case_id: "freeze".into(),
            ok: false,
            error: Some("disk full".into()),
        })
        .unwrap();
        tx.send(snapshot(Vec::new())).unwrap();
        assert!(matches!(rx.recv().unwrap(), SceneUpdate::ParseError { .. }));
        assert!(matches!(
            rx.recv().unwrap(),
            SceneUpdate::FrameFreezesSync { .. }
        ));
        assert!(matches!(
            rx.recv().unwrap(),
            SceneUpdate::FrameFreezePersisted { ok: false, .. }
        ));
        assert!(matches!(rx.recv().unwrap(), SceneUpdate::Parsed { .. }));
    }

    #[test]
    fn cloned_producers_merge_without_losing_independent_nodes() {
        let (tx, rx) = scene_update_channel();
        let threads: Vec<_> = (0..4)
            .map(|producer| {
                let tx = tx.clone();
                thread::spawn(move || {
                    for value in 0..1000 {
                        tx.send(delta(vec![node(
                            &format!("node-{producer}"),
                            &[("value", value)],
                        )]))
                        .unwrap();
                    }
                })
            })
            .collect();
        for task in threads {
            task.join().unwrap();
        }
        let SceneUpdate::UniformDelta { updated_nodes, .. } = rx.recv().unwrap() else {
            panic!("expected uniform batch");
        };
        assert_eq!(updated_nodes.len(), 4);
        assert!(
            updated_nodes
                .iter()
                .all(|node| node.params["value"] == serde_json::json!(999))
        );
        assert!(rx.is_empty());
    }

    #[test]
    fn control_backpressure_unblocks_on_receive_without_losing_messages() {
        let (tx, rx) = scene_update_channel();
        for index in 0..MAX_PENDING_ENTRIES {
            tx.send(SceneUpdate::DebugArtifactDelete {
                artifact_id: index.to_string(),
            })
            .unwrap();
        }
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let task = thread::spawn(move || {
            tx.send(SceneUpdate::DebugArtifactDelete {
                artifact_id: "last".into(),
            })
            .unwrap();
            done_tx.send(()).unwrap();
        });
        assert!(done_rx.recv_timeout(Duration::from_millis(20)).is_err());
        for index in 0..MAX_PENDING_ENTRIES {
            let SceneUpdate::DebugArtifactDelete { artifact_id } = rx.recv().unwrap() else {
                panic!("expected ordered control");
            };
            assert_eq!(artifact_id, index.to_string());
        }
        done_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        task.join().unwrap();
        let SceneUpdate::DebugArtifactDelete { artifact_id } = rx.recv().unwrap() else {
            panic!("expected final control");
        };
        assert_eq!(artifact_id, "last");
    }

    #[test]
    fn receiver_drop_unblocks_a_backpressured_sender() {
        let (tx, rx) = scene_update_channel();
        for _ in 0..MAX_PENDING_ENTRIES {
            tx.send(SceneUpdate::FrameFreezesSync { cases: Vec::new() })
                .unwrap();
        }
        let task = thread::spawn(move || tx.send(snapshot(Vec::new())));
        drop(rx);
        assert!(task.join().unwrap().is_err());
    }

    #[test]
    fn receiver_drains_pending_data_then_observes_sender_disconnect() {
        let (tx, rx) = scene_update_channel();
        tx.send(delta(vec![node("gain", &[("value", 1)])])).unwrap();
        drop(tx);
        assert!(rx.recv().is_ok());
        assert!(rx.recv().is_err());
        assert!(matches!(rx.try_recv(), Err(TryRecvError::Disconnected)));
    }

    #[test]
    fn snapshot_payload_moves_into_queue_without_deep_copy() {
        let (tx, rx) = scene_update_channel();
        let update = snapshot(vec![node("gain", &[("value", 1)])]);
        let SceneUpdate::Parsed { scene, .. } = &update else {
            unreachable!()
        };
        let address = scene.nodes.as_ptr();
        tx.send(update).unwrap();
        let SceneUpdate::Parsed { scene, .. } = rx.recv().unwrap() else {
            unreachable!()
        };
        assert_eq!(scene.nodes.as_ptr(), address);
    }

    #[test]
    #[ignore = "opt-in local queue microbenchmark; not an end-to-end GPU benchmark"]
    fn benchmark_snapshot_enqueue() {
        let make_snapshot = || {
            snapshot(
                (0..500)
                    .map(|index| node(&format!("node-{index}"), &[("value", index)]))
                    .collect(),
            )
        };
        let mut old_ms = Vec::new();
        let mut new_ms = Vec::new();
        for _ in 0..7 {
            let updates: Vec<_> = (0..200).map(|_| make_snapshot()).collect();
            let (tx, rx) = crossbeam_channel::bounded(1);
            let start = Instant::now();
            for update in updates {
                if tx.try_send(update.clone()).is_err() {
                    while rx.try_recv().is_ok() {}
                    tx.try_send(update).unwrap();
                }
            }
            old_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            let updates: Vec<_> = (0..200).map(|_| make_snapshot()).collect();
            let (tx, rx) = scene_update_channel();
            let start = Instant::now();
            for update in updates {
                tx.send(update).unwrap();
            }
            new_ms.push(start.elapsed().as_secs_f64() * 1000.0);
            assert!(matches!(rx.recv().unwrap(), SceneUpdate::Parsed { .. }));
            assert!(rx.is_empty());
        }
        old_ms.sort_by(f64::total_cmp);
        new_ms.sort_by(f64::total_cmp);
        println!(
            "queue_snapshot_benchmark scene_nodes=500 snapshots=200 rounds=7 old_median_ms={:.3} new_median_ms={:.3}",
            old_ms[3], new_ms[3]
        );
    }
}
