//! The X11 side of capture: monitors, windows, and reading pixels off the
//! screen through shared memory (MIT-SHM).

use anyhow::{Context, Result, anyhow, bail};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xproto::*;
use x11rb::rust_connection::RustConnection;

x11rb::atom_manager! {
    pub Atoms: AtomsCookie {
        UTF8_STRING,
        _NET_CLIENT_LIST_STACKING,
        _NET_WM_NAME,
        _NET_WM_PID,
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_NORMAL,
        _NET_WM_WINDOW_TYPE_DIALOG,
        _NET_WM_STATE,
        _NET_WM_STATE_HIDDEN,
        _NET_WM_STATE_ABOVE,
        _NET_WM_STATE_SKIP_TASKBAR,
        _NET_WM_STATE_SKIP_PAGER,
        _NET_WM_DESKTOP,
        _NET_CURRENT_DESKTOP,
        _NET_FRAME_EXTENTS,
        _GTK_FRAME_EXTENTS,
        _NET_ACTIVE_WINDOW,
        _NET_WM_CM_S0,
        _NET_WM_WINDOW_OPACITY,
        _NET_WM_WINDOW_TYPE_NOTIFICATION,
        _NET_WM_WINDOW_TYPE_DOCK,
        _MOTIF_WM_HINTS,
        WM_PROTOCOLS,
        WM_DELETE_WINDOW,
    }
}

/// A connection to the X server with what's asked of it everywhere.
pub struct X {
    pub conn: RustConnection,
    pub root: Window,
    pub atoms: Atoms,
    /// The whole screen (every monitor) in pixels.
    pub size: (u16, u16),
    pub depth: u8,
    pub visual: Visualid,
}

/// One monitor, in root-window pixels.
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
    /// Physical size in millimetres (0 when the monitor doesn't say).
    pub mm: (u32, u32),
}

/// A top-level window as the window manager lists it.
#[derive(Clone, Debug)]
pub struct Win {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub pid: i32,
    /// What's seen of it, title bar included, in root pixels.
    pub rect: (i32, i32, u32, u32),
    /// The client window's own position and size in root pixels.
    pub client: (i32, i32, u32, u32),
    /// Window-manager frame around the client: left, right, top, bottom.
    pub frame: [i32; 4],
    /// Invisible margin (shadows) inside a client-decorated window.
    pub margin: [i32; 4],
}

impl X {
    pub fn open() -> Result<X> {
        let (conn, screen) = x11rb::connect(None).context("connecting to the X server (is DISPLAY set?)")?;
        let s = &conn.setup().roots[screen];
        let (root, size, depth, visual) = (s.root, (s.width_in_pixels, s.height_in_pixels), s.root_depth, s.root_visual);
        let atoms = Atoms::new(&conn)?.reply()?;
        Ok(X { conn, root, atoms, size, depth, visual })
    }

