use std::collections::HashMap;

use rust_wgpu_fiber::eframe::egui::{Pos2, Rect, Vec2};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Easing {
    Linear,
    EaseOutCubic,
}

#[derive(Clone, Copy, Debug)]
pub struct AnimationSpec<T> {
    pub from: T,
    pub to: T,
    pub duration_secs: f64,
    pub easing: Easing,
}

#[derive(Clone, Copy, Debug)]
pub enum AnimationValue {
    F32(f32),
    Vec2(Vec2),
    Pos2(Pos2),
    Rect(Rect),
}

pub trait AnimationTyped: Copy {
    fn into_value(self) -> AnimationValue;
    fn from_value(value: AnimationValue) -> Option<Self>;
    fn lerp(a: Self, b: Self, t: f32) -> Self;
}

impl AnimationTyped for f32 {
    fn into_value(self) -> AnimationValue {
        AnimationValue::F32(self)
    }

    fn from_value(value: AnimationValue) -> Option<Self> {
        match value {
            AnimationValue::F32(v) => Some(v),
            _ => None,
        }
    }

    fn lerp(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl AnimationTyped for Vec2 {
    fn into_value(self) -> AnimationValue {
        AnimationValue::Vec2(self)
    }

    fn from_value(value: AnimationValue) -> Option<Self> {
        match value {
            AnimationValue::Vec2(v) => Some(v),
            _ => None,
        }
    }

    fn lerp(a: Self, b: Self, t: f32) -> Self {
        a + (b - a) * t
    }
}

impl AnimationTyped for Pos2 {
    fn into_value(self) -> AnimationValue {
        AnimationValue::Pos2(self)
    }

    fn from_value(value: AnimationValue) -> Option<Self> {
        match value {
            AnimationValue::Pos2(v) => Some(v),
            _ => None,
        }
    }

    fn lerp(a: Self, b: Self, t: f32) -> Self {
        Pos2::new(a.x + (b.x - a.x) * t, a.y + (b.y - a.y) * t)
    }
}

impl AnimationTyped for Rect {
    fn into_value(self) -> AnimationValue {
        AnimationValue::Rect(self)
    }

    fn from_value(value: AnimationValue) -> Option<Self> {
        match value {
            AnimationValue::Rect(v) => Some(v),
            _ => None,
        }
    }

    fn lerp(a: Self, b: Self, t: f32) -> Self {
        Rect::from_min_max(
            Pos2::new(
                a.min.x + (b.min.x - a.min.x) * t,
                a.min.y + (b.min.y - a.min.y) * t,
            ),
            Pos2::new(
                a.max.x + (b.max.x - a.max.x) * t,
                a.max.y + (b.max.y - a.max.y) * t,
            ),
        )
    }
}

#[derive(Clone, Copy, Debug)]
struct StoredAnimation {
    start_time: f64,
    duration_secs: f64,
    easing: Easing,
    from: AnimationValue,
    to: AnimationValue,
}

#[derive(Default)]
pub struct AnimationManager {
    animations: HashMap<&'static str, StoredAnimation>,
}

impl AnimationManager {
    pub fn start<T: AnimationTyped>(
        &mut self,
        key: &'static str,
        spec: AnimationSpec<T>,
        now: f64,
    ) {
        self.animations.insert(
            key,
            StoredAnimation {
                start_time: now,
                duration_secs: spec.duration_secs.max(0.0),
                easing: spec.easing,
                from: spec.from.into_value(),
                to: spec.to.into_value(),
            },
        );
    }

    pub fn sample_f32(&mut self, key: &'static str, now: f64) -> Option<(f32, bool)> {
        self.sample_typed::<f32>(key, now)
    }

    pub fn sample_vec2(&mut self, key: &'static str, now: f64) -> Option<(Vec2, bool)> {
        self.sample_typed::<Vec2>(key, now)
    }

    pub fn sample_pos2(&mut self, key: &'static str, now: f64) -> Option<(Pos2, bool)> {
        self.sample_typed::<Pos2>(key, now)
    }

    pub fn sample_rect(&mut self, key: &'static str, now: f64) -> Option<(Rect, bool)> {
        self.sample_typed::<Rect>(key, now)
    }

    pub fn is_active(&self, key: &'static str) -> bool {
        self.animations.contains_key(key)
    }

    pub fn clear(&mut self, key: &'static str) {
        self.animations.remove(key);
    }

    fn sample_typed<T: AnimationTyped>(
        &mut self,
        key: &'static str,
        now: f64,
    ) -> Option<(T, bool)> {
        let stored = *self.animations.get(key)?;

        let progress = if stored.duration_secs <= f64::EPSILON {
            1.0
        } else {
            ((now - stored.start_time) / stored.duration_secs).clamp(0.0, 1.0)
        };

        let eased = apply_easing(stored.easing, progress as f32);
        let from = T::from_value(stored.from)?;
        let to = T::from_value(stored.to)?;
        let value = T::lerp(from, to, eased);
        let done = progress >= 1.0;
        if done {
            self.animations.remove(key);
        }
        Some((value, done))
    }
}

fn apply_easing(easing: Easing, t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    match easing {
        Easing::Linear => t,
        Easing::EaseOutCubic => 1.0 - (1.0 - t).powi(3),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_animation_starts_progresses_and_completes() {
        let mut mgr = AnimationManager::default();
        mgr.start(
            "anim",
            AnimationSpec {
                from: 0.0f32,
                to: 10.0f32,
                duration_secs: 1.0,
                easing: Easing::Linear,
            },
            100.0,
        );

        let (v0, done0) = mgr.sample_f32("anim", 100.0).unwrap();
        assert_eq!(v0, 0.0);
        assert!(!done0);

        let (v1, done1) = mgr.sample_f32("anim", 100.5).unwrap();
        assert!((v1 - 5.0).abs() < 1e-5);
        assert!(!done1);

        let (v2, done2) = mgr.sample_f32("anim", 101.0).unwrap();
        assert_eq!(v2, 10.0);
        assert!(done2);
        assert!(!mgr.is_active("anim"));
    }

    #[test]
    fn replacing_same_key_is_deterministic() {
        let mut mgr = AnimationManager::default();
        mgr.start(
            "anim",
            AnimationSpec {
                from: 0.0f32,
                to: 10.0f32,
                duration_secs: 10.0,
                easing: Easing::Linear,
            },
            0.0,
        );
        mgr.start(
            "anim",
            AnimationSpec {
                from: 2.0f32,
                to: 6.0f32,
                duration_secs: 4.0,
                easing: Easing::Linear,
            },
            1.0,
        );

        let (value, done) = mgr.sample_f32("anim", 3.0).unwrap();
        assert!(!done);
        assert!((value - 4.0).abs() < 1e-5);
    }

    #[test]
    fn ease_out_cubic_is_monotonic_in_unit_interval() {
        let mut last = 0.0f32;
        for i in 0..=100 {
            let t = i as f32 / 100.0;
            let v = apply_easing(Easing::EaseOutCubic, t);
            assert!(v >= last);
            assert!((0.0..=1.0).contains(&v));
            last = v;
        }
    }
}
