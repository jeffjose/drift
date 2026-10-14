//! The floating recording controls (clock, pause, stop, restart, cancel).
//!
//! macOS shows them as a pill-shaped panel that is left out of the
//! recording. X11 can't leave a window out of what's read from the
//! screen, so the pill (see `pill`) is only put up where it can't be seen
//! by the recording: beside a recorded area or window, or on another
//! display. Recording a whole single display, the controls are the tray
//! icon and the global shortcuts.

use super::pill::{Pill, Place};

/// Where the bar is on screen (pixels from the top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Origin {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum State {
    Countdown(u64),
    Recording { clock: String, paused: bool },
}

pub struct RecordBar {
    pill: Option<Pill>,
    origin: Origin,
    last: Option<State>,
}

impl RecordBar {
    /// `avoid` = the screen rectangle being recorded, in pixels.
    pub fn new(_display: u32, origin: Option<Origin>, stop_hint: &str, pause_hint: &str, avoid: Option<(i32, i32, u32, u32)>) -> RecordBar {
        let place = Place { avoid, last: origin.map(|o| (o.x as i32, o.y as i32)) };
        let pill = Pill::open(place, stop_hint, pause_hint);
        let origin = pill.as_ref().map_or(origin.unwrap_or(Origin { x: 0.0, y: 0.0 }), |p| {
            let (x, y) = p.origin();
            Origin { x: x as f64, y: y as f64 }
        });
        RecordBar { pill, origin, last: None }
    }

    pub fn update(&mut self, state: State) {
        if self.last.as_ref() == Some(&state) {
            return;
        }
        if let Some(p) = &self.pill {
            p.show(&state);
        }
        self.last = Some(state);
    }

    pub fn origin(&self) -> Origin {
        match &self.pill {
            Some(p) => {
                let (x, y) = p.origin();
                Origin { x: x as f64, y: y as f64 }
            }
            None => self.origin,
        }
    }

    pub fn window_number(&self) -> isize {
        self.pill.as_ref().map_or(0, |p| p.window() as isize)
    }

    pub fn draggable(&self) -> bool {
        self.pill.is_some()
    }
}
