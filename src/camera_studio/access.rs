//! Release known macOS camera clients only after libgphoto2 reports USB occupancy.
use std::time::Duration;

use super::model::*;

pub(super) fn connect_with_recovery(
    backend: &mut dyn CameraBackend,
    release_occupied: bool,
    release: impl FnOnce() -> CameraResult<Vec<String>>,
    notice: impl FnMut(String),
) -> CameraResult<CameraInfo> {
    connect_with_recovery_wait(
        backend,
        release_occupied,
        release,
        notice,
        std::thread::sleep,
    )
}

const RECONNECT_DELAYS: [Duration; 7] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(750),
    Duration::from_secs(1),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(2),
];

pub(super) fn connect_with_recovery_wait(
    backend: &mut dyn CameraBackend,
    release_occupied: bool,
    release: impl FnOnce() -> CameraResult<Vec<String>>,
    mut notice: impl FnMut(String),
    mut wait: impl FnMut(Duration),
) -> CameraResult<CameraInfo> {
    match backend.connect() {
        Err(error) if release_occupied && error.kind == CameraErrorKind::Occupied => {
            notice("Camera USB is occupied. Releasing other camera clients…".into());
            let released = release().map_err(|e| {
                CameraError::new(
                    e.kind,
                    format!(
                        "{}\nCould not release camera clients: {}",
                        error.message, e.message
                    ),
                )
            })?;
            if released.is_empty() {
                return Err(CameraError::new(
                    CameraErrorKind::Occupied,
                    format!(
                        "{}\nNo known camera client belonging to the current user was found. Close the application holding the camera and reconnect.",
                        error.message
                    ),
                ));
            }
            let description = released.join(", ");
            let mut last_error = error;
            // Termination can temporarily make the camera unavailable to USB discovery.
            // Each attempt creates a fresh camera/list; only connection is retried, never capture.
            for (attempt, delay) in RECONNECT_DELAYS.iter().enumerate() {
                notice(format!(
                    "Released {description}. Waiting for the camera USB connection to recover… ({}/{})",
                    attempt + 1,
                    RECONNECT_DELAYS.len()
                ));
                wait(*delay);
                match backend.connect() {
                    Ok(info) => {
                        notice(format!("Connected after releasing {description}."));
                        return Ok(info);
                    }
                    Err(error) => {
                        let retry = matches!(
                            error.kind,
                            CameraErrorKind::NotFound | CameraErrorKind::Disconnected
                        );
                        last_error = error;
                        if !retry {
                            break;
                        }
                    }
                }
            }
            let hint = match last_error.kind {
                CameraErrorKind::NotFound | CameraErrorKind::Disconnected => {
                    "The camera did not reappear during USB recovery. Keep it awake; if necessary switch it off/on or reconnect its USB cable, then Connect again."
                }
                CameraErrorKind::Occupied => {
                    "Another camera client reclaimed USB after release. Its background service may have restarted."
                }
                _ => "",
            };
            last_error.message = format!(
                "Released {description}, but reconnect failed: {}\n{hint}",
                last_error.message
            );
            Err(last_error)
        }
        Ok(info) => {
            notice("Connected. New captures are kept on the card and downloaded to the selected folder.".into());
            Ok(info)
        }
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Debug, PartialEq, Eq)]
struct Process {
    pid: i32,
    uid: u32,
    executable: String,
}

#[cfg(any(target_os = "macos", test))]
fn camera_clients(list: &str, uid: u32, own_pid: i32) -> Vec<Process> {
    list.lines().filter_map(|line| {
        let mut fields = line.split_whitespace();
        let pid = fields.next()?.parse().ok()?;
        let owner = fields.next()?.parse().ok()?;
        let executable = fields.collect::<Vec<_>>().join(" ");
        if owner != uid || pid <= 1 || pid == own_pid { return None; }
        let name = executable.rsplit('/').next()?;
        let system_ptp = matches!(executable.as_str(),
            "/usr/libexec/ptpcamerad"
            | "/System/Library/Image Capture/Devices/PTPCamera.app/Contents/MacOS/PTPCamera"
            | "/System/Applications/Image Capture.app/Contents/MacOS/Image Capture"
            | "/Applications/Image Capture.app/Contents/MacOS/Image Capture");
        let canon_utility = executable.starts_with("/Applications/Canon Utilities/")
            && matches!(name, "EOS Utility" | "EOS Utility 2" | "EOS Utility 3"
                | "EOS Utility Launcher" | "EULauncher");
        let canon_webcam = executable == "/Library/CoreMediaIO/Plug-Ins/DAL/EOSWebcamUtility.plugin/Contents/Resources/EOSWebcamService";
        (system_ptp || canon_utility || canon_webcam).then_some(Process { pid, uid: owner, executable })
    }).collect()
}

