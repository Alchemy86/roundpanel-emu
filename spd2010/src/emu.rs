//! A software model of the SPD2010.
//!
//! It accepts the QSPI transactions a real driver sends, keeps the registers
//! and the display RAM those transactions change, and can be read back as an
//! image. Nothing here interprets a `Canvas` or any other host-side structure:
//! the only input is bytes on a bus.
//!
//! # The addressing is not the SH8601Z's, and this is where that bites
//!
//! Both parts take `CASET` / `RASET` / `RAMWR`. Only one of them will accept
//! any window you like. Section 14.1.18 (p.48) states the restriction on
//! `Set Column` as two lines with no qualification:
//!
//! ```text
//! - SC must be 4M, where M is integer
//! - EC must be 4N-1, where N is integer
//! ```
//!
//! and section 14.1.19 `Set Row` (p.49) states **no** restriction at all, so
//! the alignment is columns only. Both real drivers carry the same rule
//! independently: Espressif's README tells the caller to round `x_start` down
//! to a multiple of four and `x_end` up to `4N+3` before every
//! `draw_bitmap`, and ESPHome's model for this board declares
//! `draw_rounding=4`.
//!
//! This is the difference that produces a *picture* rather than an error on
//! real hardware, which is why the model treats it as a fault. Centring a
//! 384-pixel-wide frame on a 412-dot panel gives `SC = 14`, and 14 is not a
//! multiple of four -- the obvious, correct-looking arithmetic is already
//! wrong. [`Fault::ColumnStartNotAligned`] is what catches it.
//!
//! # The RAM, and what this model does about it
//!
//! Section 6.5 (p.16) gives the memory as "103058 bytes RAM (456x456/2)" while
//! the feature list on p.7 gives it as "Embedded 103058 bytes RAM
//! (454x454/2)". Only one of those two parenthetical figures is arithmetic:
//! 454 x 454 / 2 is 103,058 exactly, and 456 x 456 / 2 is 103,968. So p.7's
//! dimensions are taken as the real ones and p.16's "456x456" as a typo, which
//! is also the resolution the document's own title page supports -- "454x454
//! TDDI for IOT/Wearable".
//!
//! Either way the byte count is half a byte per pixel, and the same section
//! requires a pixel to be written as two or three whole bytes. The datasheet
//! never reconciles those, and it does not describe the memory's internal
//! organisation anywhere; the part is a "Product Preview" that says so on its
//! front page. What the RAM physically holds is therefore not knowable from
//! any public source.
//!
//! This model keeps a full 454 x 454 x 24-bit buffer instead. That is an
//! **abstraction, not a claim**: it is exact about what the host sent and
//! where in the address space it goes, which is the whole question a driver
//! needs answered, and it is silent about how the silicon stores it, which is
//! the question no source can answer. The distinction matters if anyone later
//! tries to use this model to predict colour banding or compression artefacts
//! -- it cannot, and nothing here should be read as saying the real part keeps
//! 24 bits a pixel. It plainly does not have room to.
//!
//! # What is faithful here, and what is not
//!
//! Faithful: the QSPI framing, the command opcodes and their parameters, which
//! command set is selected, the address window and how a memory write walks
//! it, the pixel formats and their bit layouts, the reset defaults, and the
//! restrictions the datasheet states as restrictions -- those are enforced, so
//! a driver bug shows up as a [`Fault`] instead of a slightly wrong picture.
//!
//! Not modelled, deliberately: timing of any kind (no clocks, no V-sync, none
//! of `SLPOUT`'s 120 ms, no tearing-effect line), the MIPI DSI interface, the
//! whole touch half of this TDDI part, gamma, CABC, the NVM, and the analogue
//! chain. Brightness (`WRDISBV`) is tracked as a register but never applied to
//! the image: the datasheet defines `DBV` as a PWM duty and leaves the mapping
//! to emitted light to the module, and `WRCTRLD`'s `BCTRL` bit can take it out
//! of circuit entirely (p.62).
//!
//! Also not modelled: reading anything back. Section 6.5 is blunt -- "Read RAM
//! is not supported" -- and the model answers no read command either. The
//! `gram_pixel` and `shown_pixel` accessors below are the *host's* window onto
//! the model's memory for writing tests and PNGs; they are not a chip feature
//! and there is no command that does what they do.

use crate::cmd::{cmdset, colmod, madctl, op, wrctrld};
use crate::pixel::PixelFormat;
use crate::qspi::{opcode, Lanes, QspiBus, Transaction};

