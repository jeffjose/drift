//! Sizing and raising other apps' windows, by asking the window manager
//! (EWMH). Sizes are of what's seen of the window, title bar included.

use anyhow::{Context, Result, anyhow};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::*;

use super::screen;
use super::x11::X;

/// Common recording sizes (points), grouped like a size menu.
pub const PRESETS: &[(&str, &[(u32, u32)])] = &[
    ("16:9", &[(1280, 720), (1920, 1080), (2560, 1440)]),
    ("16:10", &[(1280, 800), (1440, 900), (1680, 1050), (1920, 1200)]),
    ("4:3", &[(800, 600), (1024, 768), (1280, 960), (1600, 1200)]),
    ("9:16", &[(720, 1280), (900, 1600), (1080, 1920)]),
    ("Square", &[(800, 800), (1080, 1080), (1280, 1280)]),
];

/// macOS needs Accessibility permission to move other apps' windows; an
/// X11 window manager takes requests from anyone.
pub fn trusted(_prompt: bool) -> bool {
    true
}

/// The frontmost window of a process.
pub fn window_of_pid(pid: i32) -> Option<u32> {
    X::open().ok()?.windows().into_iter().find(|w| w.pid == pid).map(|w| w.id)
}

/// A window's size in points.
pub fn window_size(id: u32) -> Result<(f64, f64)> {
    screen::window(id).map(|w| w.size).ok_or_else(|| anyhow!("window {id} is not on screen"))
}

/// For each window, the size in points of the display it's on.
pub fn display_sizes(windows: &[screen::WindowInfo]) -> Vec<(f64, f64)> {
    windows.iter().map(|w| (w.display.points.0 as f64, w.display.points.1 as f64)).collect()
}

/// Window ids, frontmost first.
pub fn front_to_back() -> Vec<u32> {
    X::open().map(|x| x.windows().into_iter().map(|w| w.id).collect()).unwrap_or_default()
}

/// Bring a window to the front and give it the keyboard.
pub fn raise(id: u32) -> Result<()> {
    let x = X::open()?;
    // Source 2: a pager, which window managers let take focus.
    x.ask_wm(id, x.atoms._NET_ACTIVE_WINDOW, [2, x11rb::CURRENT_TIME, 0, 0, 0])
}

/// Resize a window to `width` x `height` points and return the size it
/// ended up with (apps and window managers have limits of their own).
pub fn resize(id: u32, width: f64, height: f64) -> Result<(f64, f64)> {
    let x = X::open()?;
    let w = x.window(id).ok_or_else(|| anyhow!("window {id} is not on screen"))?;
    let scale = screen::window(id).map_or(1.0, |i| i.display.scale);
    // What's asked for is the visible window; the client window is that
    // less the frame, plus any invisible margin of its own.
    let cw = (width * scale).round() as i32 - w.frame[0] - w.frame[1] + w.margin[0] + w.margin[1];
    let ch = (height * scale).round() as i32 - w.frame[2] - w.frame[3] + w.margin[2] + w.margin[3];
    x.conn.configure_window(id, &ConfigureWindowAux::new().width(cw.max(1) as u32).height(ch.max(1) as u32))?.check().context("resizing the window")?;
    x.conn.flush()?;
    // Give the window manager and the app a moment to settle.
    let mut got = (0.0, 0.0);
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(25));
        let now = window_size(id)?;
        if now == got && now != (0.0, 0.0) {
            break;
        }
        got = now;
    }
    Ok(got)
}
