//! Writing movies with libav: H.264/HEVC through the GPU's encoder (NVENC)
//! when there is one and x264/x265 when there isn't, ProRes, and AAC stereo
//! tracks. Export and the recorder both write through this.

use std::path::Path;

use anyhow::{Context, Result, anyhow, bail};
use ffmpeg_next as ff;
use ff::ffi;

use super::sws;
use crate::capture::mixer;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VideoCodec {
    H264,
    Hevc,
    Prores,
}

#[derive(Clone, Copy, Debug)]
pub enum Rate {
    /// Constant quality 0..1 (the encoder spends what each frame needs).
    Quality(f64),
    /// Average bits per second.
    Bitrate(u64),
}

#[derive(Clone, Copy, Debug)]
pub struct VideoParams {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub codec: VideoCodec,
    pub rate: Rate,
    /// Frames arrive as they happen (recording): favor speed and never
    /// hold frames back to reorder them.
    pub realtime: bool,
}

/// AAC frames are 1024 samples.
const AAC_FRAME: usize = 1024;
const AUDIO_BPS: usize = 192_000;

struct AudioTrack {
    encoder: ff::encoder::Audio,
    stream: usize,
    pending: Vec<[f32; 2]>,
    /// Samples handed to the encoder so far.
    written: i64,
    frame: ff::frame::Audio,
}

pub struct Muxer {
    output: ff::format::context::Output,
    video: ff::encoder::Video,
    /// BGRA to the encoder's pixel format.
    sws: *mut ffi::SwsContext,
    picture: ff::frame::Video,
    audio: Vec<AudioTrack>,
    fps: u32,
    size: (u32, u32),
    /// The encoder in use ("h264_nvenc", "libx264", ...).
    pub encoder_name: &'static str,
    pub frames: u64,
}

// SAFETY: the libav contexts are only touched through &mut self.
unsafe impl Send for Muxer {}

impl Drop for Muxer {
    fn drop(&mut self) {
        // SAFETY: `sws` came from sws_getContext and is freed once.
        unsafe { ffi::sws_freeContext(self.sws) };
    }
}

/// x264's CRF for a 0..1 quality: 0.95 (recordings, "studio") is 14,
/// visually lossless for screen content; 0.55 ("web low") is 28.
fn crf(quality: f64, encoder: &str) -> u32 {
    let base = 47.25 - 35.0 * quality.clamp(0.0, 1.0);
    // x265's scale sits a little above x264's for the same look.
    let shift = if is_hevc(encoder) { 2.0 } else { 0.0 };
    (base + shift).round().clamp(0.0, 51.0) as u32
}

/// Encoders to try, best first. `GLIDE_ENCODER=software` skips the GPU's.
fn candidates(p: &VideoParams) -> &'static [&'static str] {
    let software = std::env::var("GLIDE_ENCODER").is_ok_and(|v| v == "software");
    match (p.codec, software, p.realtime) {
        (VideoCodec::Prores, ..) => &["prores_ks"],
        // Recording has to keep up with the screen: x265 can't at full
        // size, so without a GPU encoder it's x264 whatever was asked for.
        (_, false, true) => &["hevc_nvenc", "h264_nvenc", "libx264"],
        (_, true, true) => &["libx264"],
        (VideoCodec::H264, false, false) => &["h264_nvenc", "libx264"],
        (VideoCodec::H264, true, false) => &["libx264"],
        // H.264 when there's no HEVC encoder at all (an ffmpeg built
        // without x265, and no GPU encoder).
        (VideoCodec::Hevc, false, false) => &["hevc_nvenc", "libx265", "h264_nvenc", "libx264"],
        (VideoCodec::Hevc, true, false) => &["libx265", "libx264"],
    }
}

fn is_hevc(name: &str) -> bool {
    matches!(name, "hevc_nvenc" | "libx265")
}

