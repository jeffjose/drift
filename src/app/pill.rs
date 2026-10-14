//! The recording controls as a small X11 window of their own: a dark
//! capsule with the clock and pause, stop, restart and cancel buttons,
//! drawn here pixel by pixel and shaped to its rounded outline. It runs on
//! its own thread and can be dragged by its body.

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use ab_glyph::{Font, FontRef, ScaleFont};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

use super::recbar::State;
use super::{Cmd, push};
use crate::capture::x11::X;

/// Where the pill may go.
pub struct Place {
    /// The screen rectangle being recorded (pixels), which it must stay out of.
    pub avoid: Option<(i32, i32, u32, u32)>,
    /// Where the user last left it.
    pub last: Option<(i32, i32)>,
}

/// Size at scale 1.
const W: f32 = 212.0;
const H: f32 = 44.0;
/// Left edge of the first button, and the width of each.
const BUTTONS: f32 = 92.0;
const PITCH: f32 = 28.0;

const BODY: [u8; 3] = [0x1c, 0x1c, 0x1e];
const INK: [u8; 3] = [0xf2, 0xf2, 0xf2];
const DIM: [u8; 3] = [0x8e, 0x8e, 0x93];
const RED: [u8; 3] = [0xff, 0x45, 0x3a];

pub struct Pill {
    stop: Arc<AtomicBool>,
    state: Arc<Mutex<Option<State>>>,
    at: Arc<(AtomicI32, AtomicI32)>,
    window: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
}

fn overlaps(a: (i32, i32, u32, u32), b: (i32, i32, u32, u32)) -> bool {
    a.0 < b.0 + b.2 as i32 && b.0 < a.0 + a.2 as i32 && a.1 < b.1 + b.3 as i32 && b.1 < a.1 + a.3 as i32
}

/// A spot for a `w` x `h` pill that's on a monitor and off the recording.
fn spot(x: &X, place: &Place, (w, h): (u32, u32)) -> Option<(i32, i32)> {
    let monitors = x.monitors();
    let fits = |at: (i32, i32)| {
        let r = (at.0, at.1, w, h);
        monitors.iter().any(|m| at.0 >= m.x && at.1 >= m.y && at.0 + w as i32 <= m.x + m.width as i32 && at.1 + h as i32 <= m.y + m.height as i32)
            && place.avoid.is_none_or(|a| !overlaps(r, a))
    };
    let (wi, hi, gap) = (w as i32, h as i32, 12);
    let mut tries: Vec<(i32, i32)> = place.last.into_iter().collect();
    if let Some((ax, ay, aw, ah)) = place.avoid {
        let (aw, ah) = (aw as i32, ah as i32);
        let cx = ax + (aw - wi) / 2;
        // Under it, over it, to its right, to its left.
        tries.extend([(cx, ay + ah + gap), (cx, ay - hi - gap), (ax + aw + gap, ay + ah - hi), (ax - wi - gap, ay + ah - hi)]);
    }
    // Bottom centre of each monitor, like a dock.
    tries.extend(monitors.iter().map(|m| (m.x + (m.width as i32 - wi) / 2, m.y + m.height as i32 - hi - 48)));
    tries.into_iter().find(|t| fits(*t))
}

/// Coverage (0..1) of a pixel centre by a shape with signed distance `d`.
fn cover(d: f32) -> f32 {
    (0.5 - d).clamp(0.0, 1.0)
}

fn rounded_box(px: f32, py: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let (qx, qy) = ((px - cx).abs() - hw + r, (py - cy).abs() - hh + r);
    qx.max(qy).min(0.0) + (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() - r
}

fn segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32), half: f32) -> f32 {
    let (pa, ba) = ((px - a.0, py - a.1), (b.0 - a.0, b.1 - a.1));
    let t = ((pa.0 * ba.0 + pa.1 * ba.1) / (ba.0 * ba.0 + ba.1 * ba.1)).clamp(0.0, 1.0);
    ((pa.0 - ba.0 * t).powi(2) + (pa.1 - ba.1 * t).powi(2)).sqrt() - half
}