    /// Monitors, the primary first and then left to right.
    pub fn monitors(&self) -> Vec<Monitor> {
        let mut out: Vec<Monitor> = self
            .conn
            .randr_get_monitors(self.root, true)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| {
                r.monitors
                    .iter()
                    .filter(|m| m.width > 0 && m.height > 0)
                    .map(|m| Monitor { x: m.x as i32, y: m.y as i32, width: m.width as u32, height: m.height as u32, primary: m.primary, mm: (m.width_in_millimeters, m.height_in_millimeters) })
                    .collect()
            })
            .unwrap_or_default();
        if out.is_empty() {
            out.push(Monitor { x: 0, y: 0, width: self.size.0 as u32, height: self.size.1 as u32, primary: true, mm: (0, 0) });
        }
        out.sort_by_key(|m| (!m.primary, m.x, m.y));
        out
    }

    fn property(&self, window: Window, atom: Atom, kind: impl Into<Atom>) -> Option<Vec<u8>> {
        let r = self.conn.get_property(false, window, atom, kind.into(), 0, 1 << 20).ok()?.reply().ok()?;
        (r.value_len > 0).then_some(r.value)
    }

    fn cardinals(&self, window: Window, atom: Atom) -> Vec<u32> {
        self.conn
            .get_property(false, window, atom, AtomEnum::ANY, 0, 1 << 16)
            .ok()
            .and_then(|c| c.reply().ok())
            .and_then(|r| r.value32().map(|v| v.collect()))
            .unwrap_or_default()
    }

    /// `Xft.dpi` from the resource database, as toolkits scale by.
    pub fn xft_dpi(&self) -> Option<f64> {
        let db = self.property(self.root, AtomEnum::RESOURCE_MANAGER.into(), AtomEnum::STRING)?;
        String::from_utf8_lossy(&db).lines().find_map(|l| l.strip_prefix("Xft.dpi:")).and_then(|v| v.trim().parse().ok())
    }

    /// Is a compositing manager running (are translucent windows shown as such)?
    pub fn composited(&self) -> bool {
        self.conn.get_selection_owner(self.atoms._NET_WM_CM_S0).ok().and_then(|c| c.reply().ok()).is_some_and(|r| r.owner != x11rb::NONE)
    }

    /// Where the pointer is, in root pixels, and the buttons and modifiers held.
    pub fn pointer(&self) -> Option<(i32, i32, u16)> {
        let r = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some((r.root_x as i32, r.root_y as i32, u16::from(r.mask)))
    }

    /// One window's details, or None if it's gone or not shown.
    pub fn window(&self, id: Window) -> Option<Win> {
        let attrs = self.conn.get_window_attributes(id).ok()?.reply().ok()?;
        if attrs.map_state != MapState::VIEWABLE {
            return None;
        }
        let geo = self.conn.get_geometry(id).ok()?.reply().ok()?;
        let at = self.conn.translate_coordinates(id, self.root, 0, 0).ok()?.reply().ok()?;
        let four = |atom| {
            let v = self.cardinals(id, atom);
            if v.len() == 4 { [v[0] as i32, v[1] as i32, v[2] as i32, v[3] as i32] } else { [0; 4] }
        };
        let (frame, margin) = (four(self.atoms._NET_FRAME_EXTENTS), four(self.atoms._GTK_FRAME_EXTENTS));
        let client = (at.dst_x as i32, at.dst_y as i32, geo.width as u32, geo.height as u32);
        let grow = [frame[0] - margin[0], frame[1] - margin[1], frame[2] - margin[2], frame[3] - margin[3]];
        let rect = (
            client.0 - grow[0],
            client.1 - grow[2],
            (client.2 as i32 + grow[0] + grow[1]).max(1) as u32,
            (client.3 as i32 + grow[2] + grow[3]).max(1) as u32,
        );
        let text = |v: Vec<u8>| String::from_utf8_lossy(&v).trim_end_matches('\0').to_string();
        let title = self
            .property(id, self.atoms._NET_WM_NAME, self.atoms.UTF8_STRING)
            .or_else(|| self.property(id, AtomEnum::WM_NAME.into(), AtomEnum::ANY))
            .map(text)
            .unwrap_or_default();
        // WM_CLASS is "instance\0class\0": the class is the app's name.
        let app = self
            .property(id, AtomEnum::WM_CLASS.into(), AtomEnum::STRING)
            .and_then(|v| v.split(|b| *b == 0).filter(|p| !p.is_empty()).last().map(|p| String::from_utf8_lossy(p).into_owned()))
            .unwrap_or_default();
        let pid = self.cardinals(id, self.atoms._NET_WM_PID).first().copied().unwrap_or(0) as i32;
        Some(Win { id, app, title, pid, rect, client, frame, margin })
    }

    /// Ordinary windows on the current desktop, frontmost first.
    pub fn windows(&self) -> Vec<Win> {
        let desktop = self.cardinals(self.root, self.atoms._NET_CURRENT_DESKTOP).first().copied();
        let mut out = vec![];
        for id in self.cardinals(self.root, self.atoms._NET_CLIENT_LIST_STACKING).into_iter().rev() {
            let kinds = self.cardinals(id, self.atoms._NET_WM_WINDOW_TYPE);
            if !kinds.is_empty() && !kinds.contains(&self.atoms._NET_WM_WINDOW_TYPE_NORMAL) && !kinds.contains(&self.atoms._NET_WM_WINDOW_TYPE_DIALOG) {
                continue;
            }
            if self.cardinals(id, self.atoms._NET_WM_STATE).contains(&self.atoms._NET_WM_STATE_HIDDEN) {
                continue;
            }
            let on = self.cardinals(id, self.atoms._NET_WM_DESKTOP).first().copied();
            if let (Some(d), Some(on)) = (desktop, on)
                && on != d
                && on != u32::MAX
            {
                continue;
            }
            if let Some(w) = self.window(id) {
                out.push(w);
            }
        }
        out
    }

    /// Ask the window manager to do something to a window (an EWMH client
    /// message to the root).
    pub fn ask_wm(&self, window: Window, message: Atom, data: [u32; 5]) -> Result<()> {
        let event = ClientMessageEvent::new(32, window, message, data);
        self.conn.send_event(false, self.root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event)?;
        self.conn.flush()?;
        Ok(())
    }
}

/// Reads rectangles of the screen into memory shared with the X server,
/// so the pixels are copied once and not sent down the socket.
pub struct Grabber {
    pub x: X,
    seg: u32,
    addr: *mut u8,
    capacity: usize,
}

