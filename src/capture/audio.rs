//! Audio in from PulseAudio (or PipeWire speaking its protocol): what an
//! output is playing, through its monitor source, or a microphone. Read
//! with libpulse-simple, loaded when first used so building needs none of
//! its headers.

use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::thread::JoinHandle;

use anyhow::{Result, anyhow, bail};

use super::mixer::RATE;

#[repr(C)]
struct SampleSpec {
    format: c_int,
    rate: u32,
    channels: u8,
}

#[repr(C)]
struct BufferAttr {
    maxlength: u32,
    tlength: u32,
    prebuf: u32,
    minreq: u32,
    fragsize: u32,
}

const PA_STREAM_RECORD: c_int = 2;
const PA_SAMPLE_FLOAT32LE: c_int = 5;

type SimpleNew = unsafe extern "C" fn(*const c_char, *const c_char, c_int, *const c_char, *const c_char, *const SampleSpec, *const c_void, *const BufferAttr, *mut c_int) -> *mut c_void;
type SimpleRead = unsafe extern "C" fn(*mut c_void, *mut c_void, usize, *mut c_int) -> c_int;
type SimpleFree = unsafe extern "C" fn(*mut c_void);

struct Pulse {
    new: SimpleNew,
    read: SimpleRead,
    free: SimpleFree,
}

fn pulse() -> Result<&'static Pulse> {
    static LIB: std::sync::OnceLock<Option<Pulse>> = std::sync::OnceLock::new();
    LIB.get_or_init(|| {
        // SAFETY: the library stays loaded for the life of the process, and
        // the three symbols have the signatures pulse/simple.h declares.
        unsafe {
            let lib = libc::dlopen(c"libpulse-simple.so.0".as_ptr(), libc::RTLD_NOW);
            if lib.is_null() {
                return None;
            }
            let sym = |name: &std::ffi::CStr| {
                let p = libc::dlsym(lib, name.as_ptr());
                (!p.is_null()).then_some(p)
            };
            Some(Pulse {
                new: std::mem::transmute::<*mut c_void, SimpleNew>(sym(c"pa_simple_new")?),
                read: std::mem::transmute::<*mut c_void, SimpleRead>(sym(c"pa_simple_read")?),
                free: std::mem::transmute::<*mut c_void, SimpleFree>(sym(c"pa_simple_free")?),
            })
        }
    })
    .as_ref()
    .ok_or_else(|| anyhow!("PulseAudio's client library (libpulse-simple) isn't installed"))
}

/// What to listen to.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// Whatever the default output is playing.
    System,
    /// A microphone by PulseAudio source name (None = the default).
    Mic(Option<String>),
}

/// A running capture; stops when dropped.
pub struct AudioIn {
    stop: Arc<AtomicBool>,
    level: Arc<AtomicU32>,
    thread: Option<JoinHandle<()>>,
}

/// About 10 ms a read.
const CHUNK: usize = 480;

impl AudioIn {
    /// Start reading 48 kHz stereo. `on_audio` gets each chunk with the
    /// host time (see `capture::host_now`) its first sample was heard at.
    pub fn start(input: &Input, mut on_audio: impl FnMut(f64, &[[f32; 2]]) + Send + 'static) -> Result<AudioIn> {
        let lib = pulse()?;
        let device = match input {
            Input::System => Some(CString::new("@DEFAULT_MONITOR@")?),
            Input::Mic(Some(name)) => Some(CString::new(name.as_str())?),
            Input::Mic(None) => None,
        };
        let spec = SampleSpec { format: PA_SAMPLE_FLOAT32LE, rate: RATE as u32, channels: 2 };
        let attr = BufferAttr { maxlength: u32::MAX, tlength: u32::MAX, prebuf: u32::MAX, minreq: u32::MAX, fragsize: (CHUNK * 8) as u32 };
        let mut err: c_int = 0;
        // SAFETY: every pointer outlives the call; a null stream is an error.
        let stream = unsafe {
            (lib.new)(std::ptr::null(), c"Glide".as_ptr(), PA_STREAM_RECORD, device.as_ref().map_or(std::ptr::null(), |d| d.as_ptr()), c"recording".as_ptr(), &spec, std::ptr::null(), &attr, &mut err)
        };
        if stream.is_null() {
            bail!("couldn't open {} (PulseAudio error {err})", if *input == Input::System { "system audio" } else { "the microphone" });
        }
        let stream = crate::util::SendBox(stream);
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let (st, lv) = (stop.clone(), level.clone());
        let thread = std::thread::spawn(move || {
            let stream = stream;
            let mut buf = vec![[0f32; 2]; CHUNK];
            while !st.load(Ordering::Relaxed) {
                let mut err: c_int = 0;
                // SAFETY: `buf` holds CHUNK stereo float frames.
                if unsafe { (lib.read)(stream.0, buf.as_mut_ptr() as *mut c_void, CHUNK * 8, &mut err) } < 0 {
                    break;
                }
                let heard = super::host_now() - CHUNK as f64 / RATE;
                let peak = buf.iter().fold(0f32, |m, s| m.max(s[0].abs()).max(s[1].abs())).min(1.0);
                // Non-negative f32 bits order like the floats.
                lv.fetch_max(peak.to_bits(), Ordering::Relaxed);
                on_audio(heard, &buf);
            }
            // SAFETY: the stream is freed once, by the only thread using it.
            unsafe { (lib.free)(stream.0) };
        });
        Ok(AudioIn { stop, level, thread: Some(thread) })
    }

    /// Peak level (0..1) since the last call.
    pub fn take_level(&self) -> f32 {
        f32::from_bits(self.level.swap(0, Ordering::Relaxed))
    }
}

impl Drop for AudioIn {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
