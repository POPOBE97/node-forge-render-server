use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use anyhow::{anyhow, bail};

pub const REQUEST_DEADLINE: Duration = Duration::from_secs(45);
pub const PROBE_CAP: Duration = Duration::from_secs(3);
pub const PUSH_CAP: Duration = Duration::from_secs(10);
pub const HELPER_CAP: Duration = Duration::from_secs(15);
pub const STOCK_CAP: Duration = Duration::from_secs(15);
pub const CLEANUP_GRACE: Duration = Duration::from_secs(3);

#[derive(Clone, Debug)]
pub struct AdbCommand {
    pub serial: Option<String>,
    pub args: Vec<String>,
    pub timeout: Duration,
    pub stdin: Option<Vec<u8>>,
    pub ignore_cancel: bool,
}

impl AdbCommand {
    pub fn new(
        serial: Option<&str>,
        args: impl IntoIterator<Item = impl Into<String>>,
        timeout: Duration,
    ) -> Self {
        Self {
            serial: serial.map(str::to_string),
            args: args.into_iter().map(Into::into).collect(),
            timeout,
            stdin: None,
            ignore_cancel: false,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AdbOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl AdbOutput {
    pub fn success(&self) -> bool {
        self.status == 0
    }

    pub fn stdout_utf8(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_utf8(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

pub trait AdbTransport {
    fn run(&mut self, command: AdbCommand) -> anyhow::Result<AdbOutput>;
}

#[derive(Clone, Debug)]
pub struct RealAdbTransport {
    pub adb_path: PathBuf,
    cancel: Option<Arc<AtomicBool>>,
}

impl RealAdbTransport {
    pub fn from_env() -> Self {
        let adb_path = std::env::var_os("NODE_FORGE_ADB_BIN")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("adb"));
        Self {
            adb_path,
            cancel: None,
        }
    }

    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel = Some(cancel);
        self
    }
}

impl AdbTransport for RealAdbTransport {
    fn run(&mut self, command: AdbCommand) -> anyhow::Result<AdbOutput> {
        run_adb(&self.adb_path, command, self.cancel.as_deref())
    }
}

pub fn remaining(deadline: Instant) -> Duration {
    deadline.saturating_duration_since(Instant::now())
}

pub fn child_timeout(cap: Duration, deadline: Instant) -> anyhow::Result<Duration> {
    let timeout = remaining(deadline).min(cap);
    if timeout.is_zero() {
        bail!("android screencap deadline expired");
    }
    Ok(timeout)
}

fn run_adb(
    adb_path: &Path,
    command: AdbCommand,
    cancel: Option<&AtomicBool>,
) -> anyhow::Result<AdbOutput> {
    if !command.ignore_cancel && cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
        bail!("adb {:?} cancelled", command.args);
    }
    let mut process = Command::new(adb_path);
    if let Some(serial) = &command.serial {
        process.arg("-s").arg(serial);
    }
    process.args(&command.args);
    process.stdin(if command.stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    process.stdout(Stdio::piped());
    process.stderr(Stdio::piped());
    let mut child = process
        .spawn()
        .map_err(|error| anyhow!("failed to start adb {:?}: {error}", command.args))?;
    if let Some(stdin_bytes) = command.stdin {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&stdin_bytes);
        }
    }
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("adb stdout missing"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("adb stderr missing"))?;
    let stdout_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });

    let started = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let stdout = stdout_thread.join().unwrap_or_default();
                let stderr = stderr_thread.join().unwrap_or_default();
                return Ok(AdbOutput {
                    status: status.code().unwrap_or(1),
                    stdout,
                    stderr,
                });
            }
            Ok(None) if started.elapsed() >= command.timeout => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                bail!(
                    "adb {:?} timed out after {:?}",
                    command.args,
                    command.timeout
                );
            }
            Ok(None)
                if !command.ignore_cancel
                    && cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) =>
            {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                bail!("adb {:?} cancelled", command.args);
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                bail!("adb {:?} wait failed: {error}", command.args);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScriptedAdb {
    pub scripts: Vec<ScriptedAdbResponse>,
    pub calls: Vec<AdbCommand>,
}

#[derive(Clone, Debug)]
pub struct ScriptedAdbResponse {
    pub prefix: Vec<String>,
    pub output: Result<AdbOutput, String>,
    pub min_timeout: Option<Duration>,
}

impl ScriptedAdb {
    pub fn new(scripts: Vec<ScriptedAdbResponse>) -> Self {
        Self {
            scripts,
            calls: Vec::new(),
        }
    }
}

impl AdbTransport for ScriptedAdb {
    fn run(&mut self, command: AdbCommand) -> anyhow::Result<AdbOutput> {
        self.calls.push(command.clone());
        let Some(index) = self
            .scripts
            .iter()
            .position(|script| command.args.starts_with(script.prefix.as_slice()))
        else {
            bail!("unexpected adb command {:?}", command.args);
        };
        let script = self.scripts.remove(index);
        if script
            .min_timeout
            .is_some_and(|min_timeout| command.timeout < min_timeout)
        {
            bail!(
                "adb {:?} timed out after {:?}",
                command.args,
                command.timeout
            );
        }
        return match script.output {
            Ok(output) => Ok(output),
            Err(error) => Err(anyhow::anyhow!("{error}")),
        };
    }
}

pub fn ok_stdout(stdout: impl Into<Vec<u8>>) -> Result<AdbOutput, String> {
    Ok(AdbOutput {
        status: 0,
        stdout: stdout.into(),
        stderr: Vec::new(),
    })
}

pub fn failed(stderr: &str) -> Result<AdbOutput, String> {
    Ok(AdbOutput {
        status: 1,
        stdout: Vec::new(),
        stderr: stderr.as_bytes().to_vec(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_timeout_uses_remaining_deadline() {
        let deadline = Instant::now() + Duration::from_secs(2);
        let timeout = child_timeout(Duration::from_secs(15), deadline).unwrap();
        assert!(timeout <= Duration::from_secs(2));
        assert!(timeout > Duration::from_secs(0));
    }

    #[test]
    fn cancellation_terminates_the_active_local_adb_process() {
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let trigger = thread::spawn(move || {
            thread::sleep(Duration::from_millis(30));
            worker_cancel.store(true, Ordering::Release);
        });
        let started = Instant::now();
        let error = run_adb(
            Path::new("/bin/sleep"),
            AdbCommand::new(None, ["5"], Duration::from_secs(5)),
            Some(cancel.as_ref()),
        )
        .unwrap_err();
        trigger.join().unwrap();
        assert!(error.to_string().contains("cancelled"));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn scripted_adb_rejects_unknown_commands() {
        let mut adb = ScriptedAdb::new(Vec::new());
        assert!(
            adb.run(AdbCommand::new(
                Some("x"),
                ["shell", "id"],
                Duration::from_secs(1)
            ))
            .is_err()
        );
    }
}
