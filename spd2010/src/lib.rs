//! The SPD2010 TDDI driver IC: a QSPI driver for it, and a software model of
//! the chip that driver can be pointed at.
//!
//! The board this crate is written for is the ESP32-S3 Development Board With
//! 1.46 Round Display 412 x 412 -- Waveshare's ESP32-S3-Touch-LCD-1.46B, a
//! round 412x412 touch IPS module whose display controller is an SPD2010, sold
//! by Solomon Systech as the L-WEA2010. It is a TDDI part -- touch and display
//! driver integration -- so the same die carries the self-capacitance touch
//! controller the board's touch panel uses. Only the display half is modelled
//! here; the touch half speaks I2C or SPI on its own pins and has nothing to
//! do with this bus.
//!
//! Nothing simulates this part, so this crate is both halves of the link:
//!
//! * [`driver::Spd2010`] builds the real command sequence. It is generic over
//!   [`qspi::QspiBus`], `no_std` and dependency-free, so it is the shape the
//!   firmware would take rather than a host-side sketch of it.
//! * [`emu::Spd2010Emulator`] is the other end: it decodes those transfers,
//!   keeps the registers and the display RAM, enforces the restrictions the
//!   datasheet states, and reads back as an image.
//!
//! Because the driver only ever speaks [`qspi::Transaction`], a frame rendered
//! through the emulator has been through the same bytes the panel would see.
//!
//! # Sources
//!
//! * Solomon Systech, *L-WEA2010: 454x454 TDDI for IOT/Wearable*, Product
//!   Preview Rev 0.50, April 2021, 73 pp. Every page citation in this crate is
//!   that document's printed page number. Espressif publish it alongside their
//!   driver, at `dl.espressif.com/AE/esp-iot-solution/SPD2010_L-WEA2010_0.50.pdf`,
//!   linked from the component's own README.
//! * Espressif's `esp_lcd_spd2010` component, v2.0.0~1, from the ESP component
//!   registry -- the vendor's own driver, with the init sequence, the QSPI
//!   opcodes and the four-column rounding rule.
//! * ESPHome's `mipi_spi` model `WAVESHARE-ESP32-S3-TOUCH-LCD-1.46`
//!   (`esphome/components/mipi_spi/models/spd2010.py`, PR #19056) -- the same
//!   init table reached independently, plus this specific board's 412x412 and
//!   its `draw_rounding=4`.
//!
//! A fourth thing that turns up in a search for SPD2010 drivers,
//! `mathcampbell/SPD_2010T`, is the *touch* controller
//! (`SPD2010Touch.h/.cpp`) and says nothing about the display command set. It
//! is noted here so the next reader does not spend an afternoon on it.
//!
//! # This is a different chip from the SH8601Z
//!
//! The SH8601Z is the controller on the round *AMOLED* modules this board is
//! easiest to confuse with, and the two parts rhyme: both are QSPI, both frame
//! a command as `cmd << 8` inside a 24-bit address phase, both take
//! `CASET`/`RASET`/`RAMWR`. Everything past that differs, and the differences
//! are the sort that produce a wrong picture rather than an error -- a
//! four-pixel column alignment this part demands and that one does not, a
//! pixel-format list with no grey mode in it, a different read opcode, a
//! different brightness field, different `MADCTL` bits, and a memory-write
//! rule that forbids what the other part allows. [`crate::cmd`] carries the
//! table. Nothing here is shared with an SH8601Z driver on purpose: a common
//! abstraction would hide exactly the differences that produce the wrong
//! picture.
//!
//! # Scope
//!
//! This is a command-and-memory model, not a hardware simulation. It is exact
//! about what the host sends and where in the address space it lands; it has
//! no notion of time, and does not model MIPI DSI, the touch controller,
//! gamma, CABC, the NVM, or the analogue chain. [`emu`]'s own docs list that
//! boundary in full, including the two places the datasheet contradicts itself
//! or a real driver, and what this model does about each.

#![no_std]
#![forbid(unsafe_code)]

pub mod cmd;
pub mod driver;
pub mod emu;
pub mod pixel;
pub mod qspi;

pub use driver::{align_columns, columns_aligned, Spd2010};
pub use emu::{Fault, Geometry, Spd2010Emulator, COLUMN_ALIGNMENT, PIXEL_RUN_GRANULARITY, RAM_LEN};
pub use pixel::PixelFormat;
pub use qspi::{Lanes, QspiBus, Transaction};
