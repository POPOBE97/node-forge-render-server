//! Task pool and animation task definitions.
//!
//! Each `AnimationTask` is a unit of work executed once per runloop tick.
//! Tasks read from and write to the `ValuePool`, enabling composable
//! animation strategies (state-machine delegation, direct drive, physics).

use std::collections::HashSet;

use crate::state_machine::runtime::{ExternalParams, FiredEvents, StateMachineRuntime, TickResult};
use crate::state_machine::types::OverrideKey;

use super::value_pool::{ValuePool, ValueStatus};

// ---------------------------------------------------------------------------
// TaskKind
// ---------------------------------------------------------------------------

/// Discriminant for the update strategy an `AnimationTask` uses each tick.
#[derive(Debug, Clone)]
pub enum TaskKind {
    /// Delegates to `StateMachineRuntime::tick()` and writes the resulting
    /// overrides into the ValuePool.
    StateMachineDriven,

    /// Sets `current = target` each tick (immediate snap).
    DirectDrive { target_key: OverrideKey },

    /// Spring-damper integration:
    /// `acceleration = stiffness * (target - current) - damping * velocity`.
    PhysicsSpring {
        target_key: OverrideKey,
        stiffness: f64,
        damping: f64,
    },
}

// ---------------------------------------------------------------------------
// TaskExecutionResult
// ---------------------------------------------------------------------------

/// Result of executing a single `AnimationTask` for one tick.
#[derive(Debug, Clone)]
pub struct TaskExecutionResult {
    /// Non-fatal diagnostic messages emitted during execution.
    pub diagnostics: Vec<String>,
    /// The `TickResult` from `StateMachineRuntime::tick()`, if this was a
    /// `StateMachineDriven` task.
    pub tick_result: Option<TickResult>,
}

// ---------------------------------------------------------------------------
// AnimationTask
// ---------------------------------------------------------------------------

/// A single unit of work in the task pool, executed once per runloop tick.
#[derive(Debug, Clone)]
pub struct AnimationTask {
    /// Unique identifier for this task.
    pub id: String,
    /// The update strategy.
    pub kind: TaskKind,
    /// Keys that were actively overridden on the previous tick
    /// (used by `StateMachineDriven` to detect removed overrides).
    prev_active_keys: HashSet<OverrideKey>,
}

impl AnimationTask {
    /// Create a new task with the given id and kind.
    pub fn new(id: impl Into<String>, kind: TaskKind) -> Self {
        Self {
            id: id.into(),
            kind,
            prev_active_keys: HashSet::new(),
        }
    }

    /// Execute this task for one tick, reading/writing the ValuePool.
    ///
    /// For `StateMachineDriven`, also needs the runtime, dt, params, and events.
    /// For other task kinds, `runtime` is ignored (pass `None`).
    pub fn execute(
        &mut self,
        pool: &mut ValuePool,
        runtime: Option<&mut StateMachineRuntime>,
        dt: f64,
        params: &ExternalParams,
        events: &FiredEvents,
    ) -> TaskExecutionResult {
        match &self.kind {
            TaskKind::StateMachineDriven => {
                self.execute_state_machine_driven(pool, runtime, dt, params, events)
            }
            TaskKind::DirectDrive { target_key } => {
                let target_key = target_key.clone();
                self.execute_direct_drive(pool, &target_key)
            }
            TaskKind::PhysicsSpring {
                target_key,
                stiffness,
                damping,
            } => {
                let target_key = target_key.clone();
                let stiffness = *stiffness;
                let damping = *damping;
                self.execute_physics_spring(pool, &target_key, stiffness, damping, dt)
            }
        }
    }

