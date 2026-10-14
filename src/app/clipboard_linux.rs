//! Copying a file to the X11 clipboard. A clipboard there is whoever
//! owns the CLIPBOARD selection answering whoever pastes, so a thread
//! holds it until something else is copied (or Glide quits).

use std::path::Path;

use anyhow::Result;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::*;
use x11rb::wrapper::ConnectionExt as _;

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        UTF8_STRING,
        URI_LIST: b"text/uri-list",
        GNOME_FILES: b"x-special/gnome-copied-files",
    }
}

/// `file://` URI of a path, percent-encoding what a URI can't hold.
fn file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut uri = String::from("file://");
    for &b in abs.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{b:02X}"));
        }
    }
    uri
}

/// Offer `path` on the clipboard as a copied file (file managers, chat
/// apps and browsers paste the file itself) and as its path in text.
pub fn copy_file(path: &Path) -> Result<()> {
    let (conn, screen) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen].root;
    let atoms = Atoms::new(&conn)?.reply()?;
    let window = conn.generate_id()?;
    conn.create_window(0, window, root, 0, 0, 1, 1, 0, WindowClass::INPUT_ONLY, x11rb::COPY_FROM_PARENT, &CreateWindowAux::new())?;
    conn.set_selection_owner(window, atoms.CLIPBOARD, x11rb::CURRENT_TIME)?.check()?;
    conn.flush()?;
    let uri = file_uri(path);
    let text = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()).to_string_lossy().into_owned();
    std::thread::spawn(move || {
        while let Ok(event) = conn.wait_for_event() {
            match event {
                // Something else was copied.
                Event::SelectionClear(_) => break,
                Event::SelectionRequest(r) => {
                    let property = if r.property == x11rb::NONE { r.target } else { r.property };
                    let sent = if r.target == atoms.TARGETS {
                        conn.change_property32(PropMode::REPLACE, r.requestor, property, AtomEnum::ATOM, &[atoms.TARGETS, atoms.URI_LIST, atoms.GNOME_FILES, atoms.UTF8_STRING]).is_ok()
                    } else {
                        let data = match r.target {
                            t if t == atoms.URI_LIST => Some(format!("{uri}\r\n")),
                            t if t == atoms.GNOME_FILES => Some(format!("copy\n{uri}")),
                            t if t == atoms.UTF8_STRING => Some(text.clone()),
                            _ => None,
                        };
                        data.is_some_and(|d| conn.change_property8(PropMode::REPLACE, r.requestor, property, r.target, d.as_bytes()).is_ok())
                    };
                    let reply = SelectionNotifyEvent { response_type: SELECTION_NOTIFY_EVENT, sequence: 0, time: r.time, requestor: r.requestor, selection: r.selection, target: r.target, property: if sent { property } else { x11rb::NONE } };
                    let _ = conn.send_event(false, r.requestor, EventMask::NO_EVENT, reply);
                    let _ = conn.flush();
                }
                _ => {}
            }
        }
    });
    Ok(())
}
