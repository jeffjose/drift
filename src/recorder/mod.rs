//! Recording on Linux: the X11 screen is read on an exact `1/fps` grid and
//! encoded as it comes, with system audio and the microphone on tracks of
//! their own and the input log beside them, into a `.glide` package.
//!
//! One thread reads the screen and another encodes, so a slow frame of
//! one doesn't hold up the other; a frame the encoder has no room for is
//! dropped and the one before it shown twice, keeping the frame rate
//! constant.

pub mod input;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::capture::audio::{AudioIn, Input};
use crate::capture::host_now;
use crate::capture::mixer::RATE;
use crate::capture::screen;
use crate::capture::x11::{Grabber, place};
use crate::engine::geom::Rect;
use crate::media::encode::{Muxer, Rate, VideoCodec, VideoParams};
use crate::project::recording::{AudioTrack, CursorImage, KeyEvent, MouseEvent, MouseKind, RecordingMeta, SourceKind, VideoTrack};
use crate::project::{self, Package};
use input::{InputLog, InputLogger};

#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Display { id: u32 },
    Window { id: u32 },
    /// A region of a display, in display-local points (top-left origin).
    Area { display: u32, rect: Rect },
    /// Generated test pattern (needs no screen).
    Pattern,
    /// A USB-connected iPhone or iPad screen (macOS only).
    Device { id: String },
}

#[derive(Clone, Debug)]
pub struct Options {
    pub target: Target,
    pub system_audio: bool,
    /// None = off; Some(None) = default microphone.
    pub mic: Option<Option<String>>,
    /// None = off; Some(None) = default camera.
    pub camera: Option<Option<String>>,
    /// The camera bubble's look for the new project (None = the default).
    pub camera_style: Option<crate::project::model::CameraStyle>,
    pub fps: u32,
    /// Log key presses.
    pub keys: bool,
    /// Folder for the new package.
    pub dir: PathBuf,
    /// Package file name stem (default: "Recording <date>").
    pub name: Option<String>,
    /// macOS only.
    pub hide_desktop_icons: bool,
    /// macOS only.
    pub hide_dock: bool,
    /// macOS only: PulseAudio's monitor hears everything the output plays.
    pub exclude_own_audio: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            target: Target::Display { id: 0 },
            system_audio: true,
            mic: None,
            camera: None,
            camera_style: None,
            fps: 60,
            keys: true,
            dir: project::projects_dir(),
            name: None,
            hide_desktop_icons: false,
            hide_dock: false,
            exclude_own_audio: true,
        }
    }
}

/// Encoder quality for screen recordings: near-transparent, so fine text
/// survives scrolling and re-encoding on export stays clean.
const RECORD_QUALITY: f64 = 0.95;
/// Audio that lands this far from where its time says is moved there.
const SNAP: i64 = 2400; // 50 ms

fn even(v: f64) -> u32 {
    ((v / 2.0).round() as u32 * 2).max(2)
}

/// When the recording started and when it was paused, on the host clock.
#[derive(Default)]
struct Clock {
    /// Host time of the first frame.
    t0: Option<f64>,
    pauses: Vec<(f64, Option<f64>)>,
}

impl Clock {
    /// Recording seconds at a host time, or None before the start or in a pause.
    fn at(&self, host: f64) -> Option<f64> {
        let t0 = self.t0?;
        let mut paused = 0.0;
        for &(a, b) in &self.pauses {
            let b = b.unwrap_or(f64::INFINITY);
            if host >= b {
                paused += b - a;
            } else if host >= a {
                return None;
            }
        }
        Some(host - t0 - paused).filter(|t| *t >= 0.0)
    }

    /// Recorded seconds so far (a pause still open counts up to its start).
    fn elapsed(&self, now: f64) -> f64 {
        let Some(t0) = self.t0 else { return 0.0 };
        let paused: f64 = self.pauses.iter().map(|&(a, b)| b.unwrap_or(now) - a).sum();
        (now - t0 - paused).max(0.0)
    }

    fn paused(&self) -> bool {
        self.pauses.last().is_some_and(|p| p.1.is_none())
    }
}

/// One audio source's samples on their way to the file.
#[derive(Default)]
struct Track {
    pending: Vec<[f32; 2]>,
    /// Samples placed on the track's timeline so far.
    total: i64,
}

