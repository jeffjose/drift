//! Reading recorded media with libav (ffmpeg's libraries).

use std::path::Path;
use std::sync::{Arc, Mutex, Once};

use anyhow::{Context, Result, anyhow, bail};
use ffmpeg_next as ff;
use ff::ffi;

use crate::capture::mixer;
use crate::render::VideoFrame;
use crate::util::SendBox;

/// Register libav's formats and codecs, and keep its log to errors.
pub fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = ff::init();
        ff::util::log::set_level(ff::util::log::Level::Error);
    });
}

struct Pixels {
    width: u32,
    height: u32,
    stride: usize,
    data: Mutex<Vec<u8>>,
}

/// A BGRA image shared between whoever holds it (what a CVPixelBuffer is
/// on macOS).
#[derive(Clone)]
pub struct PixelBuffer(Arc<Pixels>);

impl PixelBuffer {
    /// An opaque black image.
    pub fn new(width: u32, height: u32) -> Self {
        Self::from_vec(width, height, vec![0; width as usize * height as usize * 4])
    }

    /// Tightly packed BGRA rows.
    pub fn from_vec(width: u32, height: u32, data: Vec<u8>) -> Self {
        assert_eq!(data.len(), width as usize * height as usize * 4);
        PixelBuffer(Arc::new(Pixels { width, height, stride: width as usize * 4, data: Mutex::new(data) }))
    }

    pub fn size(&self) -> (u32, u32) {
        (self.0.width, self.0.height)
    }

    pub fn stride(&self) -> usize {
        self.0.stride
    }

    pub fn with<R>(&self, f: impl FnOnce(&[u8]) -> R) -> R {
        f(&self.0.data.lock().unwrap_or_else(|e| e.into_inner()))
    }

