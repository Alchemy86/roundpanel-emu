//! The driver: what a firmware would send this panel.
//!
//! It is generic over [`QspiBus`] and knows nothing about who is on the other
//! end, which is the only way the emulator proves anything -- the same code
//! path drives a model here and a real QSPI peripheral on a board.
//!
//! The initialisation order is Espressif's, which is the vendor's own:
//!
//! ```text
//! esp_lcd_spd2010.c, panel_spd2010_init():
//!     tx_param(.., SPD2010_CMD_SET, {0x20, 0x10, 0x00}, 3);   // user set
//!     tx_param(.., LCD_CMD_MADCTL,  {madctl_val}, 1);
//!     tx_param(.., LCD_CMD_COLMOD,  {colmod_val}, 1);
//!     ...then the vendor table, which ends {0x11, .., 0, 120}  // SLPOUT
//! ```
//!
//! ESPHome's model for this board sends the same vendor table and leaves sleep
//! out to its own framework. The two agree on everything except where `SLPOUT`
//! lives, so this driver sends it itself, after the table, where Espressif's
//! table puts it.
//!
//! The vendor table itself is not reproduced here. It is roughly 380 register
//! writes across command pages `10h`, `11h`, `12h`, `18h` and `2Dh` -- gamma,
//! GIP timing and power settings -- and both sources carry it with the same
//! comment, that it "can be different between manufacturers" and one should
//! "consult the LCD supplier". It is panel data, not chip data, and nothing in
//! it is documented anywhere public. [`Spd2010::vendor_page`] and
//! [`Spd2010::user_page`] are how a caller sends its own.

use crate::cmd::{cmdset, colmod, madctl, op, wrctrld};
use crate::emu::{pixel_lanes, COLUMN_ALIGNMENT, PIXEL_RUN_GRANULARITY};
use crate::pixel::PixelFormat;
use crate::qspi::{QspiBus, Transaction};

/// The largest run of pixels the driver puts in one transfer before continuing
/// with `RAMWRC`.
///
/// A multiple of four, as section 6.5 (p.16) requires of every memory-write
/// transfer, and small enough that even at three bytes a pixel the payload
/// (12,288 bytes) stays inside the 16,380-byte DMA limit an ESP32-S3 imposes
/// -- the same limit `sh8601-rs` picks its chunk size for. Sizing in *pixels*
/// rather than bytes is deliberate: the rule the chip states is about pixels,
/// and a byte-sized chunk silently stops being a multiple of four pixels when
/// the format changes.
pub const DEFAULT_CHUNK_PIXELS: usize = 4096;

/// A panel on a bus.
pub struct Spd2010<B> {
    bus: B,
    format: PixelFormat,
    chunk_pixels: usize,
}

impl<B: QspiBus> Spd2010<B> {
    pub fn new(bus: B) -> Self {
        Spd2010 {
            bus,
            // COLMOD's own reset default, p.57, until init says otherwise.
            format: PixelFormat::Rgb888,
            chunk_pixels: DEFAULT_CHUNK_PIXELS,
        }
    }