struct Shared {
    stop: AtomicBool,
    clock: Mutex<Clock>,
    error: Mutex<Option<String>>,
    tracks: [Mutex<Track>; 2],
}

impl Shared {
    fn fail(&self, msg: String) {
        self.error.lock().unwrap().get_or_insert(msg);
        self.stop.store(true, Ordering::SeqCst);
    }

    /// Put a chunk heard at `host` where it belongs on a track.
    fn hear(&self, track: usize, host: f64, samples: &[[f32; 2]]) {
        let Some(t) = self.clock.lock().unwrap().at(host) else { return };
        let at = (t * RATE).round() as i64;
        let mut tr = self.tracks[track].lock().unwrap();
        if at > tr.total + SNAP {
            // A gap (the source stalled, or a pause just ended): silence.
            let gap = (at - tr.total) as usize;
            tr.pending.extend(std::iter::repeat_n([0.0, 0.0], gap));
            tr.total = at;
        } else if at < tr.total - SNAP {
            return;
        }
        tr.pending.extend_from_slice(samples);
        tr.total += samples.len() as i64;
    }
}

/// What the screen-reading thread captures.
enum Region {
    Fixed(i32, i32),
    /// Follows the window as it moves.
    Window(u32, (i32, i32)),
    Pattern,
}

pub struct Recorder {
    pub package: Package,
    shared: Arc<Shared>,
    grab: Option<JoinHandle<()>>,
    encode: Option<JoinHandle<Result<u64>>>,
    audio: Vec<AudioIn>,
    input: Option<InputLogger>,
    kind: SourceKind,
    /// Captured region in screen pixels at the start.
    origin: (i32, i32),
    /// Video pixels per point.
    scale: f64,
    size: (u32, u32),
    window: Option<u32>,
    pub started: Instant,
    opts: Options,
    app_name: Option<String>,
}

impl Recorder {
    pub fn start(opts: Options) -> Result<Self> {
        let stamp = crate::util::timestamp_name();
        std::fs::create_dir_all(&opts.dir).with_context(|| format!("creating {}", opts.dir.display()))?;
        let path = match &opts.name {
            Some(n) => opts.dir.join(format!("{n}.{}", project::EXTENSION)),
            None => project::new_package_path(&opts.dir, &stamp),
        };
        let package = Package::new(&path);

        // Resolve the target: what to capture, at what size, where it is.
        let (region, kind, origin, size, scale, window, app_name) = match &opts.target {
            Target::Pattern => (Region::Pattern, SourceKind::Display, (0, 0), (2880, 1800), 2.0, None, None),
            Target::Device { .. } => bail!("iPhone and iPad recording needs macOS"),
            Target::Display { id } => {
                let d = screen::display(*id)?;
                let px = (even(d.pixels.0 as f64), even(d.pixels.1 as f64));
                (Region::Fixed(d.origin.0, d.origin.1), SourceKind::Display, d.origin, px, d.scale, None, None)
            }
            Target::Area { display, rect } => {
                let d = screen::display(*display)?;
                // Keep the area on the display (a saved area may be from a bigger one).
                let rect = rect.intersect(&Rect::new(0.0, 0.0, d.points.0 as f64, d.points.1 as f64));
                anyhow::ensure!(rect.w >= 8.0 && rect.h >= 8.0, "the selected area is outside this display");
                let at = (d.origin.0 + (rect.x * d.scale).round() as i32, d.origin.1 + (rect.y * d.scale).round() as i32);
                (Region::Fixed(at.0, at.1), SourceKind::Area, at, (even(rect.w * d.scale), even(rect.h * d.scale)), d.scale, None, None)
            }
            Target::Window { id } => {
                let w = screen::find_window(&id.to_string())?;
                let at = (w.rect.0, w.rect.1);
                (Region::Window(*id, at), SourceKind::Window, at, (even(w.rect.2 as f64), even(w.rect.3 as f64)), w.display.scale, Some(*id), Some(w.app.clone()))
            }
        };
        let grabber = match region {
            Region::Pattern => None,
            _ => Some(Grabber::new(size.0, size.1)?),
        };
        // Only now (the target resolved) create the package folder.
        std::fs::create_dir_all(package.recording_dir())?;
        let discard = |e: anyhow::Error| {
            let _ = std::fs::remove_dir_all(&package.root);
            e
        };
        let tracks = usize::from(opts.system_audio) + usize::from(opts.mic.is_some());
        let params = VideoParams { width: size.0, height: size.1, fps: opts.fps, codec: VideoCodec::Hevc, rate: Rate::Quality(RECORD_QUALITY), realtime: true };
        let muxer = Muxer::create(&package.rec("screen.mov"), &params, tracks, false).map_err(discard)?;

        let shared = Arc::new(Shared { stop: AtomicBool::new(false), clock: Mutex::default(), error: Mutex::new(None), tracks: Default::default() });
        let mut audio = vec![];
        let mut sources = vec![];
        if opts.system_audio {
            sources.push(Input::System);
        }
        if let Some(mic) = &opts.mic {
            sources.push(Input::Mic(mic.clone()));
        }
        for (i, source) in sources.iter().enumerate() {
            let sh = shared.clone();
            match AudioIn::start(source, move |host, samples| sh.hear(i, host, samples)) {
                Ok(a) => audio.push(a),
                // The track stays, silent: a recording without sound beats none.
                Err(e) => eprintln!("glide: {e:#}"),
            }
        }
        let input = InputLogger::start(package.rec("cursors"), window, opts.keys, scale);

        let (frames_tx, frames_rx) = mpsc::sync_channel::<(u64, Vec<u8>)>(2);
        let (spare_tx, spare_rx) = mpsc::channel::<Vec<u8>>();
        let (sh, fps) = (shared.clone(), opts.fps);
        let grab = std::thread::spawn(move || grab_loop(&sh, grabber, region, size, fps, frames_tx, spare_rx));
        let sh = shared.clone();
        let encode = std::thread::spawn(move || encode_loop(&sh, muxer, tracks, fps, frames_rx, spare_tx));
        Ok(Recorder { package, shared, grab: Some(grab), encode: Some(encode), audio, input: Some(input), kind, origin, scale, size, window, started: Instant::now(), opts, app_name })
    }