/// Widest column address the RAM holds, exclusive. Section 2 Features, p.7.
pub const RAM_WIDTH: u16 = 454;
/// Tallest row address the RAM holds, exclusive. Section 2 Features, p.7.
pub const RAM_HEIGHT: u16 = 454;
/// Bytes per pixel in this model's buffer. See the module docs: the real
/// part's RAM is 103,058 bytes in total and cannot be organised this way.
pub const RAM_BYTES_PER_PIXEL: usize = 3;
/// Bytes of RAM the model needs from its caller.
pub const RAM_LEN: usize = RAM_WIDTH as usize * RAM_HEIGHT as usize * RAM_BYTES_PER_PIXEL;

/// The real part's stated RAM size, in bytes. Sections 2 (p.7) and 6.5 (p.16)
/// both give this number; only p.7's accompanying dimensions are arithmetic.
pub const DATASHEET_RAM_BYTES: usize = 103_058;

/// Pixels that must arrive together in one `2Ch` or `3Ch` transfer.
///
/// Section 6.5, p.16, and repeated as a note under Figures 6-5, 6-8 and 6-11:
/// "for each time using 0x2C or 0x3C cmd to write RAM data, please write 4
/// pixels data or more", alongside "Number of pixel must be in multiple of 4".
pub const PIXEL_RUN_GRANULARITY: usize = 4;

/// The column-address alignment `CASET` demands, p.48.
pub const COLUMN_ALIGNMENT: u16 = 4;

/// Which of the RAM's addresses are wired to physical dots.
///
/// The driver IC drives up to 454x454 -- section 2 lists "various resolution,
/// from 240RGB x 240 to 454RGB x 454" -- but a module only lights the dots it
/// has. Writing outside them is not a chip error; it is a firmware bug that on
/// real hardware shows up as missing picture, so the model reports it.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub struct Geometry {
    /// Lit columns, starting at column 0.
    pub dots_x: u16,
    /// Lit rows, starting at row 0.
    pub dots_y: u16,
}

impl Geometry {
    /// The Waveshare ESP32-S3-Touch-LCD-1.46B's round panel: 412 x 412.
    ///
    /// From ESPHome's model for this exact board,
    /// `esphome/components/mipi_spi/models/spd2010.py`:
    ///
    /// ```text
    /// DriverChip(
    ///     "WAVESHARE-ESP32-S3-TOUCH-LCD-1.46",
    ///     width=412,
    ///     height=412,
    ///     ...
    /// ```
    ///
    /// 412 is a multiple of four, so a full-width window satisfies p.48's
    /// column alignment: `SC = 0` is `4M` and `EC = 411` is `4N-1`.
    pub const WAVESHARE_1_46: Geometry = Geometry {
        dots_x: 412,
        dots_y: 412,
    };

    /// The driver IC's own maximum, for a module that uses all of it.
    pub const FULL: Geometry = Geometry {
        dots_x: RAM_WIDTH,
        dots_y: RAM_HEIGHT,
    };
}

/// Something a real chip would either reject or quietly get wrong, which this
/// model reports instead so the driver can be fixed.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Fault {
    /// An opcode that is not one of section 6.2.4's.
    UnknownOpcode(u8),
    /// A read was attempted. The transport has a read opcode (`0Bh`, Figure
    /// 6-12) and the command table has read commands, but this model answers
    /// none of them, and section 6.5 rules out the one a frame harness would
    /// want: "Read RAM is not supported".
    ReadNotSupported(u8),
    /// The address phase had something in bits 23:16 or 7:0, which Figure 6-10
    /// draws as zero.
    MalformedAddress(u32),
    /// A command arrived with the wrong number of parameters for Table 13-1.
    BadParameterCount { cmd: u8, got: usize },
    /// `SC > EC`, `SP > EP`, or an address past the RAM.
    BadWindow { sc: u16, ec: u16, sp: u16, ep: u16 },
    /// "SC must be 4M, where M is integer" -- `Set Column` restriction, p.48.
    ColumnStartNotAligned(u16),
    /// "EC must be 4N-1, where N is integer" -- `Set Column` restriction,
    /// p.48. The end is inclusive, so a legal window's width is a multiple of
    /// four.
    ColumnEndNotAligned(u16),
    /// A `COLMOD` parameter that is not one of p.57's three.
    UnsupportedPixelFormat(u8),
    /// Fewer than four pixels in one memory-write transfer. Section 6.5, p.16.
    PixelRunTooShort { pixels: usize },
    /// A memory-write transfer carrying a pixel count that is not a multiple
    /// of four. "Number of pixel must be in multiple of 4", section 6.5, p.16.
    PixelCountNotMultipleOfFour { pixels: usize },
    /// A whole frame delivered by a single `2Ch` with no `3Ch` behind it.
    ///
    /// `Write memory start` restriction, p.50: "Cannot write whole frame data
    /// using 0x2C. Separate whole frame into several segment. Use 0x2C and
    /// 0x3C to write whole frame data." Section 6.5 repeats it.
    WholeFrameInOneRamwr { window_pixels: usize },
    /// Pixel data that is not a whole number of pixels. Section 6.5: "RAM must
    /// be written as 1 pixel (3 bytes ... 2 bytes ...)".
    PartialPixel { got: usize, bytes_per_pixel: usize },
    /// More pixels than the address window holds.
    WindowOverrun { window_pixels: usize, got: usize },
    /// Pixel data arrived without a `RAMWR` to start it.
    PixelDataWithoutRamwr,
    /// The address window reaches past the module's lit dots.
    OutsideVisibleArea {
        window: (u16, u16, u16, u16),
        dots: (u16, u16),
    },
    /// A memory write arrived while a vendor command page was selected by
    /// `FFh`. In that state `2Ch` is not `Write memory start`; it is whatever
    /// register `2Ch` names on that page.
    MemoryWriteInVendorSet { set: u8 },
}

