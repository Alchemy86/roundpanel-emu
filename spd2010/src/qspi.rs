//! The Quad-SPI transport, as the datasheet draws it.
//!
//! Section 6.2.4 "QSPI Timing" (p.15), Figures 6-10 to 6-12, define every
//! transfer as the same four-part shape: an 8-bit opcode, a 24-bit address, and
//! then the payload -- with the opcode and address always on SDA alone, and
//! only the payload optionally spread across all four lanes.
//!
//! The 24-bit address is not a memory address. Figure 6-10 draws it as three
//! bytes, `0x00`, `cmd[7:0]`, `0x00`: the command sitting in bits 15:8 with
//! zero above and below. Espressif's driver builds it exactly that way and
//! nothing else:
//!
//! ```text
//! esp_lcd_spd2010.c, tx_param():
//!     lcd_cmd &= 0xff;  lcd_cmd <<= 8;  lcd_cmd |= LCD_OPCODE_WRITE_CMD << 24;
//!
//! esp_lcd_spd2010.c, tx_color():
//!     lcd_cmd &= 0xff;  lcd_cmd <<= 8;  lcd_cmd |= LCD_OPCODE_WRITE_COLOR << 24;
//! ```
//!
//! and its QSPI panel-IO config sets `lcd_cmd_bits = 32` to make room for it.
//! So a command write is `02h`, `00h CCh 00h`, parameters; and a pixel write is
//! `32h`, `00h 2Ch 00h`, pixels across four lanes.
//!
//! That framing is the same as the SH8601Z's, and it is the *only* part of
//! this crate that is. The read opcode already differs -- see [`opcode`].
//!
//! This is a transaction-level model, not a wire-level one. It carries what a
//! QSPI peripheral is told to send -- opcode, address, lane width, bytes -- and
//! not the clock edges underneath. That boundary is deliberate and is the same
//! one `embedded-hal` and `esp_lcd_panel_io` draw: it is the narrowest place a
//! real driver can be cut from its bus, so a driver written against
//! [`QspiBus`] is the driver, not a host-only imitation of one.
//!
//! One consequence of drawing it there: SPI mode is not modelled, and the two
//! real drivers disagree about it. Espressif's `SPD2010_PANEL_IO_QSPI_CONFIG`
//! sets `.spi_mode = 3`; ESPHome's model for this board declares
//! `spi_mode="MODE0"`. They also disagree on clock rate -- 20 MHz against
//! 40 MHz, where the datasheet's own limit is "Data rate up to 40Mbps" (p.7).
//! Neither is observable above the wire, so neither is resolved here; it is
//! recorded because bring-up on real hardware will have to pick one.

/// Opcodes, section 6.2.4, Figures 6-10 to 6-12 (p.15), confirmed against
/// `esp_lcd_spd2010.c`:
///
/// ```text
/// #define LCD_OPCODE_WRITE_CMD        (0x02ULL)
/// #define LCD_OPCODE_READ_CMD         (0x0BULL)
/// #define LCD_OPCODE_WRITE_COLOR      (0x32ULL)
/// ```
pub mod opcode {
    /// Command write: opcode, 24-bit address, parameters -- all on SDA.
    pub const WRITE_CMD: u8 = 0x02;
    /// Command read: opcode, 24-bit address, a dummy byte, then the chip
    /// drives the bus (Figure 6-12).
    ///
    /// **This is `0Bh`, not the `03h` an SH8601Z uses.** A host that reuses a
    /// SH8601 read routine here sends `03h`, which on this part is not a read
    /// opcode at all. The datasheet's figure and Espressif's
    /// `LCD_OPCODE_READ_CMD` agree on `0Bh`.
    pub const READ_CMD: u8 = 0x0B;
    /// Pixel write with the payload across four lanes, address on SDA.
    pub const WRITE_COLOUR: u8 = 0x32;
}

/// How many data lanes the payload of a transfer uses.
///
/// The opcode and address are always single-lane; this describes the payload
/// only, which is the same split `DataMode` makes in `esp-hal` and
/// `esp_lcd_panel_io_spi`'s quad flag. Figure 6-10 is the one-lane payload and
/// Figure 6-11 the four-lane one; unlike the SH8601Z, this chip attaches no
/// pixel-format restriction to the choice, because it has no formats that are
/// 1-wire only.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Lanes {
    /// SDA only.
    Single,
    /// SDA / D1 / D2 / D3.
    Quad,
}

/// One chip-select-low to chip-select-high transfer.
#[derive(Copy, Clone, Debug)]
pub struct Transaction<'a> {
    /// The 8-bit opcode, one of [`opcode`].
    pub opcode: u8,
    /// The 24-bit address phase. Only the low 24 bits are transmitted.
    pub address: u32,
    /// How the payload is spread across the data lanes.
    pub lanes: Lanes,
    /// The payload: command parameters, or pixel data.
    pub data: &'a [u8],
}

impl<'a> Transaction<'a> {
    /// A command write: `02h`, `cmd << 8`, `params` on one lane.
    pub const fn command(cmd: u8, params: &'a [u8]) -> Self {
        Transaction {
            opcode: opcode::WRITE_CMD,
            address: (cmd as u32) << 8,
            lanes: Lanes::Single,
            data: params,
        }
    }

    /// A pixel write: `32h`, `cmd << 8`, `pixels` across four lanes.
    ///
    /// `cmd` is RAMWR (2Ch) to start a window and RAMWRC (3Ch) to continue it.
    /// On this chip that split is not an optimisation for large frames, it is
    /// required: see [`crate::emu::Fault::WholeFrameInOneRamwr`].
    pub const fn pixels(cmd: u8, pixels: &'a [u8]) -> Self {
        Transaction {
            opcode: opcode::WRITE_COLOUR,
            address: (cmd as u32) << 8,
            lanes: Lanes::Quad,
            data: pixels,
        }
    }

    /// A read: `0Bh`, `cmd << 8`, and then the chip answers.
    ///
    /// Provided so a driver can be written against the real opcode. The
    /// emulator answers none of them -- see [`crate::emu::Fault::ReadNotSupported`].
    pub const fn read(cmd: u8) -> Self {
        Transaction {
            opcode: opcode::READ_CMD,
            address: (cmd as u32) << 8,
            lanes: Lanes::Single,
            data: &[],
        }
    }

    /// The command byte carried in the address phase: `ADDR[15:8]`.
    pub const fn cmd(&self) -> u8 {
        (self.address >> 8) as u8
    }
}

/// A QSPI master a driver can talk through.
///
/// Implemented here by [`crate::emu::Spd2010Emulator`]; on a microcontroller it
/// would be implemented over that part's QSPI peripheral, which is the whole
/// point of the trait.
pub trait QspiBus {
    type Error;

    fn transfer(&mut self, t: &Transaction<'_>) -> Result<(), Self::Error>;
}

impl<T: QspiBus + ?Sized> QspiBus for &mut T {
    type Error = T::Error;

    fn transfer(&mut self, t: &Transaction<'_>) -> Result<(), Self::Error> {
        (**self).transfer(t)
    }
}
