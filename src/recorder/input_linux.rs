//! Input logging while recording: the pointer's position (polled), mouse
//! buttons and key presses (XInput2 raw events, which need no permission
//! and arrive whoever has the keyboard), the cursor's image (XFixes), and
//! the window's bounds for window recordings.
//!
//! Positions are in X screen pixels; times are `capture::host_now` seconds.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xinput::{self, ConnectionExt as _};
use x11rb::protocol::xproto::ConnectionExt as _;

use crate::capture::host_now;
use crate::capture::x11::X;
use crate::engine::geom::Rect;
use crate::project::recording::{CursorImage, MouseKind, mods};

#[derive(Clone, Copy, Debug)]
pub struct RawMouse {
    pub host: f64,
    pub x: f64,
    pub y: f64,
    pub kind: MouseKind,
    pub button: u8,
    pub cursor: u32,
}

#[derive(Clone, Debug)]
pub struct RawKey {
    pub host: f64,
    pub code: u16,
    pub key: String,
    pub mods: u8,
    pub down: bool,
}

#[derive(Default)]
pub struct InputLog {
    pub mouse: Vec<RawMouse>,
    pub keys: Vec<RawKey>,
    pub cursors: Vec<CursorImage>,
    /// Window bounds over time (host seconds, screen pixels).
    pub bounds: Vec<(f64, Rect)>,
    /// Whether key logging was available.
    pub keys_enabled: bool,
}

pub struct InputLogger {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<InputLog>>,
}

/// What's seen of a window, in screen pixels, if it still exists.
pub fn window_bounds(x: &X, id: u32) -> Option<Rect> {
    x.window(id).map(|w| Rect::new(w.rect.0 as f64, w.rect.1 as f64, w.rect.2 as f64, w.rect.3 as f64))
}

/// The names themes give the cursors Glide draws itself.
fn system_cursor(name: &str) -> Option<&'static str> {
    match name {
        "left_ptr" | "default" | "arrow" | "top_left_arrow" => Some("arrow"),
        "xterm" | "text" | "ibeam" => Some("ibeam"),
        "hand1" | "hand2" | "pointer" | "pointing_hand" => Some("pointing-hand"),
        _ => None,
    }
}

struct CursorTracker {
    dir: PathBuf,
    scale: f64,
    /// Cursor serial to index in `images`.
    seen: HashMap<u32, u32>,
    images: Vec<CursorImage>,
    current: u32,
}

