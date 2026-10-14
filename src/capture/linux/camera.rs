//! Cameras and microphones. Microphones are PulseAudio's sources; cameras
//! (V4L2) aren't captured on Linux yet, so none are offered.

use anyhow::{Result, anyhow};

pub struct Device {
    pub id: String,
    pub name: String,
}

/// iPhones and iPads are a macOS (CoreMediaIO) feature.
pub fn enable_screen_capture_devices() {}

pub fn ios_devices() -> Vec<Device> {
    vec![]
}

pub fn ios_dimensions(_id: &str) -> Option<(u32, u32)> {
    None
}

pub fn cameras() -> Vec<Device> {
    vec![]
}

/// PulseAudio's sources that aren't the monitor of an output, as `pactl`
/// lists them (id = the source's name, name = its description).
pub fn microphones() -> Vec<Device> {
    let Ok(out) = std::process::Command::new("pactl").args(["list", "sources"]).env("LC_ALL", "C").output() else { return vec![] };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut found = vec![];
    let mut id: Option<String> = None;
    for line in text.lines().map(str::trim) {
        if let Some(n) = line.strip_prefix("Name: ") {
            id = Some(n.to_string());
        } else if let Some(d) = line.strip_prefix("Description: ")
            && let Some(id) = id.take()
            && !id.ends_with(".monitor")
        {
            found.push(Device { id, name: d.to_string() });
        }
    }
    found
}

pub fn default_microphone() -> Option<String> {
    let out = std::process::Command::new("pactl").arg("get-default-source").output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!name.is_empty() && !name.ends_with(".monitor")).then_some(name)
}

/// A MacBook's built-in microphone is cut when its lid closes; nothing
/// like it here.
pub fn builtin_mic_off(_mic_id: Option<&str>) -> bool {
    false
}

pub fn lid_closed() -> bool {
    false
}

/// A microphone by id or part of its name.
pub fn resolve_mic(sel: &str) -> Result<String> {
    let lower = sel.to_lowercase();
    microphones()
        .into_iter()
        .find(|d| d.id == sel || d.name.to_lowercase().contains(&lower))
        .map(|d| d.id)
        .ok_or_else(|| anyhow!("no microphone matches {sel:?} (see `glide devices`)"))
}
