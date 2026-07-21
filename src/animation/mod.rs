//! Animation engine layer.
//!
//! Provides a render-frame animation session backed by per-property motion
//! drivers. Every session step advances once with the full frame delta.
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
pub mod timeline;

pub use session::{AnimationSession, AnimationStep};
pub use timeline::{TimelineBuffer, TimelineFrame};
