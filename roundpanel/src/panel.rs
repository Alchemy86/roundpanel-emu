//! The panel itself: a frame, encoded, sent as real QSPI transfers, and read
//! back out of the chip model's RAM.
//!
//! Nothing in this module reaches into the emulator's memory. The only way a
//! pixel gets in is a transfer [`spd2010::Spd2010`] sends, and the only way one
//! comes back out is a read of the model's RAM -- so a picture that appears in
//! the window has been through the same bytes the hardware would see, including
//! the rules the chip would enforce and this crate's tests would otherwise have
//! to imagine.
//!
//! # Two rules that will surprise you
//!
//! **Columns come in fours.** `CASET`'s start column must be a multiple of four
//! and its end column one less than a multiple of four (datasheet p.48). The
//! full-panel window is fine -- 412 is a multiple of four -- but a partial
//! update at an arbitrary x is not, so [`Panel::present_region`] snaps the
//! window outwards with [`spd2010::align_columns`] rather than moving your
//! picture. Espressif ask LVGL callers to do the same rounding and ESPHome
//! declares `draw_rounding=4` for this board.
//!
//! **A whole frame may not go out on one `RAMWR`.** Page 50: "Cannot write
//! whole frame data using 0x2C. Separate whole frame into several segment."
//! The driver splits it; the model checks that it did.
//!
//! # What persists between frames
//!
//! The chip's RAM does, and its registers do not. Each present brings the part
//! up (`MADCTL`, `COLMOD`, `SLPOUT`, brightness, `DISPON`) and then draws, so a
//! region update composites onto whatever the last frame left in RAM -- which
//! is what a real panel does, and why a partial update shows the old picture
//! around it instead of black.

use spd2010::emu::{Geometry, Spd2010Emulator, RAM_LEN};
use spd2010::{align_columns, Fault, PixelFormat, Spd2010};

use crate::frame::Frame;

/// The board's module: 412 x 412 lit dots.
pub const GEOMETRY: Geometry = Geometry::WAVESHARE_1_46;

/// A panel with the chip model behind it.
pub struct Panel {
    ram: Vec<u8>,
    dots: Vec<u8>,
    encoded: Vec<u8>,
    geom: Geometry,
    format: PixelFormat,
    /// Transfers the driver issued for the last present.
    pub commands: usize,
    /// Pixels the chip accepted for the last present.
    pub pixels: usize,
    /// Memory-write transfers in the last present's window. At least two,
    /// because p.50 forbids a whole frame on one `RAMWR`.
    pub memory_writes: usize,
}

impl Panel {
    /// A panel in `format`, at the board's own geometry.
    ///
    /// [`PixelFormat::Rgb888`] is the only lossless one and is the sensible
    /// default. The other two are here because they are what you would really
    /// use on the wire -- two thirds of the bytes for 65k colour -- and running
    /// in one of them on the desktop is how you find out what your palette
    /// looks like after the interface has rounded it.
    pub fn new(format: PixelFormat) -> Panel {
        Panel::with_geometry(format, GEOMETRY)
    }

    /// The same, on a module of another size. The SPD2010 drives anything from
    /// 240x240 to 454x454; only the 412x412 one is on this board.
    pub fn with_geometry(format: PixelFormat, geom: Geometry) -> Panel {
        Panel {
            ram: vec![0; RAM_LEN],
            dots: vec![0; geom.dots_x as usize * geom.dots_y as usize * 3],
            encoded: Vec::new(),
            geom,
            format,
            commands: 0,
            pixels: 0,
            memory_writes: 0,
        }
    }

    pub fn format(&self) -> PixelFormat {
        self.format
    }

    pub fn geometry(&self) -> Geometry {
        self.geom
    }

    /// Every lit dot as the module now shows it: three bytes a pixel, R, G, B,
    /// row-major, `dots_x` by `dots_y`. This is the readback, not the frame
    /// that was sent -- in anything but 16.7M colour the two differ.
    pub fn dots(&self) -> &[u8] {
        &self.dots
    }

    pub fn dots_size(&self) -> (usize, usize) {
        (self.geom.dots_x as usize, self.geom.dots_y as usize)
    }

    /// Draw the whole frame.
    pub fn present(&mut self, frame: &Frame) -> Result<(), String> {
        self.present_region(frame, 0, 0, self.geom.dots_x, self.geom.dots_y)
    }