/// The model.
///
/// The RAM is borrowed rather than owned so the crate stays `no_std` and
/// allocation-free; a host passes a `vec![0; RAM_LEN]`.
pub struct Spd2010Emulator<'a> {
    ram: &'a mut [u8],
    geom: Geometry,

    sc: u16,
    ec: u16,
    sp: u16,
    ep: u16,
    madctl: u8,
    colmod: u8,
    brightness: u16,
    ctrld: u8,
    cabc: u8,
    cabc_min: u16,
    tescan: u16,

    /// Which command set `FFh` last selected. [`cmdset::USER`] is Table 13-1.
    command_set: u8,

    sleeping: bool,
    display_on: bool,
    inverted: bool,
    idle: bool,
    te: Option<u8>,

    /// Where the next pixel lands. Absolute RAM coordinates.
    col: u16,
    row: u16,
    /// True once `RAMWR` has been seen and until another command interrupts.
    in_memory_write: bool,
    /// Pixels accepted since the last `RAMWR`, to enforce the window size.
    written_this_window: usize,

    /// Counters, so a test can assert the driver said what it meant to.
    pub commands_seen: usize,
    pub pixels_written: usize,
    /// Memory-write transfers since the last `RAMWR`, `RAMWR` included.
    pub transfers_this_window: usize,
    /// Commands absorbed by a vendor page rather than interpreted.
    pub vendor_writes_seen: usize,
}

impl<'a> Spd2010Emulator<'a> {
    /// A chip at the end of its power-on sequence: registers at their POR
    /// values from Table 13-1 and section 14.1, RAM as the caller left it.
    ///
    /// The RAM is not cleared. The datasheet never promises a blank one, and
    /// says the opposite about sleep -- "During sleep in mode, display data
    /// needed to be re-written into RAM before sleep out" (pp.41-42). A driver
    /// that expects a blank screen without writing one is relying on something
    /// the chip does not offer.
    pub fn new(ram: &'a mut [u8], geom: Geometry) -> Option<Self> {
        if ram.len() < RAM_LEN {
            return None;
        }
        let mut e = Spd2010Emulator {
            ram,
            geom,
            sc: 0,
            ec: 0,
            sp: 0,
            ep: 0,
            madctl: 0,
            colmod: colmod::RESET_DEFAULT,
            brightness: 0,
            ctrld: 0,
            cabc: 0,
            cabc_min: 0,
            tescan: 0,
            command_set: cmdset::USER,
            sleeping: true,
            display_on: false,
            inverted: false,
            idle: false,
            te: None,
            col: 0,
            row: 0,
            in_memory_write: false,
            written_this_window: 0,
            commands_seen: 0,
            pixels_written: 0,
            transfers_this_window: 0,
            vendor_writes_seen: 0,
        };
        e.apply_reset_defaults();
        Some(e)
    }

