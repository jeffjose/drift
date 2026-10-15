# drift

A Linux port of [Glide](https://github.com/michaelshimeles/glide), the
open-source macOS screen recorder by Michael Shimeles for making polished
product videos. Record your screen, a window or an area; drift re-renders
the cursor, zooms into what you click, adds a background, shadow and
rounded corners, and exports MP4 or GIF.

The engine, renderer and editor are Glide's, unchanged. Screen capture,
encoding and desktop integration are rewritten for Linux (X11).

```sh
glide                              # open the app
glide record -t 10                 # record the main display for 10 seconds
glide record --area 200,150,1280,720 --mic
glide export Recording.glide --height 1080
```

The binary and the `.glide` project format keep Glide's name.

## Install

Requires X11, a Vulkan-capable GPU and [Rust](https://rustup.rs). On
Debian or Ubuntu, `scripts/setup.sh` installs the system packages the
build needs (CMake, libclang, pkg-config, the ffmpeg and ALSA headers),
skipping any already there:

```sh
scripts/setup.sh
cargo build --release
cp target/release/glide ~/.local/bin/
```

`scripts/setup.sh --check` only reports what's missing. On other
distributions, install the equivalents of the packages listed at the top
of the script.

## Usage

Click Record, or press Ctrl+Alt+Shift+R from anywhere. Press the shortcut
again to stop, and the recording opens in the editor. Shortcuts can be
changed under Global shortcuts.

- Recordings are saved to `~/Videos/Glide`, settings to `~/.config/glide`.
- Video is encoded with NVENC when available, otherwise x264. Set
  `GLIDE_ENCODER=software` to force x264.
- System audio and the microphone are captured through PulseAudio, on
  separate tracks.

```sh
glide devices                      # list displays, windows and microphones
glide export Project.glide --format gif
glide transcribe Project.glide     # generate captions with Whisper
glide resize-window 1234 1920x1080 # window id from `glide devices`
glide synth demo.glide             # create a sample project to try the editor
```

Run `glide --help` for the full list.

## Differences from Glide on macOS

X11 has no way to exclude a window from screen capture, which leads to a
few differences:

- The recording controls only appear where they won't be captured: next
  to a recorded area or window, or on another display. When recording a
  whole display, use the shortcut to stop.
- The preview on the setup screen includes drift's own window.
- A window recording captures the region of the screen the window
  occupies, so anything overlapping it is recorded too.

Not yet implemented: a tray icon, camera capture, and fast export (it is
currently slower than Glide's).

Not supported: iPhone and iPad recording, Apple Speech captions and voice
isolation, which depend on macOS.

## Staying in sync with Glide

This repository keeps Glide's commit history, so upstream changes can be
merged:

```sh
git remote add upstream https://github.com/michaelshimeles/glide
git fetch upstream
git merge upstream/main
```

See `docs/PORT.md` for a detailed list of what differs from Glide and
what work remains.

## Credits

- [Glide](https://github.com/michaelshimeles/glide) by Michael Shimeles.
  Its behavior was reverse engineered from Screen Studio; neither Glide
  nor drift is affiliated with Screen Studio or contains its code or
  assets.
- Captions use [whisper.cpp](https://github.com/ggml-org/whisper.cpp).
- Inter typeface by Rasmus Andersson.

License: MIT OR Apache-2.0, same as Glide.
