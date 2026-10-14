//! Global hotkeys on X11: passive key grabs on the root window, heard on a
//! thread of their own. Several shortcuts can be bound, each re-bindable
//! at runtime.
//!
//! Shortcuts are stored as on macOS (its key codes and modifier bits), so
//! settings and defaults are shared; here ⌘ means Ctrl, ⌥ Alt and ⌃ Super.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

pub const KEY_R: u32 = 15;
pub const CMD: u32 = 1 << 8;
pub const SHIFT: u32 = 1 << 9;
pub const OPTION: u32 = 1 << 11;
pub const CONTROL: u32 = 1 << 12;

/// A key plus modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Combo {
    pub key: u32,
    pub mods: u32,
}

impl Combo {
    pub const fn new(key: u32, mods: u32) -> Self {
        Combo { key, mods }
    }

    /// "Ctrl+Alt+Shift+R" style label.
    pub fn label(&self) -> String {
        let mut s = String::new();
        for (bit, name) in [(CMD, "Ctrl+"), (CONTROL, "Super+"), (OPTION, "Alt+"), (SHIFT, "Shift+")] {
            if self.mods & bit != 0 {
                s.push_str(name);
            }
        }
        s.push_str(&key_name(self.key));
        s
    }

    /// From an egui key press; None for keys we can't map or combos
    /// without Ctrl or Alt (plain keys would hijack typing).
    pub fn from_egui(key: egui::Key, m: egui::Modifiers) -> Option<Combo> {
        let code = egui_to_vk(key)?;
        let mut mods = 0;
        if m.command || m.ctrl {
            mods |= CMD;
        }
        if m.shift {
            mods |= SHIFT;
        }
        if m.alt {
            mods |= OPTION;
        }
        let is_fkey = K_F_KEYS.iter().any(|(k, _)| *k == code);
        (mods & (CMD | CONTROL | OPTION) != 0 || is_fkey).then_some(Combo { key: code, mods })
    }

    /// The X keysym and modifier mask to grab.
    fn x11(&self) -> Option<(u32, u16)> {
        let sym = if let Some((_, c)) = LETTERS.iter().find(|(k, _)| *k == self.key) {
            c.to_ascii_lowercase() as u32
        } else if let Some(i) = K_F_KEYS.iter().position(|(k, _)| *k == self.key) {
            0xffbe + i as u32
        } else if self.key == 49 {
            0x20
        } else {
            return None;
        };
        let mut mask = 0u16;
        for (bit, x) in [(SHIFT, 1u16), (CMD, 4), (OPTION, 8), (CONTROL, 64)] {
            if self.mods & bit != 0 {
                mask |= x;
            }
        }
        Some((sym, mask))
    }
}

type Callback = Box<dyn Fn() + Send>;

struct Bound {
    code: u8,
    mask: u16,
    callback: Callback,
}

struct Keys {
    conn: RustConnection,
    root: Window,
    bound: Mutex<HashMap<u32, Bound>>,
}

/// Caps Lock and Num Lock count as modifiers to X: grab with and without.
const LOCKS: [u16; 4] = [0, 2, 16, 18];

fn keys() -> Option<&'static Arc<Keys>> {
    static KEYS: OnceLock<Option<Arc<Keys>>> = OnceLock::new();
    KEYS.get_or_init(|| {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[screen].root;
        let keys = Arc::new(Keys { conn, root, bound: Mutex::default() });
        let heard = keys.clone();
        std::thread::spawn(move || {
            while let Ok(event) = heard.conn.wait_for_event() {
                if let Event::KeyPress(e) = event {
                    let state = u16::from(e.state) & !18;
                    let bound = heard.bound.lock().unwrap_or_else(|e| e.into_inner());
                    if let Some(b) = bound.values().find(|b| b.code == e.detail && b.mask == state) {
                        (b.callback)();
                    }
                }
            }
        });
        Some(keys)
    })
    .as_ref()
}

fn keycode(k: &Keys, sym: u32) -> Option<u8> {
    let setup = k.conn.setup();
    let (first, last) = (setup.min_keycode, setup.max_keycode);
    let map = k.conn.get_keyboard_mapping(first, last - first + 1).ok()?.reply().ok()?;
    let per = map.keysyms_per_keycode as usize;
    map.keysyms.chunks(per.max(1)).position(|c| c.first() == Some(&sym)).map(|i| first + i as u8)
}

fn release(k: &Keys, b: &Bound) {
    for lock in LOCKS {
        let _ = k.conn.ungrab_key(b.code, k.root, ModMask::from(b.mask | lock));
    }
    let _ = k.conn.flush();
}

