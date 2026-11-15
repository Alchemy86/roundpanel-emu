# Flash storage: open it once

Saving to the board's 16 MB flash with `esp-storage` on bare-metal `esp-hal`
(no ESP-IDF, so no NVS library) works, with one trap that cost
[Pokewalker](https://github.com/Alchemy86/pokewalker) a save that looked
missing on a real board.

## What went wrong

The firmware opened the flash twice: once for an over-the-air updater early in
boot, and again, later, for the save. The second `FlashStorage::new` detected
the chip's size again over SPI1 after the updater's reads, **came out too
small, and every save access then failed `OutOfBounds`.** Nothing failed at
the point of the mistake; the save just read as absent. Fixed by opening the
flash exactly once and lending that one handle to everything (Pokewalker
`85ec97e5`).

## The rules

1. **One `FlashStorage`, made first, shared.** Anything else that needs flash
   borrows it.
2. **Check `capacity()` and refuse to touch the save if it is wrong.** It costs
   one comparison and turns a silent misread into a log line.
3. **Park the other core around writes** (`multicore_auto_park()`) if anything
   runs on core 1, such as an audio engine.
4. **Write your own record.** The `nvs` partition is just bytes here. Give the
   record a version, a sequence number and a checksum, and ping-pong it across
   two 4 KB sectors so a power cut mid-write leaves the other copy whole.
5. **An unreadable save is not a fresh device.** Only write over the sectors
   once they have been read without error.
6. **Save on events, not every frame.** Mark the state dirty when something
   worth keeping changes and write it once a pass. Flash wears.

[`board/flash.rs`](../board/flash.rs) is the reference code (not built by this
repo; it needs the real chip). Pokewalker's `software/pw-pet-firmware/src/save.rs`
is the full version.