    /// The register defaults a reset restores, from each command's own POR
    /// column.
    ///
    /// `SWRESET` "resets the commands and parameters to their S/W Reset
    /// default values (See default tables in each command description)" and
    /// "The display is blank immediately" (p.32).
    fn apply_reset_defaults(&mut self) {
        // CASET p.48 and RASET p.49: 0000h..018Fh on both axes. That is 0..399,
        // a 400 x 400 default window -- neither the IC's 454 x 454 maximum nor
        // this module's 412 x 412, so a driver must always set its own.
        self.sc = 0x0000;
        self.ec = 0x018F;
        self.sp = 0x0000;
        self.ep = 0x018F;
        self.madctl = madctl::RESET_DEFAULT; // 00h, p.53
        self.colmod = colmod::RESET_DEFAULT; // 77h, p.57
        self.brightness = 0x0000; // WRDISBV 00h/00h, p.61
        self.ctrld = wrctrld::RESET_DEFAULT; // 00h, p.62
        self.cabc = 0;
        self.cabc_min = 0;
        self.tescan = 0;
        // A reset leaves the user command set selected: the vendor pages are
        // reached only by an explicit FFh, and both real drivers send one
        // before they touch anything.
        self.command_set = cmdset::USER;
        // Sleep In is the reset state; SLPOUT (11h) is what leaves it, p.42.
        self.sleeping = true;
        self.display_on = false;
        self.inverted = false;
        self.idle = false;
        self.te = None;
        self.in_memory_write = false;
        self.written_this_window = 0;
        self.transfers_this_window = 0;
    }

    /// The module geometry this model was built with.
    pub fn geometry(&self) -> Geometry {
        self.geom
    }

    /// The pixel format `COLMOD` currently selects.
    pub fn format(&self) -> Option<PixelFormat> {
        PixelFormat::from_colmod(self.colmod)
    }

    /// `(SC, SP, EC, EP)` as the last `CASET`/`RASET` left them.
    pub fn window(&self) -> (u16, u16, u16, u16) {
        (self.sc, self.sp, self.ec, self.ep)
    }

    /// Whether `DISPON` (29h) has been sent and `SLPIN` has not.
    pub fn is_displaying(&self) -> bool {
        self.display_on && !self.sleeping
    }

    /// `DBV[13:0]` as `WRDISBV` (51h) last set it.
    pub fn brightness(&self) -> u16 {
        self.brightness
    }

    /// `WRCTRLD` (53h) as last written: `BCTRL`, `DD` and `BL`. p.62.
    pub fn brightness_control(&self) -> u8 {
        self.ctrld
    }

    /// Whether `IDMON` (39h) has been sent and `IDMOFF` (38h) has not.
    ///
    /// Idle mode is tracked but has no effect on [`Self::shown_pixel`]. The
    /// datasheet describes it (p.56) as a reduced-colour, reduced-power state
    /// whose exact colour reduction it never specifies for this part, so
    /// applying one would be invention.
    pub fn is_idle(&self) -> bool {
        self.idle
    }

    /// `TELOM` if the tearing-effect signal is on, `None` if `TEOFF`. pp.51-52.
    pub fn tearing(&self) -> Option<u8> {
        self.te
    }

    /// `STS[15:0]` as `TESCAN` (44h) last set it. p.59.
    pub fn tear_scanline(&self) -> u16 {
        self.tescan
    }

    /// `PS[2:0]` as `WRCABC` (55h) last set it, and `CMB` from `WRCABCMB`
    /// (5Eh). pp.64, 66. Tracked, never applied: CABC is a backlight-duty
    /// feedback loop over content this model does not run.
    pub fn cabc(&self) -> (u8, u16) {
        (self.cabc, self.cabc_min)
    }

    /// The command set `FFh` last selected; [`cmdset::USER`] is Table 13-1.
    pub fn command_set(&self) -> u8 {
        self.command_set
    }

    /// The `MADCTL` byte as written.
    pub fn madctl(&self) -> u8 {
        self.madctl
    }

    /// The 24-bit word held at a RAM address, untouched by any display mode.
    ///
    /// Host inspection, not a chip feature: "Read RAM is not supported"
    /// (section 6.5, p.16).
    pub fn gram_pixel(&self, x: u16, y: u16) -> [u8; 3] {
        let i = (y as usize * RAM_WIDTH as usize + x as usize) * RAM_BYTES_PER_PIXEL;
        [self.ram[i], self.ram[i + 1], self.ram[i + 2]]
    }

