//! Capture on Linux: the X11 screen through MIT-SHM, PulseAudio for system
//! audio and the microphone.

pub mod audio;
pub mod camera;
#[path = "../meter.rs"]
pub mod meter;
pub mod mixer;
pub mod preview;
pub mod record;
pub mod screen;
pub mod tcc;
pub mod winsize;
pub mod x11;

/// Seconds on the clock every capture thread and the input log share.
pub fn host_now() -> f64 {
    static EPOCH: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    EPOCH.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}