struct Canvas {
    w: usize,
    h: usize,
    /// BGRX.
    px: Vec<u8>,
}

impl Canvas {
    fn blend(&mut self, x: usize, y: usize, color: [u8; 3], a: f32) {
        if a <= 0.0 || x >= self.w || y >= self.h {
            return;
        }
        let p = &mut self.px[(y * self.w + x) * 4..][..3];
        for (dst, src) in p.iter_mut().zip([color[2], color[1], color[0]]) {
            *dst = (*dst as f32 + (src as f32 - *dst as f32) * a.min(1.0)).round() as u8;
        }
    }

    /// Fill where `sdf` (in scale-1 units, scaled by `s`) is inside.
    fn fill(&mut self, s: f32, color: [u8; 3], alpha: f32, sdf: impl Fn(f32, f32) -> f32) {
        for y in 0..self.h {
            for x in 0..self.w {
                let d = sdf((x as f32 + 0.5) / s, (y as f32 + 0.5) / s) * s;
                self.blend(x, y, color, cover(d) * alpha);
            }
        }
    }

    fn text(&mut self, font: &FontRef, text: &str, size: f32, left: f32, baseline: f32, color: [u8; 3]) {
        let scaled = font.as_scaled(size);
        let mut pen = left;
        for ch in text.chars() {
            let id = font.glyph_id(ch);
            // Digits take the width of a zero so the clock doesn't jitter.
            let advance = if ch.is_ascii_digit() { scaled.h_advance(font.glyph_id('0')) } else { scaled.h_advance(id) };
            let inset = (advance - scaled.h_advance(id)) / 2.0;
            if let Some(outline) = font.outline_glyph(id.with_scale_and_position(size, ab_glyph::point(pen + inset, baseline))) {
                let b = outline.px_bounds();
                outline.draw(|x, y, c| self.blend((b.min.x as i32 + x as i32).max(0) as usize, (b.min.y as i32 + y as i32).max(0) as usize, color, c));
            }
            pen += advance;
        }
    }
}

/// Which button a point (scale-1 units) is on: 0 pause, 1 stop, 2 restart, 3 cancel.
fn button_at(x: f32, y: f32) -> Option<usize> {
    let i = ((x - BUTTONS) / PITCH).floor();
    (x >= BUTTONS && (0.0..4.0).contains(&i) && (6.0..H - 6.0).contains(&y)).then_some(i as usize)
}