    /// What the dot at that panel position actually emits, with the display
    /// modes applied.
    ///
    /// # `SS` and `GS` are applied here, and that is a choice
    ///
    /// `MADCTL`'s two flip bits are the one place the sources leave real room
    /// for doubt. The command's own heading reads "This command defines write
    /// scanning direction from the host processor", which sounds like the
    /// memory-write order, and that is one way to read the SH8601Z's `MX` --
    /// as the fill running the other way along the window. But this
    /// part's bit table names them "Flip Horizontal" and "Flip Vertical", and
    /// the figures beneath it (p.53-54) are pairs captioned *Data* and
    /// *Display* showing the same data emerging mirrored on the panel. That is
    /// a scan-out property, not a write-order one.
    ///
    /// Espressif settles it the same way by how it uses them: `SS` and `GS`
    /// are what `panel_spd2010_mirror(mirror_x, mirror_y)` writes, and
    /// `esp_lcd`'s mirror is defined over the whole panel, not over the
    /// current window. ESPHome's model agrees, declaring the pair as
    /// `transforms={CONF_MIRROR_X, CONF_MIRROR_Y}` with `use_axis_flips=True`.
    ///
    /// So this model applies them at scan-out, across the module's full lit
    /// area, and leaves the RAM untouched. The two readings differ only for a
    /// window smaller than the panel, which is exactly when a driver would
    /// notice; they are recorded here rather than resolved silently, and
    /// `MADCTL` is `00h` on every path this repo actually drives.
    pub fn shown_pixel(&self, x: u16, y: u16) -> [u8; 3] {
        if x >= self.geom.dots_x || y >= self.geom.dots_y || !self.is_displaying() {
            return [0, 0, 0];
        }
        let sx = if self.madctl & madctl::SS != 0 {
            self.geom.dots_x - 1 - x
        } else {
            x
        };
        let sy = if self.madctl & madctl::GS != 0 {
            self.geom.dots_y - 1 - y
        } else {
            y
        };
        let p = self.gram_pixel(sx, sy);
        if self.inverted {
            [255 - p[0], 255 - p[1], 255 - p[2]] // INVON, p.45
        } else {
            p
        }
    }

    /// Accept one transfer.
    pub fn transfer(&mut self, t: &Transaction<'_>) -> Result<(), Fault> {
        if t.address & !0x0000_FF00 != 0 {
            return Err(Fault::MalformedAddress(t.address));
        }
        let cmd = t.cmd();
        match t.opcode {
            opcode::WRITE_CMD => self.write_cmd(cmd, t.data),
            opcode::WRITE_COLOUR => {
                // The colour opcode carries only RAMWR or RAMWRC; Figure 6-11
                // draws the address as the command sitting in bits 15:8 and
                // section 6.5 names the two commands it may be.
                if cmd != op::RAMWR && cmd != op::RAMWRC {
                    return Err(Fault::MalformedAddress(t.address));
                }
                self.write_cmd(cmd, t.data)
            }
            // A legal transfer this model has no answer for. Listed so an
            // unknown opcode stays distinguishable from one that is merely
            // unanswered, and so `0Bh` is not mistaken for `03h`.
            opcode::READ_CMD => Err(Fault::ReadNotSupported(cmd)),
            other => Err(Fault::UnknownOpcode(other)),
        }
    }

