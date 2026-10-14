//! Captions: transcribe the microphone track with Apple's Speech framework
//! (on-device when the recogniser supports it) or Whisper (see `whisper`),
//! producing word timings in source milliseconds. Results go to
//! `recording/transcript.json`.
//!
//! The audio is downmixed to 16 kHz mono WAV and split into chunks of at most
//! ~55 s at the quietest point near each boundary, because URL recognition
//! requests are limited in length.

use std::path::PathBuf;

use anyhow::{Result, anyhow};

use crate::capture::mixer;
#[cfg(not(target_os = "macos"))]
use crate::project::recording::Word;
use crate::project::{Package, write_json};

#[cfg(target_os = "macos")]
mod apple;
#[cfg(target_os = "macos")]
pub use apple::transcribe_samples;

/// Apple's Speech framework is only on macOS.
#[cfg(not(target_os = "macos"))]
pub fn transcribe_samples(_samples: &[f32]) -> Result<Vec<Word>> {
    Err(anyhow!("Apple Speech is only available on macOS — use Whisper"))
}

const RATE: f64 = 16_000.0;
const MAX_CHUNK_S: f64 = 55.0;
/// How far back from the hard limit we look for a quiet split point.
const SPLIT_WINDOW_S: f64 = 8.0;

/// Speech-to-text engine for captions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    /// Apple's Speech framework (needs Speech Recognition permission).
    Apple,
    Whisper(crate::whisper::Model),
}

impl Engine {
    pub fn parse(s: &str) -> Option<Engine> {
        match s {
            "apple" => Some(Engine::Apple),
            _ => crate::whisper::Model::parse(s.strip_prefix("whisper-").unwrap_or(s)).map(Engine::Whisper),
        }
    }

    pub fn label(self) -> String {
        match self {
            Engine::Apple => "Apple Speech".into(),
            Engine::Whisper(m) => format!("Whisper {}", m.name()),
        }
    }
}

/// Transcribe a package's microphone track. Returns the number of words.
pub fn transcribe_package(pkg: &Package, engine: Engine, progress: &std::sync::Arc<crate::whisper::Progress>) -> Result<usize> {
    let rec = pkg.load_recording()?;
    let mic = rec.meta.mic.as_ref().ok_or_else(|| anyhow!("this recording has no microphone track"))?;
    let stereo = crate::media::read_audio(&pkg.rec(&mic.file), mic.track)?;
    let mono = downsample(&stereo);
    let words = match engine {
        Engine::Apple => transcribe_samples(&mono)?,
        Engine::Whisper(model) => crate::whisper::transcribe(&mono, model, progress)?,
    };
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

/// Split points (sample indices) so no chunk exceeds `MAX_CHUNK_S`, cutting at
/// the quietest 100 ms window near each limit.
pub fn split_points(samples: &[f32]) -> Vec<usize> {
    let max = (MAX_CHUNK_S * RATE) as usize;
    let win = (0.1 * RATE) as usize;
    let mut cuts = vec![0];
    let mut start = 0;
    while samples.len() - start > max {
        let hi = start + max;
        let lo = hi - (SPLIT_WINDOW_S * RATE) as usize;
        let mut best = (f32::MAX, hi);
        let mut i = lo;
        while i + win <= hi {
            let e: f32 = samples[i..i + win].iter().map(|s| s * s).sum();
            if e < best.0 {
                best = (e, i + win / 2);
            }
            i += win / 2;
        }
        start = best.1;
        cuts.push(start);
    }
    cuts.push(samples.len());
    cuts
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

    #[test]
    fn splits_long_audio_at_quiet_points() {
        let n = (RATE * 130.0) as usize;
        let quiet_at = (RATE * 50.0) as usize;
        let samples: Vec<f32> = (0..n)
            .map(|i| if (quiet_at..quiet_at + 3200).contains(&i) { 0.0 } else { ((i as f32) * 0.05).sin() * 0.5 })
            .collect();
        let cuts = split_points(&samples);
        assert_eq!(cuts.first(), Some(&0));
        assert_eq!(cuts.last(), Some(&n));
        for w in cuts.windows(2) {
            assert!(w[1] - w[0] <= (MAX_CHUNK_S * RATE) as usize);
        }
        // The first cut lands inside the silent gap.
        assert!((quiet_at..quiet_at + 3200).contains(&cuts[1]), "cut at {}", cuts[1]);
    }

    #[test]
    fn short_audio_is_one_chunk() {
        let samples = vec![0.1f32; (RATE * 10.0) as usize];
        assert_eq!(split_points(&samples), vec![0, samples.len()]);
    }
}