#[cfg(target_os = "macos")]
pub(super) fn release_camera_clients() -> CameraResult<Vec<String>> {
    use std::{
        thread,
        time::{Duration, Instant},
    };

    fn inventory() -> CameraResult<Vec<Process>> {
        let output = std::process::Command::new("/bin/ps")
            .args(["-axo", "pid=,uid=,comm="])
            .output()
            .map_err(|e| {
                CameraError::new(
                    CameraErrorKind::Occupied,
                    format!("Inspect camera processes: {e}"),
                )
            })?;
        if !output.status.success() {
            return Err(CameraError::new(
                CameraErrorKind::Occupied,
                "Unable to inspect camera processes",
            ));
        }
        Ok(camera_clients(
            &String::from_utf8_lossy(&output.stdout),
            unsafe { libc::geteuid() },
            std::process::id() as i32,
        ))
    }
    fn signal(process: &Process, signal: i32) -> CameraResult<()> {
        // Recheck ownership and executable so a stale inventory cannot select another process.
        if !inventory()?.contains(process) {
            return Ok(());
        }
        if unsafe { libc::kill(process.pid, signal) } != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(CameraError::new(
                    CameraErrorKind::Occupied,
                    format!(
                        "Release {} (PID {}): {error}",
                        process.executable, process.pid
                    ),
                ));
            }
        }
        Ok(())
    }

    let clients = inventory()?;
    if clients.is_empty() {
        return Ok(Vec::new());
    }
    for client in &clients {
        signal(client, libc::SIGTERM)?;
    }
    let deadline = Instant::now() + Duration::from_millis(800);
    let remaining = loop {
        let current = inventory()?;
        let remaining = clients
            .iter()
            .filter(|client| current.contains(client))
            .collect::<Vec<_>>();
        if remaining.is_empty() || Instant::now() >= deadline {
            break remaining;
        }
        thread::sleep(Duration::from_millis(60));
    };
    for client in &remaining {
        signal(client, libc::SIGKILL)?;
    }
    if !remaining.is_empty() {
        thread::sleep(Duration::from_millis(80));
    }
    Ok(clients
        .iter()
        .map(|p| {
            format!(
                "{} (PID {}{})",
                p.executable.rsplit('/').next().unwrap_or("camera client"),
                p.pid,
                if remaining.contains(&p) {
                    ", force stopped"
                } else {
                    ""
                }
            )
        })
        .collect())
}

#[cfg(not(target_os = "macos"))]
pub(super) fn release_camera_clients() -> CameraResult<Vec<String>> {
    Err(CameraError::new(
        CameraErrorKind::Unsupported,
        "Automatic release of camera clients is currently available on macOS. Close other camera software and reconnect.",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn release_candidates_require_known_path_and_current_user() {
        let processes = "10 501 /usr/libexec/ptpcamerad\n\
            11 501 /Applications/Canon Utilities/EOS Utility/EU2/EOS Utility 2.app/Contents/MacOS/EOS Utility 2\n\
            12 0 /usr/libexec/ptpcamerad\n\
            13 501 /tmp/ptpcamerad\n\
            14 501 /usr/bin/camera-studio\n\
            15 501 /Library/CoreMediaIO/Plug-Ins/DAL/EOSWebcamUtility.plugin/Contents/Resources/EOSWebcamService\n\
            16 501 /System/Library/Image Capture/Support/icdd\n\
            17 501 /System/Applications/Image Capture.app/Contents/MacOS/Image Capture";
        assert_eq!(
            camera_clients(processes, 501, 17)
                .iter()
                .map(|p| p.pid)
                .collect::<Vec<_>>(),
            [10, 11, 15]
        );
    }
}
