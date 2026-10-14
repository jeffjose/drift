//! Live preview of a source for the recording-setup canvas: the screen is
//! read a few times a second, scaled to the canvas and handed over as RGBA.
//!
//! Unlike macOS, X11 can't leave a window out of what it reads, so Glide's
//! own window shows in the preview when it's on the display being shown.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use ffmpeg_next::ffi;

use super::audio::{AudioIn, Input};
use super::screen;
use super::x11::{Grabber, place};
use crate::media::PixelBuffer;

/// What to preview (display ids as `glide devices` lists them; 0 = main).
#[derive(Clone, Debug, PartialEq)]
pub enum Source {
    Display { id: u32 },
    /// A region of a display in points (x, y, w, h, top-left origin).
    Area { display: u32, rect: [f64; 4] },
    Window { id: u32 },
    /// A camera (None = the default one).
    Camera { id: Option<String> },
    /// A USB-connected iPhone or iPad's screen (macOS only).
    Device { id: String },
}

/// One preview frame, RGBA.
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The pixel size to show a `fw`x`fh` frame at in a canvas of `area` pixels:
/// its aspect ratio fitted into the area, never larger than the frame.
pub fn target_size(fw: usize, fh: usize, area: (u32, u32)) -> (usize, usize) {
    if area.0 < 2 || area.1 < 2 {
        return (fw, fh);
    }
    let s = (area.0 as f64 / fw as f64).min(area.1 as f64 / fh as f64).min(1.0);
    (((fw as f64 * s).round() as usize).max(2) & !1, ((fh as f64 * s).round() as usize).max(2) & !1)
}

/// Where a source is on the X screen right now, in pixels.
pub fn source_rect(source: &Source) -> Result<(i32, i32, u32, u32)> {
    Ok(match source {
        Source::Display { id } => {
            let d = screen::display(*id)?;
            (d.origin.0, d.origin.1, d.pixels.0 as u32, d.pixels.1 as u32)
        }
        Source::Area { display, rect } => {
            let d = screen::display(*display)?;
            let px = |v: f64| (v * d.scale).round();
            (d.origin.0 + px(rect[0]) as i32, d.origin.1 + px(rect[1]) as i32, px(rect[2]).max(2.0) as u32, px(rect[3]).max(2.0) as u32)
        }
        Source::Window { id } => screen::window(*id).map(|w| w.rect).ok_or_else(|| anyhow::anyhow!("that window is no longer on screen"))?,
        Source::Camera { .. } => bail!("cameras aren't supported on Linux yet"),
        Source::Device { .. } => bail!("iPhone and iPad recording needs macOS"),
    })
}

/// Area-averaging downscale of BGRA to RGBA (or a plain channel swap at
/// the same size).
struct Scaler(*mut ffi::SwsContext);

impl Scaler {
    fn run(&mut self, src: &[u8], (sw, sh): (u32, u32), (dw, dh): (u32, u32)) -> Option<Vec<u8>> {
        let mut out = vec![0u8; dw as usize * dh as usize * 4];
        // SAFETY: `src` holds sh rows of sw*4 bytes and `out` dh rows of
        // dw*4; the context is checked before use.
        unsafe {
            self.0 = ffi::sws_getCachedContext(self.0, sw as i32, sh as i32, ffi::AVPixelFormat::AV_PIX_FMT_BGRA, dw as i32, dh as i32, ffi::AVPixelFormat::AV_PIX_FMT_RGBA, ffi::SWS_AREA as i32, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null());
            if self.0.is_null() {
                return None;
            }
            let (planes, strides) = ([src.as_ptr(), std::ptr::null(), std::ptr::null(), std::ptr::null()], [sw as i32 * 4, 0, 0, 0]);
            let (out_planes, out_strides) = ([out.as_mut_ptr(), std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null_mut()], [dw as i32 * 4, 0, 0, 0]);
            ffi::sws_scale(self.0, planes.as_ptr(), strides.as_ptr(), 0, sh as i32, out_planes.as_ptr(), out_strides.as_ptr());
        }
        Some(out)
    }
}

