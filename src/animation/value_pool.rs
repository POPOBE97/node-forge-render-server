use std::collections::HashMap;

use serde_json;

use crate::state_machine::types::OverrideKey;

/// Lifecycle callback type for animation value transitions.
type LifecycleCallback = Option<Box<dyn Fn(&OverrideKey)>>;

// ---------------------------------------------------------------------------
// ValueStatus
// ---------------------------------------------------------------------------

/// Lifecycle phase of an animation value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueStatus {
    Idle,
    Running,
    Paused,
    Completed,
    Cancelled,
}

// ---------------------------------------------------------------------------
// AnimationValue
// ---------------------------------------------------------------------------

/// A single tracked animation parameter in the Value Pool.
pub struct AnimationValue {
    /// Current interpolated value.
    pub current: f64,
    /// Target value the animation is moving toward.
    pub target: f64,
    /// Current velocity (units/sec), used by physics tasks.
    pub velocity: f64,
    /// Lifecycle status.
    pub status: ValueStatus,
    /// Baseline value from the scene (for reset/restore).
    baseline: f64,
    /// Fired when transitioning Idle → Running.
    on_start: LifecycleCallback,
    /// Fired when transitioning to Completed.
    on_complete: LifecycleCallback,
    /// Fired when transitioning to Cancelled.
    on_cancel: LifecycleCallback,
    /// Fired when transitioning to Completed or Cancelled (after the specific callback).
    on_end: LifecycleCallback,
}

impl AnimationValue {
    /// Create a new AnimationValue initialized from a baseline.
    pub fn new(baseline: f64) -> Self {
        Self {
            current: baseline,
            target: baseline,
            velocity: 0.0,
            status: ValueStatus::Idle,
            baseline,
            on_start: None,
            on_complete: None,
            on_cancel: None,
            on_end: None,
        }
    }

    /// Get the baseline value.
    pub fn baseline(&self) -> f64 {
        self.baseline
    }

    /// Set the on_start callback.
    pub fn set_on_start(&mut self, cb: impl Fn(&OverrideKey) + 'static) {
        self.on_start = Some(Box::new(cb));
    }

    /// Set the on_complete callback.
    pub fn set_on_complete(&mut self, cb: impl Fn(&OverrideKey) + 'static) {
        self.on_complete = Some(Box::new(cb));
    }

    /// Set the on_cancel callback.
    pub fn set_on_cancel(&mut self, cb: impl Fn(&OverrideKey) + 'static) {
        self.on_cancel = Some(Box::new(cb));
    }

    /// Set the on_end callback.
    pub fn set_on_end(&mut self, cb: impl Fn(&OverrideKey) + 'static) {
        self.on_end = Some(Box::new(cb));
    }

    /// Reset this value to its baseline state.
    fn reset_to_baseline(&mut self) {
        self.current = self.baseline;
        self.target = self.baseline;
        self.velocity = 0.0;
        self.status = ValueStatus::Idle;
    }
}

impl Clone for AnimationValue {
    fn clone(&self) -> Self {
        Self {
            current: self.current,
            target: self.target,
            velocity: self.velocity,
            status: self.status,
            baseline: self.baseline,
            // Callbacks are not cloned — they are transient and must be
            // re-registered after clone if needed.
            on_start: None,
            on_complete: None,
            on_cancel: None,
            on_end: None,
        }
    }
}

impl std::fmt::Debug for AnimationValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnimationValue")
            .field("current", &self.current)
            .field("target", &self.target)
            .field("velocity", &self.velocity)
            .field("status", &self.status)
            .field("baseline", &self.baseline)
            .finish()
    }
}

// ---------------------------------------------------------------------------
// ValuePool
// ---------------------------------------------------------------------------

/// Centralized pool of tracked animation values keyed by OverrideKey.
///
/// Holds both scalar `AnimationValue` entries and a parallel map of raw JSON
/// overrides for non-scalar values (arrays, objects) that bypass the f64
/// `current` field to preserve golden-trace fidelity.
#[derive(Debug, Clone)]
pub struct ValuePool {
    values: HashMap<OverrideKey, AnimationValue>,
    /// Non-scalar JSON overrides written by StateMachineDriven tasks.
    /// Merged into the flush output alongside scalar values.
    json_overrides: HashMap<OverrideKey, serde_json::Value>,
}

impl Default for ValuePool {
    fn default() -> Self {
        Self::new()
    }
}

impl ValuePool {
    /// Create a new empty pool.
    pub fn new() -> Self {
        Self {
            values: HashMap::new(),
            json_overrides: HashMap::new(),
        }
    }

    /// Insert or update a value. If the key is new, initializes from baseline.
    /// Returns a mutable reference to the value for further configuration.
    pub fn insert(&mut self, key: OverrideKey, baseline: f64) -> &mut AnimationValue {
        self.values
            .entry(key)
            .or_insert_with(|| AnimationValue::new(baseline))
    }

    /// Get a reference to a value.
    pub fn get(&self, key: &OverrideKey) -> Option<&AnimationValue> {
        self.values.get(key)
    }

    /// Get a mutable reference to a value.
    pub fn get_mut(&mut self, key: &OverrideKey) -> Option<&mut AnimationValue> {
        self.values.get_mut(key)
    }

    /// Set current value and transition Idle → Running (firing on_start).
    ///
    /// No-op if the key is not in the pool.
    pub fn set_current(&mut self, key: &OverrideKey, value: f64) {
        if let Some(anim) = self.values.get_mut(key) {
            anim.current = value;
            if anim.status == ValueStatus::Idle {
                anim.status = ValueStatus::Running;
                if let Some(cb) = anim.on_start.as_ref() {
                    cb(key);
                }
            }
        }
    }