fn draw(c: &mut Canvas, s: f32, font: &FontRef, state: &State, hover: Option<usize>) {
    for p in c.px.chunks_exact_mut(4) {
        p.copy_from_slice(&[BODY[2], BODY[1], BODY[0], 255]);
    }
    let (label, paused, counting) = match state {
        State::Countdown(n) => (n.to_string(), false, true),
        State::Recording { clock, paused } => (clock.clone(), *paused, false),
    };
    let mid = H / 2.0;
    c.fill(s, if paused || counting { DIM } else { RED }, 1.0, |x, y| ((x - 18.0).powi(2) + (y - mid).powi(2)).sqrt() - 4.5);
    c.text(font, &label, 14.5 * s, 30.0 * s, (mid + 5.2) * s, INK);
    for i in 0..4 {
        let cx = BUTTONS + PITCH * (i as f32 + 0.5);
        let live = !counting || i == 3;
        let ink = if live { INK } else { [0x55, 0x55, 0x58] };
        if hover == Some(i) && live {
            c.fill(s, [0xff, 0xff, 0xff], 0.12, |x, y| rounded_box(x, y, cx, mid, 12.0, 12.0, 6.0));
        }
        match i {
            // Pause, or play while paused.
            0 if paused => c.fill(s, ink, 1.0, |x, y| {
                // A triangle pointing right: inside all three edges.
                let (x, y) = (x - cx, y - mid);
                (-x - 4.0).max(0.5 * x + 0.866 * y - 3.0).max(0.5 * x - 0.866 * y - 3.0)
            }),
            0 => c.fill(s, ink, 1.0, |x, y| rounded_box(x, y, cx - 3.5, mid, 1.6, 6.0, 0.8).min(rounded_box(x, y, cx + 3.5, mid, 1.6, 6.0, 0.8))),
            1 => c.fill(s, if live { RED } else { ink }, 1.0, |x, y| rounded_box(x, y, cx, mid, 6.0, 6.0, 2.0)),
            // Restart: most of a ring with an arrow head where it opens.
            2 => c.fill(s, ink, 1.0, |x, y| {
                let (dx, dy) = (x - cx, y - mid);
                let ring = ((dx * dx + dy * dy).sqrt() - 5.5).abs() - 0.8;
                // Open the ring's top-right, where the head sits.
                let open = if dx > 0.5 && dy < -1.0 { 1.0 } else { ring };
                let head = segment(x, y, (cx + 1.0, mid - 5.5), (cx + 5.6, mid - 7.6), 0.8).min(segment(x, y, (cx + 1.0, mid - 5.5), (cx + 5.0, mid - 2.6), 0.8));
                open.min(head)
            }),
            _ => c.fill(s, ink, 1.0, |x, y| segment(x, y, (cx - 4.5, mid - 4.5), (cx + 4.5, mid + 4.5), 0.85).min(segment(x, y, (cx - 4.5, mid + 4.5), (cx + 4.5, mid - 4.5), 0.85))),
        }
    }
}

impl Pill {
    /// Put the pill up, or None when there's nowhere it wouldn't be recorded.
    pub fn open(place: Place, _stop_hint: &str, _pause_hint: &str) -> Option<Pill> {
        let x = X::open().ok()?;
        let s = (x.xft_dpi().unwrap_or(96.0) / 96.0).max(1.0) as f32;
        let (w, h) = ((W * s).round() as u32, (H * s).round() as u32);
        let at = spot(&x, &place, (w, h))?;
        let stop = Arc::new(AtomicBool::new(false));
        let state: Arc<Mutex<Option<State>>> = Arc::default();
        let origin = Arc::new((AtomicI32::new(at.0), AtomicI32::new(at.1)));
        let window = Arc::new(AtomicU32::new(0));
        let (st, sh, og, wn) = (stop.clone(), state.clone(), origin.clone(), window.clone());
        let avoid = place.avoid;
        let thread = std::thread::spawn(move || {
            if let Err(e) = run(x, s, (w, h), at, avoid, &st, &sh, &og, &wn) {
                eprintln!("glide: recording controls: {e:#}");
            }
        });
        Some(Pill { stop, state, at: origin, window, thread: Some(thread) })
    }

    pub fn show(&self, state: &State) {
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) = Some(state.clone());
    }

    pub fn origin(&self) -> (i32, i32) {
        (self.at.0.load(Ordering::Relaxed), self.at.1.load(Ordering::Relaxed))
    }

    pub fn window(&self) -> u32 {
        self.window.load(Ordering::Relaxed)
    }
}

