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
    frame_gpu_duration_ms: Vec<f64>,
    frame_queue_wait_ms: Vec<f64>,
    pass_cpu_encode_ms: BTreeMap<String, Vec<f64>>,
    pass_gpu_duration_ms: BTreeMap<String, Vec<f64>>,
    pass_vertex_shader_invocations: BTreeMap<String, Vec<f64>>,
    pass_clipper_invocations: BTreeMap<String, Vec<f64>>,
    pass_clipper_primitives_out: BTreeMap<String, Vec<f64>>,
    pass_fragment_shader_invocations: BTreeMap<String, Vec<f64>>,
    pass_compute_shader_invocations: BTreeMap<String, Vec<f64>>,
}

impl ProfileAccumulator {
    pub fn observe_frame(&mut self, profile: &RenderProfile) {
        self.frame_wall_ms.push(profile.frame_wall_ms);
        self.frame_cpu_encode_ms.push(profile.frame_cpu_encode_ms);
        if let Some(gpu_ms) = profile.frame_gpu_duration_ms {
            self.frame_gpu_duration_ms.push(gpu_ms);
        }
        if let Some(wait_ms) = profile.queue_wait_ms {
            self.frame_queue_wait_ms.push(wait_ms);
        }
        for pass in &profile.passes {
            self.pass_cpu_encode_ms
                .entry(pass.pass_name.clone())
                .or_default()
                .push(pass.cpu_encode_ms);
            if let Some(gpu_ms) = pass.gpu_duration_ms {
                self.pass_gpu_duration_ms
                    .entry(pass.pass_name.clone())
                    .or_default()
                    .push(gpu_ms);
            }
            observe_u64_metric(
                &mut self.pass_vertex_shader_invocations,
                pass.pass_name.as_str(),
                pass.vertex_shader_invocations,
            );
            observe_u64_metric(
                &mut self.pass_clipper_invocations,
                pass.pass_name.as_str(),
                pass.clipper_invocations,
            );
            observe_u64_metric(
                &mut self.pass_clipper_primitives_out,
                pass.pass_name.as_str(),
                pass.clipper_primitives_out,
            );
            observe_u64_metric(
                &mut self.pass_fragment_shader_invocations,
                pass.pass_name.as_str(),
                pass.fragment_shader_invocations,
            );
            observe_u64_metric(
                &mut self.pass_compute_shader_invocations,
                pass.pass_name.as_str(),
                pass.compute_shader_invocations,
            );
        }
    }