    fn write_cmd(&mut self, cmd: u8, params: &[u8]) -> Result<(), Fault> {
        self.commands_seen += 1;

        // Any command other than a memory-write pair ends the run of pixels.
        if cmd != op::RAMWR && cmd != op::RAMWRC {
            self.in_memory_write = false;
        }

        // FFh is honoured on every page -- it is how a vendor page is left
        // again, so it cannot itself be page-dependent.
        if cmd == op::CMD_SET {
            if params.len() != 3 {
                return Err(Fault::BadParameterCount {
                    cmd,
                    got: params.len(),
                });
            }
            // The first two bytes are the fixed 20h 10h both drivers send. A
            // different pair is not something either source describes, so it
            // is recorded rather than rejected.
            self.command_set = params[2];
            return Ok(());
        }

        // While a vendor page is selected, Table 13-1's meanings do not apply:
        // the same opcode names a vendor register. Both drivers write dozens
        // of these, so they are counted and absorbed, not faulted -- except a
        // memory write, which cannot be what the host meant.
        if self.command_set != cmdset::USER {
            if cmd == op::RAMWR || cmd == op::RAMWRC {
                return Err(Fault::MemoryWriteInVendorSet {
                    set: self.command_set,
                });
            }
            self.vendor_writes_seen += 1;
            return Ok(());
        }

        let want = |n: usize| -> Result<(), Fault> {
            if params.len() == n {
                Ok(())
            } else {
                Err(Fault::BadParameterCount {
                    cmd,
                    got: params.len(),
                })
            }
        };

        match cmd {
            op::NOP => Ok(()),
            op::SWRESET => {
                self.apply_reset_defaults();
                Ok(())
            }
            op::SLPIN => {
                self.sleeping = true;
                Ok(())
            }
            op::SLPOUT => {
                self.sleeping = false;
                Ok(())
            }
            // The only display mode this part has. There is no PTLON to leave.
            op::NORON => Ok(()),
            op::INVOFF => {
                self.inverted = false;
                Ok(())
            }
            op::INVON => {
                self.inverted = true;
                Ok(())
            }
            op::DISPOFF => {
                self.display_on = false;
                Ok(())
            }
            op::DISPON => {
                self.display_on = true;
                Ok(())
            }
            op::IDMOFF => {
                self.idle = false;
                Ok(())
            }
            op::IDMON => {
                self.idle = true;
                Ok(())
            }
            op::TEOFF => {
                self.te = None;
                Ok(())
            }
            op::TEON => {
                // Table 13-1 gives TEON one parameter, TELOM. Both real
                // drivers send it with one byte of 00h -- Espressif's vendor
                // table has `{0x35, {0x00}, 1, 0}` and ESPHome's model
                // `(0x35, 0x00)` -- but a zero-length TEON is common enough in
                // vendor tables that rejecting it would be pedantry. Accept
                // either; TELOM is recorded when it is given.
                match params.len() {
                    0 => {
                        self.te = Some(0);
                        Ok(())
                    }
                    1 => {
                        self.te = Some(params[0]);
                        Ok(())
                    }
                    got => Err(Fault::BadParameterCount { cmd, got }),
                }
            }
            op::MADCTL => {
                want(1)?;
                self.madctl = params[0];
                Ok(())
            }
            op::COLMOD => {
                want(1)?;
                if PixelFormat::from_colmod(params[0]).is_none() {
                    return Err(Fault::UnsupportedPixelFormat(params[0]));
                }
                self.colmod = params[0];
                Ok(())
            }
            op::TESCAN => {
                want(2)?;
                self.tescan = u16::from_be_bytes([params[0], params[1]]);
                Ok(())
            }
            op::WRDISBV => {
                // p.61: 1st parameter DBV[13:6], 2nd `0 0 DBV[5:0]`. The field
                // is fourteen bits and the high part comes first -- the
                // opposite end and the opposite order from the SH8601Z's
                // ten-bit, low-byte-first DBV.
                want(2)?;
                self.brightness = ((params[0] as u16) << 6) | (params[1] as u16 & 0x3F);
                Ok(())
            }
            op::WRCTRLD => {
                want(1)?;
                self.ctrld = params[0];
                Ok(())
            }
            op::WRCABC => {
                want(1)?;
                self.cabc = params[0];
                Ok(())
            }
            op::WRCABCMB => {
                want(2)?;
                self.cabc_min = ((params[0] as u16) << 6) | (params[1] as u16 & 0x3F);
                Ok(())
            }
            op::CASET => {
                want(4)?;
                let sc = u16::from_be_bytes([params[0], params[1]]);
                let ec = u16::from_be_bytes([params[2], params[3]]);
                self.set_column(sc, ec)
            }
            op::RASET => {
                want(4)?;
                let sp = u16::from_be_bytes([params[0], params[1]]);
                let ep = u16::from_be_bytes([params[2], params[3]]);
                self.set_row(sp, ep)
            }
            op::RAMWR => {
                self.start_memory_write()?;
                self.accept_pixels(params, true)
            }
            op::RAMWRC => {
                if !self.in_memory_write {
                    return Err(Fault::PixelDataWithoutRamwr);
                }
                self.accept_pixels(params, false)
            }
            // Read commands the table defines but this model does not answer.
            // Reached only if a host sends one with the *write* opcode, which
            // is itself a bug; there is nothing to store, so it is a no-op.
            op::RDID
            | op::RDISPMODE
            | op::RDDPM
            | op::RDDMADCTL
            | op::RDPIX
            | op::RDDIM
            | op::RDDSM
            | op::RDDSDR
            | op::RDSCAN
            | op::RDCTRLD
            | op::RDCABC
            | op::RDCABCMB
            | op::RDDDBST
            | op::RDDDBCON => Ok(()),
            // Not in Table 13-1 at all. The datasheet states no rule for an
            // undefined command -- unlike the SH8601Z, whose section 5.1 note
            // says they are treated as NOP -- so this is the model's own
            // lenient choice, not a documented behaviour.
            _ => Ok(()),
        }
    }

