use super::{model::*, pattern::Pattern};
use crate::android_screencap::adb::{AdbCommand, AdbTransport, RealAdbTransport};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::Path,
    process::{Child, Command, Stdio},
    time::Duration,
};
const DEVICE_PORT: u16 = 48731;
pub struct DeviceConnection {
    pub device: DisplayDevice,
    port: u16,
    proxy: Option<Child>,
    android_player: Option<super::player::AndroidPlayer>,
}
pub(super) fn adb(serial: Option<&str>, args: Vec<String>) -> Result<String> {
    let output =
        RealAdbTransport::from_env().run(AdbCommand::new(serial, args, Duration::from_secs(45)))?;
    ensure!(output.success(), "ADB: {}", output.stderr_utf8());
    Ok(output.stdout_utf8().trim().to_string())
}
impl DeviceConnection {
    /// Batch preparation deploys the current signed build, including zero-brightness support.
    pub fn connect_updated_ios(requested_id: &str) -> Result<Self> {
        let id = ios_usb_id(requested_id)?;
        super::player::install_ios(&id)?;
        Self::connect(Platform::Ios, &id)
    }
    pub fn connect_available() -> Result<Self> {
        let adb_path = RealAdbTransport::from_env().adb_path;
        let android = crate::android_reference::android_devices_with_adb(&adb_path);
        let ios = ios_usb_ids("");
        let mut candidates = Vec::new();
        if let Ok(devices) = &android {
            candidates.extend(
                devices
                    .iter()
                    .filter(|d| d.usb && d.state == "device")
                    .map(|d| (Platform::Android, d.serial.clone())),
            );
        }
        if let Ok(ids) = &ios {
            candidates.extend(ids.iter().cloned().map(|id| (Platform::Ios, id)));
        }
        if candidates.is_empty() {
            if android
                .as_ref()
                .is_ok_and(|devices| devices.iter().any(|d| d.usb && d.state == "unauthorized"))
            {
                anyhow::bail!(
                    "Unlock the Android phone and accept its USB debugging prompt, then calibrate again."
                );
            }
            anyhow::bail!(
                "Connect and unlock a USB phone, then calibrate again. Android: {}; iPhone: {}",
                android
                    .err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "no ready USB device".into()),
                ios.err()
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| "no wired device".into())
            );
        }
        let (platform, id) = select_calibration_display(&candidates)?;
        Self::connect(platform, &id)
    }
    pub fn connect(platform: Platform, requested_id: &str) -> Result<Self> {
        let mut proxy = None;
        let mut android_player = None;
        let (id, port) = match platform {
            Platform::Android => {
                let transport = RealAdbTransport::from_env();
                let id = if requested_id.trim().is_empty() {
                    crate::android_reference::select_single_ready_usb_device(
                        &crate::android_reference::android_devices_with_adb(&transport.adb_path)?,
                    )?
                    .serial
                } else {
                    requested_id.trim().to_string()
                };
                android_player = Some(super::player::AndroidPlayer::prepare(&id)?);
                android_player.as_mut().unwrap().launch()?;
                let port = adb(
                    Some(&id),
                    vec![
                        "forward".into(),
                        "tcp:0".into(),
                        format!("tcp:{DEVICE_PORT}"),
                    ],
                )?
                .parse()?;
                (id, port)
            }
            Platform::Ios => {
                let id = ios_usb_id(requested_id)?;
                super::player::launch_ios(&id)?;
                let listener = TcpListener::bind("127.0.0.1:0")?;
                let port = listener.local_addr()?.port();
                drop(listener);
                proxy = Some(
                    Command::new("iproxy")
                        .args([
                            "-u",
                            &id,
                            "-l",
                            "-s",
                            "127.0.0.1",
                            &format!("{port}:{DEVICE_PORT}"),
                        ])
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .context("Install libusbmuxd for USB iPhone forwarding")?,
                );
                (id, port)
            }
        };
        let placeholder = DisplayDevice {
            id: id.clone(),
            platform,
            name: id,
            size: [0, 0],
            drawable: [0, 0],
            insets: [0; 4],
            modes: vec![],
            brightness: 0.,
            headroom: 1.,
            active: false,
            generation: String::new(),
            brightness_guard: None,
        };
        let mut connection = Self {
            device: placeholder,
            port,
            proxy,
            android_player,
        };
        let mut last = None;
        for _ in 0..20 {
            match connection.status() {
                Ok(device) if device.active && device.drawable[0] > 0 && device.drawable[1] > 0 => {
                    connection.device = device;
                    return Ok(connection);
                }
                Ok(_) => last = Some(anyhow::anyhow!("Player display is not ready")),
                Err(e) => last = Some(e),
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        Err(last.context("Player did not respond")?)
            .context("Launch the installed Screen Test player and keep it foreground")
    }
    fn rpc(&self, header: Value, payload: &[u8]) -> Result<Value> {
        self.rpc_timeout(header, payload, Duration::from_secs(15))
    }
    fn rpc_timeout(&self, header: Value, payload: &[u8], timeout: Duration) -> Result<Value> {
        let address: SocketAddr = format!("127.0.0.1:{}", self.port).parse()?;
        let mut socket = TcpStream::connect_timeout(&address, timeout.min(Duration::from_secs(2)))?;
        socket.set_read_timeout(Some(timeout))?;
        socket.set_write_timeout(Some(timeout))?;
        let bytes = serde_json::to_vec(&header)?;
        socket.write_all(&(bytes.len() as u32).to_le_bytes())?;
        socket.write_all(&bytes)?;
        socket.write_all(payload)?;
        let mut length = [0; 4];
        socket.read_exact(&mut length)?;
        let length = u32::from_le_bytes(length) as usize;
        ensure!(length <= 1_048_576, "Oversized player response");
        let mut bytes = vec![0; length];
        socket.read_exact(&mut bytes)?;
        let result: Value = serde_json::from_slice(&bytes)?;
        if let Some(error) = result.get("error").and_then(Value::as_str) {
            anyhow::bail!("Player: {error}");
        }
        Ok(result)
    }
    pub fn status(&self) -> Result<DisplayDevice> {
        let value = self.rpc(json!({"command":"status","payload_bytes":0}), &[])?;
        let mut device: DisplayDevice = serde_json::from_value(value)?;
        device.id = self.device.id.clone();
        device.platform = self.device.platform;
        Ok(device)
    }
    pub fn present(&mut self, pattern: &Pattern, brightness: f32) -> Result<DisplayDevice> {
        ensure!(
            brightness.is_finite() && (0.0..=1.).contains(&brightness),
            "Brightness must be 0–100%"
        );
        let bytes = pattern.bytes();
        let m = &pattern.manifest;
        let _:Value=self.rpc(json!({"command":"present","payload_bytes":bytes.len(),"manifest":m,"brightness":brightness}),&bytes)?;
        std::thread::sleep(Duration::from_secs(1));
        let mut device = self.status()?;
        let mut brightness_issue = None;
        for _ in 0..20 {
            brightness_issue = validate_brightness_guard(&device, brightness)
                .err()
                .map(|e| e.to_string());
            if device.active
                && device.generation == m.generation
                && device.size == m.size
                && device.drawable == m.size
                && (device.brightness - brightness).abs() < 0.015
                && (m.mode != DisplayMode::LinearHdr || device.headroom >= m.peak * 0.99)
                && brightness_issue.is_none()
            {
                self.device = device.clone();
                return Ok(device);
            }
            std::thread::sleep(Duration::from_millis(200));
            device = self.status()?;
        }
        anyhow::bail!(
            "{}. No shutter sent.",
            brightness_issue
                .as_deref()
                .unwrap_or("Display did not confirm stable presentation/brightness/HDR headroom")
        )
    }
    pub fn validate_capture(&self, ticket: &CaptureTicket) -> Result<()> {
        let device = self.status()?;
        validate_capture_state(&device, ticket)
    }
    pub fn restore_after_calibration(&mut self) -> Result<()> {
        if let Some(player) = &mut self.android_player {
            player.restore()?;
        } else if self.device.platform == Platform::Ios {
            self.rpc(json!({"command":"release","payload_bytes":0}), &[])?;
        }
        Ok(())
    }
    pub fn install_android(apk: &Path, serial: &str) -> Result<()> {
        adb(
            if serial.is_empty() {
                None
            } else {
                Some(serial)
            },
            vec![
                "install".into(),
                "-r".into(),
                apk.to_string_lossy().into_owned(),
            ],
        )?;
        Ok(())
    }
}

fn validate_brightness_guard(device: &DisplayDevice, brightness: f32) -> Result<()> {
    if device.platform != Platform::Ios {
        return Ok(());
    }
    let guard = device
        .brightness_guard
        .as_ref()
        .context("Update the iPhone Screen Test app to enable brightness protection")?;
    ensure!(
        guard.holding
            && guard.idle_timer_disabled
            && guard.settled
            && !guard.low_power_mode
            && matches!(
                guard.thermal_state,
                DisplayThermalState::Nominal | DisplayThermalState::Fair
            )
            && guard
                .requested_brightness
                .is_some_and(|v| (v - brightness).abs() <= 0.005)
            && (device.brightness - brightness).abs() <= 0.005,
        "{}",
        guard
            .issue
            .as_deref()
            .unwrap_or("iPhone brightness protection is not ready")
    );
    Ok(())
}

fn validate_capture_state(device: &DisplayDevice, ticket: &CaptureTicket) -> Result<()> {
    let expected = &ticket.conditions.device;
    validate_brightness_guard(device, ticket.conditions.brightness)?;
    if device.platform == Platform::Ios {
        let before = expected
            .brightness_guard
            .as_ref()
            .context("Capture has no initial brightness protection snapshot")?;
        let after = device
            .brightness_guard
            .as_ref()
            .context("iPhone brightness protection was lost")?;
        ensure!(
            before.revision == after.revision,
            "iPhone brightness or display conditions changed during capture, even if restored. RAW retained; capture again after settling"
        );
    }
    ensure!(
        device.active
            && device.generation == ticket.conditions.manifest.generation
            && device.size == expected.size
            && device.drawable == expected.drawable
            && (device.brightness - expected.brightness).abs() < 0.015,
        "Display state changed during capture"
    );
    if ticket.conditions.manifest.mode == DisplayMode::LinearHdr {
        ensure!(
            (device.headroom - expected.headroom).abs() <= 0.05 * expected.headroom
                && device.headroom >= ticket.conditions.manifest.peak * 0.99,
            "HDR headroom changed during capture"
        );
    }
    Ok(())
}
impl Drop for DeviceConnection {
    fn drop(&mut self) {
        if self.device.platform == Platform::Ios {
            // RPCs use short-lived sockets; release the hold only when the whole
            // display session ends, not after an individual status request.
            let _ = self.rpc_timeout(
                json!({"command":"release","payload_bytes":0}),
                &[],
                Duration::from_millis(500),
            );
        }
        if let Some(mut child) = self.proxy.take() {
            let _ = child.kill();
            let _ = child.wait();
        } else if self.device.platform == Platform::Android {
            let _ = adb(
                Some(&self.device.id),
                vec![
                    "forward".into(),
                    "--remove".into(),
                    format!("tcp:{}", self.port),
                ],
            );
        }
    }
}

/// CoreDevice identifiers are not usbmux UDIDs. Resolve wired devices explicitly.
fn ios_usb_id(requested: &str) -> Result<String> {
    let ids = ios_usb_ids(requested)?;
    ensure!(ids.len() == 1, "Connect one USB iPhone, or enter its UDID");
    Ok(ids[0].clone())
}
fn select_calibration_display(candidates: &[(Platform, String)]) -> Result<(Platform, String)> {
    ensure!(
        candidates.len() == 1,
        "Multiple USB phones are connected. Keep only the phone being calibrated connected, or use the display already connected in Screen Test."
    );
    Ok(candidates[0].clone())
}
fn ios_usb_ids(requested: &str) -> Result<Vec<String>> {
    let path = std::env::temp_dir().join(format!("{}.json", new_id("screen-ios-devices")));
    let output = Command::new("xcrun")
        .args([
            "devicectl",
            "list",
            "devices",
            "--timeout",
            "5",
            "--json-output",
        ])
        .arg(&path)
        .output()?;
    let bytes = std::fs::read(&path);
    let _ = std::fs::remove_file(&path);
    ensure!(output.status.success(), "Cannot list iPhone USB devices");
    let data: Value = serde_json::from_slice(&bytes?)?;
    let devices = data["result"]["devices"]
        .as_array()
        .context("No iOS devices found")?;
    let matches: Vec<_> = devices
        .iter()
        .filter(|d| d["connectionProperties"]["transportType"] == "wired")
        .filter(|d| {
            requested.trim().is_empty()
                || d["identifier"] == requested
                || d["hardwareProperties"]["udid"] == requested
        })
        .collect();
    matches
        .iter()
        .map(|device| {
            Ok(device["hardwareProperties"]["udid"]
                .as_str()
                .context("Missing USB UDID")?
                .to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guarded_ticket() -> CaptureTicket {
        let mut ticket = crate::screen_test::tests::capture().ticket;
        ticket.conditions.device.generation = ticket.conditions.manifest.generation.clone();
        ticket.conditions.device.brightness_guard = Some(BrightnessGuard {
            requested_brightness: Some(0.5),
            holding: true,
            idle_timer_disabled: true,
            settled: true,
            revision: 4,
            thermal_state: DisplayThermalState::Nominal,
            low_power_mode: false,
            issue: None,
        });
        ticket
    }

    #[test]
    fn ios_capture_requires_active_stable_brightness_protection() {
        let ticket = guarded_ticket();
        validate_capture_state(&ticket.conditions.device, &ticket).unwrap();
        for condition in 0..8 {
            let mut device = ticket.conditions.device.clone();
            let guard = device.brightness_guard.as_mut().unwrap();
            match condition {
                0 => guard.holding = false,
                1 => guard.settled = false,
                2 => guard.idle_timer_disabled = false,
                3 => guard.low_power_mode = true,
                4 => guard.thermal_state = DisplayThermalState::Serious,
                5 => guard.requested_brightness = Some(0.6),
                6 => device.brightness = 0.49,
                _ => device.active = false,
            }
            assert!(
                validate_capture_state(&device, &ticket).is_err(),
                "condition {condition}"
            );
        }
    }

    #[test]
    fn zero_brightness_is_accepted_without_substitution() {
        let mut ticket = guarded_ticket();
        ticket.conditions.brightness = 0.0;
        ticket.conditions.device.brightness = 0.0;
        ticket
            .conditions
            .device
            .brightness_guard
            .as_mut()
            .unwrap()
            .requested_brightness = Some(0.0);
        validate_capture_state(&ticket.conditions.device, &ticket).unwrap();
        let mut wrong = ticket.conditions.device.clone();
        wrong.brightness = 0.01;
        assert!(validate_capture_state(&wrong, &ticket).is_err());
    }

    #[test]
    fn brightness_recovery_does_not_hide_an_interruption_during_capture() {
        let ticket = guarded_ticket();
        let mut device = ticket.conditions.device.clone();
        device.brightness_guard.as_mut().unwrap().revision += 1;
        let error = validate_capture_state(&device, &ticket).unwrap_err();
        assert!(error.to_string().contains("even if restored"));
    }

    #[test]
    fn iphone_without_guard_needs_update_but_android_is_unchanged() {
        let ticket = guarded_ticket();
        let mut device = ticket.conditions.device.clone();
        device.brightness_guard = None;
        assert!(
            validate_capture_state(&device, &ticket)
                .unwrap_err()
                .to_string()
                .contains("Update")
        );
        device.platform = Platform::Android;
        validate_capture_state(&device, &ticket).unwrap();
    }

    #[test]
    fn ending_ios_connection_releases_brightness_over_usb_rpc() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut size = [0; 4];
            socket.read_exact(&mut size).unwrap();
            let mut bytes = vec![0; u32::from_le_bytes(size) as usize];
            socket.read_exact(&mut bytes).unwrap();
            let request: Value = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(request["command"], "release");
            let response = b"{}";
            socket
                .write_all(&(response.len() as u32).to_le_bytes())
                .unwrap();
            socket.write_all(response).unwrap();
        });
        drop(DeviceConnection {
            device: guarded_ticket().conditions.device,
            port,
            proxy: None,
            android_player: None,
        });
        server.join().unwrap();
    }
}

#[cfg(test)]
mod automatic_selection_tests {
    use super::*;
    #[test]
    fn calibration_selects_only_a_single_unambiguous_usb_display() {
        for platform in [Platform::Android, Platform::Ios] {
            let device = (platform, "usb-test".to_string());
            assert_eq!(
                select_calibration_display(&[device.clone()]).unwrap(),
                device
            );
        }
        assert!(select_calibration_display(&[]).is_err());
        assert!(
            select_calibration_display(&[
                (Platform::Android, "android".into()),
                (Platform::Ios, "iphone".into())
            ])
            .is_err()
        );
    }
}