    /// Override the transfer chunk size, in pixels.
    ///
    /// Panics on a size the chip's own rule forbids, because that is a
    /// firmware bug and not a runtime condition.
    pub fn with_chunk_pixels(mut self, pixels: usize) -> Self {
        assert!(
            pixels >= PIXEL_RUN_GRANULARITY && pixels % PIXEL_RUN_GRANULARITY == 0,
            "a memory write is at least 4 pixels and a multiple of 4 (p.16)"
        );
        self.chunk_pixels = pixels;
        self
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn release(self) -> B {
        self.bus
    }

    /// Send a command with no parameters.
    pub fn cmd(&mut self, cmd: u8) -> Result<(), B::Error> {
        self.bus.transfer(&Transaction::command(cmd, &[]))
    }

    /// Send a command with parameters.
    pub fn cmd_with(&mut self, cmd: u8, params: &[u8]) -> Result<(), B::Error> {
        self.bus.transfer(&Transaction::command(cmd, params))
    }

    /// Select a vendor command page: `FFh 20h 10h <page>`.
    ///
    /// While one is selected the user command set of Table 13-1 does not
    /// apply. [`Self::user_page`] is how to get back.
    pub fn vendor_page(&mut self, page: u8) -> Result<(), B::Error> {
        self.cmd_with(op::CMD_SET, &[cmdset::MAGIC[0], cmdset::MAGIC[1], page])
    }

    /// Select the user command set: `FFh 20h 10h 00h`.
    pub fn user_page(&mut self) -> Result<(), B::Error> {
        self.vendor_page(cmdset::USER)
    }

    /// Bring the panel up in `format` and turn it on.
    ///
    /// No delays are issued: a `no_std` driver has no clock of its own, and
    /// the ones the datasheet requires -- 5 ms after `SWRESET` and 120 ms
    /// between sleep transitions (pp.32, 41-42) -- belong to whatever calls
    /// this. On the model they are not observable; on hardware they must be
    /// honoured, and the caller is where the timer is.
    ///
    /// `vendor_table` is the panel maker's register dump, sent between
    /// `COLMOD` and `SLPOUT` exactly where both real drivers send theirs. Pass
    /// an empty slice to bring the chip up on its own defaults; the model does
    /// not care, and a real panel will show a picture of the wrong gamma.
    /// Each entry is `(command, parameters)` and may include the `FFh` page
    /// switches, which is how the table reaches the vendor registers at all.
    pub fn init(
        &mut self,
        format: PixelFormat,
        vendor_table: &[(u8, &[u8])],
    ) -> Result<(), B::Error> {
        self.user_page()?;
        // MADCTL 00h: RGB order, no flips. p.53.
        self.cmd_with(op::MADCTL, &[madctl::RESET_DEFAULT])?;
        self.cmd_with(op::COLMOD, &[format.to_colmod()])?;
        self.format = format;
        for (cmd, params) in vendor_table {
            self.cmd_with(*cmd, params)?;
        }
        // Whatever the table did with the page, the user set is what the rest
        // of this driver speaks. Espressif's table ends on `FFh 20h 10h 00h`
        // for the same reason; sending it again is harmless and means a
        // caller's table does not have to remember.
        self.user_page()?;
        self.cmd(op::SLPOUT)?;
        // Brightness control on, dimming off, backlight on. p.62.
        self.cmd_with(op::WRCTRLD, &[wrctrld::BCTRL | wrctrld::BL])?;
        self.set_brightness(0x3FFF)?;
        self.cmd(op::DISPON)
    }

    /// `WRDISBV` (51h), p.61. Values above `3FFFh` are clamped to it.
    ///
    /// The field is `DBV[13:0]`, sent as `DBV[13:6]` and then `0 0 DBV[5:0]`.
    /// Note the order: the high bits go first here, where the SH8601Z's
    /// ten-bit `DBV` goes low byte first. Reusing that driver's two bytes on
    /// this chip sets a brightness roughly 64 times too low.
    pub fn set_brightness(&mut self, dbv: u16) -> Result<(), B::Error> {
        let v = dbv.min(0x3FFF);
        self.cmd_with(op::WRDISBV, &[(v >> 6) as u8, (v & 0x3F) as u8])
    }

    pub fn sleep_out(&mut self) -> Result<(), B::Error> {
        self.cmd(op::SLPOUT)
    }

    pub fn sleep_in(&mut self) -> Result<(), B::Error> {
        self.cmd(op::SLPIN)
    }

    pub fn display_on(&mut self, on: bool) -> Result<(), B::Error> {
        self.cmd(if on { op::DISPON } else { op::DISPOFF })
    }

    /// `CASET` (2Ah) then `RASET` (2Bh), p.48 and p.49.
    ///
    /// Both ends are inclusive, as the chip's registers are -- a caller
    /// wanting a `w` x `h` window at the origin passes `(0, 0, w - 1, h - 1)`,
    /// which is also why Espressif's `draw_bitmap` subtracts one from its own
    /// exclusive ends before sending them.
    ///
    /// The column pair must be aligned: `sc` a multiple of four and `ec` one
    /// less than a multiple of four. This method does not silently fix that,
    /// because silently moving someone's window is how a frame ends up a few
    /// pixels off with nothing to show for it. [`align_columns`] is the
    /// deliberate way to do it, and the emulator faults if neither is used.
    pub fn set_window(&mut self, sc: u16, sp: u16, ec: u16, ep: u16) -> Result<(), B::Error> {
        self.cmd_with(
            op::CASET,
            &[(sc >> 8) as u8, sc as u8, (ec >> 8) as u8, ec as u8],
        )?;
        self.cmd_with(
            op::RASET,
            &[(sp >> 8) as u8, sp as u8, (ep >> 8) as u8, ep as u8],
        )
    }

    /// Stream a window's worth of already-encoded pixels: `RAMWR` (2Ch) and
    /// then `RAMWRC` (3Ch) for every further run.
    ///
    /// Two of this chip's rules shape what comes out, and neither is an
    /// optimisation:
    ///
    /// * Every transfer carries at least four pixels and a multiple of four
    ///   (section 6.5, p.16).
    /// * A whole frame may not go out on one `2Ch` (p.50: "Cannot write whole
    ///   frame data using 0x2C. Separate whole frame into several segment.").
    ///   So even a payload that would fit in a single chunk is split in two
    ///   whenever there are at least eight pixels to split. Below that there
    ///   is no legal split -- both halves would have to be four pixels -- and
    ///   a run that small is not a frame, so it goes out whole.
    ///
    /// Espressif's driver does neither: `panel_spd2010_draw_bitmap` sends one
    /// `RAMWR` with the entire bitmap behind it. It evidently works on real
    /// hardware, which is worth knowing; this driver follows the datasheet
    /// anyway, since splitting satisfies both documents and costs one extra
    /// transfer.
    pub fn write_pixels(&mut self, pixels: &[u8]) -> Result<(), B::Error> {
        let bpp = self.format.bytes_per_pixel();
        let lanes = pixel_lanes(self.format);
        let total_pixels = pixels.len() / bpp;

        let mut first = true;
        let mut offset = 0usize;
        while offset < pixels.len() {
            let remaining_pixels = (pixels.len() - offset) / bpp;
            let run_pixels = if first && remaining_pixels <= self.chunk_pixels {
                split_for_whole_frame_rule(remaining_pixels)
            } else {
                remaining_pixels.min(self.chunk_pixels)
            };
            let end = offset + run_pixels * bpp;
            // A payload that is not a whole number of pixels cannot be split
            // sensibly; hand the tail over as it stands and let the chip say
            // so, rather than inventing a boundary.
            let end = end.min(pixels.len());

            let cmd = if first { op::RAMWR } else { op::RAMWRC };
            let t = Transaction {
                opcode: crate::qspi::opcode::WRITE_COLOUR,
                address: (cmd as u32) << 8,
                lanes,
                data: &pixels[offset..end],
            };
            self.bus.transfer(&t)?;
            offset = end;
            first = false;
        }
        // An empty payload sends nothing at all, which is what a caller with
        // no pixels meant. `total_pixels` is only read to make that explicit.
        let _ = total_pixels;
        Ok(())
    }

    /// Set a window and fill it in one go.
    pub fn draw(
        &mut self,
        sc: u16,
        sp: u16,
        ec: u16,
        ep: u16,
        pixels: &[u8],
    ) -> Result<(), B::Error> {
        self.set_window(sc, sp, ec, ep)?;
        self.write_pixels(pixels)
    }
}

/// How many pixels the first transfer should carry so the rest is still a
/// legal second transfer.
///
/// Returns `total` unchanged when there is no legal split.
fn split_for_whole_frame_rule(total: usize) -> usize {
    if total < 2 * PIXEL_RUN_GRANULARITY {
        return total;
    }
    let half = (total / 2) & !(PIXEL_RUN_GRANULARITY - 1);
    // `total >= 8` makes `half >= 4`, and `half <= total / 2` leaves at least
    // `total / 2 >= 4` behind it.
    half.max(PIXEL_RUN_GRANULARITY)
}

/// Snap a column range out to the alignment `CASET` demands, p.48.
///
/// Returns `(sc, ec)` with `sc` rounded **down** to a multiple of four and
/// `ec` rounded **up** to one less than a multiple of four, so the window only
/// ever grows. That is the same rounding Espressif's README asks an LVGL
/// caller to do:
///
/// ```text
/// // round the start of coordinate down to the nearest 4M number
/// area->x1 = (x1 >> 2) << 2;
/// // round the end of coordinate up to the nearest 4N+3 number
/// area->x2 = ((x2 >> 2) << 2) + 3;
/// ```
///
/// Growing rather than shrinking is the right direction: a window that is too
/// small drops picture, while one that is too large only costs the pixels the
/// caller must then supply for it. The caller does have to supply them --
/// after snapping, the window is wider than the one asked for, and the chip
/// expects exactly that many pixels.
pub const fn align_columns(sc: u16, ec: u16) -> (u16, u16) {
    let a = COLUMN_ALIGNMENT;
    (sc - sc % a, ec + (a - 1 - ec % a))
}

/// Whether a column range already satisfies p.48 and needs no snapping.
pub const fn columns_aligned(sc: u16, ec: u16) -> bool {
    sc % COLUMN_ALIGNMENT == 0 && (ec + 1) % COLUMN_ALIGNMENT == 0
}

/// The `COLMOD` parameter for a format, for callers that want the byte rather
/// than the enum.
pub const fn colmod_for(format: PixelFormat) -> u8 {
    format.to_colmod()
}

/// `COLMOD`'s reset default, p.57.
pub const COLMOD_RESET: u8 = colmod::RESET_DEFAULT;