    /// `CASET` (2Ah), p.48, with the alignment restriction it carries.
    fn set_column(&mut self, sc: u16, ec: u16) -> Result<(), Fault> {
        if sc > ec || ec >= RAM_WIDTH {
            return Err(Fault::BadWindow {
                sc,
                ec,
                sp: self.sp,
                ep: self.ep,
            });
        }
        // "SC must be 4M, where M is integer"
        if sc % COLUMN_ALIGNMENT != 0 {
            return Err(Fault::ColumnStartNotAligned(sc));
        }
        // "EC must be 4N-1, where N is integer" -- so EC + 1 is a multiple of
        // four, i.e. EC is 3 mod 4. N = 0 would put EC below SC, so the
        // smallest legal window is SC = 0, EC = 3: four columns wide.
        if (ec + 1) % COLUMN_ALIGNMENT != 0 {
            return Err(Fault::ColumnEndNotAligned(ec));
        }
        self.sc = sc;
        self.ec = ec;
        Ok(())
    }

    /// `RASET` (2Bh), p.49. Its restriction field is empty: rows are free.
    fn set_row(&mut self, sp: u16, ep: u16) -> Result<(), Fault> {
        if sp > ep || ep >= RAM_HEIGHT {
            return Err(Fault::BadWindow {
                sc: self.sc,
                ec: self.ec,
                sp,
                ep,
            });
        }
        self.sp = sp;
        self.ep = ep;
        Ok(())
    }

    fn start_memory_write(&mut self) -> Result<(), Fault> {
        // Both address registers are settled by the time pixels arrive, so
        // this is where a window that runs off the module's lit dots can be
        // judged. Checking it inside CASET or RASET instead would depend on
        // which of the two the driver happened to send second.
        if self.ec >= self.geom.dots_x || self.ep >= self.geom.dots_y {
            return Err(Fault::OutsideVisibleArea {
                window: (self.sc, self.sp, self.ec, self.ep),
                dots: (self.geom.dots_x, self.geom.dots_y),
            });
        }
        // Note what is *not* checked: being asleep. The SH8601Z forbids frame
        // memory access in sleep in mode outright. This part does not: the Register Availability table under both
        // `Write memory start` (p.50) and `Write memory Continue` (p.58) gives
        // "Sleep In -- Yes". It is merely pointless, because "During sleep in
        // mode, display data needed to be re-written into RAM before sleep
        // out" (pp.41-42), and nothing is displayed until DISPON anyway.
        self.col = self.sc;
        self.row = self.sp;
        self.written_this_window = 0;
        self.transfers_this_window = 0;
        self.in_memory_write = true;
        Ok(())
    }

