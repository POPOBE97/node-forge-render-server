use std::fs::OpenOptions;
use std::io::Write;
use std::sync::Mutex;
use std::time::Instant;

static LOG_FILE: Mutex<Option<std::fs::File>> = Mutex::new(None);

fn ensure_log_file() -> &'static Mutex<Option<std::fs::File>> {
    let mut guard = LOG_FILE.lock().unwrap();
    if guard.is_none() {
        let path = std::env::temp_dir().join("node-forge-frame-metrics.log");
        eprintln!("[perf-log] writing frame metrics to {}", path.display());
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .expect("failed to open perf log file");
        *guard = Some(file);
    }
    drop(guard);
    &LOG_FILE
}

pub fn log_metric(msg: &str) {
    let file_mutex = ensure_log_file();
    if let Ok(mut guard) = file_mutex.lock() {
        if let Some(ref mut file) = *guard {
            let _ = writeln!(file, "{msg}");
        }
    }
}

pub struct FrameTimer {
    start: Instant,
    frame_number: u64,
}

static FRAME_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

impl FrameTimer {
    pub fn new() -> Self {
        let frame_number =
            FRAME_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Self {
            start: Instant::now(),
            frame_number,
        }
    }

    pub fn frame_number(&self) -> u64 {
        self.frame_number
    }

    pub fn elapsed_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }

    pub fn lap(&self) -> Instant {
        Instant::now()
    }
}

#[macro_export]
macro_rules! metric_log {
    ($($arg:tt)*) => {
        $crate::perf_log::log_metric(&format!($($arg)*))
    };
}