    /// Insert or overwrite a raw JSON override for non-scalar pass-through.
    pub fn set_json_override(&mut self, key: OverrideKey, value: serde_json::Value) {
        self.json_overrides.insert(key, value);
    }

    /// Remove a JSON override entry.
    pub fn remove_json_override(&mut self, key: &OverrideKey) {
        self.json_overrides.remove(key);
    }

    /// Transition a value's status, firing lifecycle callbacks synchronously.
    ///
    /// No-op if the key is not in the pool.
    pub fn transition_status(&mut self, key: &OverrideKey, new_status: ValueStatus) {
        if let Some(anim) = self.values.get_mut(key) {
            let old_status = anim.status;
            anim.status = new_status;

            // Fire callbacks based on the transition.
            match (old_status, new_status) {
                (ValueStatus::Idle, ValueStatus::Running) => {
                    if let Some(cb) = anim.on_start.as_ref() {
                        cb(key);
                    }
                }
                (_, ValueStatus::Completed) => {
                    if let Some(cb) = anim.on_complete.as_ref() {
                        cb(key);
                    }
                    if let Some(cb) = anim.on_end.as_ref() {
                        cb(key);
                    }
                }
                (_, ValueStatus::Cancelled) => {
                    if let Some(cb) = anim.on_cancel.as_ref() {
                        cb(key);
                    }
                    if let Some(cb) = anim.on_end.as_ref() {
                        cb(key);
                    }
                }
                _ => {}
            }
        }
    }

    /// Produce the override map: all non-Idle values as JSON, merged with
    /// raw json_overrides for non-scalar pass-through.
    pub fn flush(&self) -> HashMap<OverrideKey, serde_json::Value> {
        let mut result = HashMap::new();

        for (key, anim) in &self.values {
            if anim.status != ValueStatus::Idle {
                result.insert(key.clone(), serde_json::json!(anim.current));
            }
        }

        // Merge json_overrides (these take precedence for non-scalar values).
        for (key, value) in &self.json_overrides {
            result.insert(key.clone(), value.clone());
        }

        result
    }

    /// Reset all values to baseline.
    pub fn reset_all(&mut self) {
        for anim in self.values.values_mut() {
            anim.reset_to_baseline();
        }
        self.json_overrides.clear();
    }

    // -----------------------------------------------------------------------
    // Control APIs (Requirement 4)
    // -----------------------------------------------------------------------

    /// Start a value: Idle|Paused → Running.
    ///
    /// Fires `on_start` only for Idle → Running.
    pub fn start(&mut self, key: &OverrideKey) -> Result<(), String> {
        let anim = self
            .values
            .get_mut(key)
            .ok_or_else(|| format!("key not found: {}:{}", key.node_id, key.param_name))?;

        match anim.status {
            ValueStatus::Idle => {
                anim.status = ValueStatus::Running;
                if let Some(cb) = anim.on_start.as_ref() {
                    cb(key);
                }
                Ok(())
            }
            ValueStatus::Paused => {
                anim.status = ValueStatus::Running;
                Ok(())
            }
            other => Err(format!("cannot start value in {:?} status", other)),
        }
    }

    /// Stop a value: Running → Idle, velocity zeroed.
    pub fn stop(&mut self, key: &OverrideKey) -> Result<(), String> {
        let anim = self
            .values
            .get_mut(key)
            .ok_or_else(|| format!("key not found: {}:{}", key.node_id, key.param_name))?;

        match anim.status {
            ValueStatus::Running => {
                anim.status = ValueStatus::Idle;
                anim.velocity = 0.0;
                Ok(())
            }
            other => Err(format!("cannot stop value in {:?} status", other)),
        }
    }

    /// Pause a value: Running → Paused, preserves current/target/velocity.
    pub fn pause(&mut self, key: &OverrideKey) -> Result<(), String> {
        let anim = self
            .values
            .get_mut(key)
            .ok_or_else(|| format!("key not found: {}:{}", key.node_id, key.param_name))?;

        match anim.status {
            ValueStatus::Running => {
                anim.status = ValueStatus::Paused;
                Ok(())
            }
            other => Err(format!("cannot pause value in {:?} status", other)),
        }
    }

    /// Cancel a value: Running|Paused → Cancelled (fires on_cancel, on_end).
    pub fn cancel(&mut self, key: &OverrideKey) -> Result<(), String> {
        let anim = self
            .values
            .get_mut(key)
            .ok_or_else(|| format!("key not found: {}:{}", key.node_id, key.param_name))?;

        match anim.status {
            ValueStatus::Running | ValueStatus::Paused => {
                anim.status = ValueStatus::Cancelled;
                if let Some(cb) = anim.on_cancel.as_ref() {
                    cb(key);
                }
                if let Some(cb) = anim.on_end.as_ref() {
                    cb(key);
                }
                Ok(())
            }
            other => Err(format!("cannot cancel value in {:?} status", other)),
        }
    }

    /// Reset a single value to baseline: Any → Idle.
    pub fn reset(&mut self, key: &OverrideKey) -> Result<(), String> {
        let anim = self
            .values
            .get_mut(key)
            .ok_or_else(|| format!("key not found: {}:{}", key.node_id, key.param_name))?;

        anim.reset_to_baseline();
        self.json_overrides.remove(key);
        Ok(())
    }
}
