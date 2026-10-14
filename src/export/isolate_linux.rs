//! Voice isolation on macOS is Apple's on-device model; Linux has no
//! equivalent yet, so callers fall back to their simple DSP.

use super::audio::Stereo;

pub fn isolate_voice(_input: &[Stereo]) -> Option<Vec<Stereo>> {
    None
}
