//! Animation engine layer.
//!
//! Provides a deterministic fixed-step animation session that owns the
//! state-machine runtime and produces per-frame parameter overrides for
//! the render loop.
//!
//! # Usage (app integration)
//!
//! ```ignore
//! let session = AnimationSession::from_scene(&scene)?;
//! // Each frame:
//! let step = session.step(real_dt);
//! if step.needs_redraw {
//!     state_machine::apply_overrides(&mut scene, &step.active_overrides);
//!     apply_graph_uniform_updates(app, &scene);
//! }
//! ```

pub mod session;

pub use session::{AnimationSession, AnimationStep, FixedStepClock};
