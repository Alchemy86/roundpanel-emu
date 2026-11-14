# Time and sound: what the emulator cannot tell you

The emulator runs your code natively on your desktop. That makes it right about
pixels and wrong about time. Code that costs nothing here can take the whole
frame on the board, and the place that shows it first is sound, because audio
cannot pause and wait for you.

This page is what we learned putting music and sound effects on this board for
[Pokewalker](https://github.com/Alchemy86/pokewalker), where a battle's music
stuttered on the hardware while sounding perfect on the desktop.

## The gap is about 150x

The board is an ESP32-S3: two Xtensa cores at 240 MHz, no hardware floating
point worth relying on in a hot loop, no hardware integer divide. Measured on
the same audio code in the same state, one output frame cost **248.8 ns** on a
current desktop and **37.1 us** on the board: roughly **150 times** slower.

So an estimate made on the desktop and scaled by a guessed factor is not good
enough. Our first estimate was six times too optimistic. Calibrate the ratio
against a real board measurement, and re-measure it when your desktop changes.

## What went wrong

The game plays Game Boy music by running a model of the Game Boy's sound chip
at 16 kHz. After a sound effect finished, its noise channel was left at volume
0, silent, but the model kept stepping that channel's timer every 8 cycles for
the rest of the battle. Chip steps per output sample went from 2.84 to 35.3.

- **On the desktop:** nothing audible, nothing measurable without looking.
- **On the board:** 855 sound calls over 5 ms and 202 audio underruns in 90
  seconds. The music stuttered from the first attack onward.

## The rules it left

1. **A silent voice does no work.** If a channel is off, or at volume 0 with an
   envelope that cannot rise, stop advancing it until something retriggers it.
   This is only exact if a retrigger resets the state you skipped; check that
   for your model, and prove the output is unchanged with a render checksum.
2. **Nothing in the per-sample loop that the core can't do cheaply.** No
   division (it becomes a software routine), no 64-bit arithmetic, no floats.
   Precompute reciprocals; batch runs of identical steps.
3. **Give every audio call a time budget.** Mix ahead in small slices (we use
   128 frames) and stop when the budget is spent, rather than filling a whole
   buffer in one call.
4. **Keep audio off the render loop.** The ESP32-S3 has a second core. Run the
   I2S output and the mixer there, and let the drawing code only push commands
   onto a lock-free single-producer/single-consumer queue. A slow frame can't
   starve the audio and the audio can't freeze a frame.
5. **Watch the right numbers on the board.** Log, once a second: underruns, how
   far ahead the mixer is (lead time), the worst single call, and the audio
   core's busy share. Ours, after the fix: 0 underruns, about 1.3 ms per call,
   about 256 ms of lead, 8-11% of the audio core.

## Checking a change before it reaches the board

- Benchmark the exact audio code on the desktop and multiply by your calibrated
  ratio. Fail the test if the projection exceeds your budget.
- Then flash it and read the board's own log while the music plays. Only the
  board's numbers count.

## Where to read more

- Pokewalker: `software/README.md`, "Sound: the window cannot tell you what the
  board can afford", and the code in `software/pw-pet/src/chip.rs`,
  `chiptrack.rs`, `mixahead.rs` and `software/pw-pet-firmware/src/sound.rs`.
- TerminalGB, [`docs/embedded-audio.md`](https://github.com/Alchemy86/TerminalGB/blob/main/docs/embedded-audio.md):
  the longer write-up, and why the same pattern is harmless in an accurate
  emulator.