    pub fn is_paused(&self) -> bool {
        self.shared.clock.lock().unwrap().paused()
    }

    pub fn pause(&mut self) {
        let mut c = self.shared.clock.lock().unwrap();
        if !c.paused() && c.t0.is_some() {
            c.pauses.push((host_now(), None));
        }
    }

    pub fn resume(&mut self) {
        if let Some(p) = self.shared.clock.lock().unwrap().pauses.last_mut()
            && p.1.is_none()
        {
            p.1 = Some(host_now());
        }
    }

    /// Recorded seconds (excluding pauses).
    pub fn elapsed(&self) -> f64 {
        self.shared.clock.lock().unwrap().elapsed(host_now())
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }

    pub fn has_frames(&self) -> bool {
        self.shared.clock.lock().unwrap().t0.is_some()
    }

    /// Stop every thread; the movie is finalized. Returns the frames written.
    fn halt(&mut self) -> Result<u64> {
        // Audio first, so the encoder's last look at the tracks sees it all.
        self.audio.clear();
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(g) = self.grab.take() {
            let _ = g.join();
        }
        match self.encode.take().map(|e| e.join()) {
            Some(Ok(r)) => r,
            Some(Err(_)) => bail!("the encoder crashed"),
            None => Ok(0),
        }
    }

    /// Discard the recording.
    pub fn cancel(mut self) {
        if let Some(i) = self.input.take() {
            i.stop();
        }
        let _ = self.halt();
        let _ = std::fs::remove_dir_all(&self.package.root);
    }

    /// Stop and write the package. Returns its path.
    pub fn stop(mut self) -> Result<PathBuf> {
        if self.is_paused() {
            self.resume();
        }
        let input = self.input.take().map(|i| i.stop()).unwrap_or_default();
        let frames = self.halt();
        let t0 = self.shared.clock.lock().unwrap().t0;
        let (Some(t0), Ok(frames)) = (t0, &frames) else {
            let _ = std::fs::remove_dir_all(&self.package.root);
            return Err(frames.err().unwrap_or_else(|| anyhow::anyhow!("no frames were captured"))).context("finalizing screen recording");
        };
        let duration_ms = *frames as f64 * 1000.0 / self.opts.fps as f64;
        self.write_package(input, t0, duration_ms)?;
        Ok(self.package.root.clone())
    }

