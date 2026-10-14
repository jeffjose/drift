//! Small helpers shared across the crate.

/// Wrapper asserting a value may cross threads. All uses are serialized by
/// a Mutex or are of types documented as thread-safe.
pub struct SendBox<T>(pub T);
unsafe impl<T> Send for SendBox<T> {}
unsafe impl<T> Sync for SendBox<T> {}

pub const fn fourcc(s: &[u8; 4]) -> u32 {
    ((s[0] as u32) << 24) | ((s[1] as u32) << 16) | ((s[2] as u32) << 8) | (s[3] as u32)
}
pub const PIX_420V: u32 = fourcc(b"420v");
pub const PIX_BGRA: u32 = fourcc(b"BGRA");



/// Local date-time like "2026-10-11 at 11.30.12" (for file names).
pub fn timestamp_name() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        format!(
            "{:04}-{:02}-{:02} at {:02}.{:02}.{:02}",
            tm.tm_year + 1900,
            tm.tm_mon + 1,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_sec
        )
    }
}
