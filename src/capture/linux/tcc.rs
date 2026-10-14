//! macOS asks the user before an app may see the screen, hear the
//! microphone or use the camera. X11 and PulseAudio ask nothing, so
//! everything is granted.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Granted,
    Denied,
    NotDetermined,
    Restricted,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Status::Granted => "granted",
            Status::Denied => "DENIED",
            Status::NotDetermined => "not yet requested",
            Status::Restricted => "restricted",
        })
    }
}

pub fn screen_status() -> Status {
    Status::Granted
}

pub fn camera_status() -> Status {
    Status::Granted
}

pub fn mic_status() -> Status {
    Status::Granted
}

pub fn request_all(_screen: bool, _mic: bool, _camera: bool, _wait: Duration) {}

pub fn report() -> (Status, Status, Status) {
    (Status::Granted, Status::Granted, Status::Granted)
}