impl CursorTracker {
    fn sample(&mut self, x: &X) {
        let Some(c) = x.conn.xfixes_get_cursor_image_and_name().ok().and_then(|c| c.reply().ok()) else { return };
        if let Some(&id) = self.seen.get(&c.cursor_serial) {
            self.current = id;
            return;
        }
        let (w, h) = (c.width as u32, c.height as u32);
        if w == 0 || h == 0 || c.cursor_image.len() < (w * h) as usize {
            return;
        }
        let id = self.images.len() as u32;
        // ARGB with the colour already multiplied by alpha: undo that.
        let mut rgba = Vec::with_capacity((w * h * 4) as usize);
        for px in &c.cursor_image[..(w * h) as usize] {
            let [b, g, r, a] = px.to_le_bytes();
            let un = |v: u8| if a == 0 { 0 } else { ((v as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8 };
            rgba.extend_from_slice(&[un(r), un(g), un(b), a]);
        }
        let file = format!("cursor-{id}.png");
        let saved = image::save_buffer(self.dir.join(&file), &rgba, w, h, image::ExtendedColorType::Rgba8).is_ok();
        let name = String::from_utf8_lossy(&c.name).into_owned();
        self.images.push(CursorImage {
            id,
            file: saved.then_some(file),
            hotspot: (c.xhot as f64 / w as f64, c.yhot as f64 / h as f64),
            size: (w as f64 / self.scale, h as f64 / self.scale),
            system: system_cursor(&name).map(str::to_string),
        });
        self.seen.insert(c.cursor_serial, id);
        self.current = id;
    }
}

/// Keycodes to keysyms, as the server maps them.
struct Keymap {
    first: u8,
    per: usize,
    syms: Vec<u32>,
}

impl Keymap {
    fn load(x: &X) -> Option<Keymap> {
        let setup = x.conn.setup();
        let (first, last) = (setup.min_keycode, setup.max_keycode);
        let r = x.conn.get_keyboard_mapping(first, last - first + 1).ok()?.reply().ok()?;
        Some(Keymap { first, per: r.keysyms_per_keycode as usize, syms: r.keysyms })
    }

    fn sym(&self, code: u32, shifted: bool) -> u32 {
        let Some(i) = code.checked_sub(self.first as u32).map(|i| i as usize * self.per) else { return 0 };
        let plain = self.syms.get(i).copied().unwrap_or(0);
        let shift = if self.per > 1 { self.syms.get(i + 1).copied().unwrap_or(0) } else { 0 };
        if shifted && shift != 0 { shift } else { plain }
    }
}

/// The modifier a keysym is, if it is one.
fn modifier(sym: u32) -> Option<u8> {
    match sym {
        0xffe1 | 0xffe2 => Some(mods::SHIFT),
        0xffe3 | 0xffe4 => Some(mods::CTRL),
        0xffe7..=0xffea => Some(mods::OPT),
        0xffeb | 0xffec => Some(mods::CMD),
        // Caps Lock, Num Lock, level shifts and the like: not shown.
        0xffe5 | 0xffe6 | 0xffed | 0xffee | 0xff7f | 0xfe00..=0xfe0f => Some(0),
        _ => None,
    }
}

/// Label for a key (specials as glyphs, else the typed character), as the
/// macOS build writes them.
fn key_label(sym: u32) -> String {
    let special = match sym {
        0xff0d | 0xff8d => "⏎",
        0xff09 | 0xfe20 => "⇥",
        0x20 => "Space",
        0xff08 => "⌫",
        0xff1b => "⎋",
        0xffff => "⌦",
        0xff51 => "←",
        0xff53 => "→",
        0xff54 => "↓",
        0xff52 => "↑",
        0xff50 => "Home",
        0xff57 => "End",
        0xff55 => "⇞",
        0xff56 => "⇟",
        _ => "",
    };
    if !special.is_empty() {
        return special.into();
    }
    if (0xffbe..=0xffc9).contains(&sym) {
        return format!("F{}", sym - 0xffbe + 1);
    }
    // Keypad digits and operators sit 0xff80 above their ASCII.
    let sym = if (0xffaa..=0xffb9).contains(&sym) { sym - 0xff80 } else { sym };
    let ch = match sym {
        0x21..=0xff => char::from_u32(sym),
        0x0100_0000..=0x0110_ffff => char::from_u32(sym - 0x0100_0000),
        _ => None,
    };
    ch.filter(|c| !c.is_control()).map(|c| c.to_uppercase().collect()).unwrap_or_default()
}

impl InputLogger {
    /// Start logging. `window` = window id to track for window recordings;
    /// `scale` = pixels per point where the recording happens.
    pub fn start(cursor_dir: PathBuf, window: Option<u32>, log_keys: bool, scale: f64) -> Self {
        let _ = std::fs::create_dir_all(&cursor_dir);
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let thread = std::thread::spawn(move || run(cursor_dir, window, log_keys, scale, &st).unwrap_or_default());
        InputLogger { stop, thread: Some(thread) }
    }

    pub fn stop(mut self) -> InputLog {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take().and_then(|t| t.join().ok()).unwrap_or_default()
    }
}

fn run(cursor_dir: PathBuf, window: Option<u32>, log_keys: bool, scale: f64, stop: &AtomicBool) -> Option<InputLog> {
    let x = X::open().ok()?;
    let mut log = InputLog::default();
    let mut wanted = xinput::XIEventMask::RAW_BUTTON_PRESS | xinput::XIEventMask::RAW_BUTTON_RELEASE;
    if log_keys {
        wanted |= xinput::XIEventMask::RAW_KEY_PRESS | xinput::XIEventMask::RAW_KEY_RELEASE;
    }
    // Device 1 is every master device (XIAllMasterDevices).
    let raw = x.conn.xinput_xi_query_version(2, 0).ok().and_then(|c| c.reply().ok()).is_some()
        && x.conn.xinput_xi_select_events(x.root, &[xinput::EventMask { deviceid: 1, mask: vec![wanted] }]).ok().and_then(|c| c.check().ok()).is_some();
    log.keys_enabled = raw && log_keys;
    let keymap = Keymap::load(&x);
    let mut cursors = CursorTracker { dir: cursor_dir, scale, seen: HashMap::new(), images: vec![], current: 0 };
    if x.conn.xfixes_query_version(4, 0).ok().and_then(|c| c.reply().ok()).is_some() {
        let _ = x.conn.xfixes_select_cursor_input(x.root, xfixes::CursorNotifyMask::DISPLAY_CURSOR);
    }
    cursors.sample(&x);
    let _ = x.conn.flush();

    let (mut last, mut held, mut mod_state) = ((i32::MIN, i32::MIN), 0u32, 0u8);
    if let Some((_, _, mask)) = x.pointer() {
        // Shift, Control, Mod1 (Alt) and Mod4 (Super) in the core mask.
        for (bit, m) in [(1u16, mods::SHIFT), (4, mods::CTRL), (8, mods::OPT), (64, mods::CMD)] {
            if mask & bit != 0 {
                mod_state |= m;
            }
        }
    }
    let mut bounds_at = Instant::now() - Duration::from_secs(1);
    while !stop.load(Ordering::SeqCst) {
        let now = host_now();
        let Some((px, py, _)) = x.pointer() else { break };
        while let Ok(Some(event)) = x.conn.poll_for_event() {
            match event {
                Event::XfixesCursorNotify(_) => cursors.sample(&x),
                Event::XinputRawButtonPress(e) | Event::XinputRawButtonRelease(e) => {
                    // 1 left, 2 middle, 3 right; 4 to 7 are the scroll wheel.
                    let button = match e.detail {
                        1 => 0,
                        3 => 1,
                        2 | 8.. => 2,
                        _ => continue,
                    };
                    let down = e.event_type == xinput::RAW_BUTTON_PRESS_EVENT;
                    held = if down { held | 1 << button } else { held & !(1 << button) };
                    let kind = if down { MouseKind::Down } else { MouseKind::Up };
                    log.mouse.push(RawMouse { host: now, x: px as f64, y: py as f64, kind, button, cursor: cursors.current });
                }
                Event::XinputRawKeyPress(e) | Event::XinputRawKeyRelease(e) => {
                    let Some(map) = &keymap else { continue };
                    let down = e.event_type == xinput::RAW_KEY_PRESS_EVENT;
                    if let Some(m) = modifier(map.sym(e.detail, false)) {
                        mod_state = if down { mod_state | m } else { mod_state & !m };
                        continue;
                    }
                    let key = key_label(map.sym(e.detail, mod_state & mods::SHIFT != 0));
                    log.keys.push(RawKey { host: now, code: e.detail as u16, key, mods: mod_state, down });
                }
                _ => {}
            }
        }
        if (px, py) != last {
            last = (px, py);
            let kind = if held & 1 != 0 { MouseKind::Drag } else { MouseKind::Move };
            log.mouse.push(RawMouse { host: now, x: px as f64, y: py as f64, kind, button: 0, cursor: cursors.current });
        }
        if let Some(id) = window
            && bounds_at.elapsed() >= Duration::from_millis(16)
        {
            bounds_at = Instant::now();
            if let Some(b) = window_bounds(&x, id)
                && log.bounds.last().is_none_or(|(_, last)| *last != b)
            {
                log.bounds.push((now, b));
            }
        }
        std::thread::sleep(Duration::from_millis(4));
    }
    log.cursors = cursors.images;
    Some(log)
}
