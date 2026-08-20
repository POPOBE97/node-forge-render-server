//! Easing functions for state-machine transitions.

use std::f64::consts::PI;

use super::types::TimelinePreset;

/// Evaluate a Timeline preset and its first derivative at `t` ∈ [0, 1].
pub fn timeline_curve(kind: TimelinePreset, t: f64) -> (f64, f64) {
    let t = t.clamp(0.0, 1.0);
    match kind {
        TimelinePreset::Linear => (t, 1.0),
        TimelinePreset::EaseIn => ease_in(t, 2),
        TimelinePreset::EaseOut => ease_out(t, 2),
        TimelinePreset::EaseInOut => ease_in_out(t, 2),
        TimelinePreset::EaseInCubic => ease_in(t, 3),
        TimelinePreset::EaseOutCubic => ease_out(t, 3),
        TimelinePreset::EaseInOutCubic => ease_in_out(t, 3),
        TimelinePreset::EaseInQuartic => ease_in(t, 4),
        TimelinePreset::EaseOutQuartic => ease_out(t, 4),
        TimelinePreset::EaseInOutQuartic => ease_in_out(t, 4),
        TimelinePreset::SineIn | TimelinePreset::CosineOut => {
            (1.0 - (PI * t / 2.0).cos(), PI * (PI * t / 2.0).sin() / 2.0)
        }
        TimelinePreset::SineOut | TimelinePreset::CosineIn => {
            ((PI * t / 2.0).sin(), PI * (PI * t / 2.0).cos() / 2.0)
        }
        TimelinePreset::SineInOut | TimelinePreset::CosineInOut => {
            ((1.0 - (PI * t).cos()) / 2.0, PI * (PI * t).sin() / 2.0)
        }
    }
}

fn ease_in(t: f64, power: i32) -> (f64, f64) {
    (t.powi(power), f64::from(power) * t.powi(power - 1))
}

fn ease_out(t: f64, power: i32) -> (f64, f64) {
    (
        1.0 - (1.0 - t).powi(power),
        f64::from(power) * (1.0 - t).powi(power - 1),
    )
}

fn ease_in_out(t: f64, power: i32) -> (f64, f64) {
    if t < 0.5 {
        (
            2.0_f64.powi(power - 1) * t.powi(power),
            f64::from(power) * 2.0_f64.powi(power - 1) * t.powi(power - 1),
        )
    } else {
        (
            1.0 - (2.0 - 2.0 * t).powi(power) / 2.0,
            f64::from(power) * (2.0 - 2.0 * t).powi(power - 1),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_endpoints() {
        assert!((timeline_curve(TimelinePreset::Linear, 0.0).0).abs() < f64::EPSILON);
        assert!((timeline_curve(TimelinePreset::Linear, 1.0).0 - 1.0).abs() < f64::EPSILON);
        assert!((timeline_curve(TimelinePreset::Linear, 0.5).0 - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn polynomial_easings_have_expected_samples_and_derivatives() {
        let cases = [
            (TimelinePreset::EaseIn, 0.25, 0.0625, 0.5),
            (TimelinePreset::EaseOut, 0.25, 0.4375, 1.5),
            (TimelinePreset::EaseInOut, 0.25, 0.125, 1.0),
            (TimelinePreset::EaseInCubic, 0.25, 0.015625, 0.1875),
            (TimelinePreset::EaseOutCubic, 0.25, 0.578125, 1.6875),
            (TimelinePreset::EaseInOutCubic, 0.25, 0.0625, 0.75),
            (TimelinePreset::EaseInQuartic, 0.25, 0.00390625, 0.0625),
            (TimelinePreset::EaseOutQuartic, 0.25, 0.68359375, 1.6875),
            (TimelinePreset::EaseInOutQuartic, 0.25, 0.03125, 0.5),
        ];

        for (kind, t, expected_value, expected_derivative) in cases {
            assert!(
                (timeline_curve(kind, 0.0).0).abs() < f64::EPSILON,
                "{kind:?} start"
            );
            assert!(
                (timeline_curve(kind, 1.0).0 - 1.0).abs() < f64::EPSILON,
                "{kind:?} end"
            );
            let (value, derivative) = timeline_curve(kind, t);
            assert!(
                (value - expected_value).abs() < f64::EPSILON,
                "{kind:?} value"
            );
            assert!(
                (derivative - expected_derivative).abs() < f64::EPSILON,
                "{kind:?} derivative"
            );
        }

        for kind in [
            TimelinePreset::EaseInOut,
            TimelinePreset::EaseInOutCubic,
            TimelinePreset::EaseInOutQuartic,
        ] {
            assert!(
                (timeline_curve(kind, 0.5).0 - 0.5).abs() < f64::EPSILON,
                "{kind:?} midpoint"
            );
        }
    }

    #[test]
    fn clamped_outside_range() {
        assert!((timeline_curve(TimelinePreset::Linear, -0.5).0).abs() < f64::EPSILON);
        assert!((timeline_curve(TimelinePreset::Linear, 1.5).0 - 1.0).abs() < f64::EPSILON);
    }
}
