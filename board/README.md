# On-device reference code

The files here are **reference code for the real ESP32-S3 board, not part of
the build.** No `Cargo.toml` names them, so `cargo build`, `cargo test` and CI
never compile them, and they could not run in the emulator anyway: they talk
to `esp-hal` peripherals (I2C, SPI, flash, the radio, the second core) that only exist on the chip.

They are trimmed from [Pokewalker](https://github.com/Alchemy86/pokewalker)'s
firmware, which runs on this board, and each one is the version that worked
after the version that did not. Copy them into an `esp-hal` firmware crate and
adjust; the docs say what each line is guarding against.

| File | What | Read |
|---|---|---|
| `qmi8658.rs` | the IMU init that keeps the accelerometer live, the CTRL9 handshake, and polling at 62.5 Hz | [docs/imu-and-steps.md](../docs/imu-and-steps.md) |
| `wifi.rs` | bringing the radio up only when asked, with a deadline on every wait | [docs/wifi.md](../docs/wifi.md) |
| `flash.rs` | opening the flash once and sharing it | [docs/flash-storage.md](../docs/flash-storage.md) |
| `present.rs` | core 1 sending each frame while core 0 draws the next: the mailbox, the colour table and the half-band DMA pipeline | [docs/two-cores.md](../docs/two-cores.md) |
| `lend.rs` | lending core 1 the I2C bus so touch and the IMU are read while core 0 draws | [docs/two-cores.md](../docs/two-cores.md) |
| `rows.rs` | both cores taking rows of one picture from a shared counter, and when not to | [docs/two-cores.md](../docs/two-cores.md) |

Versions they were written against: `esp-hal 1.2.2` (`unstable`),
`esp-radio =1.0.0-beta.1` (`wifi`, `unstable`), `esp-rtos 0.4.0`,
`esp-alloc 0.11.0`, `esp-storage 0.10.0`, `smoltcp 0.12`, `allocator-api2`.
`present.rs` needs `Spd2010::bus_mut`, which `spd2010` gained after 0.1.0. These APIs move
between releases; check the signatures against the versions you pin.

The parts that *can* run on a desktop are in the examples:
`roundpanel/examples/common/footfall.rs` is the step detector these samples
feed, `roundpanel/examples/common/chipsynth.rs` is the sound engine, and
`roundpanel/examples/common/colourtable.rs` is the colour table `present.rs`
builds, tested against per-pixel encoding.
