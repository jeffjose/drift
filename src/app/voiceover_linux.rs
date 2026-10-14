//! Voice-over capture in the editor: the default microphone, kept in
//! memory while the preview plays (muted, so the speakers don't bleed
//! into the microphone) and written into the package as AAC on stop.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Result;

use crate::capture::audio::{AudioIn, Input};

pub struct VoiceRecorder {
    mic: Option<AudioIn>,
    heard: Arc<Mutex<Vec<[f32; 2]>>>,
    path: PathBuf,
    started: Instant,
    /// File name inside recording/.
    pub file: String,
    /// Source time where the narration starts.
    pub start_source: f64,
}

impl VoiceRecorder {
    /// Start recording the default microphone into `recording/<file>`.
    pub fn start(package: &Path, file: &str, start_source: f64) -> Result<Self> {
        let heard: Arc<Mutex<Vec<[f32; 2]>>> = Arc::default();
        let into = heard.clone();
        let mic = AudioIn::start(&Input::Mic(None), move |_, samples| into.lock().unwrap().extend_from_slice(samples))?;
        Ok(VoiceRecorder { mic: Some(mic), heard, path: package.join("recording").join(file), started: Instant::now(), file: file.to_string(), start_source })
    }

    pub fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Stop and return the recorded length in ms.
    pub fn stop(mut self) -> f64 {
        let ms = self.started.elapsed().as_secs_f64() * 1000.0;
        // Dropping the microphone waits for its last chunk.
        self.mic = None;
        let samples = std::mem::take(&mut *self.heard.lock().unwrap());
        if let Err(e) = crate::media::encode::write_aac(&self.path, &samples) {
            eprintln!("glide: voice-over: {e:#}");
        }
        ms
    }
}
