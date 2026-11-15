# Steps: the IMU's pedometer never counted, so count them yourself

The board has a QST QMI8658 6-axis IMU on the shared I2C bus (GPIO 11 SDA,
10 SCL, address 0x6B). It has a hardware pedometer. On
[Pokewalker](https://github.com/Alchemy86/pokewalker)'s real board that
pedometer never counted a single step, through a day of trying every
configuration we could find a reference for. The accelerometer underneath it
was fine, and a software step detector on its samples is what worked.

This page is what that day found, so you can skip it. The emulator has no IMU;
the `steps` example runs the detector on a recorded-style trace instead.

## What the chip is

- Waveshare's schematic says **QMI8658C**. The C datasheet (Rev 0.6) has no
  pedometer at all: `CTRL8` is reserved, there is no `STEP_CNT`, and the CTRL9
  "done" bit is `STATUS1` bit 0.
- The part on the board answered `WHO_AM_I 0x05`, **revision `0x7c`, firmware
  `01030e`**, and acknowledged every CTRL9 command on **`STATUSINT` bit 7**:
  the **QMI8658A** command set. Write the driver from the A datasheet (Rev A).
- At rest at +/-4 g it reads about 8,190 counts on whichever axis is down
  (974-1012 mg). That is the quick check that the accelerometer is alive.

## What went wrong, in order

1. **The chip's step count never moved.** A real one-minute walk left
   `STEP_CNT` at 0 while the accelerometer magnitude swung between 31k and 45k
   counts. Every control register read back exactly what was written. Neither
   a gait-friendly parameter set nor SensorLib's own example parameters made it
   count.
2. **The CTRL9 handshake accepted the wrong "done" bit.** It also took
   `STATUS1` bit 0 (the C part's bit, reserved on the A), so a read could run
   before the chip had finished. The sensor initialised and then died at the
   first runtime FIFO drain. Use **`STATUSINT` bit 7 only**, with a deadline on
   every wait.
3. **The FIFO and the data registers are either-or** (datasheet 6.2). With the
   FIFO in stream mode, `AX..AZ` freeze, so anything that polls them must put
   `FIFO_CTRL` in bypass first.
4. **The FIFO and the pedometer cannot run together on this part.** The FIFO
   passed its first few drains and then stopped for good, FIFO and data
   registers both, whenever the step count was also being read. It did that at
   boot and in clean experiments alike. No vendor reference combines them:
   Waveshare's own demo for this board polls the data registers with the
   accelerometer and gyro on (`CTRL7 0x43`), and SensorLib's pedometer example
   runs both at 112 Hz with no FIFO.
5. **So the working init is the vendors' sequence, polled.** Soft reset,
   `CTRL1` auto-increment, `CTRL8 0x80` and `COPY_USID`, FIFO in bypass,
   +/-4 g and 1024 dps at 112 Hz, both low-pass filters, accelerometer *and*
   gyroscope on, then the pedometer configured as SensorLib does it.
   [`board/qmi8658.rs`](../board/qmi8658.rs) is that sequence with every line
   cited to Waveshare's demo, SensorLib or the datasheet.
6. **The accelerometer is read by polling, between display bands.** The frame
   loop is busy pushing the panel, so it reads the six data registers between
   bands, about 55-70 times a second, and a resampler hands each reading on
   once per 16 ms period so the detector sees a steady 62.5 Hz.

The first build that counted real steps on the board was Pokewalker's tag
`steps-working-2026-11-14` (commit `28f461bc`), and it counted them in
software. The chip's count stayed at 0 even then.

## Counting in software, and what a real walk taught

`roundpanel/examples/common/footfall.rs` is the detector, written as device
code: integer maths, no allocation, no `std`. It takes the magnitude of each
sample (so orientation does not matter), smooths it over 192 ms, subtracts a
one-second baseline (gravity and drift go, the bumps stay), and finds peaks
with hysteresis.

The front end was right first time. The gates after it were not:

- **Strict gates dropped most real steps.** The first board build wanted eight
  regular steps in a row before counting anything, each gap within 40% of the
  one before, and reset the run on any bump that came too soon. A real 60-90 s
  walk counted **14 steps**. The board's log showed why: the weaker foot's bumps
  were under the threshold (`small`), real gaps varied more than 40%
  (`irregular`), and a heel strike's echo reset whole runs (`soon`).
- **Lenient footfalls worked.** A threshold off the weaker foot (a quarter of
  the second-smallest recent peak, floor 40 mg); three footfalls near the
  *median* gap to start; then every footfall 0.26-2 s after the last counts, a
  gap of about twice the usual one counts two, and an echo is ignored rather
  than fatal.
- **Shaking is the one thing rejected.** A shaken device crosses its baseline
  on every beat, steadily, faster than about 3.5 Hz. Four such crossings in a
  row end the walk and mute every bump for a second. The first second of a
  shake can still slip a few steps through before the pattern is seen; the
  example shows that honestly.

On the example's synthetic trace (95 true footfalls, plus 6 s of shaking) the
lenient gates count 95 from the walking and 5 from the start of the shake; the
strict gates count 38.

## Rules

1. **Prove the accelerometer is live before blaming anything else.** A dozen
   reads over 200 ms should not all be identical. Log `live`/`STALE` at boot and
   in a heartbeat.
2. **`STATUSINT` bit 7 only for CTRL9, and a deadline on every wait.**
3. **No FIFO with the pedometer on this part.** Poll the data registers, with
   the FIFO in bypass.
4. **Count steps in software, from the magnitude.** Read the chip's count as
   well if you like, but only prefer it once you have seen it move.
5. **Gate leniently and reject shaking specifically.** A walk is irregular; a
   shake is regular.
6. **Prove a change with a real walk, not a shake.** Log, every ten seconds: the
   chip's count, the software count, the read rate, and why candidate peaks were
   dropped (`small`, `irregular`, `secondary`, `shake`). The reasons are what
   tell you which gate is wrong.

## Checking it

```sh
cargo run --release --example steps                    # the detector on the glass
cargo run --release --example steps -- --trace walk.csv
cargo test -p roundpanel --test steps
```

To get a trace off a real board, log the three raw axes at 62.5 Hz over serial
(`ax,ay,az` a line) during a walk, and play it with `--trace`. Score it against
your own count of steps, not against the chip's.

## Where to read more

- Pokewalker: `software/pw-pet/src/stepdetect.rs` and `activity.rs` (the two
  detectors, with host tests that feed them walks shaped like the board's),
  `software/pw-pet/src/qmi8658.rs` (the CTRL9 protocol against a fake chip that
  wedges on any protocol violation) and `software/pw-pet-firmware/src/imu.rs`.
- Waveshare, `waveshareteam/ESP32-S3-Touch-LCD-1.46`,
  `example/ESP-IDF-5.3.2/.../main/QMI8658/QMI8658.c`.
- SensorLib, `examples/sensor/qmi8658_pedometer/qmi8658_pedometer.ino`.