impl Drop for Scaler {
    fn drop(&mut self) {
        // SAFETY: freeing a context this owns (null is fine).
        unsafe { ffi::sws_freeContext(self.0) };
    }
}

/// A running preview; stops when dropped.
pub struct LivePreview {
    latest: Arc<Mutex<Option<Frame>>>,
    /// Canvas size in pixels, packed as (w << 32) | h.
    target: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    audio: Option<AudioIn>,
}

impl LivePreview {
    /// Start previewing. `on_frame` is called (on another thread) whenever a
    /// new frame is ready. Frames are scaled to the size last given to
    /// `set_target`, or to at most `max_px` on the long edge before then.
    pub fn start(source: &Source, max_px: u32, fps: u32, audio: bool, on_frame: impl Fn() + Send + Sync + 'static) -> Result<LivePreview> {
        let first = source_rect(source)?;
        let whole = super::x11::X::open()?.size;
        let mut grabber = Grabber::new(whole.0 as u32, whole.1 as u32)?;
        let latest: Arc<Mutex<Option<Frame>>> = Arc::default();
        let target = Arc::new(AtomicU64::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let (out, tgt, st, source) = (latest.clone(), target.clone(), stop.clone(), source.clone());
        let thread = std::thread::spawn(move || {
            let mut scaler = Scaler(std::ptr::null_mut());
            let mut rect = first;
            let mut looked = Instant::now();
            let every = Duration::from_secs_f64(1.0 / fps.clamp(1, 60) as f64);
            while !st.load(Ordering::Relaxed) {
                let began = Instant::now();
                // Windows move: look again a few times a second.
                if matches!(source, Source::Window { .. }) && looked.elapsed() > Duration::from_millis(250) {
                    looked = Instant::now();
                    match source_rect(&source) {
                        Ok(r) => rect = r,
                        Err(_) => break,
                    }
                }
                let (w, h) = (rect.2.max(2), rect.3.max(2));
                let packed = tgt.load(Ordering::Relaxed);
                let area = ((packed >> 32) as u32, packed as u32);
                let (tw, th) = if area.0 >= 2 && area.1 >= 2 {
                    target_size(w as usize, h as usize, area)
                } else if max_px > 0 {
                    target_size(w as usize, h as usize, (max_px, max_px))
                } else {
                    (w as usize, h as usize)
                };
                let frame = grabber.grab(rect.0, rect.1, w, h).ok().and_then(|(rows, got)| {
                    // Off the edge of the screen: fill what's missing.
                    let whole = (got.2, got.3) == (w, h);
                    let mut full = vec![];
                    if !whole {
                        full = vec![0u8; w as usize * h as usize * 4];
                        place(&mut full, w, h, rows, (got.0 - rect.0, got.1 - rect.1, got.2, got.3));
                    }
                    let rgba = scaler.run(if whole { rows } else { &full }, (w, h), (tw as u32, th as u32))?;
                    Some(Frame { width: tw as u32, height: th as u32, rgba })
                });
                if let Some(mut f) = frame {
                    for px in f.rgba.chunks_exact_mut(4) {
                        px[3] = 255;
                    }
                    *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
                    on_frame();
                }
                std::thread::sleep(every.saturating_sub(began.elapsed()));
            }
        });
        let audio = if audio { AudioIn::start(&Input::System, |_, _| {}).ok() } else { None };
        Ok(LivePreview { latest, target, stop, thread: Some(thread), audio })
    }

    /// Peak system-audio level (0..1) since the last call.
    pub fn take_level(&self) -> f32 {
        self.audio.as_ref().map_or(0.0, |a| a.take_level())
    }

    /// The canvas size in pixels to scale frames to.
    pub fn set_target(&self, w: u32, h: u32) {
        self.target.store(((w as u64) << 32) | h as u64, Ordering::Relaxed);
    }

    /// macOS hands frames to the GPU in place; here they always come
    /// through `take_frame`.
    pub fn take_buffer(&self) -> Option<PixelBuffer> {
        None
    }

    /// The newest frame, if there's one since the last call.
    pub fn take_frame(&self) -> Option<Frame> {
        self.latest.lock().unwrap_or_else(|e| e.into_inner()).take()
    }
}

impl Drop for LivePreview {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