    fn write_package(&self, input: InputLog, t0: f64, duration_ms: f64) -> Result<()> {
        let pkg = &self.package;
        let clock = self.shared.clock.lock().unwrap();
        let to_ms = |host: f64| -> Option<f64> {
            // Before the first frame is negative time, not no time.
            if host < t0 { Some((host - t0) * 1000.0) } else { clock.at(host).map(|t| t * 1000.0) }
        };
        // Window recordings: origin follows the window as it moves.
        let origin_at = |host: f64| -> (f64, f64) {
            if self.window.is_some()
                && let Some((_, b)) = input.bounds.iter().rev().find(|(h, _)| *h <= host).or(input.bounds.first())
            {
                return (b.x, b.y);
            }
            (self.origin.0 as f64, self.origin.1 as f64)
        };
        // The logger starts before the first video frame: keep the last
        // event from before the start so the cursor has a position at 0.
        let first_inside = input.mouse.iter().position(|m| to_ms(m.host).is_some_and(|t| t >= 0.0));
        let lead = first_inside.unwrap_or(input.mouse.len()).saturating_sub(1);
        let mouse: Vec<MouseEvent> = input
            .mouse
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let mut t = to_ms(m.host)?;
                if t < 0.0 && i == lead && m.kind != MouseKind::Up {
                    t = 0.0;
                }
                if t < 0.0 || t > duration_ms + 50.0 {
                    return None;
                }
                // Screen pixels are video pixels.
                let (ox, oy) = origin_at(m.host);
                Some(MouseEvent { t: t.max(0.0), x: m.x - ox, y: m.y - oy, kind: m.kind, button: m.button, cursor: m.cursor })
            })
            .collect();
        let keys: Vec<KeyEvent> = input
            .keys
            .iter()
            .filter_map(|k| {
                let t = to_ms(k.host)?;
                (t >= 0.0 && t <= duration_ms).then(|| KeyEvent { t, code: k.code, key: k.key.clone(), mods: k.mods, down: k.down })
            })
            .collect();
        let cursors: Vec<CursorImage> = input.cursors.clone();
        let meta = RecordingMeta {
            version: 1,
            kind: self.kind,
            duration_ms,
            screen: VideoTrack { file: "screen.mov".into(), width: self.size.0, height: self.size.1, fps: self.opts.fps as f64, offset_ms: 0.0 },
            scale: self.scale,
            bounds: Rect::new(self.origin.0 as f64 / self.scale, self.origin.1 as f64 / self.scale, self.size.0 as f64 / self.scale, self.size.1 as f64 / self.scale),
            camera: None,
            system_audio: self.opts.system_audio.then(|| AudioTrack { file: "screen.mov".into(), track: 0 }),
            mic: self.opts.mic.is_some().then(|| AudioTrack { file: "screen.mov".into(), track: if self.opts.system_audio { 1 } else { 0 } }),
            app_name: self.app_name.clone(),
            created_ms: now_ms(),
        };
        project::write_json(&pkg.rec("meta.json"), &meta)?;
        project::write_json(&pkg.rec("mouse.json"), &mouse)?;
        project::write_json(&pkg.rec("keys.json"), &keys)?;
        project::write_json(&pkg.rec("cursors.json"), &cursors)?;
        let rec = pkg.load_recording()?;
        let mut proj = project::new_project(&pkg.name(), &rec);
        // The user's default style preset, if any; auto zooms take its zoom level.
        if let Some(style) = project::presets::default_style() {
            project::presets::apply(&mut proj.style, &style);
            for z in &mut proj.timeline.zooms {
                z.scale = proj.style.default_zoom;
            }
        }
        pkg.save_project(&proj)?;
        Ok(())
    }
}