// SAFETY: the mapping is only read through &mut self.
unsafe impl Send for Grabber {}

impl Grabber {
    /// For rectangles up to `width` x `height` pixels.
    pub fn new(width: u32, height: u32) -> Result<Grabber> {
        let x = X::open()?;
        if x.depth != 24 && x.depth != 32 {
            bail!("the screen is {}-bit; only 24-bit colour is supported", x.depth);
        }
        x.conn.shm_query_version()?.reply().map_err(|_| anyhow!("the X server has no shared memory (MIT-SHM): is it on another machine?"))?;
        let capacity = width as usize * height as usize * 4;
        // SAFETY: a private segment of `capacity` bytes, mapped once and
        // marked for removal so it goes when both sides let go.
        let (id, addr) = unsafe {
            let id = libc::shmget(libc::IPC_PRIVATE, capacity, libc::IPC_CREAT | 0o600);
            if id < 0 {
                bail!("no shared memory for the screen ({})", std::io::Error::last_os_error());
            }
            let addr = libc::shmat(id, std::ptr::null(), 0);
            if addr as isize == -1 {
                libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut());
                bail!("couldn't map shared memory ({})", std::io::Error::last_os_error());
            }
            (id, addr as *mut u8)
        };
        let seg = x.conn.generate_id()?;
        let attached = x.conn.shm_attach(seg, id as u32, false).map_err(anyhow::Error::from).and_then(|c| c.check().map_err(anyhow::Error::from));
        // SAFETY: the server holds its own reference by now (or never will).
        unsafe { libc::shmctl(id, libc::IPC_RMID, std::ptr::null_mut()) };
        if let Err(e) = attached {
            // SAFETY: unmapping what was mapped above.
            unsafe { libc::shmdt(addr as *const libc::c_void) };
            bail!("the X server couldn't share memory: {e}");
        }
        Ok(Grabber { x, seg, addr, capacity })
    }

    /// The pixels of a rectangle of the root window (BGRX, 4 bytes each,
    /// rows packed). What falls outside the screen is left out: the rows
    /// returned are for the rectangle clipped to it, given back with them.
    pub fn grab(&mut self, x: i32, y: i32, width: u32, height: u32) -> Result<(&[u8], (i32, i32, u32, u32))> {
        let (sw, sh) = (self.x.size.0 as i32, self.x.size.1 as i32);
        let (x0, y0) = (x.clamp(0, sw), y.clamp(0, sh));
        let (x1, y1) = ((x + width as i32).clamp(0, sw), (y + height as i32).clamp(0, sh));
        let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
        if w == 0 || h == 0 {
            return Ok((&[], (x0, y0, 0, 0)));
        }
        let bytes = w as usize * h as usize * 4;
        if bytes > self.capacity {
            bail!("{w}x{h} is more than this grabber was made for");
        }
        self.x.conn.shm_get_image(self.x.root, x0 as i16, y0 as i16, w as u16, h as u16, !0, ImageFormat::Z_PIXMAP.into(), self.seg, 0)?.reply().context("reading the screen")?;
        // SAFETY: the server just wrote `bytes` bytes at the start of the mapping.
        Ok((unsafe { std::slice::from_raw_parts(self.addr, bytes) }, (x0, y0, w, h)))
    }
}

impl Drop for Grabber {
    fn drop(&mut self) {
        let _ = self.x.conn.shm_detach(self.seg);
        let _ = self.x.conn.flush();
        // SAFETY: unmapping what `new` mapped.
        unsafe { libc::shmdt(self.addr as *const libc::c_void) };
    }
}

/// Copy grabbed rows into an opaque BGRA image of `width` x `height`,
/// placing them `(dx, dy)` from its top-left corner; the rest is black.
pub fn place(dst: &mut [u8], width: u32, height: u32, rows: &[u8], (dx, dy, w, h): (i32, i32, u32, u32)) {
    let stride = width as usize * 4;
    if w < width || h < height {
        for px in dst.chunks_exact_mut(4) {
            px.copy_from_slice(&[0, 0, 0, 255]);
        }
    }
    let cols = (w as i32).min(width as i32 - dx).max(0) as usize;
    for row in 0..(h as i32).min(height as i32 - dy).max(0) as usize {
        let src = &rows[row * w as usize * 4..][..cols * 4];
        let at = (row + dy as usize) * stride + dx as usize * 4;
        let out = &mut dst[at..at + cols * 4];
        out.copy_from_slice(src);
        // The fourth byte is padding on a 24-bit screen: make it opaque.
        for px in out.chunks_exact_mut(4) {
            px[3] = 255;
        }
    }
}
