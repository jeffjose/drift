//! The menu bar recorder's place on Linux is the system tray, which Glide
//! doesn't have an icon in yet: this keeps the state the tray will show.
//! Until then recording is started from the window and the global
//! shortcut, and stopped from the shortcut or the recording controls.

/// What the tray shows.
pub struct View<'a> {
    pub recording: bool,
    pub paused: bool,
    pub finishing: bool,
    pub clock: &'a str,
}

pub struct MenuBar {
    title: Option<String>,
}

impl MenuBar {
    pub fn new() -> MenuBar {
        MenuBar { title: None }
    }

    /// The record shortcut's label, and whether it could be registered.
    pub fn set_hotkey(&self, _label: &str, _ok: bool) {}

    /// The clock while recording.
    pub fn title(&self) -> Option<String> {
        self.title.clone()
    }

    pub fn update(&mut self, v: &View) {
        self.title = v.recording.then(|| v.clock.to_string());
    }
}
