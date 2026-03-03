//! Easing functions for state-machine transitions.

use super::types::EasingKind;

/// Evaluate an easing curve at `t` ∈ [0, 1].
///
/// Values outside the range are clamped.
pub fn ease(kind: EasingKind, t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    match kind {
        EasingKind::Linear => t,
        EasingKind::EaseIn => ease_in(t),
        EasingKind::EaseOut => ease_out(t),
        EasingKind::EaseInOut => ease_in_out(t),
    }
}

/// Quadratic ease-in.
fn ease_in(t: f64) -> f64 {
    t * t
}

/// Quadratic ease-out.
fn ease_out(t: f64) -> f64 {
    t * (2.0 - t)
}

/// Quadratic ease-in-out.
fn ease_in_out(t: f64) -> f64 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        -1.0 + (4.0 - 2.0 * t) * t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_endpoints() {
        assert!((ease(EasingKind::Linear, 0.0)).abs() < f64::EPSILON);
        assert!((ease(EasingKind::Linear, 1.0) - 1.0).abs() < f64::EPSILON);
        assert!((ease(EasingKind::Linear, 0.5) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn ease_in_endpoints() {
        assert!((ease(EasingKind::EaseIn, 0.0)).abs() < f64::EPSILON);
        assert!((ease(EasingKind::EaseIn, 1.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ease_out_endpoints() {
        assert!((ease(EasingKind::EaseOut, 0.0)).abs() < f64::EPSILON);
        assert!((ease(EasingKind::EaseOut, 1.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn ease_in_out_endpoints() {
        assert!((ease(EasingKind::EaseInOut, 0.0)).abs() < f64::EPSILON);
        assert!((ease(EasingKind::EaseInOut, 1.0) - 1.0).abs() < f64::EPSILON);
        // Midpoint should be ~0.5 for a symmetric curve.
        assert!((ease(EasingKind::EaseInOut, 0.5) - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn clamped_outside_range() {
        assert!((ease(EasingKind::Linear, -0.5)).abs() < f64::EPSILON);
        assert!((ease(EasingKind::Linear, 1.5) - 1.0).abs() < f64::EPSILON);
    }
}
