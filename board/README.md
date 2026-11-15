# On-device reference code

The files here are **reference code for the real ESP32-S3 board, not part of
the build.** No `Cargo.toml` names them, so `cargo build`, `cargo test` and CI
never compile them, and they could not run in the emulator anyway: they talk
to `esp-hal` peripherals (I2C, flash, the radio) that only exist on the chip.

They are trimmed from [Pokewalker](https://github.com/Alchemy86/pokewalker)'s
firmware, which runs on this board, and each one is the version that worked
after the version that did not. Copy them into an `esp-hal` firmware crate and
adjust; the docs say what each line is guarding against.

| File | What | Read |
|---|---|---|
| `qmi8658.rs` | the IMU init that keeps the accelerometer live, the CTRL9 handshake, and polling at 62.5 Hz | [docs/imu-and-steps.md](../docs/imu-and-steps.md) |
| `wifi.rs` | bringing the radio up only when asked, with a deadline on every wait | [docs/wifi.md](../docs/wifi.md) |
| `flash.rs` | opening the flash once and sharing it | [docs/flash-storage.md](../docs/flash-storage.md) |

Versions they were written against: `esp-hal 1.2.2` (`unstable`),
`esp-radio =1.0.0-beta.1` (`wifi`, `unstable`), `esp-rtos 0.4.0`,
`esp-alloc 0.11.0`, `esp-storage 0.10.0`, `smoltcp 0.12`. These APIs move
between releases; check the signatures against the versions you pin.

The parts that *can* run on a desktop are in the examples:
`roundpanel/examples/common/footfall.rs` is the step detector these samples
feed, and `roundpanel/examples/common/chipsynth.rs` is the sound engine.
