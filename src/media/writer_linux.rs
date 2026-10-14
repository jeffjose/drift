//! Offline movie writer for export: H.264/HEVC/ProRes through libav, plus
//! an optional AAC stereo track.

use std::path::Path;

use anyhow::Result;

use super::PixelBuffer;
use super::encode::{Muxer, Rate, VideoCodec, VideoParams};
use crate::capture::mixer;
use crate::project::model::Codec;

pub struct WriterSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: Codec,
    /// Average bitrate, used when `quality` is None (fit to a file size).
    pub bitrate: u64,
    /// Constant quality 0..1 (the encoder spends what each frame needs).
    pub quality: Option<f64>,
    pub audio: bool,
}

pub struct MovieWriter {
    muxer: Muxer,
    soundtrack: Option<std::sync::Arc<Vec<[f32; 2]>>>,
    audio_written: usize,
    fps: u32,
    pub frames: u64,
    path: std::path::PathBuf,
}

impl MovieWriter {
    pub fn create(path: &Path, s: &WriterSpec) -> Result<Self> {
        let abs = std::path::absolute(path)?;
        let params = VideoParams {
            width: s.width,
            height: s.height,
            fps: s.fps,
            codec: match s.codec {
                Codec::H264 => VideoCodec::H264,
                Codec::Hevc => VideoCodec::Hevc,
                Codec::Prores => VideoCodec::Prores,
            },
            rate: match s.quality {
                Some(q) => Rate::Quality(q),
                None => Rate::Bitrate(s.bitrate),
            },
            realtime: false,
        };
        let muxer = Muxer::create(&abs, &params, usize::from(s.audio), true)?;
        Ok(MovieWriter { muxer, soundtrack: None, audio_written: 0, fps: s.fps, frames: 0, path: abs })
    }

    /// A BGRA buffer of the movie's size to render or copy a frame into.
    pub fn buffer(&self) -> Result<PixelBuffer> {
        let (w, h) = self.muxer.size();
        Ok(PixelBuffer::new(w, h))
    }

    /// Interleaved stereo at 48 kHz to write alongside the frames.
    pub fn set_soundtrack(&mut self, mix: std::sync::Arc<Vec<[f32; 2]>>) {
        self.soundtrack = Some(mix);
    }

    /// Write the soundtrack up to sample `until`.
    fn feed_audio(&mut self, until: usize) -> Result<()> {
        let Some(mix) = self.soundtrack.clone() else { return Ok(()) };
        let until = until.min(mix.len());
        if until > self.audio_written {
            self.muxer.push_audio(0, &mix[self.audio_written..until])?;
            self.audio_written = until;
        }
        Ok(())
    }

    /// Append the next frame (frames are evenly spaced at the export fps),
    /// with the audio that goes beside it.
    pub fn push_frame(&mut self, pb: &PixelBuffer) -> Result<()> {
        let t_s = self.frames as f64 / self.fps as f64;
        self.feed_audio(((t_s + 1.0) * mixer::RATE) as usize)?;
        pb.with(|bgra| self.muxer.push_frame(bgra, pb.stride()))?;
        self.frames += 1;
        Ok(())
    }

    pub fn finish(mut self) -> Result<()> {
        self.feed_audio(usize::MAX)?;
        self.muxer.finish()
    }

    pub fn cancel(self) {
        drop(self.muxer);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Copy BGRA rows `src_stride` bytes apart into a pixel buffer.
pub fn fill_bgra(pb: &PixelBuffer, bgra: &[u8], width: u32, height: u32, src_stride: usize) {
    let (stride, row) = (pb.stride(), (width * 4) as usize);
    pb.with_mut(|dst| {
        for y in 0..height as usize {
            dst[y * stride..y * stride + row].copy_from_slice(&bgra[y * src_stride..y * src_stride + row]);
        }
    });
}
