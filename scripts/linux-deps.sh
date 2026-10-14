#!/bin/sh
# Linux build dependencies without root: fetch the -dev packages (headers,
# pkg-config files) and unpack them under ~/.cache/glide-sysroot. The shared
# libraries themselves come with the system's ffmpeg and ALSA.
#
# With root, this does the same job and this script isn't needed:
#   sudo apt install libavcodec-dev libavformat-dev libavutil-dev \
#     libswscale-dev libswresample-dev libasound2-dev
set -eu
ROOT="${GLIDE_SYSROOT:-$HOME/.cache/glide-sysroot}"
LIBDIR=/usr/lib/x86_64-linux-gnu
PKGS="libavcodec-dev libavformat-dev libavutil-dev libswscale-dev libswresample-dev libasound2-dev"
mkdir -p "$ROOT/debs"
cd "$ROOT/debs"
apt-get download $PKGS
for deb in *.deb; do dpkg -x "$deb" "$ROOT"; done
# The unpacked libfoo.so links are relative and dangle here: point them at
# the system's libraries.
for link in "$ROOT$LIBDIR"/*.so; do
  name="$(basename "$link")"
  real="$(ls "$LIBDIR/$name".* 2>/dev/null | head -1)"
  if [ -n "$real" ]; then ln -sf "$real" "$link"; fi
done
# And the pkg-config files say /usr: say where they were unpacked.
for pc in "$ROOT$LIBDIR"/pkgconfig/*.pc; do
  sed -i -e "s|^prefix=/usr|prefix=$ROOT/usr|" -e "s|^libdir=/usr|libdir=$ROOT/usr|" -e "s|^includedir=/usr|includedir=$ROOT/usr|" "$pc"
done
echo "unpacked into $ROOT (scripts/env.sh picks it up)"
