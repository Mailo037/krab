//! Startup timing (`krab --trace`), used to keep the cold start honest.
//!
//! Marks are collected in memory and written out in one go: writing to
//! stderr costs milliseconds per line, which would distort the very thing
//! being measured.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static ENABLED: AtomicBool = AtomicBool::new(false);
static MARKS: Mutex<Vec<(f64, String)>> = Mutex::new(Vec::new());

/// Call first thing in `main` so timings are relative to process start.
pub fn init() {
    let _ = START.set(Instant::now());
}

pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Record the time since process start, if tracing is on.
pub fn mark(label: &str) {
    if ENABLED.load(Ordering::Relaxed) {
        if let (Some(t0), Ok(mut marks)) = (START.get(), MARKS.lock()) {
            marks.push((t0.elapsed().as_secs_f64() * 1000.0, label.to_string()));
        }
    }
}

/// Print everything recorded so far.
pub fn flush() {
    if let Ok(mut marks) = MARKS.lock() {
        for (ms, label) in marks.drain(..) {
            eprintln!("[krab {ms:>7.2} ms] {label}");
        }
    }
}
