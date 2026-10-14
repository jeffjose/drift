//! Captions: transcribe the microphone track with Whisper (see `whisper`),
//! producing word timings in source milliseconds. Results go to
//! `recording/transcript.json`.

use std::path::PathBuf;

use anyhow::{Result, anyhow};

use crate::capture::mixer;
use crate::project::{Package, write_json};

/// What Whisper listens at.
const RATE: f64 = 16_000.0;

/// Speech-to-text engine for captions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Whisper(crate::whisper::Model),
}

impl Engine {
    pub fn parse(s: &str) -> Option<Engine> {
        crate::whisper::Model::parse(s.strip_prefix("whisper-").unwrap_or(s)).map(Engine::Whisper)
    }

    pub fn label(self) -> String {
        let Engine::Whisper(m) = self;
        format!("Whisper {}", m.name())
    }
}

/// Transcribe a package's microphone track. Returns the number of words.
pub fn transcribe_package(pkg: &Package, engine: Engine, progress: &std::sync::Arc<crate::whisper::Progress>) -> Result<usize> {
    let rec = pkg.load_recording()?;
    let mic = rec.meta.mic.as_ref().ok_or_else(|| anyhow!("this recording has no microphone track"))?;
    let stereo = crate::media::read_audio(&pkg.rec(&mic.file), mic.track)?;
    let mono = downsample(&stereo);
    let Engine::Whisper(model) = engine;
    let words = crate::whisper::transcribe(&mono, model, progress)?;
    write_json(&pkg.rec("transcript.json"), &words)?;
    Ok(words.len())
}

/// 48 kHz stereo → 16 kHz mono (box filter over each group of three).
pub fn downsample(stereo: &[[f32; 2]]) -> Vec<f32> {
    let step = (mixer::RATE / RATE).round() as usize;
    stereo
        .chunks(step)
        .map(|c| c.iter().map(|s| (s[0] + s[1]) * 0.5).sum::<f32>() / c.len() as f32)
        .collect()
}

/// Where a package's transcript lives.
pub fn transcript_path(pkg: &Package) -> PathBuf {
    pkg.rec("transcript.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downsample_averages_and_downmixes() {
        let s = vec![[1.0, 0.0]; 6];
        let m = downsample(&s);
        assert_eq!(m.len(), 2);
        assert!((m[0] - 0.5).abs() < 1e-6);
    }
}