    pub fn summary(&self) -> Value {
        let pass_cpu_rows = sorted_stats_rows(&self.pass_cpu_encode_ms);
        let pass_gpu_rows = sorted_stats_rows(&self.pass_gpu_duration_ms);
        let pass_fragment_rows = sorted_stats_rows(&self.pass_fragment_shader_invocations);
        let pass_compute_rows = sorted_stats_rows(&self.pass_compute_shader_invocations);

        json!({
            "frames": {
                "wallMs": stats_value(&self.frame_wall_ms),
                "cpuEncodeMs": stats_value(&self.frame_cpu_encode_ms),
                "gpuDurationMs": stats_value(&self.frame_gpu_duration_ms),
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
            "topPassesByGpuDurationMs": pass_gpu_rows
                .into_iter()
                .take(8)
                .map(|(pass_id, stats, _)| json!({
                    "passId": pass_id,
                    "gpuDurationMs": stats,
                }))
                .collect::<Vec<_>>(),
            "topPassesByFragmentShaderInvocations": pass_fragment_rows
                .into_iter()
                .take(8)
                .map(|(pass_id, stats, _)| json!({
                    "passId": pass_id,
                    "fragmentShaderInvocations": stats,
                }))
                .collect::<Vec<_>>(),
            "topPassesByComputeShaderInvocations": pass_compute_rows
                .into_iter()
                .take(8)
                .map(|(pass_id, stats, _)| json!({
                    "passId": pass_id,
                    "computeShaderInvocations": stats,
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

pub fn adapter_info_event(run_id: &str, adapter: &wgpu::Adapter, device: &wgpu::Device) -> Value {
    let info = adapter.get_info();
    let features = device.features();
    let metal_runtime_counter_provider = metal_runtime_counter_provider_status();
    let hardware_counters_available = metal_runtime_counter_provider
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
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
        "capabilities": {
            "timestampQuery": features.contains(wgpu::Features::TIMESTAMP_QUERY),
            "pipelineStatisticsQuery": features.contains(wgpu::Features::PIPELINE_STATISTICS_QUERY),
            "gpuDurationSource": if features.contains(wgpu::Features::TIMESTAMP_QUERY) {
                "wgpu.timestamp_query"
            } else {
                "unavailable"
            },
            "hardwareCounters": false,
            "hardwareCountersCollected": false,
            "hardwareCountersProviderAvailable": hardware_counters_available,
            "hardwareCountersSource": if hardware_counters_available {
                Some("metal.runtime.counter_sets")
            } else {
                None
            },
            "advancedCounterProviders": {
                "metalRuntime": metal_runtime_counter_provider,
            },
            "advancedMetricFields": [
                "gpu.duration.ms",
                "gpu.pipeline.vertex_shader_invocations.count",
                "gpu.pipeline.clipper_invocations.count",
                "gpu.pipeline.clipper_primitives_out.count",
                "gpu.pipeline.fragment_shader_invocations.count",
                "gpu.pipeline.compute_shader_invocations.count",
                "gpu.occupancy.total.percent",
                "gpu.limiter.alu.percent",
                "gpu.memory.bandwidth.bytes_per_second"
            ],
        }
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
    let timestamp = timestamp_capability(profile);
    let pipeline_statistics = pipeline_statistics_capability(profile);
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
            "gpu.duration.ms": profile.frame_gpu_duration_ms,
        },
        "passCount": profile.passes.len(),
        "capabilities": {
            "gpu.duration.ms": timestamp,
            "timestampQuery": {
                "supported": profile.timestamp_query_supported,
                "used": profile.timestamp_query_used,
                "periodNs": profile.timestamp_period_ns,
                "error": profile.timestamp_error.as_deref(),
            },
            "pipelineStatisticsQuery": {
                "supported": profile.pipeline_statistics_supported,
                "used": profile.pipeline_statistics_used,
                "error": profile.pipeline_statistics_error.as_deref(),
                "metrics": pipeline_statistics,
            },
            "hardwareCounters": "not_collected",
            "advancedMetrics": advanced_metric_capabilities(),
        }
    })
}

pub fn pass_sample_event(
    run_id: &str,
    frame_index: u32,
    sample: &PassProfileSample,
    pass_info: Option<&PassInfo>,
) -> Value {
    let frame_percent = None::<f64>;
    let gpu_duration_capability = if sample.gpu_duration_ms.is_some() {
        "timestamp_query"
    } else {
        "unavailable"
    };
    let pipeline_statistics_capability = if sample.vertex_shader_invocations.is_some()
        || sample.clipper_invocations.is_some()
        || sample.clipper_primitives_out.is_some()
        || sample.fragment_shader_invocations.is_some()
        || sample.compute_shader_invocations.is_some()
    {
        "pipeline_statistics_query"
    } else {
        "unavailable"
    };
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
            "gpu.duration.ms": sample.gpu_duration_ms,
            "gpu.pipeline.vertex_shader_invocations.count": sample.vertex_shader_invocations,
            "gpu.pipeline.clipper_invocations.count": sample.clipper_invocations,
            "gpu.pipeline.clipper_primitives_out.count": sample.clipper_primitives_out,
            "gpu.pipeline.fragment_shader_invocations.count": sample.fragment_shader_invocations,
            "gpu.pipeline.compute_shader_invocations.count": sample.compute_shader_invocations,
            "gpu.occupancy.total.percent": sample.gpu_occupancy_total_percent,
            "gpu.limiter.alu.percent": sample.gpu_limiter_alu_percent,
            "gpu.memory.bandwidth.bytes_per_second": sample.gpu_memory_bandwidth_bytes_per_second,
        },
        "capabilities": {
            "gpu.duration.ms": gpu_duration_capability,
            "pipelineStatistics": pipeline_statistics_capability,
            "hardwareCounters": "not_collected",
            "advancedMetrics": advanced_metric_capabilities(),
        }
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

fn observe_u64_metric(rows: &mut BTreeMap<String, Vec<f64>>, pass_id: &str, value: Option<u64>) {
    if let Some(value) = value {
        rows.entry(pass_id.to_string())
            .or_default()
            .push(value as f64);
    }
}

fn timestamp_capability(profile: &RenderProfile) -> &'static str {
    if profile.frame_gpu_duration_ms.is_some()
        || profile
            .passes
            .iter()
            .any(|pass| pass.gpu_duration_ms.is_some())
    {
        "timestamp_query"
    } else if profile.timestamp_query_used && profile.timestamp_error.is_some() {
        "timestamp_query_failed"
    } else if profile.timestamp_query_supported {
        "timestamp_query_supported_not_used"
    } else {
        "unavailable"
    }
}

fn pipeline_statistics_capability(profile: &RenderProfile) -> Value {
    let status = if profile
        .passes
        .iter()
        .any(|pass| {
            pass.vertex_shader_invocations.is_some()
                || pass.clipper_invocations.is_some()
                || pass.clipper_primitives_out.is_some()
                || pass.fragment_shader_invocations.is_some()
                || pass.compute_shader_invocations.is_some()
        }) {
        "pipeline_statistics_query"
    } else if profile.pipeline_statistics_used && profile.pipeline_statistics_error.is_some() {
        "pipeline_statistics_query_failed"
    } else if profile.pipeline_statistics_supported {
        "pipeline_statistics_query_supported_not_used"
    } else {
        "unavailable"
    };

    json!({
        "status": status,
        "fields": [
            "gpu.pipeline.vertex_shader_invocations.count",
            "gpu.pipeline.clipper_invocations.count",
            "gpu.pipeline.clipper_primitives_out.count",
            "gpu.pipeline.fragment_shader_invocations.count",
            "gpu.pipeline.compute_shader_invocations.count"
        ]
    })
}

fn advanced_metric_capabilities() -> Value {
    json!({
        "gpu.pipeline.vertex_shader_invocations.count": {
            "status": "collected_when_supported",
            "source": "wgpu.pipeline_statistics_query",
        },
        "gpu.pipeline.clipper_invocations.count": {
            "status": "collected_when_supported",
            "source": "wgpu.pipeline_statistics_query",
        },
        "gpu.pipeline.clipper_primitives_out.count": {
            "status": "collected_when_supported",
            "source": "wgpu.pipeline_statistics_query",
        },
        "gpu.pipeline.fragment_shader_invocations.count": {
            "status": "collected_when_supported",
            "source": "wgpu.pipeline_statistics_query",
        },
        "gpu.pipeline.compute_shader_invocations.count": {
            "status": "collected_when_supported",
            "source": "wgpu.pipeline_statistics_query",
        },
        "gpu.occupancy.total.percent": {
            "status": "unsupported_by_wgpu_and_public_metal_runtime_on_this_device",
            "source": null,
        },
        "gpu.limiter.alu.percent": {
            "status": "unsupported_by_wgpu_and_public_metal_runtime_on_this_device",
            "source": null,
        },
        "gpu.memory.bandwidth.bytes_per_second": {
            "status": "unsupported_by_wgpu_and_public_metal_runtime_on_this_device",
            "source": null,
        },
    })
}

#[cfg(target_os = "macos")]
fn metal_runtime_counter_provider_status() -> Value {
    let Some(device) = metal::Device::system_default() else {
        return json!({
            "available": false,
            "status": "device_unavailable",
            "collectionMode": "in_process_runtime_api",
        });
    };

    let counter_set_names = device
        .counter_sets()
        .iter()
        .map(|set| set.name().to_string())
        .collect::<Vec<_>>();
    let normalized_counter_sets = counter_set_names
        .iter()
        .map(|name| normalize_counter_set_name(name))
        .collect::<Vec<_>>();
    let has_counter_set = |name: &str| {
        normalized_counter_sets
            .iter()
            .any(|counter_set| counter_set == name)
    };
    let has_occupancy_named_set = normalized_counter_sets
        .iter()
        .any(|counter_set| counter_set.contains("occupancy"));

    json!({
        "available": !counter_set_names.is_empty(),
        "status": if counter_set_names.is_empty() {
            "unavailable"
        } else {
            "available"
        },
        "collectionMode": "in_process_runtime_api",
        "device": device.name(),
        "samplingPoints": {
            "stageBoundary": device.supports_counter_sampling(metal::MTLCounterSamplingPoint::AtStageBoundary),
            "drawBoundary": device.supports_counter_sampling(metal::MTLCounterSamplingPoint::AtDrawBoundary),
            "dispatchBoundary": device.supports_counter_sampling(metal::MTLCounterSamplingPoint::AtDispatchBoundary),
            "blitBoundary": device.supports_counter_sampling(metal::MTLCounterSamplingPoint::AtBlitBoundary),
        },
        "counterSets": counter_set_names,
        "commonCounterSets": {
            "timestamp": has_counter_set("timestamp"),
            "stageUtilization": has_counter_set("stageutilization"),
            "statistic": has_counter_set("statistic") || has_counter_set("statistics"),
            "occupancy": has_occupancy_named_set,
        },
        "notes": [
            "Public Metal runtime counters are limited to the device's exposed counterSets.",
            "On this device, occupancy is unavailable unless an exposed counter set contains occupancy."
        ],
    })
}

#[cfg(not(target_os = "macos"))]
fn metal_runtime_counter_provider_status() -> Value {
    json!({
        "available": false,
        "status": "non_macos",
        "collectionMode": "in_process_runtime_api",
    })
}

fn normalize_counter_set_name(name: &str) -> String {
    name.chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
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
