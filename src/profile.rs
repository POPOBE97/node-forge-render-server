use std::{
    collections::BTreeMap,
    fs::OpenOptions,
    io::{self, Write},
    path::PathBuf,
};

use anyhow::{Context, Result};
use rust_wgpu_fiber::{
    eframe::wgpu,
    shader_space::{PassProfileSample, RenderProfile},
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::ui::resource_tree::{PassInfo, ResourceSnapshot};

pub const PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug)]
pub struct ProfileRunConfig {
    pub frames: u32,
    pub warmup_frames: u32,
}

impl Default for ProfileRunConfig {
    fn default() -> Self {
        Self {
            frames: 1,
            warmup_frames: 0,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ProfileOutputTarget {
    Stdout,
    File(PathBuf),
}

impl ProfileOutputTarget {
    pub fn is_stdout(&self) -> bool {
        matches!(self, Self::Stdout)
    }
}

pub struct ProfileWriter {
    out: Box<dyn Write>,
}

impl ProfileWriter {
    pub fn new(target: &ProfileOutputTarget) -> Result<Self> {
        let out: Box<dyn Write> = match target {
            ProfileOutputTarget::Stdout => Box::new(io::stdout()),
            ProfileOutputTarget::File(path) => {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).with_context(|| {
                        format!("failed to create profile output dir {}", parent.display())
                    })?;
                }
                Box::new(
                    OpenOptions::new()
                        .create(true)
                        .truncate(true)
                        .write(true)
                        .open(path)
                        .with_context(|| {
                            format!("failed to open profile output {}", path.display())
                        })?,
                )
            }
        };
        Ok(Self { out })
    }

    pub fn emit<T: Serialize>(&mut self, event: &T) -> Result<()> {
        serde_json::to_writer(&mut self.out, event).context("failed to write profile event")?;
        self.out
            .write_all(b"\n")
            .context("failed to write profile newline")?;
        Ok(())
    }

    pub fn flush(&mut self) -> Result<()> {
        self.out.flush().context("failed to flush profile output")
    }
}

#[derive(Default)]
pub struct ProfileAccumulator {
    frame_wall_ms: Vec<f64>,
    frame_cpu_encode_ms: Vec<f64>,
    frame_queue_wait_ms: Vec<f64>,
    pass_cpu_encode_ms: BTreeMap<String, Vec<f64>>,
}

impl ProfileAccumulator {
    pub fn observe_frame(&mut self, profile: &RenderProfile) {
        self.frame_wall_ms.push(profile.frame_wall_ms);
        self.frame_cpu_encode_ms.push(profile.frame_cpu_encode_ms);
        if let Some(wait_ms) = profile.queue_wait_ms {
            self.frame_queue_wait_ms.push(wait_ms);
        }
        for pass in &profile.passes {
            self.pass_cpu_encode_ms
                .entry(pass.pass_name.clone())
                .or_default()
                .push(pass.cpu_encode_ms);
        }
    }

