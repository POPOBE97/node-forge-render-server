//! USB player deployment and restoration of the Android foreground task.
use super::model::new_id;
use anyhow::{Context, Result, ensure};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::Command,
};

const PACKAGE: &str = "com.nodeforge.screentest";

trait Host {
    fn adb(&mut self, serial: &str, args: &[&str]) -> Result<String>;
    fn ios(&mut self, args: &[&str]) -> Result<Value>;
    fn settle(&mut self) {
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}
struct UsbHost;
impl Host for UsbHost {
    fn adb(&mut self, serial: &str, args: &[&str]) -> Result<String> {
        super::device::adb(Some(serial), args.iter().map(|s| s.to_string()).collect())
    }
    fn ios(&mut self, args: &[&str]) -> Result<Value> {
        let path = std::env::temp_dir().join(format!("{}.json", new_id("screen-ios-operation")));
        let output = Command::new("xcrun")
            .arg("devicectl")
            .args(args)
            .args(["--timeout", "60", "--json-output"])
            .arg(&path)
            .output()?;
        let bytes = std::fs::read(&path);
        let _ = std::fs::remove_file(&path);
        ensure!(
            output.status.success(),
            "iPhone USB operation failed: {} {}",
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&output.stdout)
        );
        Ok(serde_json::from_slice(&bytes?)?)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Foreground {
    component: String,
    task: Option<u32>,
}
fn foreground(dump: &str) -> Option<Foreground> {
    // Only the resumed record, never a historical ActivityRecord elsewhere in dumpsys.
    for key in [
        "topResumedActivity=",
        "mResumedActivity:",
        "ResumedActivity:",
    ] {
        for line in dump.lines().filter(|line| line.contains(key)) {
            let tokens: Vec<_> = line
                .split_whitespace()
                .map(|s| s.trim_matches(|c| c == '}' || c == ','))
                .collect();
            let component = tokens.iter().find(|s| {
                s.contains('/')
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._/$".contains(c))
            })?;
            let task = tokens
                .iter()
                .find_map(|s| s.strip_prefix('t')?.parse().ok());
            return Some(Foreground {
                component: component.to_string(),
                task,
            });
        }
    }
    None
}
fn current(host: &mut impl Host, serial: &str) -> Result<Foreground> {
    foreground(&host.adb(serial, &["shell", "dumpsys", "activity", "activities"])?)
        .context("Cannot identify the current Android activity; unlock the phone and retry")
}
fn wait_resumed(host: &mut impl Host, serial: &str, component: &str) -> bool {
    for _ in 0..10 {
        if current(host, serial).is_ok_and(|v| v.component == component) {
            return true;
        }
        host.settle();
    }
    false
}
fn shell_ok(output: String) -> Result<()> {
    ensure!(
        !output.contains("Error:") && !output.contains("Exception") && !output.contains("Failure"),
        "Android: {output}"
    );
    Ok(())
}

pub(super) struct AndroidPlayer {
    serial: String,
    previous: Option<Foreground>,
    armed: bool,
}
impl AndroidPlayer {
    pub fn prepare(serial: &str) -> Result<Self> {
        let apk = std::env::var_os("NODE_FORGE_SCREEN_TEST_APK")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
                    "tools/screen-test-player/android/app/build/outputs/apk/debug/app-debug.apk",
                )
            });
        Self::prepare_with(&mut UsbHost, serial, &apk)
    }
    fn prepare_with(host: &mut impl Host, serial: &str, apk: &Path) -> Result<Self> {
        let previous = current(host, serial)?;
        let previous =
            (!previous.component.starts_with(&format!("{PACKAGE}/"))).then_some(previous);
        let installed = host.adb(serial, &["shell", "pm", "list", "packages", PACKAGE])?;
        if !installed
            .lines()
            .any(|s| s.trim() == format!("package:{PACKAGE}"))
        {
            ensure!(
                apk.is_file(),
                "Screen Test APK is missing: {}",
                apk.display()
            );
            let output = host.adb(serial, &["install", "-r", &apk.to_string_lossy()])?;
            ensure!(
                output.lines().any(|s| s.trim() == "Success"),
                "Android installation failed: {output}"
            );
        }
        // Construct before launch so a partial launch/forwarding/handshake failure also restores.
        Ok(Self {
            serial: serial.into(),
            previous,
            armed: false,
        })
    }
    pub fn launch(&mut self) -> Result<()> {
        self.armed = true;
        shell_ok(UsbHost.adb(
            &self.serial,
            &[
                "shell",
                "am",
                "start",
                "-W",
                "-n",
                "com.nodeforge.screentest/.MainActivity",
            ],
        )?)
    }
    pub fn restore(&mut self) -> Result<()> {
        self.restore_with(&mut UsbHost)
    }
    fn restore_with(&mut self, host: &mut impl Host) -> Result<()> {
        if !self.armed {
            return Ok(());
        }
        shell_ok(host.adb(&self.serial, &["shell", "am", "force-stop", PACKAGE])?)?;
        if let Some(previous) = &self.previous {
            // Stopping the temporary player normally reveals the exact previous instance.
            if current(host, &self.serial)
                .as_ref()
                .is_ok_and(|v| v.component == previous.component)
            {
                self.armed = false;
                return Ok(());
            }
            if let Some(task) = previous.task {
                // Focus the existing task, including non-exported activities and their state.
                let _ = host.adb(
                    &self.serial,
                    &["shell", "am", "task", "focus", &task.to_string()],
                );
            }
            if !wait_resumed(host, &self.serial, &previous.component) {
                // Task may have been reclaimed; start only the captured component, never its launcher.
                shell_ok(host.adb(
                    &self.serial,
                    &[
                        "shell",
                        "am",
                        "start",
                        "-W",
                        "--activity-reorder-to-front",
                        "-n",
                        &previous.component,
                    ],
                )?)?;
            }
            ensure!(
                wait_resumed(host, &self.serial, &previous.component),
                "Could not restore Android activity {}",
                previous.component
            );
        }
        self.armed = false;
        Ok(())
    }
}
impl Drop for AndroidPlayer {
    fn drop(&mut self) {
        let _ = self.restore();
    }
}