    /// StateMachineDriven: delegate to runtime, write overrides into pool,
    /// detect removed keys and transition them to Completed.
    fn execute_state_machine_driven(
        &mut self,
        pool: &mut ValuePool,
        runtime: Option<&mut StateMachineRuntime>,
        dt: f64,
        params: &ExternalParams,
        events: &FiredEvents,
    ) -> TaskExecutionResult {
        let mut diagnostics = Vec::new();

        let Some(runtime) = runtime else {
            diagnostics.push("StateMachineDriven task has no runtime".into());
            return TaskExecutionResult {
                diagnostics,
                tick_result: None,
            };
        };

        let tick_result = runtime.tick(dt, params, events);

        // Track which keys are active this tick.
        let mut current_active_keys = HashSet::new();

        for (key, value) in &tick_result.overrides {
            current_active_keys.insert(key.clone());

            // Update the AnimationValue.current for numeric values (so physics
            // tasks can read it), and always store the raw JSON in json_overrides
            // so flush() returns the exact original serde_json::Value (preserving
            // integer vs float distinction for golden trace fidelity).
            if let Some(num) = value.as_f64() {
                pool.set_current(key, num);
            } else {
                // Non-numeric: ensure pool entry exists and is Running.
                if pool.get(key).is_none() {
                    pool.insert(key.clone(), 0.0);
                }
                if pool.get(key).is_some_and(|a| a.status == ValueStatus::Idle) {
                    pool.transition_status(key, ValueStatus::Running);
                }
            }
            // Always store raw JSON override for exact pass-through in flush().
            pool.set_json_override(key.clone(), value.clone());
        }

        // Detect removed keys: previously Running but no longer in overrides.
        for key in &self.prev_active_keys {
            if !current_active_keys.contains(key) {
                // Transition to Completed and restore current to baseline.
                if let Some(anim) = pool.get(key)
                    && anim.status == ValueStatus::Running
                {
                    let baseline = anim.baseline();
                    pool.transition_status(key, ValueStatus::Completed);
                    if let Some(anim) = pool.get_mut(key) {
                        anim.current = baseline;
                    }
                }
                // Also remove any json override for this key.
                pool.remove_json_override(key);
            }
        }

        self.prev_active_keys = current_active_keys;

        diagnostics.extend(tick_result.diagnostics.clone());

        TaskExecutionResult {
            diagnostics,
            tick_result: Some(tick_result),
        }
    }

    /// DirectDrive: set current = target if status is Running.
    fn execute_direct_drive(
        &self,
        pool: &mut ValuePool,
        target_key: &OverrideKey,
    ) -> TaskExecutionResult {
        if let Some(anim) = pool.get_mut(target_key)
            && anim.status == ValueStatus::Running
        {
            anim.current = anim.target;
        }

        TaskExecutionResult {
            diagnostics: Vec::new(),
            tick_result: None,
        }
    }

    /// PhysicsSpring: integrate spring-damper equation.
    /// Skip if Paused or Idle. Clamp NaN/infinity to 0.0 with diagnostic.
    fn execute_physics_spring(
        &self,
        pool: &mut ValuePool,
        target_key: &OverrideKey,
        stiffness: f64,
        damping: f64,
        dt: f64,
    ) -> TaskExecutionResult {
        let mut diagnostics = Vec::new();

        if let Some(anim) = pool.get_mut(target_key) {
            // Skip if Paused or Idle.
            if anim.status == ValueStatus::Paused || anim.status == ValueStatus::Idle {
                return TaskExecutionResult {
                    diagnostics,
                    tick_result: None,
                };
            }

            let acceleration = stiffness * (anim.target - anim.current) - damping * anim.velocity;
            anim.velocity += acceleration * dt;
            anim.current += anim.velocity * dt;

            // Clamp NaN/infinity to 0.0.
            if !anim.velocity.is_finite() {
                diagnostics.push(format!(
                    "PhysicsSpring: velocity became non-finite for {}:{}, clamped to 0.0",
                    target_key.node_id, target_key.param_name
                ));
                anim.velocity = 0.0;
            }
            if !anim.current.is_finite() {
                diagnostics.push(format!(
                    "PhysicsSpring: current became non-finite for {}:{}, clamped to 0.0",
                    target_key.node_id, target_key.param_name
                ));
                anim.current = 0.0;
            }
        }

        TaskExecutionResult {
            diagnostics,
            tick_result: None,
        }
    }
}

// ---------------------------------------------------------------------------
// TaskPool
// ---------------------------------------------------------------------------

/// Ordered collection of `AnimationTask` instances executed sequentially
/// each runloop tick.
#[derive(Debug, Default, Clone)]
pub struct TaskPool {
    tasks: Vec<AnimationTask>,
}

impl TaskPool {
    /// Create a new empty task pool.
    pub fn new() -> Self {
        Self { tasks: Vec::new() }
    }

    /// Add a task to the pool (appended at the end, preserving insertion order).
    pub fn add(&mut self, task: AnimationTask) {
        self.tasks.push(task);
    }

    /// Remove a task by id. Returns `true` if a task was found and removed.
    pub fn remove(&mut self, task_id: &str) -> bool {
        let len_before = self.tasks.len();
        self.tasks.retain(|t| t.id != task_id);
        self.tasks.len() < len_before
    }

    /// Get a slice of all tasks in insertion order.
    pub fn tasks(&self) -> &[AnimationTask] {
        &self.tasks
    }

    /// Get a mutable slice of all tasks (for runloop iteration).
    pub fn tasks_mut(&mut self) -> &mut [AnimationTask] {
        &mut self.tasks
    }
}