    pub fn summary(&self) -> Value {
        let pass_cpu_rows = sorted_stats_rows(&self.pass_cpu_encode_ms);

        json!({
            "frames": {
                "wallMs": stats_value(&self.frame_wall_ms),
                "cpuEncodeMs": stats_value(&self.frame_cpu_encode_ms),
                "queueWaitMs": stats_value(&self.frame_queue_wait_ms),
            },
            "topPassesByCpuEncodeMs": pass_cpu_rows
                .into_iter()
                .take(8)
                .map(|(pass_id, stats, _)| json!({
                    "passId": pass_id,
                    "cpuEncodeMs": stats,
                }))
                .collect::<Vec<_>>(),
        })
    }
}

pub fn run_id() -> String {
    format!("profile-{}", crate::protocol::now_millis())
}

pub fn run_start_event(run_id: &str, config: &ProfileRunConfig, output_path: &str) -> Value {
    json!({
        "event": "profile_run_start",
        "schemaVersion": PROFILE_SCHEMA_VERSION,
        "runId": run_id,
        "timestamp": crate::protocol::now_millis(),
        "config": {
            "frames": config.frames,
            "warmupFrames": config.warmup_frames,
            "outputPath": output_path,
        }
    })
}

pub fn adapter_info_event(run_id: &str, adapter: &wgpu::Adapter, _device: &wgpu::Device) -> Value {
    let info = adapter.get_info();
    json!({
        "event": "adapter_info",
        "runId": run_id,
        "timestamp": crate::protocol::now_millis(),
        "adapter": {
            "name": info.name,
            "vendor": info.vendor,
            "device": info.device,
            "deviceType": format!("{:?}", info.device_type),
            "driver": info.driver,
            "driverInfo": info.driver_info,
            "backend": format!("{:?}", info.backend),
        },
    })
}

pub fn scene_info_event(
    run_id: &str,
    resolution: [u32; 2],
    output_texture: &str,
    export_texture: &str,
    snapshot: &ResourceSnapshot,
) -> Value {
    json!({
        "event": "scene_info",
        "runId": run_id,
        "timestamp": crate::protocol::now_millis(),
        "scene": {
            "resolution": resolution,
            "outputTexture": output_texture,
            "exportTexture": export_texture,
            "passCount": snapshot.passes.len(),
        }
    })
}

pub fn warning_event(run_id: &str, code: &str, message: &str) -> Value {
    json!({
        "event": "profile_warning",
        "runId": run_id,
        "timestamp": crate::protocol::now_millis(),
        "code": code,
        "message": message,
    })
}

pub fn frame_sample_event(run_id: &str, frame_index: u32, profile: &RenderProfile) -> Value {
    json!({
        "event": "frame_sample",
        "runId": run_id,
        "timestamp": crate::protocol::now_millis(),
        "frameIndex": frame_index,
        "metrics": {
            "cpu.frame_encode.ms": profile.frame_cpu_encode_ms,
            "cpu.submit.ms": profile.submit_cpu_ms,
            "cpu.queue_wait.ms": profile.queue_wait_ms,
            "wall.frame.ms": profile.frame_wall_ms,
        },
        "passCount": profile.passes.len(),
    })
}

pub fn pass_sample_event(
    run_id: &str,
    frame_index: u32,
    sample: &PassProfileSample,
    pass_info: Option<&PassInfo>,
) -> Value {
    let frame_percent = None::<f64>;
    let pass = json!({
        "passId": sample.pass_name,
        "orderIndex": sample.order_index,
        "nodeId": pass_info.and_then(|info| info.source_node_id.as_deref()),
        "nodeType": pass_info.and_then(|info| info.source_node_type.as_deref()),
        "pipelineKind": sample.pipeline_kind.as_str(),
        "targetTexture": pass_info.and_then(|info| info.target_texture.as_deref()),
        "targetSize": pass_info.and_then(|info| info.target_size.map(|(w, h)| [w, h])),
        "targetFormat": pass_info.and_then(|info| info.target_format.as_deref()),
        "sampledTextures": pass_info
            .map(|info| info.sampled_textures.clone())
            .unwrap_or_default(),
        "draw": {
            "vertices": sample.vertex_count,
            "instances": sample.instance_count,
        },
        "compute": {
            "workgroups": sample.workgroup_count,
        }
    });

    json!({
        "event": "pass_sample",
        "runId": run_id,
        "timestamp": crate::protocol::now_millis(),
        "frameIndex": frame_index,
        "pass": pass,
        "metrics": {
            "cpu.encode.ms": sample.cpu_encode_ms,
            "frame.percent": frame_percent,
        },
    })
}

pub fn run_end_event(run_id: &str, output_path: &str, accumulator: &ProfileAccumulator) -> Value {
    json!({
        "event": "profile_run_end",
        "runId": run_id,
        "timestamp": crate::protocol::now_millis(),
        "status": "ok",
        "outputPath": output_path,
        "summary": accumulator.summary(),
    })
}

pub fn pass_info_by_name(snapshot: &ResourceSnapshot) -> BTreeMap<&str, &PassInfo> {
    snapshot
        .passes
        .iter()
        .map(|info| (info.name.as_str(), info))
        .collect()
}

fn sorted_stats_rows(rows: &BTreeMap<String, Vec<f64>>) -> Vec<(String, Value, f64)> {
    let mut pass_rows = rows
        .iter()
        .map(|(pass_id, values)| {
            let stats = stats_value(values);
            let mean = stats
                .get("mean")
                .and_then(Value::as_f64)
                .unwrap_or_default();
            (pass_id.clone(), stats, mean)
        })
        .collect::<Vec<_>>();
    pass_rows.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
    pass_rows
}

fn stats_value(values: &[f64]) -> Value {
    if values.is_empty() {
        return json!({
            "count": 0,
            "mean": null,
            "median": null,
            "p95": null,
            "min": null,
            "max": null,
        });
    }

    let mut sorted = values.to_vec();
    sorted.sort_by(f64::total_cmp);
    let count = sorted.len();
    let sum = sorted.iter().sum::<f64>();
    let mean = sum / count as f64;
    let median = percentile_sorted(&sorted, 0.5);
    let p95 = percentile_sorted(&sorted, 0.95);
    json!({
        "count": count,
        "mean": mean,
        "median": median,
        "p95": p95,
        "min": sorted[0],
        "max": sorted[count - 1],
    })
}

fn percentile_sorted(sorted: &[f64], p: f64) -> f64 {
    if sorted.len() == 1 {
        return sorted[0];
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