fn options(name: &str, p: &VideoParams) -> ff::Dictionary<'static> {
    let mut o = ff::Dictionary::new();
    match name {
        "h264_nvenc" | "hevc_nvenc" => {
            o.set("preset", if p.realtime { "p4" } else { "p6" });
            o.set("tune", "hq");
            o.set("rc", "vbr");
            if let Rate::Quality(q) = p.rate {
                o.set("cq", &crf(q, name).to_string());
            }
            if p.realtime {
                o.set("bf", "0");
            }
        }
        "libx264" => {
            o.set("preset", if p.realtime { "veryfast" } else { "medium" });
            if let Rate::Quality(q) = p.rate {
                o.set("crf", &crf(q, name).to_string());
            }
        }
        "libx265" => {
            o.set("preset", if p.realtime { "ultrafast" } else { "medium" });
            o.set("x265-params", "log-level=error");
            if let Rate::Quality(q) = p.rate {
                o.set("crf", &crf(q, name).to_string());
            }
        }
        "prores_ks" => {
            // 4444, as the macOS build writes.
            o.set("profile", "4");
            o.set("vendor", "apl0");
        }
        _ => {}
    }
    o
}

fn pixel_format(name: &str) -> ff::format::Pixel {
    match name {
        "h264_nvenc" | "hevc_nvenc" => ff::format::Pixel::NV12,
        "prores_ks" => ff::format::Pixel::YUV444P10LE,
        _ => ff::format::Pixel::YUV420P,
    }
}

fn open_video(p: &VideoParams, global_header: bool) -> Result<(ff::encoder::Video, ff::Codec, &'static str)> {
    let mut last = anyhow!("no encoder for {:?}", p.codec);
    for &name in candidates(p) {
        let Some(codec) = ff::encoder::find_by_name(name) else { continue };
        let mut ctx = ff::codec::context::Context::new_with_codec(codec);
        if global_header {
            ctx.set_flags(ff::codec::Flags::GLOBAL_HEADER);
        }
        let mut enc = ctx.encoder().video()?;
        enc.set_width(p.width);
        enc.set_height(p.height);
        enc.set_format(pixel_format(name));
        enc.set_time_base(ff::Rational(1, p.fps as i32));
        enc.set_frame_rate(Some(ff::Rational(p.fps as i32, 1)));
        // A key frame every two seconds, as the macOS build asks for.
        enc.set_gop(p.fps * 2);
        // Frames are sRGB-encoded screen pixels: BT.709 primaries and
        // matrix with the sRGB transfer function, so players show exactly
        // the recorded colors.
        enc.set_colorspace(ff::color::Space::BT709);
        enc.set_color_range(ff::color::Range::MPEG);
        enc.set_color_primaries(ff::color::Primaries::BT709);
        enc.set_color_transfer_characteristic(ff::color::TransferCharacteristic::IEC61966_2_1);
        match p.rate {
            Rate::Bitrate(b) => {
                enc.set_bit_rate(b as usize);
                enc.set_max_bit_rate((b as f64 * 1.5) as usize);
                // SAFETY: a plain field of the context being set up.
                unsafe { (*enc.as_mut_ptr()).rc_buffer_size = (b * 2).min(i32::MAX as u64) as i32 };
            }
            Rate::Quality(_) => enc.set_bit_rate(0),
        }
        match enc.open_with(options(name, p)) {
            Ok(opened) => return Ok((opened, codec, name)),
            Err(e) => last = anyhow!("{name}: {e}"),
        }
    }
    Err(last)
}

fn open_audio(global_header: bool) -> Result<(ff::encoder::Audio, ff::Codec)> {
    let codec = ff::encoder::find(ff::codec::Id::AAC).context("no AAC encoder")?;
    let mut ctx = ff::codec::context::Context::new_with_codec(codec);
    if global_header {
        ctx.set_flags(ff::codec::Flags::GLOBAL_HEADER);
    }
    let mut enc = ctx.encoder().audio()?;
    enc.set_rate(mixer::RATE as i32);
    // Sets the channel count too: the layout carries it.
    enc.set_channel_layout(ff::ChannelLayout::STEREO);
    enc.set_format(ff::format::Sample::F32(ff::format::sample::Type::Planar));
    enc.set_bit_rate(AUDIO_BPS);
    enc.set_time_base(ff::Rational(1, mixer::RATE as i32));
    Ok((enc.open().context("opening the AAC encoder")?, codec))
}

