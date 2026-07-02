use std::{
    borrow::Cow,
    collections::VecDeque,
    env, fs,
    io::{self, BufRead, BufReader, Read},
    net::TcpListener,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidDevice {
    pub serial: String,
    pub state: String,
    pub usb: bool,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct AndroidReferenceFrame {
    pub id: u64,
    pub serial: String,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct AndroidReferenceStatus {
    pub running: bool,
    pub label: String,
    pub serial: Option<String>,
    pub frame_count: u64,
    pub size: Option<[u32; 2]>,
    pub fps: f32,
    pub last_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AndroidScreencapClipboardResult {
    pub serial: String,
    pub width: u32,
    pub height: u32,
    pub png_byte_len: usize,
}

#[derive(Default)]
pub struct AndroidReferenceState {
    session: Option<AndroidReferenceSession>,
    last_status: String,
    last_error: Option<String>,
}

struct AndroidReferenceSession {
    serial: String,
    port: u16,
    adb_path: PathBuf,
    server_process: Child,
    ffmpeg_process: Child,
    shared: Arc<Mutex<AndroidReferenceShared>>,
}

#[derive(Default)]
struct AndroidReferenceShared {
    latest_frame: Option<AndroidReferenceFrame>,
    frame_count: u64,
    size: Option<[u32; 2]>,
    fps: f32,
    error: Option<String>,
    done: bool,
}

struct AndroidReferenceVideoOptions {
    codec: String,
    bit_rate: u32,
    max_fps: u32,
}

impl AndroidReferenceVideoOptions {
    fn from_env() -> anyhow::Result<Self> {
        let codec = env::var("NODE_FORGE_ANDROID_REFERENCE_VIDEO_CODEC")
            .unwrap_or_else(|_| "h264".to_string());
        if !matches!(codec.as_str(), "h264" | "h265" | "av1") {
            anyhow::bail!(
                "NODE_FORGE_ANDROID_REFERENCE_VIDEO_CODEC must be h264, h265, or av1; got {codec}"
            );
        }

        Ok(Self {
            codec,
            bit_rate: env_number_or_default(
                "NODE_FORGE_ANDROID_REFERENCE_VIDEO_BIT_RATE",
                256_000_000,
            )?,
            max_fps: env_number_or_default("NODE_FORGE_ANDROID_REFERENCE_MAX_FPS", 60)?,
        })
    }
}

#[derive(Default)]
struct AndroidReferencePipePerf {
    window_start: Option<Instant>,
    frames: u32,
    overwritten_pending: u32,
    read_total: Duration,
    read_max: Duration,
    frame_bytes: usize,
}

impl AndroidReferencePipePerf {
    fn record(
        &mut self,
        now: Instant,
        frame_id: u64,
        size: [u32; 2],
        frame_bytes: usize,
        read_elapsed: Duration,
        overwrote_pending: bool,
    ) {
        let window_start = *self.window_start.get_or_insert(now);
        self.frames += 1;
        self.frame_bytes += frame_bytes;
        self.read_total += read_elapsed;
        self.read_max = self.read_max.max(read_elapsed);
        if overwrote_pending {
            self.overwritten_pending += 1;
        }

        let window_elapsed = now.duration_since(window_start);
        if window_elapsed < Duration::from_secs(1) {
            return;
        }

        let frames = self.frames.max(1) as f64;
        let fps = frames / window_elapsed.as_secs_f64().max(0.001);
        let avg_read_ms = self.read_total.as_secs_f64() * 1000.0 / frames;
        let max_read_ms = self.read_max.as_secs_f64() * 1000.0;
        let mb = self.frame_bytes as f64 / (1024.0 * 1024.0);
        let mbps = mb / window_elapsed.as_secs_f64().max(0.001);
        eprintln!(
            "[android-reference:pipe] fps={fps:.1} frames={} read_ms_avg={avg_read_ms:.2} read_ms_max={max_read_ms:.2} pending_overwrites={} last_frame={} size={}x{} frame_mb={mb:.1} frame_mb_s={mbps:.1}",
            self.frames, self.overwritten_pending, frame_id, size[0], size[1],
        );

        *self = Self {
            window_start: Some(now),
            ..Self::default()
        };
    }
}

impl Drop for AndroidReferenceSession {
    fn drop(&mut self) {
        let _ = self.ffmpeg_process.kill();
        let _ = self.ffmpeg_process.wait();
        let _ = self.server_process.kill();
        let _ = self.server_process.wait();
        let _ = Command::new(&self.adb_path)
            .args(["-s", self.serial.as_str(), "forward", "--remove"])
            .arg(format!("tcp:{}", self.port))
            .status();
    }
}

impl AndroidReferenceState {
    pub fn start_usb(&mut self) -> anyhow::Result<String> {
        if self.session.is_some() {
            return Ok(self.last_status.clone());
        }

        let adb_path = resolve_command_path("NODE_FORGE_ADB_BIN", "adb");
        let ffmpeg_path = resolve_command_path("NODE_FORGE_FFMPEG_BIN", "ffmpeg");
        let server_jar = resolve_scrcpy_server_jar()?;
        let devices = android_devices_with_adb(&adb_path)?;
        let device = select_single_ready_usb_device(&devices)?;
        let port = pick_local_port()?;
        let remote_jar = "/data/local/tmp/node-forge-scrcpy-server.jar";

        run_checked(
            Command::new(&adb_path)
                .args(["-s", device.serial.as_str(), "push"])
                .arg(&server_jar)
                .arg(remote_jar),
            "adb push scrcpy-server",
        )?;
        run_checked(
            Command::new(&adb_path)
                .args(["-s", device.serial.as_str(), "forward"])
                .arg(format!("tcp:{port}"))
                .arg("localabstract:scrcpy"),
            "adb forward scrcpy socket",
        )?;

        let version =
            env::var("NODE_FORGE_SCRCPY_SERVER_VERSION").unwrap_or_else(|_| "4.0".to_string());
        let video_options = AndroidReferenceVideoOptions::from_env()?;
        eprintln!(
            "[android-reference] starting scrcpy encoder codec={} bit_rate={} max_fps={}",
            video_options.codec, video_options.bit_rate, video_options.max_fps
        );

        let mut server_command = Command::new(&adb_path);
        server_command
            .args([
                "-s",
                device.serial.as_str(),
                "shell",
                format!("CLASSPATH={remote_jar}").as_str(),
                "app_process",
                "/",
                "com.genymobile.scrcpy.Server",
                version.as_str(),
                "tunnel_forward=true",
                "audio=false",
                "control=false",
                "cleanup=false",
                "raw_stream=true",
                "max_size=0",
            ])
            .arg(format!("video_codec={}", video_options.codec))
            .arg(format!("video_bit_rate={}", video_options.bit_rate))
            .arg(format!("max_fps={}", video_options.max_fps))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut server_process = server_command.spawn().map_err(|error| {
            anyhow::anyhow!("failed to start scrcpy-server through adb: {error}")
        })?;

        // Give app_process enough time to publish the forwarded socket and video config packet.
        thread::sleep(Duration::from_millis(800));

        let tcp_url = format!("tcp://127.0.0.1:{port}?tcp_nodelay=1");
        let mut ffmpeg_process = match Command::new(&ffmpeg_path)
            .args([
                "-hide_banner",
                "-loglevel",
                "warning",
                "-flags",
                "low_delay",
                "-probesize",
                "50000000",
                "-analyzeduration",
                "5000000",
                "-f",
                "h264",
                "-i",
                tcp_url.as_str(),
                "-an",
                "-pix_fmt",
                "rgba",
                "-f",
                "image2pipe",
                "-vcodec",
                "pam",
                "-",
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
        {
            Ok(process) => process,
            Err(error) => {
                cleanup_start_failure(
                    &adb_path,
                    device.serial.as_str(),
                    port,
                    &mut server_process,
                    None,
                );
                return Err(anyhow::anyhow!(
                    "failed to start ffmpeg for scrcpy reference: {error}"
                ));
            }
        };

        let Some(stdout) = ffmpeg_process.stdout.take() else {
            cleanup_start_failure(
                &adb_path,
                device.serial.as_str(),
                port,
                &mut server_process,
                Some(&mut ffmpeg_process),
            );
            anyhow::bail!("ffmpeg stdout unavailable");
        };
        let ffmpeg_stderr = ffmpeg_process.stderr.take();
        let server_stderr = server_process.stderr.take();
        let shared = Arc::new(Mutex::new(AndroidReferenceShared::default()));

        spawn_pam_reader(stdout, shared.clone(), device.serial.clone());
        spawn_stderr_collector("ffmpeg", ffmpeg_stderr, shared.clone());
        spawn_stderr_collector("scrcpy-server", server_stderr, shared.clone());

        let status = format!("Scrcpy USB {}", device.serial);
        self.session = Some(AndroidReferenceSession {
            serial: device.serial.clone(),
            port,
            adb_path,
            server_process,
            ffmpeg_process,
            shared,
        });
        self.last_status = status.clone();
        self.last_error = None;
        Ok(status)
    }

    pub fn stop(&mut self) -> String {
        self.session = None;
        self.last_status = "Android reference stopped".to_string();
        self.last_status.clone()
    }

    pub fn take_latest_frame(&mut self) -> Option<AndroidReferenceFrame> {
        self.refresh_done_state();
        let session = self.session.as_ref()?;
        let mut shared = session.shared.lock().ok()?;
        shared.latest_frame.take()
    }

    pub fn status(&mut self) -> AndroidReferenceStatus {
        self.refresh_done_state();
        let Some(session) = self.session.as_ref() else {
            return AndroidReferenceStatus {
                running: false,
                label: if self.last_status.is_empty() {
                    "Image".to_string()
                } else {
                    self.last_status.clone()
                },
                serial: None,
                frame_count: 0,
                size: None,
                fps: 0.0,
                last_error: self.last_error.clone(),
            };
        };

        let shared = session.shared.lock().ok();
        AndroidReferenceStatus {
            running: true,
            label: self.last_status.clone(),
            serial: Some(session.serial.clone()),
            frame_count: shared.as_ref().map(|s| s.frame_count).unwrap_or_default(),
            size: shared.as_ref().and_then(|s| s.size),
            fps: shared.as_ref().map(|s| s.fps).unwrap_or_default(),
            last_error: shared
                .as_ref()
                .and_then(|s| s.error.clone())
                .or_else(|| self.last_error.clone()),
        }
    }

    fn refresh_done_state(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let done = session.shared.lock().map(|s| s.done).unwrap_or(true);
        if !done {
            return;
        }
        self.last_error = session.shared.lock().ok().and_then(|s| s.error.clone());
        if let Some(error) = self.last_error.as_ref() {
            self.last_status = format!("Android reference stopped: {error}");
        } else {
            self.last_status = "Android reference stopped".to_string();
        }
        self.session = None;
    }
}

fn resolve_command_path(env_key: &str, fallback: &str) -> PathBuf {
    env::var_os(env_key)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(fallback))
}

fn env_number_or_default(env_key: &str, default: u32) -> anyhow::Result<u32> {
    match env::var(env_key) {
        Ok(value) if !value.trim().is_empty() => parse_number_with_suffix(value.trim(), env_key),
        _ => Ok(default),
    }
}

fn parse_number_with_suffix(value: &str, label: &str) -> anyhow::Result<u32> {
    let (number, multiplier) = match value.as_bytes().last().copied() {
        Some(b'k' | b'K') => (&value[..value.len() - 1], 1_000_u64),
        Some(b'm' | b'M') => (&value[..value.len() - 1], 1_000_000_u64),
        _ => (value, 1_u64),
    };
    let number: u64 = number
        .parse()
        .map_err(|error| anyhow::anyhow!("invalid {label} value {value:?}: {error}"))?;
    let value = number
        .checked_mul(multiplier)
        .filter(|value| *value <= u32::MAX as u64)
        .ok_or_else(|| anyhow::anyhow!("{label} value {value:?} is too large"))?;
    Ok(value as u32)
}

fn resolve_scrcpy_server_jar() -> anyhow::Result<PathBuf> {
    for env_key in ["NODE_FORGE_SCRCPY_SERVER_JAR", "SCRCPY_SERVER_PATH"] {
        if let Some(path) = env::var_os(env_key).filter(|value| !value.is_empty()) {
            let path = PathBuf::from(path);
            if path.exists() {
                return Ok(path);
            }
            anyhow::bail!(
                "{env_key} points to missing scrcpy-server jar: {}",
                path.display()
            );
        }
    }

    let mut candidates = vec![
        PathBuf::from("scrcpy-server"),
        PathBuf::from("scrcpy-server.jar"),
        PathBuf::from("scrcpy-server-v4.0"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/scrcpy-server"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/scrcpy-server.jar"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/scrcpy-server-v4.0"),
    ];
    candidates.extend(homebrew_scrcpy_server_candidates(
        "/opt/homebrew/Cellar/scrcpy",
    ));
    candidates.extend(homebrew_scrcpy_server_candidates(
        "/usr/local/Cellar/scrcpy",
    ));

    candidates
        .into_iter()
        .find(|path| path.exists())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "scrcpy-server jar not found; set NODE_FORGE_SCRCPY_SERVER_JAR or SCRCPY_SERVER_PATH"
            )
        })
}

fn homebrew_scrcpy_server_candidates(root: &str) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut versions: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    versions.sort();
    versions
        .into_iter()
        .rev()
        .flat_map(|version_dir| {
            [
                version_dir.join("share/scrcpy/scrcpy-server"),
                version_dir.join("share/scrcpy/scrcpy-server.jar"),
            ]
        })
        .collect()
}

fn run_checked(command: &mut Command, label: &str) -> anyhow::Result<()> {
    let output = command
        .output()
        .map_err(|error| anyhow::anyhow!("{label} failed to start: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    anyhow::bail!("{label} failed: {}{}", stdout, stderr);
}

fn cleanup_start_failure(
    adb_path: &Path,
    serial: &str,
    port: u16,
    server_process: &mut Child,
    ffmpeg_process: Option<&mut Child>,
) {
    if let Some(ffmpeg_process) = ffmpeg_process {
        let _ = ffmpeg_process.kill();
        let _ = ffmpeg_process.wait();
    }
    let _ = server_process.kill();
    let _ = server_process.wait();
    let _ = Command::new(adb_path)
        .args(["-s", serial, "forward", "--remove"])
        .arg(format!("tcp:{port}"))
        .status();
}

fn pick_local_port() -> anyhow::Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

pub fn android_devices_with_adb(adb_path: &Path) -> anyhow::Result<Vec<AndroidDevice>> {
    let output = Command::new(adb_path)
        .arg("devices")
        .arg("-l")
        .output()
        .map_err(|error| anyhow::anyhow!("failed to run adb devices -l: {error}"))?;
    if !output.status.success() {
        anyhow::bail!(
            "adb devices -l failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(parse_adb_devices(&String::from_utf8_lossy(&output.stdout)))
}

pub fn parse_adb_devices(output: &str) -> Vec<AndroidDevice> {
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with("List of devices") {
                return None;
            }
            let mut parts = line.split_whitespace();
            let serial = parts.next()?.to_string();
            let state = parts.next().unwrap_or("unknown").to_string();
            let details: Vec<&str> = parts.collect();
            let usb = details.iter().any(|part| part.starts_with("usb:"))
                || (!serial.contains(':') && !serial.starts_with("emulator-"));
            Some(AndroidDevice {
                serial,
                state,
                usb,
                description: details.join(" "),
            })
        })
        .collect()
}

pub fn select_single_ready_usb_device(devices: &[AndroidDevice]) -> anyhow::Result<AndroidDevice> {
    let ready_usb: Vec<&AndroidDevice> = devices
        .iter()
        .filter(|device| device.usb && device.state == "device")
        .collect();
    match ready_usb.as_slice() {
        [device] => Ok((*device).clone()),
        [] => {
            if devices
                .iter()
                .any(|device| device.usb && device.state == "unauthorized")
            {
                anyhow::bail!(
                    "USB Android device is unauthorized; accept the debugging prompt on the device"
                );
            }
            anyhow::bail!("no ready USB Android device found");
        }
        many => anyhow::bail!(
            "multiple ready USB Android devices found: {}",
            many.iter()
                .map(|device| device.serial.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

pub fn copy_screencap_png_to_clipboard() -> anyhow::Result<AndroidScreencapClipboardResult> {
    let adb_path = resolve_command_path("NODE_FORGE_ADB_BIN", "adb");
    let devices = android_devices_with_adb(&adb_path)?;
    let device = select_single_ready_usb_device(&devices)?;
    let output = Command::new(&adb_path)
        .args(["-s", device.serial.as_str(), "exec-out", "screencap", "-p"])
        .output()
        .map_err(|error| anyhow::anyhow!("failed to run adb screencap: {error}"))?;

    if !output.status.success() {
        anyhow::bail!(
            "adb screencap failed: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    if output.stdout.is_empty() {
        anyhow::bail!("adb screencap returned no PNG data");
    }

    let png_byte_len = output.stdout.len();
    let rgba = image::load_from_memory(output.stdout.as_slice())
        .map_err(|error| anyhow::anyhow!("failed to decode adb screencap PNG: {error}"))?
        .to_rgba8();
    let width = rgba.width();
    let height = rgba.height();
    arboard::Clipboard::new()
        .and_then(|mut clipboard| {
            clipboard.set_image(arboard::ImageData {
                width: width as usize,
                height: height as usize,
                bytes: Cow::Owned(rgba.into_raw()),
            })
        })
        .map_err(|error| anyhow::anyhow!("failed to copy adb screencap to clipboard: {error}"))?;

    Ok(AndroidScreencapClipboardResult {
        serial: device.serial,
        width,
        height,
        png_byte_len,
    })
}

fn spawn_pam_reader(
    stdout: impl Read + Send + 'static,
    shared: Arc<Mutex<AndroidReferenceShared>>,
    serial: String,
) {
    thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        let mut next_id = 1;
        let mut frame_times = VecDeque::new();
        let mut perf = AndroidReferencePipePerf::default();
        loop {
            let read_start = Instant::now();
            match read_pam_frame(&mut reader) {
                Ok(Some(frame)) => {
                    let now = Instant::now();
                    let read_elapsed = now.duration_since(read_start);
                    frame_times.push_back(now);
                    while frame_times
                        .front()
                        .is_some_and(|time| now.duration_since(*time) > Duration::from_secs(1))
                    {
                        frame_times.pop_front();
                    }
                    let size = [frame.width, frame.height];
                    let frame_bytes = frame.rgba.len();
                    let reference_frame = AndroidReferenceFrame {
                        id: next_id,
                        serial: serial.clone(),
                        width: frame.width,
                        height: frame.height,
                        rgba: frame.rgba,
                    };
                    next_id += 1;
                    let frame_id = reference_frame.id;
                    let overwrote_pending;
                    if let Ok(mut shared) = shared.lock() {
                        overwrote_pending = shared.latest_frame.is_some();
                        shared.frame_count = reference_frame.id;
                        shared.size = Some(size);
                        shared.fps = frame_times.len() as f32;
                        shared.latest_frame = Some(reference_frame);
                    } else {
                        break;
                    }
                    perf.record(
                        now,
                        frame_id,
                        size,
                        frame_bytes,
                        read_elapsed,
                        overwrote_pending,
                    );
                }
                Ok(None) => {
                    if let Ok(mut shared) = shared.lock() {
                        shared.done = true;
                    }
                    break;
                }
                Err(error) => {
                    if let Ok(mut shared) = shared.lock() {
                        shared.error = Some(format!("failed to read decoded frame: {error}"));
                        shared.done = true;
                    }
                    break;
                }
            }
        }
    });
}

fn spawn_stderr_collector(
    label: &'static str,
    stderr: Option<impl Read + Send + 'static>,
    shared: Arc<Mutex<AndroidReferenceShared>>,
) {
    let Some(stderr) = stderr else {
        return;
    };
    thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => break,
                Ok(_) => {
                    let message = line.trim();
                    if message.is_empty() {
                        continue;
                    }
                    eprintln!("[android-reference:{label}] {message}");
                    if let Ok(mut shared) = shared.lock() {
                        shared.error = Some(format!("{label}: {message}"));
                    }
                }
                Err(_) => break,
            }
        }
    });
}

struct PamFrame {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn read_pam_frame(reader: &mut impl BufRead) -> io::Result<Option<PamFrame>> {
    let Some(magic) = read_pam_header_line(reader)? else {
        return Ok(None);
    };
    if magic != "P7" {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported image2pipe frame magic {magic:?}"),
        ));
    }

    let mut width = None;
    let mut height = None;
    let mut depth = None;
    let mut max_value = None;
    let mut tuple_type = None;
    loop {
        let Some(line) = read_pam_header_line(reader)? else {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "missing PAM ENDHDR",
            ));
        };
        if line == "ENDHDR" {
            break;
        }
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let Some(key) = parts.next() else {
            continue;
        };
        let Some(value) = parts.next() else {
            continue;
        };
        match key {
            "WIDTH" => width = Some(parse_pam_u32(value, "width")?),
            "HEIGHT" => height = Some(parse_pam_u32(value, "height")?),
            "DEPTH" => depth = Some(parse_pam_u32(value, "depth")?),
            "MAXVAL" => max_value = Some(parse_pam_u32(value, "max value")?),
            "TUPLTYPE" => tuple_type = Some(value.to_string()),
            _ => {}
        }
    }

    let width =
        width.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing PAM width"))?;
    let height =
        height.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing PAM height"))?;
    let depth =
        depth.ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing PAM depth"))?;
    let max_value = max_value
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing PAM max value"))?;
    if depth != 4 || max_value != 255 || tuple_type.as_deref() != Some("RGB_ALPHA") {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "unsupported PAM format depth={depth} max_value={max_value} tuple_type={:?}",
                tuple_type
            ),
        ));
    }

    let byte_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "PAM frame too large"))?;
    let mut rgba = vec![0; byte_len as usize];
    reader.read_exact(&mut rgba)?;
    Ok(Some(PamFrame {
        width,
        height,
        rgba,
    }))
}

