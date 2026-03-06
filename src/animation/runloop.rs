//! Runloop orchestrator for the animation subsystem.
//!
//! The `Runloop` owns a `ValuePool` and `TaskPool`, executing all tasks
//! in insertion order each tick and flushing the pool into an override map.

use std::collections::HashMap;

use crate::state_machine::runtime::{ExternalParams, FiredEvents, StateMachineRuntime, TickResult};
use crate::state_machine::types::OverrideKey;

use super::task::TaskPool;
use super::value_pool::ValuePool;

// ---------------------------------------------------------------------------
// RunloopTickResult
// ---------------------------------------------------------------------------

/// Result of a single `Runloop::tick()` call.
#[derive(Debug, Clone)]
pub struct RunloopTickResult {
    /// Override map produced by `ValuePool::flush()`.
    pub overrides: HashMap<OverrideKey, serde_json::Value>,
    /// Collected diagnostics from all tasks.
    pub diagnostics: Vec<String>,
    /// The `TickResult` from the `StateMachineDriven` task, if present.
    pub tick_result: Option<TickResult>,
}

// ---------------------------------------------------------------------------
// Runloop
// ---------------------------------------------------------------------------

/// Orchestrator that drives the task pool and value pool each tick.
#[derive(Debug, Clone)]
pub struct Runloop {
    pub value_pool: ValuePool,
    pub task_pool: TaskPool,
}

impl Default for Runloop {
    fn default() -> Self {
        Self::new()
    }
}

impl Runloop {
    /// Create a new runloop with empty pools.
    pub fn new() -> Self {
        Self {
            value_pool: ValuePool::new(),
            task_pool: TaskPool::new(),
        }
    }

    /// Execute one fixed-step tick:
    /// 1. Iterate tasks in insertion order, each reading/writing the ValuePool.
    /// 2. Flush the ValuePool to produce the override map.
    pub fn tick(
        &mut self,
        runtime: &mut StateMachineRuntime,
        dt: f64,
        params: &ExternalParams,
        events: &FiredEvents,
    ) -> RunloopTickResult {
        let mut diagnostics = Vec::new();
        let mut last_tick_result: Option<TickResult> = None;

        // We need to iterate tasks mutably while also passing &mut value_pool.
        // Split borrow: take tasks out, iterate, put back.
        let mut tasks = std::mem::take(&mut self.task_pool);
        for task in tasks.tasks_mut() {
            let result = task.execute(&mut self.value_pool, Some(runtime), dt, params, events);
            diagnostics.extend(result.diagnostics);
            if result.tick_result.is_some() {
                last_tick_result = result.tick_result;
            }
        }
        self.task_pool = tasks;

        let overrides = self.value_pool.flush();

        RunloopTickResult {
            overrides,
            diagnostics,
            tick_result: last_tick_result,
        }
    }

    /// Reset all state: clear the value pool and task-internal state.
    pub fn reset(&mut self) {
        self.value_pool.reset_all();
        for task in self.task_pool.tasks_mut() {
            task.reset();
        }
    }
}
