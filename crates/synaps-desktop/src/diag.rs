//! Startup checkpoints and an opt-in stderr logger (`SYNAPS_DESKTOP_LOG=info`)
//! stamped with milliseconds since `main`, so GPUI/wgpu startup is visible.

use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static T0: OnceLock<Instant> = OnceLock::new();
static LEVEL: OnceLock<log::LevelFilter> = OnceLock::new();
static MARKS: Mutex<Vec<(&'static str, f64)>> = Mutex::new(Vec::new());

pub fn init(t0: Instant) {
    let _ = T0.set(t0);
    if let Ok(level) = std::env::var("SYNAPS_DESKTOP_LOG") {
        let level = level.parse().unwrap_or(log::LevelFilter::Info);
        let _ = LEVEL.set(level);
        if log::set_logger(&LOGGER).is_ok() {
            log::set_max_level(level);
        }
    }
}

pub fn since_main_ms() -> f64 {
    T0.get().map(|t| t.elapsed().as_secs_f64() * 1e3).unwrap_or(0.0)
}

pub fn mark(name: &'static str) {
    if let Ok(mut m) = MARKS.lock()
        && !m.iter().any(|(n, _)| *n == name)
    {
        m.push((name, (since_main_ms() * 100.0).round() / 100.0));
    }
}

pub fn marks() -> serde_json::Value {
    let m = MARKS.lock().map(|m| m.clone()).unwrap_or_default();
    serde_json::Value::Object(m.into_iter().map(|(k, v)| (k.to_string(), serde_json::json!(v))).collect())
}

struct Logger;
static LOGGER: Logger = Logger;

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= *LEVEL.get().unwrap_or(&log::LevelFilter::Off) && !m.target().starts_with("zbus") && !m.target().starts_with("tracing")
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("[{:>9.2} ms {:<5} {}] {}", since_main_ms(), r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}