/// Bind `combo` to slot `id` (replacing any previous binding of that slot).
/// Fails if another app owns the combo.
pub fn bind(id: u32, combo: Combo, callback: impl Fn() + Send + 'static) -> Result<(), i32> {
    let k = keys().ok_or(-1)?;
    unbind(id);
    let (sym, mask) = combo.x11().ok_or(-2)?;
    let code = keycode(k, sym).ok_or(-2)?;
    let bound = Bound { code, mask, callback: Box::new(callback) };
    for lock in LOCKS {
        let grabbed = k.conn.grab_key(false, k.root, ModMask::from(mask | lock), code, GrabMode::ASYNC, GrabMode::ASYNC).ok().and_then(|c| c.check().ok());
        if grabbed.is_none() {
            release(k, &bound);
            // BadAccess: another client has it.
            return Err(10);
        }
    }
    k.bound.lock().unwrap_or_else(|e| e.into_inner()).insert(id, bound);
    Ok(())
}

pub fn unbind(id: u32) {
    let Some(k) = keys() else { return };
    if let Some(b) = k.bound.lock().unwrap_or_else(|e| e.into_inner()).remove(&id) {
        release(k, &b);
    }
}

/// Bind one shortcut (slot 0).
pub fn register(key: u32, modifiers: u32, callback: impl Fn() + Send + 'static) -> Result<(), i32> {
    bind(0, Combo::new(key, modifiers), callback)
}

const K_F_KEYS: [(u32, &str); 12] = [
    (122, "F1"),
    (120, "F2"),
    (99, "F3"),
    (118, "F4"),
    (96, "F5"),
    (97, "F6"),
    (98, "F7"),
    (100, "F8"),
    (101, "F9"),
    (109, "F10"),
    (103, "F11"),
    (111, "F12"),
];

/// macOS ANSI virtual key codes for letters and digits.
const LETTERS: [(u32, char); 36] = [
    (0, 'A'), (11, 'B'), (8, 'C'), (2, 'D'), (14, 'E'), (3, 'F'), (5, 'G'), (4, 'H'), (34, 'I'),
    (38, 'J'), (40, 'K'), (37, 'L'), (46, 'M'), (45, 'N'), (31, 'O'), (35, 'P'), (12, 'Q'), (15, 'R'),
    (1, 'S'), (17, 'T'), (32, 'U'), (9, 'V'), (13, 'W'), (7, 'X'), (16, 'Y'), (6, 'Z'), (29, '0'),
    (18, '1'), (19, '2'), (20, '3'), (21, '4'), (23, '5'), (22, '6'), (26, '7'), (28, '8'), (25, '9'),
];

fn key_name(code: u32) -> String {
    if let Some((_, c)) = LETTERS.iter().find(|(k, _)| *k == code) {
        return c.to_string();
    }
    if let Some((_, n)) = K_F_KEYS.iter().find(|(k, _)| *k == code) {
        return n.to_string();
    }
    match code {
        49 => "Space".into(),
        _ => format!("key {code}"),
    }
}

fn egui_to_vk(key: egui::Key) -> Option<u32> {
    use egui::Key::*;
    let name = key.name();
    if name.len() == 1 {
        let c = name.chars().next()?.to_ascii_uppercase();
        return LETTERS.iter().find(|(_, l)| *l == c).map(|(k, _)| *k);
    }
    let f = [F1, F2, F3, F4, F5, F6, F7, F8, F9, F10, F11, F12];
    if let Some(i) = f.iter().position(|k| *k == key) {
        return Some(K_F_KEYS[i].0);
    }
    match key {
        Space => Some(49),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_egui_mapping() {
        assert_eq!(Combo::new(KEY_R, CMD | SHIFT | OPTION).label(), "Ctrl+Alt+Shift+R");
        let m = egui::Modifiers { alt: true, shift: true, command: true, ctrl: true, ..Default::default() };
        assert_eq!(Combo::from_egui(egui::Key::R, m), Some(Combo::new(KEY_R, CMD | SHIFT | OPTION)));
        // Plain letters are rejected; F-keys alone are fine.
        assert_eq!(Combo::from_egui(egui::Key::R, egui::Modifiers::default()), None);
        assert_eq!(Combo::from_egui(egui::Key::F5, egui::Modifiers::default()), Some(Combo::new(96, 0)));
        assert_eq!(Combo::new(KEY_R, CMD | SHIFT | OPTION).x11(), Some(('r' as u32, 1 | 4 | 8)));
    }
}
