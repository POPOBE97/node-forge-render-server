use anyhow::{Result, bail};

/// Deterministic time schedule for state-machine ticks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickSchedule {
    pub start_secs: f64,
    pub end_secs: f64,
    pub fps: u32,
    pub include_end: bool,
}

/// One deterministic tick sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TickSample {
    pub frame_index: usize,
    pub time_secs: f64,
    pub dt_secs: f64,
}

impl TickSchedule {
    pub fn new(start_secs: f64, end_secs: f64, fps: u32, include_end: bool) -> Result<Self> {
        if fps == 0 {
            bail!("tick schedule fps must be > 0");
        }
        if !start_secs.is_finite() || !end_secs.is_finite() {
            bail!("tick schedule start/end must be finite");
        }
        if end_secs < start_secs {
            bail!("tick schedule requires end_secs >= start_secs (got {end_secs} < {start_secs})");
        }

        let duration = end_secs - start_secs;
        let raw_steps = duration * (fps as f64);
        let rounded_steps = raw_steps.round();
        if (raw_steps - rounded_steps).abs() > 1e-9 {
            bail!(
                "tick schedule duration ({duration}) is not aligned to fps ({fps}); expected integral frame steps"
            );
        }

        Ok(Self {
            start_secs,
            end_secs,
            fps,
            include_end,
        })
    }

    pub fn frame_count(&self) -> usize {
        let duration = self.end_secs - self.start_secs;
        let steps = (duration * (self.fps as f64)).round() as usize;
        if self.include_end { steps + 1 } else { steps }
    }

    pub fn samples(&self) -> Vec<TickSample> {
        let frame_count = self.frame_count();
        let mut out = Vec::with_capacity(frame_count);
        let step = 1.0 / (self.fps as f64);

        for idx in 0..frame_count {
            let time_secs = self.start_secs + (idx as f64) * step;
            let dt_secs = if idx == 0 { 0.0 } else { step };
            out.push(TickSample {
                frame_index: idx,
                time_secs,
                dt_secs,
            });
        }

        out
    }
}

/// Build `frame_count` evenly-spaced samples over `[start_secs, end_secs]`.
///
/// This helper is useful for coarse rendering sweeps while keeping tick/sample
/// generation deterministic and shared with animation tests.
pub fn evenly_spaced_samples(
    start_secs: f64,
    end_secs: f64,
    frame_count: usize,
) -> Result<Vec<TickSample>> {
    if frame_count == 0 {
        bail!("frame_count must be > 0");
    }
    if !start_secs.is_finite() || !end_secs.is_finite() {
        bail!("start/end must be finite");
    }
    if end_secs < start_secs {
        bail!("end_secs must be >= start_secs");
    }

    if frame_count == 1 {
        return Ok(vec![TickSample {
            frame_index: 0,
            time_secs: start_secs,
            dt_secs: 0.0,
        }]);
    }

    let mut out: Vec<TickSample> = Vec::with_capacity(frame_count);
    for idx in 0..frame_count {
        let alpha = (idx as f64) / ((frame_count - 1) as f64);
        let time_secs = start_secs + (end_secs - start_secs) * alpha;
        let dt_secs = if idx == 0 {
            0.0
        } else {
            time_secs - out[idx - 1].time_secs
        };
        out.push(TickSample {
            frame_index: idx,
            time_secs,
            dt_secs,
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inclusive_0_to_10_at_60fps_has_601_frames() {
        let schedule = TickSchedule::new(0.0, 10.0, 60, true).unwrap();
        let samples = schedule.samples();

        assert_eq!(samples.len(), 601);
        assert!((samples[0].time_secs - 0.0).abs() < 1e-12);
        assert!((samples[600].time_secs - 10.0).abs() < 1e-12);
    }

    #[test]
    fn dt_is_one_over_fps_after_frame_zero() {
        let schedule = TickSchedule::new(0.0, 10.0, 60, true).unwrap();
        let samples = schedule.samples();
        let expected = 1.0 / 60.0;

        for sample in samples.iter().skip(1) {
            assert!((sample.dt_secs - expected).abs() < 1e-12);
        }
    }

    #[test]
    fn evenly_spaced_helper_matches_expected_endpoints() {
        let samples = evenly_spaced_samples(0.0, 10.0, 10).unwrap();
        assert_eq!(samples.len(), 10);
        assert!((samples.first().unwrap().time_secs - 0.0).abs() < 1e-12);
        assert!((samples.last().unwrap().time_secs - 10.0).abs() < 1e-12);
    }
}
