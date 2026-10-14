//! Drift: Glide, a fast native screen recorder and editor, on Linux.
//!
//! The engine, renderer and editor are Glide's (by Michael Shimeles, for
//! macOS), whose behaviour was reverse engineered from Screen Studio with
//! REA (see `analysis/SCREEN_STUDIO.md`). Capture (X11, PulseAudio),
//! media (libav) and the desktop integration are written for Linux.

pub mod engine;
pub mod project;
pub mod render;
pub mod synth;
pub mod capture;
pub mod util;
pub mod media;
pub mod export;
pub mod recorder;
pub mod transcribe;
pub mod exposure;
pub mod audiocheck;
pub mod whisper;
pub mod app;
