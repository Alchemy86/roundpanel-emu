# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

## What this is

A desktop emulator for the Waveshare ESP32-S3-Touch-LCD-1.46B (412 x 412 round
SPD2010 panel). `spd2010/` is `no_std`, dependency-free and device-bound;
`roundpanel/` is the host harness and is the only crate that may depend on a
window, `std` or `png`. Do not let the two merge -- the driver being the same
code on the desktop and on the board is the whole claim.

## Working on it

- `cargo test` is the whole check: `spd2010/tests/protocol.rs` is 36 datasheet
  checks and every one names the page or source line it came from. Never relax
  one -- if it fails, either the code is wrong or the page was misread.
- `cargo run --release --example test-pattern -- --shot out.png` renders without
  a display. Use it rather than trying to drive a window; there is no mouse to
  synthesise here.
- The panel present costs about 1.1 ms a frame at 412x412 Rgb888 on this
  machine (`cargo test --release --test headless -- --nocapture` prints it).
  If that number moves a lot, something started copying frames.

## Publishing and CI

- `spd2010` is published to crates.io (`publish = true`, real keywords/categories
  in `spd2010/Cargo.toml`); `roundpanel` stays git-only (`publish = false`) since
  the README already tells people to depend on it via git and it's the harness,
  not the reusable half. `.github/workflows/ci.yml` runs build/test/clippy/fmt
  plus `cargo publish --dry-run -p spd2010` on every push and PR, so a metadata
  break is caught before a real publish. `.github/workflows/release.yml` builds
  every example for Linux/macOS/Windows and attaches them to a GitHub Release on
  a pushed `v*` tag; its `publish-crate` job actually runs `cargo publish -p
  spd2010` on a tag but only fires once a `CARGO_REGISTRY_TOKEN` repo secret
  exists -- that secret is the one manual step left to turn on real publishing.

## Sharp edges

- The framebuffer is square and the glass is the inscribed circle. Anything
  drawn in the corners is addressable and invisible on the hardware.
- The emulator is right about pixels and wrong about time: the board is ~150x
  slower per step than a desktop and nothing here emulates sound. Never judge a
  hot loop (especially audio) by how it runs here; see
  `docs/timing-and-audio.md`.
- The board's IMU, radio and flash are not emulated. What we learned about
  them is in `docs/imu-and-steps.md` (the QMI8658's own pedometer never
  counted; no FIFO with the pedometer; CTRL9 on `STATUSINT` only),
  `docs/wifi.md` (no radio at boot, internal heap before PSRAM, event channel
  16/4, deadlines) and `docs/flash-storage.md` (open the flash once).
  `board/*.rs` is on-device reference code that nothing compiles; keep it
  out of every `Cargo.toml` and say so in each file's header.
- `roundpanel/examples/common/footfall.rs` and `walktrace.rs` are shared by the
  `steps` example and `roundpanel/tests/steps.rs`. The shipped trace
  (`examples/data/walk-trace.csv`) is synthetic; regenerate it with
  `--make-trace`, never edit it by hand.
- `roundpanel/examples/common/chipsynth.rs` is shared by the `chiptune`
  example and `roundpanel/tests/chiptune.rs` (via `#[path]`). Any change to the
  synth must keep that test's checksums equal with the skip and the batching on
  and off; `--board-projection` is the cost check.
- `Frame::clear` deliberately ignores `clip_to_glass`; every other draw call
  honours it.
- The logo is generated, never hand-edited: `python3 brand/make.py` with
  [Glyphsmith](https://github.com/Alchemy86/Glyphsmith) installed.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
