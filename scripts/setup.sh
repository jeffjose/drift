#!/bin/sh
# Install what building and running drift needs on Debian or Ubuntu, and
# say what's still missing elsewhere. Safe to run again: it only installs
# packages that aren't there. Usage: scripts/setup.sh [--check]
set -eu

# Build: a C toolchain and CMake for whisper.cpp, libclang for the
# bindings to ffmpeg and whisper.cpp, pkg-config to find the libraries,
# and the headers of ffmpeg's libraries and ALSA.
# Run: the Vulkan loader and PulseAudio's client library.
PACKAGES="build-essential pkg-config cmake libclang-dev
libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev
libasound2-dev libvulkan1 libpulse0"

if ! command -v dpkg-query >/dev/null 2>&1; then
  echo "Not a Debian-based system. Install the equivalents of:"
  echo "  $PACKAGES" | tr '\n' ' '
  echo
  exit 1
fi

missing=""
for p in $PACKAGES; do
  if ! dpkg-query -W -f '${Status}' "$p" 2>/dev/null | grep -q "install ok installed"; then
    missing="$missing $p"
  fi
done

if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust isn't installed: see https://rustup.rs"
  rust_missing=1
else
  rust_missing=0
fi

if [ -z "$missing" ]; then
  echo "All system packages are installed."
elif [ "${1:-}" = "--check" ]; then
  echo "Missing:$missing"
  echo "Install them with: sudo apt install$missing"
  exit 1
else
  echo "Installing:$missing"
  sudo apt-get update
  # shellcheck disable=SC2086
  sudo apt-get install -y $missing
fi

[ "$rust_missing" = 0 ] || exit 1
echo "Ready: cargo build --release"
