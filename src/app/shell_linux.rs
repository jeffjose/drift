//! What the app asks of the desktop on Linux: displays, the file manager,
//! the trash, the wallpaper. (`macos.rs` is its counterpart.)

use std::path::Path;
use std::process::{Command, Stdio};

use winit::window::Window;

use crate::capture::screen;
use crate::capture::x11::X;

pub fn is_fullscreen(window: &Window) -> bool {
    window.fullscreen().is_some()
}

/// The display the window is full screen on.
pub fn fullscreen_display(window: &Window) -> Option<u32> {
    window.fullscreen()?;
    let at = window.outer_position().ok()?;
    let ds = screen::displays().ok()?;
    ds.iter().find(|d| d.origin == (at.x, at.y)).or(ds.first()).map(|d| d.id)
}

pub fn main_display() -> u32 {
    1
}

/// The window manager draws the title bar: there are no buttons to place.
pub fn inset_traffic_lights(_window: &Window, _x: f64, _bar_h: f64) {}

/// macOS apps are brought forward as a whole; here windows are focused
/// one by one, where they're shown.
pub fn activate() {}

fn run(program: &str, args: &[&std::ffi::OsStr]) -> bool {
    Command::new(program).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().is_ok()
}

/// Show a file in the file manager, selected where the file manager can.
pub fn reveal(path: &Path) {
    let uri = format!("file://{}", path.display());
    let shown = Command::new("dbus-send")
        .args(["--session", "--print-reply", "--dest=org.freedesktop.FileManager1", "/org/freedesktop/FileManager1", "org.freedesktop.FileManager1.ShowItems"])
        .arg(format!("array:string:{uri}"))
        .arg("string:")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !shown && let Some(dir) = path.parent() {
        open_folder(dir);
    }
}

pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    run("xdg-open", &[dir.as_os_str()]);
}

pub fn open(path: &Path) {
    run("xdg-open", &[path.as_os_str()]);
}

/// Put a file on the clipboard, as copying it in a file manager does.
pub fn copy_file(path: &Path) {
    if let Err(e) = super::clipboard::copy_file(path) {
        eprintln!("glide: couldn't copy {}: {e:#}", path.display());
    }
}

/// Move a file or folder to the trash (the freedesktop.org one).
pub fn trash(path: &Path) {
    if Command::new("gio").arg("trash").arg(path).stdin(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()) {
        return;
    }
    let Some(home) = std::env::var_os("HOME").map(std::path::PathBuf::from) else { return };
    let trash = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| home.join(".local/share")).join("Trash");
    let (files, info) = (trash.join("files"), trash.join("info"));
    if std::fs::create_dir_all(&files).is_err() || std::fs::create_dir_all(&info).is_err() {
        return;
    }
    let Some(name) = path.file_name().map(|n| n.to_string_lossy().into_owned()) else { return };
    let mut to = name.clone();
    let mut n = 1;
    while files.join(&to).exists() {
        n += 1;
        to = format!("{name}.{n}");
    }
    let stamp = crate::util::timestamp_name();
    // "2026-10-11 at 11.30.12" to the trash's "2026-10-11T11:30:12".
    let when = stamp.replace(" at ", "T").replacen('.', ":", 2);
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    if std::fs::write(info.join(format!("{to}.trashinfo")), format!("[Trash Info]\nPath={}\nDeletionDate={when}\n", abs.display())).is_ok()
        && std::fs::rename(path, files.join(&to)).is_err()
    {
        let _ = std::fs::remove_file(info.join(format!("{to}.trashinfo")));
    }
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Where the desktop keeps its wallpaper: Xfce, then GNOME and its kin.
fn wallpaper_path() -> Option<std::path::PathBuf> {
    if let Some(list) = output("xfconf-query", &["-c", "xfce4-desktop", "-l"]) {
        for prop in list.lines().filter(|l| l.ends_with("/last-image")) {
            if let Some(p) = output("xfconf-query", &["-c", "xfce4-desktop", "-p", prop]).filter(|p| Path::new(p).is_file()) {
                return Some(p.into());
            }
        }
    }
    for key in ["picture-uri-dark", "picture-uri"] {
        for schema in ["org.gnome.desktop.background", "org.cinnamon.desktop.background", "org.mate.background"] {
            let key = if schema == "org.mate.background" { "picture-filename" } else { key };
            if let Some(v) = output("gsettings", &["get", schema, key]) {
                let v = v.trim_matches('\'');
                let p = v.strip_prefix("file://").unwrap_or(v);
                if Path::new(p).is_file() {
                    return Some(p.into());
                }
            }
        }
    }
    None
}

/// Save the current desktop picture to `dest` (by its extension).
pub fn save_desktop_wallpaper(dest: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    let from = wallpaper_path().context("couldn't find the desktop picture")?;
    let img = image::open(&from).with_context(|| format!("reading {}", from.display()))?;
    img.to_rgb8().save(dest).with_context(|| format!("writing {}", dest.display()))?;
    Ok(())
}

pub fn display_ids() -> Vec<u32> {
    screen::displays().map(|ds| ds.iter().map(|d| d.id).collect()).unwrap_or_else(|_| vec![1])
}

pub fn display_under_pointer() -> u32 {
    let (Ok(x), Ok(ds)) = (X::open(), screen::displays()) else { return 1 };
    let Some((px, py, _)) = x.pointer() else { return 1 };
    ds.iter()
        .find(|d| px >= d.origin.0 && py >= d.origin.1 && px < d.origin.0 + d.pixels.0 as i32 && py < d.origin.1 + d.pixels.1 as i32)
        .map_or(1, |d| d.id)
}

/// A display's id and frame in points (0 = the primary display).
pub fn display_frame(id: u32) -> (u32, f64, f64, f64, f64) {
    match screen::display(id) {
        Ok(d) => (d.id, d.origin.0 as f64 / d.scale, d.origin.1 as f64 / d.scale, d.points.0 as f64, d.points.1 as f64),
        Err(_) => (1, 0.0, 0.0, 1920.0, 1080.0),
    }
}

/// The X window id (macOS: the window number), for taking its picture.
pub fn window_number(window: &Window) -> Option<isize> {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(h) => Some(h.window as isize),
        RawWindowHandle::Xcb(h) => Some(h.window.get() as isize),
        _ => None,
    }
}

/// macOS hands opened documents to a running app through an event; here
/// they come as arguments.
pub fn install_open_handler() {}

/// Tell the capture code the scale each monitor's windows are drawn at,
/// so points mean the same to the picker overlays and the recorder.
pub fn note_monitors(el: &winit::event_loop::ActiveEventLoop) {
    let scales = el.available_monitors().map(|m| ((m.position().x, m.position().y), m.scale_factor())).collect();
    screen::set_ui_scales(scales);
}