/// Read the screen on the frame grid and hand frames to the encoder.
fn grab_loop(shared: &Shared, mut grabber: Option<Grabber>, mut region: Region, size: (u32, u32), fps: u32, frames: mpsc::SyncSender<(u64, Vec<u8>)>, spare: mpsc::Receiver<Vec<u8>>) {
    let bytes = size.0 as usize * size.1 as usize * 4;
    let mut made = 0;
    let mut next = 0u64;
    while !shared.stop.load(Ordering::SeqCst) {
        let now = host_now();
        let (started, at) = {
            let c = shared.clock.lock().unwrap();
            (c.t0.is_some(), c.at(now))
        };
        let index = match (started, at) {
            (false, _) => 0,
            (true, Some(t)) => (t * fps as f64).floor() as u64,
            // Paused.
            (true, None) => {
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
        };
        if index < next {
            let wait = next as f64 / fps as f64 - at.unwrap_or(0.0);
            std::thread::sleep(Duration::from_secs_f64(wait.clamp(0.0005, 0.05)));
            continue;
        }
        // A buffer the encoder is done with, or a new one while few exist;
        // with none to be had the encoder is behind, and this frame is skipped.
        let mut buf = match spare.try_recv() {
            Ok(b) => b,
            Err(_) if made < 4 => {
                made += 1;
                vec![0u8; bytes]
            }
            Err(_) => {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
        };
        let grabbed = match (&mut region, &mut grabber) {
            (Region::Pattern, _) => {
                buf.copy_from_slice(&crate::synth::screen_bgra(size.0, size.1, index as f64 * 1000.0 / fps as f64));
                Ok(())
            }
            (Region::Fixed(x, y), Some(g)) => g.grab(*x, *y, size.0, size.1).map(|(rows, got)| place(&mut buf, size.0, size.1, rows, (got.0 - *x, got.1 - *y, got.2, got.3))),
            (Region::Window(id, at), Some(g)) => {
                // Where the window is now; where it was if it's gone.
                if let Some(w) = g.x.window(*id) {
                    *at = (w.rect.0, w.rect.1);
                }
                let (x, y) = *at;
                g.grab(x, y, size.0, size.1).map(|(rows, got)| place(&mut buf, size.0, size.1, rows, (got.0 - x, got.1 - y, got.2, got.3)))
            }
            (_, None) => Err(anyhow::anyhow!("no screen to read")),
        };
        if let Err(e) = grabbed {
            shared.fail(format!("{e:#}"));
            break;
        }
        if !started {
            // The first frame is time zero.
            shared.clock.lock().unwrap().t0 = Some(host_now());
        }
        match frames.try_send((index, buf)) {
            Ok(()) => next = index + 1,
            // The encoder is behind: this frame is dropped.
            Err(mpsc::TrySendError::Full(_)) => next = index + 1,
            Err(mpsc::TrySendError::Disconnected(_)) => break,
        }
    }
}

/// Encode frames as they come, repeating the last for any that were
/// dropped, with the audio heard so far beside them.
fn encode_loop(shared: &Shared, mut muxer: Muxer, tracks: usize, fps: u32, frames: mpsc::Receiver<(u64, Vec<u8>)>, spare: mpsc::Sender<Vec<u8>>) -> Result<u64> {
    let stride = muxer.size().0 as usize * 4;
    let mut last: Option<Vec<u8>> = None;
    let mut repeated = 0u64;
    let write_audio = |muxer: &mut Muxer| -> Result<()> {
        for i in 0..tracks {
            let pending = std::mem::take(&mut shared.tracks[i].lock().unwrap().pending);
            muxer.push_audio(i, &pending)?;
        }
        Ok(())
    };
    let run = (|| -> Result<()> {
        loop {
            let (index, buf) = match frames.recv_timeout(Duration::from_millis(100)) {
                Ok(f) => f,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    write_audio(&mut muxer)?;
                    continue;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            if let Some(prev) = &last {
                while muxer.frames < index {
                    muxer.push_frame(prev, stride)?;
                    repeated += 1;
                }
            }
            muxer.push_frame(&buf, stride)?;
            if let Some(prev) = last.replace(buf) {
                let _ = spare.send(prev);
            }
            write_audio(&mut muxer)?;
        }
        write_audio(&mut muxer)?;
        // Tracks run the length of the video: silence where a source
        // started late or stopped early.
        let want = (muxer.frames as f64 / fps as f64 * RATE) as usize;
        for i in 0..tracks {
            let have = muxer.audio_written(i);
            if have < want {
                muxer.push_audio(i, &vec![[0.0, 0.0]; want - have])?;
            }
        }
        Ok(())
    })();
    if let Err(e) = &run {
        shared.fail(format!("{e:#}"));
    }
    let written = muxer.frames;
    if std::env::var_os("GLIDE_PROFILE").is_some() {
        eprintln!("profile: {written} frames with {}, {repeated} of them repeats of the frame before", muxer.encoder_name);
    }
    muxer.finish()?;
    run.map(|_| written)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}
