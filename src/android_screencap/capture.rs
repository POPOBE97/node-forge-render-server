use std::time::Instant;

use anyhow::bail;

use crate::android_reference::{parse_adb_devices, select_single_ready_usb_device};
use crate::reference_image::{
    DecodedReferencePixels, MAGIC, PayloadKind, ReferencePayloadKind, decode_png_bytes,
    decode_private_envelope, encode_srgb_png_from_linear, envelope_from_encoded_png,
    figma_srgb_png_from_source, parse_envelope, payload_kind_label,
};

use super::{
    adb::{
        self, AdbCommand, AdbTransport, HELPER_CAP, PROBE_CAP, REQUEST_DEADLINE, RealAdbTransport,
        STOCK_CAP, child_timeout,
    },
    deploy::{
        self, RootForm, cleanup_remote_pid, ensure_helper, helper_launch_script, helper_supported,
        probe_identity, probe_root, root_exec_out_args,
    },
    helper::embedded_helper_bytes,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublicEncoding {
    Source,
    FigmaSrgb,
}

impl PublicEncoding {
    pub fn label(self) -> &'static str {
        match self {
            Self::Source => "source-profile",
            Self::FigmaSrgb => "figma-srgb",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScreencapReady {
    pub serial: String,
    pub width: u32,
    pub height: u32,
    pub private_envelope: Vec<u8>,
    pub public_png: Vec<u8>,
    pub public_encoding: PublicEncoding,
    pub payload_kind: ReferencePayloadKind,
    pub fallback_reason: Option<String>,
    pub captured_hdr_layers: Option<bool>,
    pub alpha: crate::reference_image::Alpha,
    pub high_precision: bool,
}

impl ScreencapReady {
    pub fn log_line(&self) -> String {
        format!(
            "copied {}x{} {} from {} kind={} fallback={} alpha={:?} format={} private={} public={}",
            self.width,
            self.height,
            self.public_encoding.label(),
            self.serial,
            payload_kind_label(self.payload_kind),
            self.fallback_reason.as_deref().unwrap_or("none"),
            self.alpha,
            if self.high_precision {
                "Rgba16Float"
            } else {
                "Rgba8Unorm"
            },
            self.private_envelope.len(),
            self.public_png.len(),
        )
    }
}

pub fn capture_screencap(encoding: PublicEncoding) -> anyhow::Result<ScreencapReady> {
    let mut adb = RealAdbTransport::from_env();
    capture_with(
        &mut adb,
        encoding,
        Instant::now() + REQUEST_DEADLINE,
        next_request_id(),
    )
}

pub fn capture_screencap_with_cancel(
    encoding: PublicEncoding,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<ScreencapReady> {
    let mut adb = RealAdbTransport::from_env().with_cancel(cancel);
    capture_with(
        &mut adb,
        encoding,
        Instant::now() + REQUEST_DEADLINE,
        next_request_id(),
    )
}

fn next_request_id() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

pub fn capture_with(
    adb: &mut dyn AdbTransport,
    encoding: PublicEncoding,
    deadline: Instant,
    request_id: u64,
) -> anyhow::Result<ScreencapReady> {
    let serial = select_serial(adb, deadline)?;
    let helper_attempt = match try_helper(adb, &serial, deadline, request_id) {
        Ok(Some(bundle)) => Some(bundle),
        Ok(None) => None,
        Err(error) => {
            eprintln!("[android-screencap] helper unavailable: {error:#}");
            deploy::invalidate_deployment();
            None
        }
    };

    if let Some(bundle) = helper_attempt {
        return finish_helper_bundle(adb, &serial, encoding, deadline, bundle);
    }

    let png = stock_screencap(adb, &serial, deadline)?;
    let decoded = decode_png_bytes(&png)?;
    let envelope = envelope_from_encoded_png(
        decoded.width,
        decoded.height,
        png.clone(),
        decoded.payload_kind == ReferencePayloadKind::GainmapPng,
    )
    .encode()?;
    finish_public(
        serial,
        encoding,
        envelope,
        png,
        decoded,
        Some("helper-unavailable".to_string()),
        None,
    )
}

struct HelperBundle {
    envelope: crate::reference_image::ReferenceImageEnvelope,
    decoded: DecodedReferencePixels,
    companion_png: Option<Vec<u8>>,
}

fn try_helper(
    adb: &mut dyn AdbTransport,
    serial: &str,
    deadline: Instant,
    request_id: u64,
) -> anyhow::Result<Option<HelperBundle>> {
    if embedded_helper_bytes().is_none() {
        super::helper::log_helper_capability_once();
        return Ok(None);
    }
    let cached = deploy::cached_deployment(serial);
    let (identity, root) = if let Some(state) = cached {
        (state.identity, state.root)
    } else {
        let identity = probe_identity(adb, serial, deadline)?;
        if !helper_supported(&identity) {
            return Ok(None);
        }
        let root = probe_root(adb, serial, deadline)?;
        (identity, root)
    };
    if !helper_supported(&identity) {
        return Ok(None);
    }
    let sha = ensure_helper(adb, serial, root, deadline)?;
    deploy::store_deployment(deploy::DeploymentState {
        identity: identity.clone(),
        root,
        helper_sha256: sha,
    });
    match run_helper(adb, serial, root, deadline, request_id) {
        Ok(bundle) => Ok(Some(bundle)),
        Err(error) => {
            cleanup_remote_pid(
                adb,
                serial,
                root,
                request_id,
                Instant::now() + adb::CLEANUP_GRACE,
            );
            Err(error)
        }
    }
}

fn run_helper(
    adb: &mut dyn AdbTransport,
    serial: &str,
    root: RootForm,
    deadline: Instant,
    request_id: u64,
) -> anyhow::Result<HelperBundle> {
    let script = helper_launch_script(request_id);
    let output = adb.run(AdbCommand::new(
        Some(serial),
        root_exec_out_args(root, &script),
        child_timeout(HELPER_CAP, deadline)?,
    ));
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            cleanup_remote_pid(
                adb,
                serial,
                root,
                request_id,
                Instant::now() + adb::CLEANUP_GRACE,
            );
            return Err(error);
        }
    };
    if !output.success() {
        bail!(
            "helper failed: {}{}",
            output.stdout_utf8(),
            output.stderr_utf8()
        );
    }
    if output.stdout.get(0..8) != Some(&MAGIC) {
        bail!("helper stdout did not begin with protocol magic");
    }
    let envelope = parse_envelope(&output.stdout)?;
    if envelope.payload_kind != PayloadKind::LinearRgba16F {
        bail!("helper did not emit LinearRgba16F primary payload");
    }
    let decoded = decode_private_envelope(&output.stdout)?;
    let companion_png = envelope.source_png.clone();
    Ok(HelperBundle {
        envelope,
        decoded,
        companion_png,
    })
}

fn finish_helper_bundle(
    adb: &mut dyn AdbTransport,
    serial: &str,
    encoding: PublicEncoding,
    deadline: Instant,
    bundle: HelperBundle,
) -> anyhow::Result<ScreencapReady> {
    let mut public_png = match bundle.companion_png {
        Some(png) => png,
        None => match stock_screencap(adb, serial, deadline) {
            Ok(png) => png,
            Err(error) => {
                eprintln!(
                    "[android-screencap] companion/stock public PNG failed; synthesizing from raw: {error:#}"
                );
                encode_srgb_png_from_linear(
                    bundle.decoded.width,
                    bundle.decoded.height,
                    &bundle.decoded.source_linear_rgba,
                    bundle.decoded.alpha,
                )?
            }
        },
    };
    let envelope_bytes = bundle.envelope.encode()?;
    if encoding == PublicEncoding::FigmaSrgb {
        public_png = figma_srgb_png_from_source(&public_png, Some(&bundle.decoded))?;
    } else if encoding == PublicEncoding::Source {
        // Keep companion/stock/synthesized bytes.
    }
    Ok(ScreencapReady {
        serial: serial.to_string(),
        width: bundle.decoded.width,
        height: bundle.decoded.height,
        private_envelope: envelope_bytes,
        public_png,
        public_encoding: encoding,
        payload_kind: if bundle.envelope.captured_hdr_layers {
            ReferencePayloadKind::RawFp16
        } else {
            ReferencePayloadKind::SdrPng
        },
        fallback_reason: None,
        captured_hdr_layers: Some(bundle.envelope.captured_hdr_layers),
        alpha: bundle.decoded.alpha,
        high_precision: bundle.decoded.high_precision,
    })
}

fn finish_public(
    serial: String,
    encoding: PublicEncoding,
    envelope: Vec<u8>,
    source_png: Vec<u8>,
    decoded: DecodedReferencePixels,
    fallback_reason: Option<String>,
    captured_hdr_layers: Option<bool>,
) -> anyhow::Result<ScreencapReady> {
    let public_png = match encoding {
        PublicEncoding::Source => source_png,
        PublicEncoding::FigmaSrgb => figma_srgb_png_from_source(&source_png, None)?,
    };
    Ok(ScreencapReady {
        serial,
        width: decoded.width,
        height: decoded.height,
        private_envelope: envelope,
        public_png,
        public_encoding: encoding,
        payload_kind: decoded.payload_kind,
        fallback_reason,
        captured_hdr_layers,
        alpha: decoded.alpha,
        high_precision: decoded.high_precision,
    })
}

fn stock_screencap(
    adb: &mut dyn AdbTransport,
    serial: &str,
    deadline: Instant,
) -> anyhow::Result<Vec<u8>> {
    let output = adb.run(AdbCommand::new(
        Some(serial),
        ["exec-out", "screencap", "-p"],
        child_timeout(STOCK_CAP, deadline)?,
    ))?;
    if !output.success() {
        bail!(
            "adb screencap failed: {}{}",
            output.stdout_utf8(),
            output.stderr_utf8()
        );
    }
    if output.stdout.is_empty() {
        bail!("adb screencap returned no PNG data");
    }
    Ok(output.stdout)
}

fn select_serial(adb: &mut dyn AdbTransport, deadline: Instant) -> anyhow::Result<String> {
    let output = adb.run(AdbCommand::new(
        None::<&str>,
        ["devices", "-l"],
        child_timeout(PROBE_CAP, deadline)?,
    ))?;
    if !output.success() {
        bail!("adb devices -l failed: {}", output.stderr_utf8());
    }
    let devices = parse_adb_devices(&output.stdout_utf8());
    Ok(select_single_ready_usb_device(&devices)?.serial)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::android_screencap::adb::{ScriptedAdb, ScriptedAdbResponse, ok_stdout};
    use crate::android_screencap::helper::{TARGET_ABI, TARGET_FINGERPRINT, TARGET_SDK};
    use crate::reference_image::{encode_srgb_png, envelope_from_raw_fp16};
    use image::RgbaImage;
    use std::time::Duration;

    fn png() -> Vec<u8> {
        encode_srgb_png(&RgbaImage::from_raw(1, 1, vec![10, 20, 30, 255]).unwrap()).unwrap()
    }

    fn raw_envelope(hdr: bool, with_png: bool) -> Vec<u8> {
        envelope_from_raw_fp16(1, 1, &[1.5, 0.2, 0.1, 1.0], hdr, with_png.then(png))
            .unwrap()
            .encode()
            .unwrap()
    }

    fn devices_ok() -> ScriptedAdbResponse {
        ScriptedAdbResponse {
            prefix: vec!["devices".into(), "-l".into()],
            output: ok_stdout("List of devices attached\nZY22 device usb:1 product:nubia\n"),
            min_timeout: None,
        }
    }

    fn identity_scripts(fingerprint: &str) -> Vec<ScriptedAdbResponse> {
        vec![
            ScriptedAdbResponse {
                prefix: vec![
                    "shell".into(),
                    format!("getprop {key}", key = "ro.product.cpu.abi"),
                ],
                output: ok_stdout(TARGET_ABI),
                min_timeout: None,
            },
            ScriptedAdbResponse {
                prefix: vec!["shell".into(), "getprop ro.build.version.sdk".into()],
                output: ok_stdout(TARGET_SDK),
                min_timeout: None,
            },
            ScriptedAdbResponse {
                prefix: vec!["shell".into(), "getprop ro.build.fingerprint".into()],
                output: ok_stdout(fingerprint),
                min_timeout: None,
            },
            ScriptedAdbResponse {
                prefix: vec!["shell".into(), "cat /linkerconfig/ld.config.txt".into()],
                output: ok_stdout(
                    "dir./data/local/tmp = unrestricted\n[unrestricted]\nnamespace.default.isolated = false\n",
                ),
                min_timeout: None,
            },
        ]
    }

    #[test]
    fn fingerprint_mismatch_never_executes_helper() {
        deploy::invalidate_deployment();
        let mut scripts = vec![devices_ok()];
        scripts.extend(identity_scripts("other-fingerprint"));
        scripts.push(ScriptedAdbResponse {
            prefix: vec!["exec-out".into(), "screencap".into(), "-p".into()],
            output: ok_stdout(png()),
            min_timeout: None,
        });
        let mut adb = ScriptedAdb::new(scripts);
        let ready = capture_with(
            &mut adb,
            PublicEncoding::Source,
            Instant::now() + Duration::from_secs(45),
            1,
        )
        .unwrap();
        assert_eq!(ready.payload_kind, ReferencePayloadKind::SdrPng);
        assert!(!adb.calls.iter().any(|call| {
            call.args
                .iter()
                .any(|arg| arg.contains("node-forge-hdr-screencap"))
        }));
    }

    fn helper_hash_hit() -> ScriptedAdbResponse {
        ScriptedAdbResponse {
            prefix: vec!["shell".into(), "sh".into(), "-c".into()],
            output: ok_stdout(format!(
                "{}  /data/local/tmp/node-forge/node-forge-hdr-screencap",
                crate::android_screencap::helper::helper_sha256_hex().unwrap()
            )),
            min_timeout: None,
        }
    }

    #[test]
    fn helper_stdout_banner_falls_back_to_stock_png() {
        deploy::invalidate_deployment();
        let mut scripts = vec![devices_ok()];
        scripts.extend(identity_scripts(TARGET_FINGERPRINT));
        scripts.push(ScriptedAdbResponse {
            prefix: vec!["shell".into(), "id -u".into()],
            output: ok_stdout("0"),
            min_timeout: None,
        });
        scripts.push(helper_hash_hit());
        scripts.push(ScriptedAdbResponse {
            prefix: vec!["exec-out".into(), "sh".into()],
            output: ok_stdout(b"Warning\nNFRCLIP\0".to_vec()),
            min_timeout: None,
        });
        scripts.push(ScriptedAdbResponse {
            prefix: vec!["exec-out".into(), "screencap".into(), "-p".into()],
            output: ok_stdout(png()),
            min_timeout: None,
        });
        let mut adb = ScriptedAdb::new(scripts);
        let ready = capture_with(
            &mut adb,
            PublicEncoding::Source,
            Instant::now() + Duration::from_secs(45),
            7,
        )
        .unwrap();
        assert_eq!(ready.fallback_reason.as_deref(), Some("helper-unavailable"));
    }

    #[test]
    fn helper_raw_without_companion_preserves_private_and_fetches_public_png() {
        deploy::invalidate_deployment();
        let envelope = raw_envelope(true, false);
        let mut scripts = vec![devices_ok()];
        scripts.extend(identity_scripts(TARGET_FINGERPRINT));
        scripts.push(ScriptedAdbResponse {
            prefix: vec!["shell".into(), "id -u".into()],
            output: ok_stdout("0"),
            min_timeout: None,
        });
        scripts.push(helper_hash_hit());
        scripts.push(ScriptedAdbResponse {
            prefix: vec!["exec-out".into(), "sh".into()],
            output: ok_stdout(envelope.clone()),
            min_timeout: None,
        });
        scripts.push(ScriptedAdbResponse {
            prefix: vec!["exec-out".into(), "screencap".into(), "-p".into()],
            output: ok_stdout(png()),
            min_timeout: None,
        });
        let mut adb = ScriptedAdb::new(scripts);
        let ready = capture_with(
            &mut adb,
            PublicEncoding::Source,
            Instant::now() + Duration::from_secs(45),
            3,
        )
        .unwrap();
        assert_eq!(ready.payload_kind, ReferencePayloadKind::RawFp16);
        assert_eq!(ready.private_envelope, envelope);
        assert_eq!(ready.public_png, png());
        assert!(ready.high_precision);
    }

    #[test]
    fn expired_deadline_never_starts_helper_or_stock() {
        deploy::invalidate_deployment();
        let mut adb = ScriptedAdb::new(vec![devices_ok()]);
        let error = capture_with(
            &mut adb,
            PublicEncoding::Source,
            Instant::now() - Duration::from_secs(1),
            1,
        )
        .unwrap_err();
        assert!(error.to_string().contains("deadline"));
    }

    #[test]
    fn figma_mode_flattens_public_png_without_gainmap_chunks() {
        deploy::invalidate_deployment();
        let mut scripts = vec![devices_ok()];
        scripts.extend(identity_scripts("other"));
        scripts.push(ScriptedAdbResponse {
            prefix: vec!["exec-out".into(), "screencap".into(), "-p".into()],
            output: ok_stdout(png()),
            min_timeout: None,
        });
        let mut adb = ScriptedAdb::new(scripts);
        let ready = capture_with(
            &mut adb,
            PublicEncoding::FigmaSrgb,
            Instant::now() + Duration::from_secs(45),
            1,
        )
        .unwrap();
        assert!(ready.public_png.windows(4).any(|bytes| bytes == b"sRGB"));
        assert!(!ready.public_png.windows(4).any(|bytes| bytes == b"gmAP"));
    }
}
