//! Displays and windows that can be recorded.
//!
//! "Points" are what the interface is laid out in: pixels divided by the
//! scale toolkits draw at (1 on an ordinary monitor, 2 on a HiDPI one set
//! up as such), the same scale the app's own windows get.

use std::sync::Mutex;

use anyhow::{Result, anyhow, bail};

use super::x11::{Monitor, Win, X};

#[derive(Clone, Debug)]
pub struct DisplayInfo {
    /// 1 for the primary monitor, then 2, 3… left to right.
    pub id: u32,
    pub points: (i64, i64),
    pub pixels: (usize, usize),
    /// Top-left corner on the X screen, in pixels.
    pub origin: (i32, i32),
    /// Pixels per point.
    pub scale: f64,
}

/// Scales the windowing library reported, by monitor origin: when the app
/// is running these win over what's worked out here, so the picker
/// overlays and the recorder agree to the pixel.
static KNOWN_SCALES: Mutex<Vec<((i32, i32), f64)>> = Mutex::new(vec![]);

pub fn set_ui_scales(scales: Vec<((i32, i32), f64)>) {
    *KNOWN_SCALES.lock().unwrap_or_else(|e| e.into_inner()) = scales;
}

/// The scale toolkits use on a monitor: `Xft.dpi` over 96 when it's set,
/// else from the monitor's physical size in twelfths, as winit does.
fn ui_scale(x: &X, m: &Monitor) -> f64 {
    if let Some((_, s)) = KNOWN_SCALES.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(at, _)| *at == (m.x, m.y)) {
        return *s;
    }
    if let Some(s) = std::env::var("WINIT_X11_SCALE_FACTOR").ok().and_then(|v| v.parse::<f64>().ok()).filter(|s| *s > 0.0) {
        return s;
    }
    if let Some(dpi) = x.xft_dpi().filter(|d| *d > 0.0) {
        return dpi / 96.0;
    }
    if m.mm.0 == 0 || m.mm.1 == 0 {
        return 1.0;
    }
    let ppmm = ((m.width as f64 * m.height as f64) / (m.mm.0 as f64 * m.mm.1 as f64)).sqrt();
    let scale = ((ppmm * (12.0 * 25.4 / 96.0)).round() / 12.0).max(1.0);
    if scale > 20.0 { 1.0 } else { scale }
}

fn display_infos(x: &X) -> Vec<DisplayInfo> {
    x.monitors()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let scale = ui_scale(x, m);
            DisplayInfo {
                id: i as u32 + 1,
                points: ((m.width as f64 / scale).round() as i64, (m.height as f64 / scale).round() as i64),
                pixels: (m.width as usize, m.height as usize),
                origin: (m.x, m.y),
                scale,
            }
        })
        .collect()
}

pub fn displays() -> Result<Vec<DisplayInfo>> {
    Ok(display_infos(&X::open()?))
}

/// A display by id (0 = the primary one).
pub fn display(id: u32) -> Result<DisplayInfo> {
    let ds = displays()?;
    ds.iter().find(|d| d.id == id).or(if id == 0 { ds.first() } else { None }).cloned().ok_or_else(|| anyhow!("display not found"))
}

#[derive(Clone, Debug)]
pub struct WindowInfo {
    pub id: u32,
    pub app: String,
    pub title: String,
    /// Size in points.
    pub size: (f64, f64),
    pub pid: i32,
    /// What's seen of it, title bar included, in screen pixels.
    pub rect: (i32, i32, u32, u32),
    /// The display most of it is on.
    pub display: DisplayInfo,
}

fn on_display<'a>(ds: &'a [DisplayInfo], rect: (i32, i32, u32, u32)) -> &'a DisplayInfo {
    let center = (rect.0 + rect.2 as i32 / 2, rect.1 + rect.3 as i32 / 2);
    ds.iter()
        .find(|d| center.0 >= d.origin.0 && center.1 >= d.origin.1 && center.0 < d.origin.0 + d.pixels.0 as i32 && center.1 < d.origin.1 + d.pixels.1 as i32)
        .unwrap_or(&ds[0])
}

fn info(ds: &[DisplayInfo], w: Win) -> WindowInfo {
    let display = on_display(ds, w.rect).clone();
    WindowInfo { id: w.id, app: w.app, title: w.title, size: (w.rect.2 as f64 / display.scale, w.rect.3 as f64 / display.scale), pid: w.pid, rect: w.rect, display }
}

/// On-screen windows of the current desktop, frontmost first.
pub fn windows() -> Result<Vec<WindowInfo>> {
    let x = X::open()?;
    let ds = display_infos(&x);
    Ok(x.windows().into_iter().filter(|w| !w.title.is_empty() || !w.app.is_empty()).map(|w| info(&ds, w)).collect())
}

/// One window as it is now, if it's still on screen.
pub fn window(id: u32) -> Option<WindowInfo> {
    let x = X::open().ok()?;
    let ds = display_infos(&x);
    x.window(id).map(|w| info(&ds, w))
}

/// A window by id or by part of its app name or title (the largest match).
pub fn find_window(sel: &str) -> Result<WindowInfo> {
    // An empty selector would match every window.
    if sel.trim().is_empty() {
        bail!("no window given (use an id from `glide devices` or part of its name)");
    }
    let ws = windows()?;
    if let Ok(id) = sel.parse::<u32>() {
        if let Some(w) = ws.into_iter().find(|w| w.id == id) {
            return Ok(w);
        }
        bail!("no on-screen window with id {id} (see `glide devices`)");
    }
    let lower = sel.to_lowercase();
    ws.into_iter()
        .filter(|w| format!("{} {}", w.app, w.title).to_lowercase().contains(&lower))
        .max_by(|a, b| (a.size.0 * a.size.1).total_cmp(&(b.size.0 * b.size.1)))
        .ok_or_else(|| anyhow!("no on-screen window matches {sel:?} (see `glide devices`)"))
}