fn parse_pam_u32(token: &str, name: &str) -> io::Result<u32> {
    token.parse::<u32>().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid PAM {name} {token:?}: {error}"),
        )
    })
}

fn read_pam_header_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    let count = reader.read_line(&mut line)?;
    if count == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim().to_string()))
}

#[cfg(test)]
mod tests {
    use super::{
        parse_adb_devices, parse_number_with_suffix, read_pam_frame, select_single_ready_usb_device,
    };
    use std::io::BufReader;

    #[test]
    fn adb_device_selection_prefers_single_ready_usb() {
        let devices = parse_adb_devices(
            "List of devices attached\n\
             ZY22 device usb:336592896X product:foo model:bar device:baz\n\
             192.168.0.4:5555 device product:wifi\n",
        );
        let selected = select_single_ready_usb_device(&devices).unwrap();
        assert_eq!(selected.serial, "ZY22");
    }

    #[test]
    fn adb_device_selection_rejects_ambiguous_usb() {
        let devices = parse_adb_devices(
            "List of devices attached\n\
             A device usb:1\n\
             B device usb:2\n",
        );
        assert!(select_single_ready_usb_device(&devices).is_err());
    }

    #[test]
    fn pam_reader_reads_multiple_rgba_frames() {
        let bytes = b"P7\nWIDTH 2\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n\xff\0\0\xff\0\xff\0\x80P7\n# comment\nWIDTH 1\nHEIGHT 1\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n\0\0\xff\x40";
        let mut reader = BufReader::new(&bytes[..]);
        let first = read_pam_frame(&mut reader).unwrap().unwrap();
        let second = read_pam_frame(&mut reader).unwrap().unwrap();
        assert_eq!((first.width, first.height, first.rgba.len()), (2, 1, 8));
        assert_eq!(
            (second.width, second.height, second.rgba),
            (1, 1, vec![0, 0, 255, 64])
        );
    }

    #[test]
    fn parse_number_with_suffix_accepts_video_option_units() {
        assert_eq!(
            parse_number_with_suffix("128M", "bitrate").unwrap(),
            128_000_000
        );
        assert_eq!(parse_number_with_suffix("60", "fps").unwrap(), 60);
    }
}
