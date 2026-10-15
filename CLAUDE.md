# Drift

Linux/X11 port of Glide (macOS screen recorder and editor). Read
`docs/PORT.md` before changing anything: it records how this tree differs
from upstream, what is and isn't verified, the open work, and the traps.
Update it in the same commit as any change it describes.

- Build and test: `cargo build --release`, `cargo test --release`.
  System packages: `scripts/setup.sh` (`--check` to only report). A new
  build dependency that needs a system library goes in its package list.
- Binary: `target/release/glide` (the app keeps the Glide name).
- Engine, renderer and editor UI stay 1:1 with upstream; platform work goes
  in the Linux files listed in `docs/PORT.md`.
- Test GUI changes in Xvfb with a throwaway `HOME`, not on the user's
  desktop (recipe in `docs/PORT.md`).
- `upstream` remote and branch are Glide; `main` is Drift.