    /// Draw one rectangle of the frame, leaving the rest of the panel as it
    /// was.
    ///
    /// The column range is snapped outwards to the alignment p.48 demands, so
    /// the window can come back wider than the one you asked for. The extra
    /// columns are taken from `frame` rather than written dark, because `frame`
    /// holds the whole panel and the dots either side of your rectangle are
    /// real picture.
    pub fn present_region(
        &mut self,
        frame: &Frame,
        x: u16,
        y: u16,
        w: u16,
        h: u16,
    ) -> Result<(), String> {
        if frame.width() != self.geom.dots_x as usize || frame.height() != self.geom.dots_y as usize
        {
            return Err(format!(
                "a {}x{} frame does not fit a {}x{} dot module",
                frame.width(),
                frame.height(),
                self.geom.dots_x,
                self.geom.dots_y
            ));
        }
        if w == 0 || h == 0 {
            return Ok(());
        }
        let (sc, ec) = align_columns(x, x + w - 1);
        let (sp, ep) = (y, y + h - 1);
        if ec >= self.geom.dots_x || ep >= self.geom.dots_y {
            return Err(format!(
                "the window ({sc},{sp})-({ec},{ep}) leaves a {}x{} dot module",
                self.geom.dots_x, self.geom.dots_y
            ));
        }

        let bpp = self.format.bytes_per_pixel();
        let pixels = (ec - sc + 1) as usize * (ep - sp + 1) as usize;
        self.encoded.resize(pixels * bpp, 0);
        let mut i = 0;
        for row in sp..=ep {
            for col in sc..=ec {
                self.format.encode(
                    frame.get(col as i32, row as i32),
                    &mut self.encoded[i..i + bpp],
                );
                i += bpp;
            }
        }

        let emu = Spd2010Emulator::new(&mut self.ram, self.geom).ok_or("RAM buffer too small")?;
        let mut panel = Spd2010::new(emu);
        // No vendor table. It is panel data -- roughly 380 undocumented gamma,
        // GIP and power writes that "can be different between manufacturers" --
        // and the model has no gamma to set. A real board sends the one its
        // supplier gives it; see `spd2010::driver` for where it goes.
        panel.init(self.format, &[]).map_err(fault)?;
        panel.draw(sc, sp, ec, ep, &self.encoded).map_err(fault)?;
        let emu = panel.release();

        let (got, want) = emu.window_fill();
        if got != want {
            return Err(format!("the memory write filled {got} of {want} pixels"));
        }
        if emu.transfers_this_window < 2 {
            return Err(format!(
                "the frame went out in {} memory write(s); p.50 forbids a whole frame on one RAMWR",
                emu.transfers_this_window
            ));
        }
        if !emu.is_displaying() {
            return Err("the panel is not displaying after init".into());
        }

        self.commands = emu.commands_seen;
        self.pixels = emu.pixels_written;
        self.memory_writes = emu.transfers_this_window;

        for row in 0..self.geom.dots_y {
            for col in 0..self.geom.dots_x {
                let i = (row as usize * self.geom.dots_x as usize + col as usize) * 3;
                self.dots[i..i + 3].copy_from_slice(&emu.shown_pixel(col, row));
            }
        }
        Ok(())
    }
}

fn fault(f: Fault) -> String {
    format!("the panel rejected a transfer: {f:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{rgb, WHITE};

    /// The whole join, end to end: something drawn in the frame comes back out
    /// of the chip's RAM in the right place and the right colour, having been
    /// nowhere except through the command set.
    #[test]
    fn a_pixel_survives_the_round_trip_to_ram_and_back() {
        let mut p = Panel::new(PixelFormat::Rgb888);
        let mut f = Frame::new();
        f.set(200, 100, rgb(0x4e_a3_ff));
        p.present(&f).expect("present");
        let (w, _) = p.dots_size();
        let i = (100 * w + 200) * 3;
        assert_eq!(&p.dots()[i..i + 3], &[0x4e, 0xa3, 0xff]);
        assert_eq!(p.dots()[0..3], [0, 0, 0], "the rest of the panel is black");
    }

    #[test]
    fn a_full_frame_never_goes_out_on_one_ramwr() {
        let mut p = Panel::new(PixelFormat::Rgb888);
        p.present(&Frame::new()).expect("present");
        assert!(p.memory_writes >= 2, "p.50 forbids that");
        assert_eq!(p.pixels, 412 * 412);
    }

    /// 65k colour is what you would really put on the wire, and it rounds.
    /// The harness should be able to show you that rather than hide it.
    #[test]
    fn a_narrower_format_rounds_the_colour_and_says_so() {
        let mut p = Panel::new(PixelFormat::Rgb565);
        let mut f = Frame::new();
        f.set(206, 206, rgb(0x4e_a3_ff));
        p.present(&f).expect("present");
        let (w, _) = p.dots_size();
        let i = (206 * w + 206) * 3;
        let back = &p.dots()[i..i + 3];
        assert_ne!(back, [0x4e, 0xa3, 0xff], "65k colour cannot carry that");
        for ch in 0..3 {
            assert!(back[ch].abs_diff([0x4e, 0xa3, 0xff][ch]) <= 8);
        }
    }

    /// A region update composites onto what RAM already held, which is what a
    /// real panel does and the reason partial updates are worth having.
    #[test]
    fn a_region_update_leaves_the_rest_of_the_panel_alone() {
        let mut p = Panel::new(PixelFormat::Rgb888);
        let mut f = Frame::new();
        f.clear(WHITE);
        p.present(&f).expect("full frame");

        let mut g = Frame::new();
        g.rect(100, 100, 40, 40, rgb(0xff_00_00));
        p.present_region(&g, 100, 100, 40, 40).expect("region");

        let (w, _) = p.dots_size();
        let inside = (110 * w + 110) * 3;
        let outside = (300 * w + 300) * 3;
        assert_eq!(&p.dots()[inside..inside + 3], &[0xff, 0, 0]);
        assert_eq!(
            &p.dots()[outside..outside + 3],
            &[255, 255, 255],
            "the region update wiped the panel"
        );
    }

    /// An unaligned x is snapped outwards, never sideways: the picture stays
    /// where it was put.
    #[test]
    fn an_unaligned_region_grows_rather_than_moving_the_picture() {
        let mut p = Panel::new(PixelFormat::Rgb888);
        let mut f = Frame::new();
        f.set(101, 50, WHITE);
        p.present_region(&f, 101, 50, 2, 2).expect("region");
        let (w, _) = p.dots_size();
        let i = (50 * w + 101) * 3;
        assert_eq!(&p.dots()[i..i + 3], &[255, 255, 255]);
    }
}
