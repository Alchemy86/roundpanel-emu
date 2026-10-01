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
  both examples for Linux/macOS/Windows and attaches them to a GitHub Release on
  a pushed `v*` tag; its `publish-crate` job actually runs `cargo publish -p
  spd2010` on a tag but only fires once a `CARGO_REGISTRY_TOKEN` repo secret
  exists -- that secret is the one manual step left to turn on real publishing.

## Sharp edges

- The framebuffer is square and the glass is the inscribed circle. Anything
  drawn in the corners is addressable and invisible on the hardware.
- `Frame::clear` deliberately ignores `clip_to_glass`; every other draw call
  honours it.
- The logo is generated, never hand-edited: `python3 brand/make.py` with
  [Glyphsmith](https://github.com/Alchemy86/Glyphsmith) installed.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.
