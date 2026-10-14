# drift

A screen recorder for Linux that makes the recording look finished.
Record the screen, a window or an area; drift draws the cursor again
larger and smoother, zooms into what you click, sets the picture on a
background with a shadow and rounded corners, and exports MP4 or GIF. It
is a port to X11 of [Glide](https://github.com/michaelshimeles/glide), by
Michael Shimeles, which does this on macOS: the engine, the renderer and
the editor are Glide's as they are, and what reads the screen, encodes it
and talks to the desktop is written for Linux.

```sh
glide                              # the app: record, then edit and export
glide record -t 10                 # the main display, for ten seconds
glide record --area 200,150,1280,720 --mic
glide export Recording.glide --height 1080
```

The program and its `.glide` projects keep Glide's name.

## Install

It needs X11, a GPU with Vulkan, CMake and ffmpeg's libraries. On Debian
or Ubuntu:

```sh
sudo apt install cmake libavcodec-dev libavformat-dev libavutil-dev \
  libswscale-dev libswresample-dev libasound2-dev
cargo build --release
cp target/release/glide ~/.local/bin/
```

## Use

Press Record, or Ctrl+Alt+Shift+R from anywhere; the same key stops it,
and the recording opens in the editor. The shortcuts are set under
Global shortcuts. Recordings are kept in `~/Videos/Glide`, settings in
`~/.config/glide`.

It records with the GPU's encoder (NVENC) when there is one and x264
when there is not; `GLIDE_ENCODER=software` asks for x264. What the
computer plays and the microphone are heard through PulseAudio, each on
a track of its own.

```sh
glide devices                      # displays, windows, microphones
glide export Project.glide --format gif
glide transcribe Project.glide     # captions, with Whisper
glide resize-window 1234 1920x1080 # a window id from `glide devices`
glide synth demo.glide             # a made-up recording to try the editor on
```

`glide --help` lists the rest.

## Not as on macOS

X11 cannot leave a window out of what is read from the screen. So the
recording's controls are put up only where they will not be recorded,
beside an area or a window or on another display, and a whole display is
stopped with the key; the preview before recording shows drift's own
window in it; and a window is recorded as the part of the screen it is
on, with whatever is over it.

There is no tray icon yet and no camera, and exporting is slower than
Glide's. Recording an iPhone, Apple's speech recognition and voice
isolation are not here and will not be.

## Glide

`docs/PORT.md` says what differs from Glide and what is left to do. This
repository keeps Glide's history, so its changes can be merged:

```sh
git remote add upstream https://github.com/michaelshimeles/glide
git fetch upstream
git merge upstream/main
```

Glide's behaviour was worked out from Screen Studio; neither it nor
drift is affiliated with Screen Studio or contains its code or assets.
Captions use [whisper.cpp](https://github.com/ggml-org/whisper.cpp); the
typeface is Inter, by Rasmus Andersson.

MIT or Apache-2.0, as Glide is.
