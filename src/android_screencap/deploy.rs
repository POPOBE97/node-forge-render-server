#[cfg(not(test))]
use std::sync::Mutex;
use std::time::Instant;

use anyhow::{anyhow, bail};

use super::{
    adb::{self, AdbCommand, AdbTransport, PROBE_CAP, PUSH_CAP, child_timeout},
    helper::{
        self, TARGET_ABI, TARGET_FINGERPRINT, TARGET_SDK, embedded_helper_bytes,
        helper_remote_path, helper_sha256_hex,
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootForm {
    Direct,
    Su0,
    SuDashC,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceIdentity {
    pub serial: String,
    pub abi: String,
    pub sdk: String,
    pub fingerprint: String,
    pub linker_unrestricted: bool,
}

#[derive(Clone, Debug)]
pub struct DeploymentState {
    pub identity: DeviceIdentity,
    pub root: RootForm,
    pub helper_sha256: String,
}

#[cfg(not(test))]
static DEPLOYMENT_CACHE: Mutex<Option<DeploymentState>> = Mutex::new(None);

#[cfg(test)]
thread_local! {
    static DEPLOYMENT_CACHE: std::cell::RefCell<Option<DeploymentState>> =
        const { std::cell::RefCell::new(None) };
}

pub fn cached_deployment(serial: &str) -> Option<DeploymentState> {
    #[cfg(test)]
    {
        return DEPLOYMENT_CACHE.with(|cache| {
            cache
                .borrow()
                .as_ref()
                .filter(|state| state.identity.serial == serial)
                .cloned()
        });
    }
    #[cfg(not(test))]
    {
        DEPLOYMENT_CACHE
            .lock()
            .ok()?
            .as_ref()
            .filter(|state| state.identity.serial == serial)
            .cloned()
    }
}

pub fn store_deployment(state: DeploymentState) {
    #[cfg(test)]
    {
        DEPLOYMENT_CACHE.with(|cache| *cache.borrow_mut() = Some(state));
        return;
    }
    #[cfg(not(test))]
    if let Ok(mut cache) = DEPLOYMENT_CACHE.lock() {
        *cache = Some(state);
    }
}

pub fn invalidate_deployment() {
    #[cfg(test)]
    {
        DEPLOYMENT_CACHE.with(|cache| *cache.borrow_mut() = None);
        return;
    }
    #[cfg(not(test))]
    if let Ok(mut cache) = DEPLOYMENT_CACHE.lock() {
        *cache = None;
    }
}

pub fn probe_identity(
    adb: &mut dyn AdbTransport,
    serial: &str,
    deadline: Instant,
) -> anyhow::Result<DeviceIdentity> {
    let abi = getprop(adb, serial, "ro.product.cpu.abi", deadline)?;
    let sdk = getprop(adb, serial, "ro.build.version.sdk", deadline)?;
    let fingerprint = getprop(adb, serial, "ro.build.fingerprint", deadline)?;
    let linker =
        shell_stdout(adb, serial, "cat /linkerconfig/ld.config.txt", deadline).unwrap_or_default();
    Ok(DeviceIdentity {
        serial: serial.to_string(),
        abi,
        sdk,
        fingerprint,
        linker_unrestricted: linker_maps_tmp_unrestricted(&linker),
    })
}

pub fn helper_supported(identity: &DeviceIdentity) -> bool {
    identity.abi == TARGET_ABI
        && identity.sdk == TARGET_SDK
        && identity.fingerprint == TARGET_FINGERPRINT
        && identity.linker_unrestricted
        && embedded_helper_bytes().is_some()
}

pub fn probe_root(
    adb: &mut dyn AdbTransport,
    serial: &str,
    deadline: Instant,
) -> anyhow::Result<RootForm> {
    if uid_is_root(&shell_stdout(adb, serial, "id -u", deadline).unwrap_or_default()) {
        return Ok(RootForm::Direct);
    }
    if uid_is_root(
        &root_shell_stdout(adb, serial, RootForm::Su0, "id -u", deadline).unwrap_or_default(),
    ) {
        return Ok(RootForm::Su0);
    }
    if uid_is_root(
        &root_shell_stdout(adb, serial, RootForm::SuDashC, "id -u", deadline).unwrap_or_default(),
    ) {
        return Ok(RootForm::SuDashC);
    }
    bail!("device does not accept a non-restarting root shell");
}

pub fn ensure_helper(
    adb: &mut dyn AdbTransport,
    serial: &str,
    root: RootForm,
    deadline: Instant,
) -> anyhow::Result<String> {
    let bytes =
        embedded_helper_bytes().ok_or_else(|| anyhow!("HDR helper prebuilt is not embedded"))?;
    let expected = helper_sha256_hex().expect("helper sha");
    let remote = helper_remote_path();
    let hash_output = root_shell_stdout(
        adb,
        serial,
        root,
        &format!("sha256sum {remote} 2>/dev/null || true"),
        deadline,
    );
    let skip_deploy = hash_output
        .as_deref()
        .ok()
        .and_then(sha256_token)
        .is_some_and(|actual| actual.eq_ignore_ascii_case(&expected));
    if skip_deploy {
        return Ok(expected);
    }
    let timeout = child_timeout(PROBE_CAP, deadline)?;
    let mkdir = adb.run(AdbCommand::new(
        Some(serial),
        ["shell", "mkdir", "-p", helper::HELPER_REMOTE_DIR],
        timeout,
    ))?;
    if !mkdir.success() {
        bail!(
            "failed to create {}: {}",
            helper::HELPER_REMOTE_DIR,
            mkdir.stderr_utf8()
        );
    }
    let local = write_temp_helper(bytes)?;
    let push = adb.run(AdbCommand::new(
        Some(serial),
        [
            "push".to_string(),
            local.display().to_string(),
            remote.clone(),
        ],
        child_timeout(PUSH_CAP, deadline)?,
    ))?;
    let _ = std::fs::remove_file(&local);
    if !push.success() {
        bail!(
            "adb push helper failed: {}{}",
            push.stdout_utf8(),
            push.stderr_utf8()
        );
    }
    let chmod = adb.run(AdbCommand::new(
        Some(serial),
        ["shell", "chmod", "0755", remote.as_str()],
        child_timeout(PROBE_CAP, deadline)?,
    ))?;
    if !chmod.success() {
        bail!("chmod helper failed: {}", chmod.stderr_utf8());
    }
    let actual = root_shell_stdout(
        adb,
        serial,
        root,
        &format!("sha256sum {remote} 2>/dev/null || true"),
        deadline,
    )?;
    let actual = sha256_token(&actual)
        .ok_or_else(|| anyhow!("deployed HDR helper SHA-256 is unavailable"))?;
    if !actual.eq_ignore_ascii_case(&expected) {
        bail!("deployed HDR helper SHA-256 mismatch: expected {expected}, got {actual}");
    }
    Ok(expected)
}

fn sha256_token(output: &str) -> Option<&str> {
    let token = output.split_ascii_whitespace().next()?;
    (token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(token)
}

fn write_temp_helper(bytes: &[u8]) -> anyhow::Result<std::path::PathBuf> {
    let path = std::env::temp_dir().join(format!(
        "node-forge-hdr-screencap-{}-{}",
        std::process::id(),
        helper::HELPER_REMOTE_NAME
    ));
    std::fs::write(&path, bytes)?;
    Ok(path)
}

pub fn root_exec_out_args(root: RootForm, script: &str) -> Vec<String> {
    match root {
        RootForm::Direct => vec![
            "exec-out".to_string(),
            "sh".to_string(),
            "-c".to_string(),
            script.to_string(),
        ],
        RootForm::Su0 => vec![
            "exec-out".to_string(),
            "su".to_string(),
            "0".to_string(),
            "sh".to_string(),
            "-c".to_string(),
            script.to_string(),
        ],
        RootForm::SuDashC => vec![
            "exec-out".to_string(),
            "su".to_string(),
            "-c".to_string(),
            script.to_string(),
        ],
    }
}

pub fn helper_launch_script(request_id: u64) -> String {
    format!(
        "start=$(awk '{{print $22}}' /proc/$$/stat 2>/dev/null); printf '%s %s\\n' $$ \"$start\" > {pid}; exec {helper}",
        pid = helper::pid_path(request_id),
        helper = helper_remote_path()
    )
}

pub fn cleanup_remote_pid(
    adb: &mut dyn AdbTransport,
    serial: &str,
    root: RootForm,
    request_id: u64,
    deadline: Instant,
) {
    let pid = helper::pid_path(request_id);
    let script = format!(
        "read pid start < {pid} 2>/dev/null; current=$(awk '{{print $22}}' /proc/\"$pid\"/stat 2>/dev/null); if [ -n \"$pid\" ] && [ -n \"$start\" ] && [ \"$current\" = \"$start\" ]; then kill -TERM -- \"-$pid\" 2>/dev/null || kill -TERM \"$pid\" 2>/dev/null; fi; rm -f {pid}"
    );
    let timeout = adb::remaining(deadline)
        .min(adb::CLEANUP_GRACE)
        .max(std::time::Duration::from_millis(1));
    let _ = adb.run(AdbCommand {
        serial: Some(serial.to_string()),
        args: root_exec_out_args(root, &script)
            .into_iter()
            .map(|arg| {
                if arg == "exec-out" {
                    "shell".to_string()
                } else {
                    arg
                }
            })
            .collect(),
        timeout,
        stdin: None,
        ignore_cancel: true,
    });
}

fn getprop(
    adb: &mut dyn AdbTransport,
    serial: &str,
    key: &str,
    deadline: Instant,
) -> anyhow::Result<String> {
    shell_stdout(adb, serial, &format!("getprop {key}"), deadline)
}

fn shell_stdout(
    adb: &mut dyn AdbTransport,
    serial: &str,
    command: &str,
    deadline: Instant,
) -> anyhow::Result<String> {
    let output = adb.run(AdbCommand::new(
        Some(serial),
        ["shell", command],
        child_timeout(PROBE_CAP, deadline)?,
    ))?;
    if !output.success() {
        bail!(
            "{command} failed: {}{}",
            output.stdout_utf8(),
            output.stderr_utf8()
        );
    }
    Ok(output.stdout_utf8().trim().to_string())
}

fn root_shell_stdout(
    adb: &mut dyn AdbTransport,
    serial: &str,
    root: RootForm,
    command: &str,
    deadline: Instant,
) -> anyhow::Result<String> {
    let mut args = root_exec_out_args(root, command);
    if args.first().map(String::as_str) == Some("exec-out") {
        args[0] = "shell".to_string();
    }
    let output = adb.run(AdbCommand::new(
        Some(serial),
        args,
        child_timeout(PROBE_CAP, deadline)?,
    ))?;
    if !output.success() {
        bail!(
            "{command} failed: {}{}",
            output.stdout_utf8(),
            output.stderr_utf8()
        );
    }
    Ok(output.stdout_utf8().trim().to_string())
}

fn uid_is_root(output: &str) -> bool {
    output.trim() == "0"
}

pub fn linker_maps_tmp_unrestricted(config: &str) -> bool {
    let mut mapped = false;
    for line in config.lines() {
        let line = line.trim();
        if line.starts_with("dir.")
            && line.contains("/data/local/tmp")
            && line.contains("unrestricted")
        {
            mapped = true;
        }
    }
    if !mapped {
        return false;
    }
    let mut in_unrestricted = false;
    let mut isolated_false = false;
    for line in config.lines() {
        let line = line.trim();
        if line == "[unrestricted]" {
            in_unrestricted = true;
            continue;
        }
        if line.starts_with('[') {
            in_unrestricted = false;
        }
        if in_unrestricted && line.replace(' ', "") == "namespace.default.isolated=false" {
            isolated_false = true;
        }
    }
    isolated_false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linker_mapping_requires_unrestricted_tmp_and_non_isolated_default() {
        let config = "\n\
            dir./data/local/tmp = unrestricted\n\
            [unrestricted]\n\
            namespace.default.isolated = false\n\
            namespace.default.search.paths = /system/${LIB}\n";
        assert!(linker_maps_tmp_unrestricted(config));
        assert!(!linker_maps_tmp_unrestricted(
            "dir./data/local/tmp = isolated"
        ));
    }

    #[test]
    fn root_argv_forms_match_direct_su0_and_su_dash_c() {
        assert_eq!(root_exec_out_args(RootForm::Direct, "id -u")[0], "exec-out");
        assert_eq!(
            root_exec_out_args(RootForm::Su0, "id -u"),
            vec!["exec-out", "su", "0", "sh", "-c", "id -u"]
        );
        assert_eq!(
            root_exec_out_args(RootForm::SuDashC, "id -u"),
            vec!["exec-out", "su", "-c", "id -u"]
        );
    }

    #[test]
    fn helper_hash_match_requires_an_exact_sha256_token() {
        let expected = "a".repeat(64);
        assert_eq!(
            sha256_token(&format!("{expected}  helper")),
            Some(expected.as_str())
        );
        assert_eq!(
            sha256_token(&format!("prefix{expected}suffix  helper")),
            None
        );
        assert_eq!(sha256_token("not-a-digest"), None);
    }

    #[test]
    fn helper_lifecycle_uses_process_group_and_pid_start_identity() {
        let launch = helper_launch_script(7);
        assert!(launch.contains("/proc/$$/stat"));
        assert!(launch.contains(&helper::pid_path(7)));

        let serial = "ZY22";
        let mut adb = crate::android_screencap::adb::ScriptedAdb::new(vec![
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "sh".into(), "-c".into()],
                output: crate::android_screencap::adb::ok_stdout(""),
                min_timeout: None,
            },
        ]);
        cleanup_remote_pid(
            &mut adb,
            serial,
            RootForm::Direct,
            7,
            Instant::now() + std::time::Duration::from_secs(3),
        );
        let script = adb.calls[0].args.last().unwrap();
        assert!(script.contains("/proc/\"$pid\"/stat"));
        assert!(script.contains("kill -TERM -- \"-$pid\""));
    }

    #[test]
    fn missing_sha256sum_and_hash_mismatch_deploy_the_helper() {
        let serial = "ZY22";
        let deadline = Instant::now() + std::time::Duration::from_secs(45);
        let mut adb = crate::android_screencap::adb::ScriptedAdb::new(vec![
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "sh".into(), "-c".into()],
                output: crate::android_screencap::adb::failed("sha256sum: not found"),
                min_timeout: None,
            },
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "mkdir".into()],
                output: crate::android_screencap::adb::ok_stdout(""),
                min_timeout: None,
            },
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["push".into()],
                output: crate::android_screencap::adb::ok_stdout("pushed"),
                min_timeout: None,
            },
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "chmod".into()],
                output: crate::android_screencap::adb::ok_stdout(""),
                min_timeout: None,
            },
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "sh".into(), "-c".into()],
                output: crate::android_screencap::adb::ok_stdout(format!(
                    "{}  helper",
                    crate::android_screencap::helper::helper_sha256_hex().unwrap()
                )),
                min_timeout: None,
            },
        ]);
        ensure_helper(&mut adb, serial, RootForm::Direct, deadline).unwrap();
        assert!(
            adb.calls
                .iter()
                .any(|call| call.args.first().map(String::as_str) == Some("push"))
        );
        assert!(
            adb.calls
                .iter()
                .any(|call| call.args.iter().any(|arg| arg == "0755"))
        );

        let expected = crate::android_screencap::helper::helper_sha256_hex().unwrap();
        let mut adb = crate::android_screencap::adb::ScriptedAdb::new(vec![
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "sh".into(), "-c".into()],
                output: crate::android_screencap::adb::ok_stdout(format!("{expected}  helper")),
                min_timeout: None,
            },
        ]);
        ensure_helper(&mut adb, serial, RootForm::Direct, deadline).unwrap();
        assert!(
            !adb.calls
                .iter()
                .any(|call| call.args.first().map(String::as_str) == Some("push"))
        );
    }

    #[test]
    fn post_push_hash_mismatch_rejects_deployment() {
        let serial = "ZY22";
        let deadline = Instant::now() + std::time::Duration::from_secs(45);
        let mut adb = crate::android_screencap::adb::ScriptedAdb::new(vec![
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "sh".into(), "-c".into()],
                output: crate::android_screencap::adb::ok_stdout("missing"),
                min_timeout: None,
            },
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "mkdir".into()],
                output: crate::android_screencap::adb::ok_stdout(""),
                min_timeout: None,
            },
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["push".into()],
                output: crate::android_screencap::adb::ok_stdout("pushed"),
                min_timeout: None,
            },
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "chmod".into()],
                output: crate::android_screencap::adb::ok_stdout(""),
                min_timeout: None,
            },
            crate::android_screencap::adb::ScriptedAdbResponse {
                prefix: vec!["shell".into(), "sh".into(), "-c".into()],
                output: crate::android_screencap::adb::ok_stdout(format!(
                    "{}  helper",
                    "0".repeat(64)
                )),
                min_timeout: None,
            },
        ]);
        let error = ensure_helper(&mut adb, serial, RootForm::Direct, deadline).unwrap_err();
        assert!(error.to_string().contains("SHA-256 mismatch"));
    }

    #[test]
    fn helper_supported_is_fingerprint_locked() {
        let identity = DeviceIdentity {
            serial: "x".into(),
            abi: TARGET_ABI.into(),
            sdk: TARGET_SDK.into(),
            fingerprint: TARGET_FINGERPRINT.into(),
            linker_unrestricted: true,
        };
        assert_eq!(
            helper_supported(&identity),
            embedded_helper_bytes().is_some()
        );
        let mut mismatch = identity.clone();
        mismatch.fingerprint = "other".into();
        assert!(!helper_supported(&mismatch));
    }
}
