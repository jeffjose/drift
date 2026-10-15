# Drift: what differs from Glide, and what's left to do

Drift is [Glide](https://github.com/michaelshimeles/glide) (macOS) ported to
Linux/X11. This file is the working record of the port: how the tree differs
from upstream, why, what has and hasn't been checked, and what to do next.
Keep it current: when you finish or change something here, edit this file in
the same commit.

Baseline: upstream `2d4ce59` ("pixel-perfect exports at native resolution").
The `upstream` branch mirrors Glide's `main`; `main` is Drift.

## The rule the port follows

The user's requirement: the product itself (what a rendered frame looks
like, the editor's layout and behaviour) is 1:1 with Glide; the window and
desktop integration may differ to be native to Linux.

So `src/engine`, `src/render` (shader, compositor) and the egui editor code
are Glide's and should stay that way. Don't restyle the UI, change spacing,
or "improve" the renderer here: a change there is a divergence from upstream
that every merge then has to carry. Platform work goes in the files listed
under [Linux files](#linux-files).

## Layout of the delta

### Linux files

These exist upstream under the same path as macOS code; here they are
complete rewrites (about 3,900 lines). An upstream change to any of them
conflicts on merge: keep ours, read what upstream's change does, port it if
it applies.

| File | What it is | Replaces (upstream) |
|---|---|---|
| `src/capture/x11.rs` | X connection, monitors (RandR), window list (EWMH), `Grabber` (MIT-SHM `GetImage` of the root), `place()` | new |
| `src/capture/screen.rs` | `DisplayInfo`, `WindowInfo`, `displays()`, `windows()`, `find_window()`, UI scale per monitor | ScreenCaptureKit |
| `src/capture/preview.rs` | `LivePreview`: grab, swscale down to canvas size, RGBA frames | SCStream + Metal scaler |
| `src/capture/audio.rs` | `AudioIn`: PulseAudio via `libpulse-simple`, `dlopen`ed (no headers needed) | SCK audio, AVCapture mic |
| `src/capture/camera.rs` | microphone list via `pactl`; cameras and iOS devices return empty | AVFoundation |
| `src/capture/winsize.rs` | resize/raise windows via EWMH | Accessibility API |
| `src/capture/tcc.rs` | permissions: everything "granted" | TCC |
| `src/capture/mixer.rs`, `record.rs` | just `RATE` and `clock()` | real-time mixer, CLI session |
| `src/capture/meter.rs` | unchanged from upstream (cpal mic meter) | — |
| `src/media/decode.rs` | `PixelBuffer`, `Frame`, `VideoReader`, `read_audio` (ffmpeg-next) | `media/mod.rs` (AVAssetReader) |
| `src/media/encode.rs` | `Muxer`: video + N AAC tracks; `write_aac` | new |
| `src/media/writer.rs` | `MovieWriter` for export, on `Muxer` | AVAssetWriter |
| `src/media/mod.rs` | `FrameSource`, `Prefetch` (upstream's, moved here verbatim) | — |
| `src/recorder/mod.rs` | `Recorder`: grab thread + encode thread, audio tracks, `write_package` | Session/Sink based recorder |
| `src/recorder/input.rs` | XInput2 raw buttons/keys, polled pointer, XFixes cursor images, window bounds | CGEvent tap |
| `src/app/hotkey.rs` | `XGrabKey` on a thread; `Combo` keeps macOS key codes | Carbon |
| `src/app/recbar.rs`, `pill.rs` | recording controls: self-drawn override-redirect X window | NSPanel |
| `src/app/menubar.rs` | **stub** (holds state, shows nothing) | NSStatusItem |
| `src/app/shell.rs` | displays, reveal/open/trash, wallpaper, `display_picture` | `macos.rs` |
| `src/app/clipboard.rs` | copy a file to CLIPBOARD (selection-owner thread) | NSPasteboard |
| `src/app/voiceover.rs` | mic to memory, AAC on stop | AVAudioRecorder |
| `src/render/interop.rs` | **stub**: `TextureCache::new` is always `None` | IOSurface↔Metal |
| `src/export/isolate.rs` | **stub**: `isolate_voice` is always `None` | AUSoundIsolation |
| `tests/media.rs` | encode→decode round trip keeps colours and timestamps | new |

Deleted outright: `src/app/macos.rs`, `src/capture/{compose,pattern,writer}.rs`,
`src/util/mac.rs` (never upstream; the port's split), `support/Info.plist`,
`build.rs`, `scripts/`.

### Shared files with edits

Upstream changes here merge textually; expect small conflicts.

- `src/app/mod.rs` (the most edits): module list; `shell::` instead of
  `macos::`; window attributes (X11 class instead of transparent title bar);
  `Win.backdrop` and the picker backdrop; live preview uploads a
  `ColorImage` (`Live.screen_tex`) instead of wrapping an IOSurface;
  `RecordBar::new` takes the recorded rectangle; autorelease pools, traffic
  lights, `activate()`, open-document handler and iOS device enabling removed.
- `src/app/home.rs`: settings path; `Thumbs` joins its worker on drop (see
  [Gotchas](#gotchas)); iPhone source and Hide Dock/desktop icons removed
  from the UI.
- `src/app/inspector.rs`: caption engine picker removed (Whisper only).
- `src/app/theme.rs`: no traffic-light inset (`toolbar_left()` is 16);
  `system_accent()` is fixed blue.
- `src/app/winpick.rs`: `is_own()`, `frame()` use X11 window info.
- `src/app/player.rs`: App Nap activity token removed.
- `src/transcribe.rs`: Whisper only; Apple Speech chunking removed.
- `src/project/mod.rs`: `config_dir()` (`~/.config/glide`), projects in
  `~/Videos/Glide`. `presets.rs` and `whisper.rs` use `config_dir()`.
- `src/render/gpu.rs`: Vulkan backend.
- `src/util.rs`: only `SendBox`, `fourcc`, `timestamp_name` remain.
- `src/main.rs`, `src/lib.rs`, `src/exposure.rs`, `src/app/editor.rs`: small.
- `Cargo.toml`: `objc2*` gone; `ffmpeg-next`, `x11rb` added; `whisper-rs`
  without `metal`.

### Left in place on purpose

To keep shared files close to upstream, some macOS-shaped things remain:

- `SourceKind::Device`, `Target::Device`, `Source::Device`, `Settings.ios_device`,
  `hide_dock`, `hide_desktop_icons`, `exclude_own_audio`: kept in the data
  model (settings and macOS-made recordings still load); recording a Device
  bails with a message.
- `capture::tcc` and `render::interop` exist as stubs because shared code
  calls them. `export/mod.rs` still contains `mp4_zero_copy`, never taken.
- CLI flags `--hide-icons`, `--hide-dock`, `--device`, `permissions`.
- The `GLIDE_GUI_SELFTEST` / `GLIDE_GUI_PLAYTEST` machinery in `app/mod.rs`
  shells out to macOS's `screencapture`; those screenshots fail quietly.
- `PERFORMANCE.md`, `PROGRESS.md`, `docs/GOAL.md`, screenshots: Glide's.

## Design decisions (and why)

- **X11 only, libav linked** (user's choice). Wayland has no global input
  logging, which auto-zoom depends on.
- **Points vs pixels.** "Points" = pixels / UI scale, where the scale is the
  one winit gives the app's windows. `screen::ui_scale` mirrors winit's
  logic (`WINIT_X11_SCALE_FACTOR`, `Xft.dpi`/96, else from physical size);
  `shell::note_monitors` overrides it with winit's own values once the app
  runs. Recorder and input log work in **pixels**; `meta.scale` and
  `meta.bounds` convert. Only tested at scale 1.
- **Display ids** are 1-based in primary-first, left-to-right order; 0 means
  primary.
- **Recording pipeline.** Grab thread reads the root window on a `1/fps`
  grid into recycled buffers; encode thread converts BGRA→NV12/I420 with
  swscale and encodes. If the encoder is behind, the grab is dropped and the
  previous frame repeated, so output is constant frame rate.
  `GLIDE_PROFILE=1` prints frames and repeats at the end.
- **Encoders.** Recording tries `hevc_nvenc`, `h264_nvenc`, `libx264` (x265
  can't keep up in real time). Export tries NVENC then x264/x265 for the
  codec asked. `GLIDE_ENCODER=software` skips NVENC. Quality 0..1 maps to
  CRF/CQ by `crf()` in `encode.rs` (0.95 → 14). Colour: BT.709 matrix and
  primaries, sRGB transfer, limited range, both directions set explicitly on
  swscale; HEVC is tagged `hvc1`.
- **Audio.** Each source is its own AAC track (system = track 0, mic after
  it). Chunks are placed on the track by host timestamp with a 50 ms snap;
  gaps become silence; tracks are padded to the video length.
- **Clock.** `capture::host_now()` (seconds since process start) is shared
  by frames, audio and the input log; `recorder::Clock` removes pauses.
- **Hotkeys** store macOS virtual key codes and Carbon modifier bits so
  settings and defaults match upstream. Mapping: ⌘→Ctrl, ⌥→Alt, ⌃→Super.
  Default record shortcut is therefore Ctrl+Alt+Shift+R.
- **Key log** labels match Glide's glyphs; `code` is the X keycode;
  modifiers map Ctrl→`CTRL`, Alt→`OPT`, Super→`CMD`.
- **Cursor names.** XFixes cursor names `left_ptr`/`default`, `xterm`/`text`,
  `hand2`/`pointer` map to Glide's `arrow`, `ibeam`, `pointing-hand`.
- **Controls pill** is only opened where it can't be recorded
  (`pill::spot`); for a whole single display there is nowhere, so no pill.
- **Picker without a compositor.** `shell::display_picture` screenshots the
  display before the overlay opens; the overlay draws it as its background.
  With a compositor (`_NET_WM_CM_S0` owned) it returns `None` and the
  overlay is see-through as upstream.
- **Window recording** crops the root at the window's rectangle (frame
  included, GTK shadow margins excluded) and follows it. Not XComposite.

## Verified, and how

Machine: Ubuntu 22.04, XFCE (no compositor), RTX 3090, ffmpeg 4.4,
PulseAudio, one 3840×2160 display at scale 1.

- `cargo test --release`: 72 unit tests, plus audio, background, resample,
  media round trip and UI smoke.
- Editor rendered offscreen (`glide snapshot`) matches Glide's screenshot by
  eye. No pixel diff against a macOS render has been done.
- CLI: `synth`, `frame`, `export` (mp4, gif), `record --pattern`,
  `record` of the real display at 4K60 with 0 repeated frames (NVENC) and a
  1080p area with x264.
- GUI under Xvfb: launch, live preview, hotkey start/stop, click and key
  logging, auto-zoom created, editor opens, area picker draws with backdrop.
- The user ran the GUI on the real desktop and reported it working; which
  features they exercised wasn't itemised.

### Never exercised

Microphone track; voice-over; Whisper captions on Linux; window recording;
the pill's drawing and buttons; dragging in the area picker; window picker;
clipboard copy; trash; reveal; wallpaper import; resize-window; pause/resume;
multi-monitor; HiDPI scale ≠ 1; a compositing desktop; a machine without
NVENC (only via `GLIDE_ENCODER=software`); PipeWire-only audio.

## Future work

Roughly in order of value.

1. **Tray icon** (`src/app/menubar.rs`). Needed so a whole-display recording
   can be stopped without the shortcut. StatusNotifierItem over D-Bus; no
   D-Bus crate is in the tree. `~/scripts/hotshot/src/{dbus,tray}.rs` is the
   user's own hand-rolled implementation to adapt. Menu: record/stop, pause,
   record display/window/area, show window, open folder, quit (the `Cmd`s
   the macOS menu bar pushed). Show the clock while recording.
2. **Export speed** (1.1–1.8× real time; Glide is several times faster).
   Costs, per frame: swscale YUV→BGRA on one thread (`Converter` in
   `decode.rs`), GPU readback, a fresh `PixelBuffer` allocation
   (`MovieWriter::buffer`), swscale BGRA→YUV on one thread (`Muxer`). Ideas,
   cheapest first: reuse buffers; do both conversions in parallel bands, or
   in Rust across threads (check `tests/media.rs` still passes); convert on
   the GPU (upload Y/UV planes, convert in a shader; render to NV12);
   NVDEC/Vulkan zero-copy. Measure first.
3. **Camera** (V4L2): `camera::cameras()`, `Source::Camera` in
   `LivePreview`, a camera recording beside the screen (`camera.mov`,
   `meta.camera` with `offset_ms`), then restore "Screen + Cam". No camera
   on the dev machine. libavdevice's v4l2 input is the short path
   (`libavdevice-dev`, ffmpeg-next `device` feature).
4. **Window recording that ignores overlapping windows**: XComposite
   `NameWindowPixmap` + SHM `GetImage` on the pixmap.
5. **Recording controls on a single display**: nothing can hide a window
   from X11 capture. Options: tray only (item 1), or composite the recording
   from per-window pixmaps (large).
6. **Live preview without Glide's own window** (same limitation).
7. **Voice isolation**: RNNoise or DeepFilterNet behind `isolate_voice`.
8. **Whisper on the GPU**: `whisper-rs` `cuda` feature needs the CUDA
   toolkit (not installed).
9. **Follow the desktop's dark/light setting and accent colour.**
10. **Wayland**: xdg-desktop-portal ScreenCast + PipeWire for capture;
    there is no route to global click/key logging, so auto-zoom would need
    another signal.
11. **Remove the leftovers** listed above, if staying close to upstream
    stops mattering: Device variants, `tcc`, `interop`, the macOS selftest.
12. **Rename** binary/app/config dir from glide to drift, if wanted. Touches
    the window class (`with_name`), `config_dir()`, `projects_dir()`, the
    package extension, and every string in the UI.
13. **Packaging**: `.desktop` file, icon (`glide icon` renders one), file
    association for `.glide`.

## Gotchas

- **Exit crash.** A thread still inside the NVIDIA Vulkan driver when the
  process exits segfaults it. `Thumbs` (thumbnail worker) is joined on drop
  for this reason. If you add a background thread that touches the GPU or
  libav, join it before exit.
- **`pkill -f Xvfb` kills your own shell** if its command line contains
  "Xvfb". Use `pkill -x Xvfb`.
- **A locked or sleeping display records black.** Check `xset q` ("Monitor
  is Off") before concluding capture is broken. Don't wake or unlock the
  user's screen; test in Xvfb.
- **Xvfb has no window manager**: no focus, so keyboard input to the app
  and some drags don't arrive. Global hotkeys do (`xdotool key`).
- **Two ffmpeg versions must build**: 4.4 (Ubuntu 22.04) and 8.x (the
  user's newer machine runs 8.1.2). Both were checked: 4.4 natively, 8.1.2
  by building it in a scratch prefix (`./configure --prefix=... --enable-shared
  --disable-programs --disable-x86asm --enable-nvenc --enable-gpl
  --enable-libx264`, NVENC headers from nv-codec-headers tag `n13.0.19.0`;
  git master is too new for 8.x) and pointing `PKG_CONFIG_PATH`,
  `LD_LIBRARY_PATH` and `RUSTFLAGS="-L native=<prefix>/lib"` at it (the
  last because another crate puts the system lib dir first on the link
  line). Rules that keep both working:
  - swscale flag and colour-space values come from `media/sws.rs`, never
    `ffi::SWS_*`: ffmpeg 8 made the flags an enum the bindings name
    differently.
  - Prefer `ffmpeg-next`'s safe API wherever its types change across
    versions (channel layouts, threading config); its `#[cfg]`s handle it.
    `set_channels` and `Config { safe, .. }` don't exist on 7+.
  - Raw `ffi` calls stay limited to swscale and frame functions whose
    signatures haven't changed.
- **The GPU may be shared.** ComfyUI on the dev machine can hold almost
  all of the 3090's memory; then NVENC fails to open ("out of memory",
  "No capable devices") and tests fail with "creating GPU device". Check
  `nvidia-smi` before blaming the code.
- **The setup preview is a mirror** when Glide's window is on the display
  being previewed. Expected.
- Synthetic recordings (`glide synth`, `record --pattern`) need no display.

## Testing recipes

```sh
cargo test --release

# Headless
target/release/glide synth /tmp/demo.glide --tone 880
target/release/glide frame /tmp/demo.glide -t 2 -o /tmp/frame.png
target/release/glide export /tmp/demo.glide -o /tmp/demo.mp4
target/release/glide snapshot /tmp/demo.glide -o /tmp/editor.png   # the UI, offscreen
GLIDE_PROFILE=1 target/release/glide record --pattern -t 3 --dir /tmp/rec --name p

# A private display, without touching the user's settings or screen
Xvfb :77 -screen 0 1920x1080x24 -nolisten tcp &
export DISPLAY=:77
HOME=/tmp/glide-home target/release/glide &          # throwaway settings and recordings
xdotool key ctrl+alt+shift+r                          # start (3 s countdown); again to stop
target/release/glide preview --secs 1 -o /tmp/shot.png   # screenshot of that display
pkill -x Xvfb
```

## Merging upstream

```sh
git fetch upstream
git branch -f upstream upstream/main    # keep the mirror branch current
git merge upstream/main
```

For each conflict in a [Linux file](#linux-files): keep ours
(`git checkout --ours <file>`), then read upstream's diff for that file
(`git diff ORIG_HEAD...upstream/main -- <file>`) and decide whether the
behaviour change needs porting. New upstream files that use `objc2` are
macOS code: delete them and provide a Linux counterpart if shared code calls
them. After merging: `cargo test --release`, a headless export, and update
the baseline commit at the top of this file.
