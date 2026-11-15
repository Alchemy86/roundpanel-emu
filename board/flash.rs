// REFERENCE CODE FOR THE REAL BOARD -- not compiled by this workspace.
// See board/README.md and docs/flash-storage.md.
//
// Open the flash ONCE and share it. On a real board a second
// `FlashStorage::new`, made after an OTA updater had read the flash, detected
// the chip's size again over SPI1, came out too small, and every later save
// access failed `OutOfBounds`: the save simply looked absent, with no error at
// the point of the mistake.
//
// esp-storage 0.10.0, esp-hal 1.2.2. `FlashStorage::new` takes the real
// `p.FLASH` peripheral (older examples showing a no-argument `new()` are a
// different build of the crate).

use embedded_storage::{ReadStorage, Storage};
use esp_storage::FlashStorage;

/// What the module really has: 16 MB on the ESP32-S3-Touch-LCD-1.46B.
const FLASH_BYTES: usize = 16 * 1024 * 1024;

pub struct Nvs {
    flash: FlashStorage<'static>,
    usable: bool,
}

impl Nvs {
    pub fn new(flash: esp_hal::peripherals::FLASH<'static>) -> Self {
        // `multicore_auto_park`: if core 1 is running (the audio engine in
        // `roundpanel/examples/chiptune.rs` would be), it is parked around
        // each write, because code running from flash on the other core
        // during a write is a crash.
        let flash = FlashStorage::new(flash).multicore_auto_park();
        // Check the size it detected and refuse to touch the save if it is
        // wrong, rather than let a mis-sized handle fail every access later.
        let usable = flash.capacity() >= FLASH_BYTES;
        Nvs { flash, usable }
    }

    /// The one handle, for anything else that needs flash (an updater, a
    /// second record). Never make another `FlashStorage`.
    pub fn flash(&mut self) -> &mut FlashStorage<'static> {
        &mut self.flash
    }

    pub fn read(&mut self, offset: u32, buf: &mut [u8]) -> bool {
        self.usable && self.flash.read(offset, buf).is_ok()
    }

    pub fn write(&mut self, offset: u32, data: &[u8]) -> bool {
        self.usable && self.flash.write(offset, data).is_ok()
    }
}

// In main(), before anything else touches flash:
//
//     let mut nvs = Nvs::new(p.FLASH);
//     let ota = OtaHost::boot(nvs.flash(), ...);   // borrows the same handle
//     let saved = load_save(&mut nvs);
//
// Two more things the save on this board needed:
//   * There is no NVS component under a bare-metal esp-hal build: the `nvs`
//     partition is just bytes. Write your own record with a version, a
//     sequence number and a checksum, ping-ponged across two 4 KB sectors so
//     a power cut mid-write leaves the other copy whole.
//   * An unreadable save is not a fresh device. Only write over the sectors
//     once they have been read without error, or one bad boot erases the
//     player's progress.