struct IosBundle {
    path: PathBuf,
    unpacked: Option<PathBuf>,
}
impl IosBundle {
    fn open(path: &Path) -> Result<Self> {
        if path.is_dir() && path.extension().is_some_and(|s| s == "app") {
            return Ok(Self {
                path: path.into(),
                unpacked: None,
            });
        }
        ensure!(
            path.is_file() && path.extension().is_some_and(|s| s == "ipa"),
            "Signed Screen Test iPhone app is missing: {}",
            path.display()
        );
        let root = std::env::temp_dir().join(new_id("screen-ipa"));
        std::fs::create_dir(&root)?;
        let mut bundle = Self {
            path: PathBuf::new(),
            unpacked: Some(root.clone()),
        };
        let listing = Command::new("/usr/bin/unzip")
            .args(["-Z1"])
            .arg(path)
            .output()?;
        ensure!(
            listing.status.success()
                && String::from_utf8_lossy(&listing.stdout)
                    .lines()
                    .all(|p| !p.starts_with('/') && !p.split('/').any(|s| s == "..")),
            "Invalid IPA archive"
        );
        ensure!(
            Command::new("/usr/bin/ditto")
                .args(["-x", "-k"])
                .arg(path)
                .arg(&root)
                .status()?
                .success(),
            "Could not unpack signed iPhone app"
        );
        let apps = std::fs::read_dir(root.join("Payload"))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|s| s == "app"))
            .collect::<Vec<_>>();
        ensure!(apps.len() == 1, "IPA must contain one app bundle");
        bundle.path = apps[0].clone();
        Ok(bundle)
    }
}
impl Drop for IosBundle {
    fn drop(&mut self) {
        if let Some(root) = &self.unpacked {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}
pub(super) fn launch_ios(id: &str) -> Result<()> {
    launch_ios_with(&mut UsbHost, id, &ios_bundle_path())
}
fn ios_bundle_path() -> PathBuf {
    std::env::var_os("NODE_FORGE_SCREEN_TEST_IOS_APP")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
                "tools/screen-test-player/ios/build/Build/Products/Debug-iphoneos/ScreenTest.app",
            )
        })
}
pub(super) fn install_ios(id: &str) -> Result<()> {
    install_ios_with(&mut UsbHost, id, &ios_bundle_path())
}
fn install_ios_with(host: &mut impl Host, id: &str, path: &Path) -> Result<()> {
    let bundle = IosBundle::open(path)?;
    host.ios(&["device", "install", "app", "--device", id, &bundle.path.to_string_lossy()])
        .context("Could not install signed Screen Test app; unlock/trust the iPhone and enable Developer Mode")?;
    Ok(())
}
fn launch_ios_with(host: &mut impl Host, id: &str, path: &Path) -> Result<()> {
    let data = host.ios(&[
        "device",
        "info",
        "apps",
        "--device",
        id,
        "--bundle-id",
        PACKAGE,
    ])?;
    let apps = data["result"]["apps"]
        .as_array()
        .context("Invalid iPhone installed-app response")?;
    if !apps.iter().any(|app| app["bundleIdentifier"] == PACKAGE) {
        install_ios_with(host, id, path)?;
    }
    host.ios(&["device", "process", "launch", "--device", id, PACKAGE])?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    #[derive(Default)]
    struct Fake {
        calls: Vec<Vec<String>>,
        answers: VecDeque<Result<String>>,
    }
    impl Host for Fake {
        fn settle(&mut self) {}
        fn adb(&mut self, _: &str, args: &[&str]) -> Result<String> {
            self.calls
                .push(args.iter().map(|s| s.to_string()).collect());
            self.answers.pop_front().expect("unexpected USB command")
        }
        fn ios(&mut self, args: &[&str]) -> Result<Value> {
            Ok(serde_json::from_str(&self.adb("", args)?)?)
        }
    }
    fn activity(name: &str) -> String {
        format!("topResumedActivity=ActivityRecord{{abc u0 {name} t42}}")
    }
    #[test]
    fn android_returns_to_exact_task_without_relaunching_activity() {
        let mut host = Fake::default();
        host.answers = [
            Ok(String::new()),
            Ok(activity("com.home/.Home")),
            Ok(String::new()),
            Ok(activity("com.test/.PrivateActivity")),
            Ok(activity("com.test/.PrivateActivity")),
        ]
        .into();
        let mut player = AndroidPlayer {
            serial: "usb".into(),
            previous: foreground(&activity("com.test/.PrivateActivity")),
            armed: true,
        };
        player.restore_with(&mut host).unwrap();
        assert!(!player.armed);
        assert!(
            host.calls
                .iter()
                .any(|a| a == &["shell", "am", "task", "focus", "42"])
        );
        assert!(!host.calls.iter().any(|a| a.iter().any(|s| s == "start")));
    }
    #[test]
    fn android_already_resumed_is_not_restarted() {
        let mut host = Fake::default();
        host.answers = [Ok(String::new()), Ok(activity("com.test/.Page"))].into();
        let mut player = AndroidPlayer {
            serial: "usb".into(),
            previous: foreground(&activity("com.test/.Page")),
            armed: true,
        };
        player.restore_with(&mut host).unwrap();
        assert_eq!(host.calls.len(), 2);
    }
    #[test]
    fn ios_installs_only_if_missing_and_launches_after_install() {
        let root = std::env::temp_dir().join(new_id("player-test"));
        let app = root.join("ScreenTest.app");
        std::fs::create_dir_all(&app).unwrap();
        for installed in [false, true] {
            let mut host = Fake::default();
            host.answers.push_back(Ok(if installed {
                r#"{"result":{"apps":[{"bundleIdentifier":"com.nodeforge.screentest"}]}}"#
            } else {
                r#"{"result":{"apps":[]}}"#
            }
            .into()));
            if !installed {
                host.answers.push_back(Ok("{}".into()));
            }
            host.answers.push_back(Ok("{}".into()));
            launch_ios_with(&mut host, "usb", &app).unwrap();
            assert_eq!(host.calls.len(), if installed { 2 } else { 3 });
            assert_eq!(
                &host.calls.last().unwrap()[..3],
                &["device", "process", "launch"]
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn foreground_ignores_historical_records() {
        assert!(foreground("ActivityRecord{abc u0 com.test/.Old t9}").is_none());
        assert_eq!(
            foreground("mResumedActivity: ActivityRecord{abc u0 com.test/.Page t8}")
                .unwrap()
                .task,
            Some(8)
        );
    }
    #[test]
    fn android_installs_missing_apk_after_remembering_foreground() {
        let apk = std::env::temp_dir().join(new_id("test-apk"));
        std::fs::write(&apk, b"fixture").unwrap();
        let mut host = Fake::default();
        host.answers = [
            Ok(activity("com.test/.PrivateActivity")),
            Ok(String::new()),
            Ok("Success".into()),
        ]
        .into();
        let player = AndroidPlayer::prepare_with(&mut host, "usb", &apk).unwrap();
        assert_eq!(
            player.previous.as_ref().unwrap().component,
            "com.test/.PrivateActivity"
        );
        assert!(!player.armed);
        assert_eq!(&host.calls[0][..3], &["shell", "dumpsys", "activity"]);
        assert_eq!(&host.calls[2][..2], &["install", "-r"]);
        std::fs::remove_file(apk).unwrap();
    }
    #[test]
    fn failed_ios_install_never_launches_player() {
        let app = std::env::temp_dir()
            .join(new_id("player-test"))
            .join("ScreenTest.app");
        std::fs::create_dir_all(&app).unwrap();
        let mut host = Fake::default();
        host.answers = [
            Ok(r#"{"result":{"apps":[]}}"#.into()),
            Err(anyhow::anyhow!("locked")),
        ]
        .into();
        assert!(launch_ios_with(&mut host, "usb", &app).is_err());
        assert_eq!(host.calls.len(), 2);
        std::fs::remove_dir_all(app.parent().unwrap()).unwrap();
    }
}