impl Muxer {
    /// A movie at `path` (.mp4 or .mov) with one video track and
    /// `audio_tracks` stereo AAC tracks.
    pub fn create(path: &Path, p: &VideoParams, audio_tracks: usize, faststart: bool) -> Result<Self> {
        super::init();
        if p.width % 2 != 0 || p.height % 2 != 0 {
            bail!("video size {}x{} must be even", p.width, p.height);
        }
        if path.exists() {
            std::fs::remove_file(path).with_context(|| format!("replacing {}", path.display()))?;
        }
        let mut output = ff::format::output(path).with_context(|| format!("creating {}", path.display()))?;
        let global_header = output.format().flags().contains(ff::format::Flags::GLOBAL_HEADER);
        let (video, codec, encoder_name) = open_video(p, global_header)?;
        {
            let mut st = output.add_stream(codec)?;
            st.set_parameters(&video);
            st.set_time_base(ff::Rational(1, p.fps as i32));
            st.set_avg_frame_rate(ff::Rational(p.fps as i32, 1));
            if is_hevc(encoder_name) {
                // "hvc1", the tag QuickTime and Safari play.
                // SAFETY: the stream's own parameters, just set above.
                unsafe { (*(*st.as_mut_ptr()).codecpar).codec_tag = u32::from_le_bytes(*b"hvc1") };
            }
        }
        let mut audio = vec![];
        for i in 0..audio_tracks {
            let (encoder, codec) = open_audio(global_header)?;
            let mut st = output.add_stream(codec)?;
            st.set_parameters(&encoder);
            st.set_time_base(ff::Rational(1, mixer::RATE as i32));
            audio.push(AudioTrack::new(encoder, i + 1));
        }
        let mut header = ff::Dictionary::new();
        if faststart {
            header.set("movflags", "+faststart");
        }
        output.write_header_with(header).context("writing the movie header")?;

        let format = pixel_format(encoder_name);
        let mut picture = ff::frame::Video::new(format, p.width, p.height);
        picture.set_color_space(ff::color::Space::BT709);
        picture.set_color_range(ff::color::Range::MPEG);
        picture.set_color_primaries(ff::color::Primaries::BT709);
        picture.set_color_transfer_characteristic(ff::color::TransferCharacteristic::IEC61966_2_1);
        let flags = sws::BILINEAR | sws::ACCURATE_RND | sws::FULL_CHR_H_INP;
        // SAFETY: sizes are the encoder's; the context is checked.
        let sws = unsafe {
            let (w, h) = (p.width as i32, p.height as i32);
            let ctx = ffi::sws_getContext(w, h, ffi::AVPixelFormat::AV_PIX_FMT_BGRA, w, h, format.into(), flags, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            if ctx.is_null() {
                bail!("no conversion from BGRA to {format:?}");
            }
            // Full-range RGB in, limited-range BT.709 out.
            let bt709 = ffi::sws_getCoefficients(sws::CS_ITU709);
            ffi::sws_setColorspaceDetails(ctx, bt709, 1, bt709, 0, 0, 1 << 16, 1 << 16);
            ctx
        };
        Ok(Muxer { output, video, sws, picture, audio, fps: p.fps, size: (p.width, p.height), encoder_name, frames: 0 })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    fn drain_video(&mut self) -> Result<()> {
        let mut packet = ff::Packet::empty();
        let to = self.output.stream(0).map(|s| s.time_base()).context("video stream")?;
        while self.video.receive_packet(&mut packet).is_ok() {
            packet.set_stream(0);
            packet.rescale_ts(ff::Rational(1, self.fps as i32), to);
            packet.write_interleaved(&mut self.output).context("writing video")?;
        }
        Ok(())
    }


    /// Encode the next frame (frames are evenly spaced at the frame rate)
    /// from BGRA rows `stride` bytes apart.
    pub fn push_frame(&mut self, bgra: &[u8], stride: usize) -> Result<()> {
        let (w, h) = self.size;
        if bgra.len() < stride * (h as usize - 1) + w as usize * 4 {
            bail!("frame {} is too small for {w}x{h}", self.frames);
        }
        // SAFETY: the encoder may still hold the last picture's buffers, so
        // they're made this frame's own before being written; the source
        // rows were checked above.
        unsafe {
            if ffi::av_frame_make_writable(self.picture.as_mut_ptr()) < 0 {
                bail!("no memory for a video frame");
            }
            let src = [bgra.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null()];
            let strides = [stride as i32, 0, 0, 0];
            let dst = &mut *self.picture.as_mut_ptr();
            ffi::sws_scale(self.sws, src.as_ptr(), strides.as_ptr(), 0, h as i32, dst.data.as_ptr() as *const *mut u8, dst.linesize.as_ptr());
        }
        self.picture.set_pts(Some(self.frames as i64));
        self.video.send_frame(&self.picture).with_context(|| format!("encoding frame {}", self.frames))?;
        self.frames += 1;
        self.drain_video()
    }

    /// Samples of audio written to `track` so far.
    pub fn audio_written(&self, track: usize) -> usize {
        self.audio.get(track).map_or(0, |t| t.written as usize + t.pending.len())
    }

    /// Append 48 kHz stereo to an audio track (tracks start at time zero
    /// and have no gaps: pad silence where there was none).
    pub fn push_audio(&mut self, track: usize, samples: &[[f32; 2]]) -> Result<()> {
        let Some(t) = self.audio.get_mut(track) else { return Ok(()) };
        t.pending.extend_from_slice(samples);
        t.encode(&mut self.output, false)
    }

    /// Write what's left and close the file.
    pub fn finish(mut self) -> Result<()> {
        self.video.send_eof().context("finishing video")?;
        self.drain_video()?;
        for t in &mut self.audio {
            t.finish(&mut self.output)?;
        }
        self.output.write_trailer().context("finalizing the movie")?;
        Ok(())
    }
}

impl AudioTrack {
    fn new(encoder: ff::encoder::Audio, stream: usize) -> Self {
        let mut frame = ff::frame::Audio::new(encoder.format(), AAC_FRAME, ff::ChannelLayout::STEREO);
        frame.set_rate(mixer::RATE as u32);
        AudioTrack { encoder, stream, pending: vec![], written: 0, frame }
    }

    fn drain(&mut self, output: &mut ff::format::context::Output) -> Result<()> {
        let mut packet = ff::Packet::empty();
        let to = output.stream(self.stream).map(|s| s.time_base()).context("audio stream")?;
        while self.encoder.receive_packet(&mut packet).is_ok() {
            packet.set_stream(self.stream);
            packet.rescale_ts(ff::Rational(1, mixer::RATE as i32), to);
            packet.write_interleaved(output).context("writing audio")?;
        }
        Ok(())
    }

    /// Encode whole frames of what's pending (and, flushing, the rest
    /// padded with silence).
    fn encode(&mut self, output: &mut ff::format::context::Output, flush: bool) -> Result<()> {
        let mut used = 0;
        while self.pending.len() - used >= AAC_FRAME || (flush && self.pending.len() > used) {
            let chunk = &self.pending[used..(used + AAC_FRAME).min(self.pending.len())];
            // SAFETY: as with video, the encoder may hold the last frame.
            if unsafe { ffi::av_frame_make_writable(self.frame.as_mut_ptr()) } < 0 {
                bail!("no memory for an audio frame");
            }
            for ch in 0..2 {
                let plane = self.frame.plane_mut::<f32>(ch);
                for (i, slot) in plane.iter_mut().enumerate() {
                    *slot = chunk.get(i).map_or(0.0, |s| s[ch]);
                }
            }
            used += chunk.len();
            self.frame.set_pts(Some(self.written));
            self.encoder.send_frame(&self.frame).context("encoding audio")?;
            self.written += AAC_FRAME as i64;
            self.drain(output)?;
        }
        self.pending.drain(..used);
        Ok(())
    }

    fn finish(&mut self, output: &mut ff::format::context::Output) -> Result<()> {
        self.encode(output, true)?;
        self.encoder.send_eof().context("finishing audio")?;
        self.drain(output)
    }
}

/// Write 48 kHz stereo as an AAC file (.m4a).
pub fn write_aac(path: &Path, samples: &[[f32; 2]]) -> Result<()> {
    super::init();
    let mut output = ff::format::output(path).with_context(|| format!("creating {}", path.display()))?;
    let global_header = output.format().flags().contains(ff::format::Flags::GLOBAL_HEADER);
    let (encoder, codec) = open_audio(global_header)?;
    {
        let mut st = output.add_stream(codec)?;
        st.set_parameters(&encoder);
        st.set_time_base(ff::Rational(1, mixer::RATE as i32));
    }
    output.write_header().context("writing the audio header")?;
    let mut track = AudioTrack::new(encoder, 0);
    track.pending.extend_from_slice(samples);
    track.finish(&mut output)?;
    output.write_trailer().context("finalizing the audio file")?;
    Ok(())
}