    /// Take one memory-write transfer's payload.
    ///
    /// `is_start` says whether this arrived on `2Ch` rather than `3Ch`, which
    /// is what the whole-frame restriction turns on.
    fn accept_pixels(&mut self, data: &[u8], is_start: bool) -> Result<(), Fault> {
        let fmt = self
            .format()
            .ok_or(Fault::UnsupportedPixelFormat(self.colmod))?;
        let bpp = fmt.bytes_per_pixel();
        self.transfers_this_window += 1;

        // Section 6.5: a pixel is 2 or 3 whole bytes, at least four of them,
        // and a multiple of four. All three are per transfer -- the sentence
        // about four or more is written "for each time using 0x2C or 0x3C".
        //
        // The multiple-of-four rule would follow from the column alignment
        // alone for a full window, since p.48 makes every legal window a
        // multiple of four columns wide; it is enforced here as well because
        // a chunked driver can break it without breaking the window.
        if data.len() % bpp != 0 {
            return Err(Fault::PartialPixel {
                got: data.len(),
                bytes_per_pixel: bpp,
            });
        }
        let pixels = data.len() / bpp;
        if pixels < PIXEL_RUN_GRANULARITY {
            return Err(Fault::PixelRunTooShort { pixels });
        }
        if pixels % PIXEL_RUN_GRANULARITY != 0 {
            return Err(Fault::PixelCountNotMultipleOfFour { pixels });
        }

        let window_pixels = self.window_pixels();

        for chunk in data.chunks_exact(bpp) {
            if self.written_this_window >= window_pixels {
                return Err(Fault::WindowOverrun {
                    window_pixels,
                    got: self.written_this_window + 1,
                });
            }

            let rgb = fmt.decode(chunk);
            // MADCTL D3, BGR: the incoming channel order, p.53. This one is a
            // property of the data as it arrives, so unlike SS and GS it
            // belongs here rather than at scan-out.
            let rgb = if self.madctl & madctl::BGR != 0 {
                [rgb[2], rgb[1], rgb[0]]
            } else {
                rgb
            };

            let i =
                (self.row as usize * RAM_WIDTH as usize + self.col as usize) * RAM_BYTES_PER_PIXEL;
            self.ram[i] = rgb[0];
            self.ram[i + 1] = rgb[1];
            self.ram[i + 2] = rgb[2];

            self.written_this_window += 1;
            self.pixels_written += 1;

            // The column is the fast axis and wraps to SC at EC, stepping the
            // row -- the ordinary DCS fill, and the one both real drivers
            // assume when they hand `draw_bitmap` a row-major buffer.
            if self.col >= self.ec {
                self.col = self.sc;
                self.row = if self.row >= self.ep {
                    self.sp
                } else {
                    self.row + 1
                };
            } else {
                self.col += 1;
            }
        }

        // p.50: "Cannot write whole frame data using 0x2C. Separate whole
        // frame into several segment."
        //
        // Read literally that is about a *frame*, so the fault fires only when
        // the window is the module's whole lit area and one 2Ch filled it. A
        // stricter reading -- any window filled by a lone 2Ch -- is defensible
        // too, and would make a four-pixel test write illegal, which the
        // datasheet plainly does not intend given it tells you four pixels is
        // the minimum transfer.
        //
        // Espressif's own driver breaks this rule: `panel_spd2010_draw_bitmap`
        // sends CASET, RASET and then a single `tx_color(.., LCD_CMD_RAMWR,
        // color_data, len)` with the entire bitmap behind it and never issues
        // a 3Ch at all. So the vendor's driver contradicts the vendor's
        // datasheet. This model follows the datasheet, because the restriction
        // is stated in two separate places (pp.16 and 50) and a driver that
        // splits its writes satisfies both documents -- see
        // `crate::driver::Spd2010::write_pixels`.
        if is_start && self.written_this_window == window_pixels && self.window_is_whole_frame() {
            return Err(Fault::WholeFrameInOneRamwr { window_pixels });
        }

        Ok(())
    }

    /// Whether the address window covers every lit dot of the module.
    fn window_is_whole_frame(&self) -> bool {
        self.sc == 0
            && self.sp == 0
            && self.ec == self.geom.dots_x - 1
            && self.ep == self.geom.dots_y - 1
    }

    /// Pixels in the current address window.
    pub fn window_pixels(&self) -> usize {
        (self.ec - self.sc + 1) as usize * (self.ep - self.sp + 1) as usize
    }

    /// How many pixels the last memory write has delivered into the current
    /// window, against how many the window holds.
    pub fn window_fill(&self) -> (usize, usize) {
        (self.written_this_window, self.window_pixels())
    }

    /// Read a rectangle of the panel back out as 8-bit grey, row-major.
    ///
    /// The three channels are averaged. Unlike on an SH8601Z they are not
    /// guaranteed equal even for a grey frame: 65k colour gives red and blue
    /// five bits and green six, so a neutral grey comes back very slightly
    /// tinted. `out` must hold `w * h` bytes.
    ///
    /// This is host inspection of the model, not a chip read; section 6.5 says
    /// "Read RAM is not supported".
    pub fn read_grey8(&self, x: u16, y: u16, w: u16, h: u16, out: &mut [u8]) {
        for row in 0..h {
            for c in 0..w {
                let p = self.shown_pixel(x + c, y + row);
                let v = (p[0] as u16 + p[1] as u16 + p[2] as u16 + 1) / 3;
                out[row as usize * w as usize + c as usize] = v as u8;
            }
        }
    }
}

impl QspiBus for Spd2010Emulator<'_> {
    type Error = Fault;

    fn transfer(&mut self, t: &Transaction<'_>) -> Result<(), Fault> {
        Spd2010Emulator::transfer(self, t)
    }
}

/// The lane width a payload of this format must go out on.
///
/// Always [`Lanes::Quad`] for pixels: this chip has no 1-wire-only formats, so
/// unlike the SH8601Z there is no format-dependent choice to get wrong. Kept
/// as a function so the driver states the reason rather than assuming it.
pub const fn pixel_lanes(_format: PixelFormat) -> Lanes {
    Lanes::Quad
}