impl Drop for Pill {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run(x: X, s: f32, (w, h): (u32, u32), mut at: (i32, i32), avoid: Option<(i32, i32, u32, u32)>, stop: &AtomicBool, shared: &Mutex<Option<State>>, origin: &(AtomicI32, AtomicI32), window: &AtomicU32) -> anyhow::Result<()> {
    let font = FontRef::try_from_slice(super::theme::INTER_MEDIUM)?;
    let conn = &x.conn;
    let win = conn.generate_id()?;
    let events = EventMask::EXPOSURE | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE | EventMask::POINTER_MOTION | EventMask::LEAVE_WINDOW;
    // Unmanaged (no frame, no taskbar entry) and above everything.
    let aux = CreateWindowAux::new().override_redirect(1).event_mask(events).background_pixel(0x1c1c1e);
    conn.create_window(x11rb::COPY_FROM_PARENT as u8, win, x.root, at.0 as i16, at.1 as i16, w as u16, h as u16, 0, WindowClass::INPUT_OUTPUT, x11rb::COPY_FROM_PARENT, &aux)?;
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"Glide recording controls")?;
    // Shape the window to the capsule: one rectangle per row.
    let r = h as f32 / 2.0;
    let rows: Vec<Rectangle> = (0..h)
        .map(|y| {
            let dy = (y as f32 + 0.5 - r).abs();
            let inset = (r - (r * r - dy * dy).max(0.0).sqrt()).round() as i16;
            Rectangle { x: inset, y: y as i16, width: (w as i16 - 2 * inset).max(0) as u16, height: 1 }
        })
        .collect();
    if conn.shape_query_version().ok().and_then(|c| c.reply().ok()).is_some() {
        conn.shape_rectangles(shape::SO::SET, shape::SK::BOUNDING, ClipOrdering::Y_SORTED, win, 0, 0, &rows)?;
    }
    conn.map_window(win)?;
    let gc = conn.generate_id()?;
    conn.create_gc(gc, win, &CreateGCAux::new())?;
    conn.flush()?;
    window.store(win, Ordering::Relaxed);

    let mut canvas = Canvas { w: w as usize, h: h as usize, px: vec![0; (w * h * 4) as usize] };
    let mut shown: Option<(State, Option<usize>)> = None;
    let mut hover: Option<usize> = None;
    // A press on the body: where in the pill it was, and whether it moved.
    let mut drag: Option<((i32, i32), bool)> = None;
    let mut dirty = true;
    while !stop.load(Ordering::SeqCst) {
        while let Some(event) = conn.poll_for_event()? {
            match event {
                Event::Expose(_) => dirty = true,
                Event::MotionNotify(e) => match &mut drag {
                    Some((grip, moved)) => {
                        let to = (e.root_x as i32 - grip.0, e.root_y as i32 - grip.1);
                        // Never over what's being recorded.
                        if to != at && avoid.is_none_or(|a| !overlaps((to.0, to.1, w, h), a)) {
                            at = to;
                            *moved = true;
                            conn.configure_window(win, &ConfigureWindowAux::new().x(at.0).y(at.1))?;
                            origin.0.store(at.0, Ordering::Relaxed);
                            origin.1.store(at.1, Ordering::Relaxed);
                        }
                    }
                    None => hover = button_at(e.event_x as f32 / s, e.event_y as f32 / s),
                },
                Event::LeaveNotify(_) => hover = None,
                Event::ButtonPress(e) if e.detail == 1 => {
                    if button_at(e.event_x as f32 / s, e.event_y as f32 / s).is_none() {
                        drag = Some(((e.event_x as i32, e.event_y as i32), false));
                    }
                }
                Event::ButtonRelease(e) if e.detail == 1 => {
                    if drag.take().is_none()
                        && let Some(i) = button_at(e.event_x as f32 / s, e.event_y as f32 / s)
                    {
                        let counting = matches!(shown, Some((State::Countdown(_), _)));
                        match i {
                            0 if !counting => push(Cmd::TogglePause),
                            1 if !counting => push(Cmd::ToggleRecording),
                            2 if !counting => push(Cmd::Restart),
                            3 => push(Cmd::Cancel),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        let state = shared.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if let Some(state) = state
            && (dirty || shown.as_ref().is_none_or(|(st, hv)| *st != state || *hv != hover))
        {
            draw(&mut canvas, s, &font, &state, hover);
            conn.put_image(ImageFormat::Z_PIXMAP, win, gc, w as u16, h as u16, 0, 0, 0, x.depth, &canvas.px)?;
            // Stay above windows opened since.
            conn.configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE))?;
            shown = Some((state, hover));
            dirty = false;
        }
        conn.flush()?;
        std::thread::sleep(Duration::from_millis(16));
    }
    conn.destroy_window(win)?;
    conn.flush()?;
    Ok(())
}
