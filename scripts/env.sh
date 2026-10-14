# Shared build environment for the scripts (sourced, from the repo root).
# Keep build output outside the repo.
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cache/cargo-target/screenstudio-rs}"
# whisper.cpp is built with CMake: use the active Xcode's own SDK so its
# linker can read it (a newer Command Line Tools SDK may not be readable).
if [ -z "${SDKROOT:-}" ]; then
  sdk="$(xcode-select -p 2>/dev/null)/Platforms/MacOSX.platform/Developer/SDKs/MacOSX.sdk"
  if [ -d "$sdk" ]; then export SDKROOT="$sdk"; fi
fi
# Linux: -dev packages unpacked by scripts/linux-deps.sh, if they're there.
sysroot="${GLIDE_SYSROOT:-$HOME/.cache/glide-sysroot}"
if [ -d "$sysroot/usr/lib/x86_64-linux-gnu/pkgconfig" ]; then
  export PKG_CONFIG_PATH="$sysroot/usr/lib/x86_64-linux-gnu/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
fi