    pub fn with_mut<R>(&self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        f(&mut self.0.data.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

/// One decoded video frame.
pub struct Frame {
    pub pts_ms: f64,
    pub pb: SendBox<PixelBuffer>,
}

impl Clone for Frame {
    fn clone(&self) -> Self {
        Frame { pts_ms: self.pts_ms, pb: SendBox(self.pb.0.clone()) }
    }
}

impl Frame {
    pub fn size(&self) -> (u32, u32) {
        self.pb.0.size()
    }

    /// Hand the pixels to `f` as BGRA.
    pub fn with_bgra<R>(&self, f: impl FnOnce(&VideoFrame) -> R) -> R {
        let pb = &self.pb.0;
        let (width, height) = pb.size();
        pb.with(|bgra| f(&VideoFrame { width, height, stride: pb.stride(), bgra }))
    }
}

/// YUV to BGRA at the same size, with the matrix and range the frames say.
struct Converter {
    ctx: *mut ffi::SwsContext,
    key: (i32, u32, u32, i32, i32),
}

impl Drop for Converter {
    fn drop(&mut self) {
        // SAFETY: `ctx` came from sws_getContext and is freed once.
        unsafe { ffi::sws_freeContext(self.ctx) };
    }
}

impl Converter {
    fn new(frame: &ff::frame::Video) -> Result<Self> {
        // SAFETY: plain field reads of a decoded frame.
        let (format, space, range) = unsafe {
            let f = &*frame.as_ptr();
            (f.format, f.colorspace as i32, f.color_range as i32)
        };
        let (w, h) = (frame.width(), frame.height());
        // Untagged video is BT.709 limited range, as screen recordings are;
        // only what says it is BT.601 is read as that.
        let matrix = match frame.color_space() {
            ff::color::Space::BT470BG | ff::color::Space::SMPTE170M => ffi::SWS_CS_ITU601,
            ff::color::Space::BT2020NCL | ff::color::Space::BT2020CL => ffi::SWS_CS_BT2020,
            _ => ffi::SWS_CS_ITU709,
        };
        let full = matches!(frame.color_range(), ff::color::Range::JPEG);
        let flags = ffi::SWS_BILINEAR | ffi::SWS_ACCURATE_RND | ffi::SWS_FULL_CHR_H_INT;
        // SAFETY: sizes and formats come from a decoded frame; the context
        // is checked before use.
        let ctx = unsafe {
            let src: ffi::AVPixelFormat = frame.format().into();
            let ctx = ffi::sws_getContext(w as i32, h as i32, src, w as i32, h as i32, ffi::AVPixelFormat::AV_PIX_FMT_BGRA, flags as i32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            if ctx.is_null() {
                bail!("no conversion from {:?} to BGRA", frame.format());
            }
            ffi::sws_setColorspaceDetails(ctx, ffi::sws_getCoefficients(matrix as i32), full as i32, ffi::sws_getCoefficients(ffi::SWS_CS_DEFAULT as i32), 1, 0, 1 << 16, 1 << 16);
            ctx
        };
        Ok(Converter { ctx, key: (format, w, h, space, range) })
    }

    fn fits(&self, frame: &ff::frame::Video) -> bool {
        // SAFETY: plain field reads of a decoded frame.
        let f = unsafe { &*frame.as_ptr() };
        self.key == (f.format, frame.width(), frame.height(), f.colorspace as i32, f.color_range as i32)
    }

    fn run(&self, frame: &ff::frame::Video) -> PixelBuffer {
        let (w, h) = (frame.width(), frame.height());
        let mut out = vec![0u8; w as usize * h as usize * 4];
        let planes = [out.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()];
        let strides = [w as i32 * 4, 0, 0, 0];
        // SAFETY: the source planes belong to `frame`; the destination is
        // `out`, which holds h rows of w*4 bytes.
        unsafe {
            let f = &*frame.as_ptr();
            ffi::sws_scale(self.ctx, f.data.as_ptr() as *const *const u8, f.linesize.as_ptr(), 0, h as i32, planes.as_ptr(), strides.as_ptr());
        }
        PixelBuffer::from_vec(w, h, out)
    }
}

/// Sequential video reader that can restart at any time.
pub struct VideoReader {
    input: ff::format::context::Input,
    stream: usize,
    decoder: ff::decoder::Video,
    converter: Option<Converter>,
    /// Milliseconds per tick of the stream's time base.
    tick_ms: f64,
    pub size: (u32, u32),
    pub fps: f64,
    pub duration_ms: f64,
    /// Time of the first sample in the file (frames are reported relative to it).
    origin_ms: f64,
    /// Frames before this file time are decoded and dropped.
    start_ms: f64,
    drained: bool,
}

// SAFETY: the libav contexts are only touched through &mut self.
unsafe impl Send for VideoReader {}

fn rational(r: ff::Rational) -> f64 {
    if r.denominator() == 0 { 0.0 } else { r.numerator() as f64 / r.denominator() as f64 }
}

fn open_input(path: &Path) -> Result<ff::format::context::Input> {
    init();
    if !path.exists() {
        bail!("missing media file {}", path.display());
    }
    ff::format::input(path).with_context(|| format!("opening {}", path.display()))
}

impl VideoReader {
    pub fn open(path: &Path) -> Result<Self> {
        let input = open_input(path)?;
        let (stream, tick_ms, fps, start, duration, params) = {
            let s = input.streams().best(ff::media::Type::Video).ok_or_else(|| anyhow!("no video track in {}", path.display()))?;
            let tick_ms = rational(s.time_base()) * 1000.0;
            let fps = match rational(s.avg_frame_rate()) {
                f if f > 0.0 => f,
                _ => rational(s.rate()),
            };
            (s.index(), tick_ms, fps, s.start_time(), s.duration(), s.parameters())
        };
        let mut ctx = ff::codec::context::Context::from_parameters(params)?;
        ctx.set_threading(ff::threading::Config { kind: ff::threading::Type::Frame, count: 0, safe: false });
        let decoder = ctx.decoder().video().context("opening the video decoder")?;
        let origin_ms = if start == ffi::AV_NOPTS_VALUE { 0.0 } else { start as f64 * tick_ms };
        let duration_ms = if duration != ffi::AV_NOPTS_VALUE && duration > 0 {
            duration as f64 * tick_ms
        } else {
            input.duration().max(0) as f64 * 1000.0 / ffi::AV_TIME_BASE as f64
        };
        Ok(VideoReader {
            size: (decoder.width(), decoder.height()),
            input,
            stream,
            decoder,
            converter: None,
            tick_ms,
            fps: if fps > 0.0 { fps } else { 60.0 },
            duration_ms,
            origin_ms,
            start_ms: 0.0,
            drained: false,
        })
    }

    /// Restart decoding so the next frame is the one shown at `t_ms`.
    pub fn seek(&mut self, t_ms: f64) -> Result<()> {
        let start = (t_ms - 1000.0 / self.fps).max(0.0) + self.origin_ms;
        // To the key frame at or before the start; what's between is
        // decoded and dropped in `next_frame`.
        let ts = (start / 1000.0 * ffi::AV_TIME_BASE as f64) as i64;
        self.input.seek(ts, ..ts).or_else(|_| self.input.seek(0, ..0)).context("seeking")?;
        self.decoder.flush();
        self.start_ms = start;
        self.drained = false;
        Ok(())
    }

    pub fn next_frame(&mut self) -> Option<Frame> {
        let mut decoded = ff::frame::Video::empty();
        loop {
            if self.decoder.receive_frame(&mut decoded).is_ok() {
                let Some(ts) = decoded.timestamp().or(decoded.pts()) else { continue };
                let at = ts as f64 * self.tick_ms;
                // A tenth of a tick's slack for rounding in the seek time.
                if at < self.start_ms - self.tick_ms * 0.1 - 0.01 {
                    continue;
                }
                if self.converter.as_ref().is_none_or(|c| !c.fits(&decoded)) {
                    self.converter = Converter::new(&decoded).ok();
                }
                let pb = self.converter.as_ref()?.run(&decoded);
                return Some(Frame { pts_ms: at - self.origin_ms, pb: SendBox(pb) });
            }
            if self.drained {
                return None;
            }
            let mut packet = ff::Packet::empty();
            match packet.read(&mut self.input) {
                Ok(()) => {
                    if packet.stream() == self.stream {
                        let _ = self.decoder.send_packet(&packet);
                    }
                }
                Err(_) => {
                    let _ = self.decoder.send_eof();
                    self.drained = true;
                }
            }
        }
    }
}

/// Decode an audio track to 48 kHz interleaved stereo.
pub fn read_audio(path: &Path, track_index: usize) -> Result<Vec<[f32; 2]>> {
    let mut input = open_input(path)?;
    let (stream, tick, start, params) = {
        let s = input
            .streams()
            .filter(|s| s.parameters().medium() == ff::media::Type::Audio)
            .nth(track_index)
            .ok_or_else(|| anyhow!("no audio track {track_index} in {}", path.display()))?;
        (s.index(), rational(s.time_base()), s.start_time(), s.parameters())
    };
    let origin = if start == ffi::AV_NOPTS_VALUE { 0.0 } else { start as f64 * tick };
    let mut decoder = ff::codec::context::Context::from_parameters(params)?.decoder().audio().context("opening the audio decoder")?;
    let mut resampler: Option<ff::software::resampling::Context> = None;
    let mut out: Vec<[f32; 2]> = vec![];
    let mut decoded = ff::frame::Audio::empty();
    let mut converted = ff::frame::Audio::empty();
    let append = |out: &mut Vec<[f32; 2]>, f: &ff::frame::Audio| {
        let n = f.samples();
        if n > 0 {
            let pairs: &[[f32; 2]] = bytemuck::cast_slice(&f.data(0)[..n * 8]);
            out.extend_from_slice(pairs);
        }
    };
    let mut drain = |decoder: &mut ff::decoder::Audio, out: &mut Vec<[f32; 2]>| -> Result<()> {
        while decoder.receive_frame(&mut decoded).is_ok() {
            if decoded.channel_layout().is_empty() {
                decoded.set_channel_layout(ff::ChannelLayout::default(decoded.channels() as i32));
            }
            let r = match &mut resampler {
                Some(r) => r,
                None => resampler.insert(
                    ff::software::resampling::Context::get(
                        decoded.format(),
                        decoded.channel_layout(),
                        decoded.rate(),
                        ff::format::Sample::F32(ff::format::sample::Type::Packed),
                        ff::ChannelLayout::STEREO,
                        mixer::RATE as u32,
                    )
                    .context("audio resampler")?,
                ),
            };
            // Place by timestamp so gaps (pauses, drops) stay in sync.
            if let Some(ts) = decoded.timestamp().or(decoded.pts()) {
                let at = ((ts as f64 * tick - origin) * mixer::RATE).round().max(0.0) as usize;
                if at > out.len() && at - out.len() > 48 {
                    out.resize(at, [0.0, 0.0]);
                }
            }
            r.run(&decoded, &mut converted).context("resampling audio")?;
            append(out, &converted);
        }
        Ok(())
    };
    let mut packet = ff::Packet::empty();
    while packet.read(&mut input).is_ok() {
        if packet.stream() == stream && decoder.send_packet(&packet).is_ok() {
            drain(&mut decoder, &mut out)?;
        }
    }
    let _ = decoder.send_eof();
    drain(&mut decoder, &mut out)?;
    if let Some(r) = &mut resampler {
        while let Ok(Some(_)) = r.flush(&mut converted) {
            if converted.samples() == 0 {
                break;
            }
            append(&mut out, &converted);
        }
    }
    Ok(out)
}

/// Number of audio tracks in a file.
pub fn audio_track_count(path: &Path) -> Result<usize> {
    let input = open_input(path).context("opening media")?;
    Ok(input.streams().filter(|s| s.parameters().medium() == ff::media::Type::Audio).count())
}
