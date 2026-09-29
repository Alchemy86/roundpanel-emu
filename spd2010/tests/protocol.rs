//! What the driver puts on the bus, and what the chip does with it.
//!
//! These are checks against the datasheet -- Solomon Systech *L-WEA2010* Rev
//! 0.50 -- and against the two real drivers, not against the implementation.
//! Each one names the page or the source line it comes from, so a failure is
//! either a real bug or a page that was misread, and either way the next
//! reader knows where to look.
//!
//! A theme runs through the second half: several of these would fail on an
//! SH8601Z, which is why this crate does not share a line of code with a
//! driver for that part.

use spd2010::cmd::{cmdset, colmod, madctl, op, wrctrld};
use spd2010::emu::{
    Fault, Geometry, Spd2010Emulator, COLUMN_ALIGNMENT, DATASHEET_RAM_BYTES, PIXEL_RUN_GRANULARITY,
    RAM_HEIGHT, RAM_LEN, RAM_WIDTH,
};
use spd2010::qspi::{opcode, Lanes, QspiBus, Transaction};
use spd2010::{align_columns, columns_aligned, PixelFormat, Spd2010};

fn ram() -> Vec<u8> {
    vec![0u8; RAM_LEN]
}

/// A bus that writes everything down, so a test can assert on the wire format
/// rather than on the emulator's reaction to it.
#[derive(Default)]
struct Recorder {
    sent: Vec<(u8, u32, Lanes, Vec<u8>)>,
}

impl Recorder {
    /// The command byte of every transfer, in order.
    fn cmds(&self) -> Vec<u8> {
        self.sent
            .iter()
            .map(|(_, a, _, _)| (a >> 8) as u8)
            .collect()
    }
    fn params_of(&self, cmd: u8) -> Option<&Vec<u8>> {
        self.sent
            .iter()
            .find(|(_, a, _, _)| (a >> 8) as u8 == cmd)
            .map(|(_, _, _, d)| d)
    }
}

impl QspiBus for Recorder {
    type Error = ();
    fn transfer(&mut self, t: &Transaction<'_>) -> Result<(), ()> {
        self.sent
            .push((t.opcode, t.address, t.lanes, t.data.to_vec()));
        Ok(())
    }
}

/// A panel that has been through init and is ready to take pixels. No vendor
/// table: the model does not need one and the real one is panel data.
fn ready(r: &mut [u8], geom: Geometry, fmt: PixelFormat) -> Spd2010<Spd2010Emulator<'_>> {
    let e = Spd2010Emulator::new(r, geom).unwrap();
    let mut d = Spd2010::new(e);
    d.init(fmt, &[]).unwrap();
    d
}

// -- the memory ----------------------------------------------------------

#[test]
fn the_datasheets_two_ram_figures_do_not_agree_and_only_one_is_arithmetic() {
    // Section 2, p.7: "Embedded 103058 bytes RAM (454x454/2)".
    // Section 6.5, p.16: "There are 103058 bytes RAM (456x456/2)".
    // Both give the same byte count; only p.7's dimensions produce it, which
    // is why this crate's RAM_WIDTH/RAM_HEIGHT are 454 and not 456.
    assert_eq!(454 * 454 / 2, DATASHEET_RAM_BYTES);
    assert_ne!(456 * 456 / 2, DATASHEET_RAM_BYTES);
    assert_eq!(RAM_WIDTH, 454);
    assert_eq!(RAM_HEIGHT, 454);
}

#[test]
fn the_models_buffer_is_deliberately_bigger_than_the_real_ram() {
    // The real part has half a byte per pixel and this model keeps three, so
    // the model's buffer is twenty-four times the silicon's. That is an
    // abstraction the emu docs argue for at length; this test exists so nobody
    // later reads RAM_LEN as a claim about the hardware.
    assert_eq!(RAM_LEN, 454 * 454 * 3);
    assert_eq!(RAM_LEN, DATASHEET_RAM_BYTES * 6);
}

#[test]
fn reset_defaults_match_the_datasheet() {
    let mut r = ram();
    let e = Spd2010Emulator::new(&mut r, Geometry::FULL).unwrap();
    // CASET p.48 and RASET p.49 both POR to 0000h..018Fh -- 0..399. Note this
    // is neither the IC's 454x454 maximum nor any module's size, so a driver
    // must always set its own window.
    assert_eq!(e.window(), (0x0000, 0x0000, 0x018F, 0x018F));
    assert_eq!(0x018F, 399);
    // COLMOD p.57: 77h, which is 16.7M colour.
    assert_eq!(e.format(), Some(PixelFormat::Rgb888));
    // MADCTL p.53: 00h.
    assert_eq!(e.madctl(), madctl::RESET_DEFAULT);
    // WRDISBV p.61: 00h, 00h -- dark, unlike the SH8601Z's 0_FFh.
    assert_eq!(e.brightness(), 0);
    // WRCTRLD p.62: 00h.
    assert_eq!(e.brightness_control(), wrctrld::RESET_DEFAULT);
    // Sleep In is the reset state; SLPOUT (11h) leaves it, p.42.
    assert!(!e.is_displaying());
    // A reset leaves the user command set live.
    assert_eq!(e.command_set(), cmdset::USER);
}

// -- the wire ------------------------------------------------------------

#[test]
fn a_command_is_opcode_02_then_the_command_in_bits_15_8() {
    // Section 6.2.4, Figure 6-10, p.15, and esp_lcd_spd2010.c's tx_param():
    // address = cmd << 8, opcode = 0x02 in bits 31:24.
    let mut d = Spd2010::new(Recorder::default());
    d.cmd_with(op::CASET, &[0x00, 0x00, 0x01, 0x7F]).unwrap();
    let bus = d.release();
    assert_eq!(bus.sent.len(), 1);
    let (opc, addr, lanes, data) = &bus.sent[0];
    assert_eq!(*opc, opcode::WRITE_CMD);
    assert_eq!(*opc, 0x02);
    assert_eq!(*addr, 0x002A00);
    assert_eq!(*lanes, Lanes::Single);
    assert_eq!(data, &[0x00, 0x00, 0x01, 0x7F]);
}

#[test]
fn a_pixel_write_is_opcode_32_at_0x002c00_then_0x003c00() {
    // Figure 6-11, p.15, and esp_lcd_spd2010.c's tx_color(). The continuation
    // is RAMWRC, which on this chip is required rather than merely useful.
    let mut d = Spd2010::new(Recorder::default()).with_chunk_pixels(4);
    d.init(PixelFormat::Rgb565, &[]).unwrap();
    d.write_pixels(&[0u8; 24]).unwrap(); // 12 pixels at 2 bytes each
    let bus = d.release();
    let pixel_writes: Vec<_> = bus
        .sent
        .iter()
        .filter(|(o, ..)| *o == opcode::WRITE_COLOUR)
        .collect();
    assert_eq!(pixel_writes.len(), 3, "12 pixels in runs of 4");
    assert_eq!(pixel_writes[0].1, 0x002C00, "first is RAMWR");
    assert_eq!(pixel_writes[1].1, 0x003C00, "then RAMWRC");
    assert_eq!(pixel_writes[2].1, 0x003C00);
    // Unlike the SH8601Z, no format here is restricted to one lane, so every
    // pixel payload goes out quad.
    assert!(pixel_writes.iter().all(|(_, _, l, _)| *l == Lanes::Quad));
}

#[test]
fn the_read_opcode_is_0b_and_not_the_sh8601s_03() {
    // Figure 6-12, p.15, and esp_lcd_spd2010.c:
    //     #define LCD_OPCODE_READ_CMD (0x0BULL)
    // A host that reuses an SH8601 read routine sends 03h, which is not a read
    // opcode on this part at all -- it is not an opcode on this part at all.
    assert_eq!(opcode::READ_CMD, 0x0B);
    let mut r = ram();
    let mut e = Spd2010Emulator::new(&mut r, Geometry::FULL).unwrap();
    // The real read opcode is recognised, and refused for a stated reason.
    assert_eq!(
        e.transfer(&Transaction::read(op::RDID)),
        Err(Fault::ReadNotSupported(op::RDID))
    );
    // The SH8601's is not recognised at all.
    let sh8601_read = Transaction {
        opcode: 0x03,
        address: (op::RDID as u32) << 8,
        lanes: Lanes::Single,
        data: &[],
    };
    assert_eq!(e.transfer(&sh8601_read), Err(Fault::UnknownOpcode(0x03)));
}

#[test]
fn the_address_phase_has_nothing_outside_bits_15_8() {
    let mut r = ram();
    let mut e = Spd2010Emulator::new(&mut r, Geometry::FULL).unwrap();
    let bad = Transaction {
        opcode: opcode::WRITE_CMD,
        address: 0x012A00,
        lanes: Lanes::Single,
        data: &[],
    };
    assert_eq!(e.transfer(&bad), Err(Fault::MalformedAddress(0x012A00)));
}

#[test]
fn init_opens_the_way_the_vendor_driver_opens() {
    // esp_lcd_spd2010.c, panel_spd2010_init(): the command-set select first,
    // then MADCTL, then COLMOD, then the vendor table, and the table ends on
    // SLPOUT.
    let mut d = Spd2010::new(Recorder::default());
    d.init(PixelFormat::Rgb565, &[]).unwrap();
    let bus = d.release();
    assert_eq!(
        bus.cmds(),
        vec![
            op::CMD_SET,
            op::MADCTL,
            op::COLMOD,
            op::CMD_SET,
            op::SLPOUT,
            op::WRCTRLD,
            op::WRDISBV,
            op::DISPON,
        ]
    );
    // FFh 20h 10h 00h is the user command set, byte for byte as both real
    // drivers send it.
    assert_eq!(
        bus.params_of(op::CMD_SET).unwrap(),
        &vec![cmdset::MAGIC[0], cmdset::MAGIC[1], cmdset::USER]
    );
    // COLMOD's parameter for 65k colour is 75h: the fixed 01110b high bits
    // with DBI = 5, p.57.
    assert_eq!(bus.params_of(op::COLMOD).unwrap(), &vec![0x75]);
    assert_eq!(colmod::RGB565, 0x75);
}

#[test]
fn a_vendor_table_is_sent_between_colmod_and_sleep_out() {
    // Where both real drivers put it. The bytes here are the first three
    // entries of Espressif's own default table.
    let table: &[(u8, &[u8])] = &[
        (op::CMD_SET, &[0x20, 0x10, 0x10]),
        (0x0C, &[0x11]),
        (0x10, &[0x02]),
    ];
    let mut d = Spd2010::new(Recorder::default());
    d.init(PixelFormat::Rgb888, table).unwrap();
    let bus = d.release();
    let cmds = bus.cmds();
    let colmod_at = cmds.iter().position(|c| *c == op::COLMOD).unwrap();
    let slpout_at = cmds.iter().position(|c| *c == op::SLPOUT).unwrap();
    let vendor_at = cmds.iter().position(|c| *c == 0x0C).unwrap();
    assert!(colmod_at < vendor_at && vendor_at < slpout_at);
}

// -- the addressing model -------------------------------------------------

#[test]
fn pixels_land_in_the_window_and_nowhere_else() {
    // CASET/RASET set one rectangle and RAMWR fills it, column first.
    // The window is 4-aligned because p.48 leaves no choice.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    let px: Vec<u8> = (1..=8u8).flat_map(|v| [v, v, v]).collect();
    d.draw(8, 20, 11, 21, &px).unwrap();
    let e = d.release();

    assert_eq!(e.gram_pixel(8, 20), [1, 1, 1]);
    assert_eq!(e.gram_pixel(9, 20), [2, 2, 2]);
    assert_eq!(e.gram_pixel(11, 20), [4, 4, 4]);
    assert_eq!(e.gram_pixel(8, 21), [5, 5, 5], "column wraps, row steps");
    assert_eq!(e.gram_pixel(11, 21), [8, 8, 8]);
    // Just outside, on every side.
    assert_eq!(e.gram_pixel(7, 20), [0, 0, 0]);
    assert_eq!(e.gram_pixel(12, 20), [0, 0, 0]);
    assert_eq!(e.gram_pixel(8, 19), [0, 0, 0]);
    assert_eq!(e.gram_pixel(8, 22), [0, 0, 0]);
    assert_eq!(e.window_fill(), (8, 8));
}

#[test]
fn a_chunked_write_continues_where_the_last_one_stopped() {
    // RAMWRC, p.58: the column and row registers carry on from where the last
    // transfer left them.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888).with_chunk_pixels(4);
    d.set_window(0, 0, 7, 1).unwrap();
    let px: Vec<u8> = (1..=16u8).flat_map(|v| [v, v, v]).collect();
    d.write_pixels(&px).unwrap();
    let e = d.release();
    for i in 0..16u8 {
        let (x, y) = (i as u16 % 8, i as u16 / 8);
        assert_eq!(e.gram_pixel(x, y)[0], i + 1, "pixel {i} at ({x},{y})");
    }
    assert_eq!(e.pixels_written, 16);
    assert_eq!(e.transfers_this_window, 4, "one RAMWR and three RAMWRC");
}

#[test]
fn madctl_bgr_swaps_the_incoming_channel_order() {
    // MADCTL D3, p.53. This one is about the data as it arrives, so it lands
    // in memory swapped.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    d.cmd_with(op::MADCTL, &[madctl::BGR]).unwrap();
    d.draw(0, 0, 3, 0, &[0x11, 0x22, 0x33, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let e = d.release();
    assert_eq!(e.gram_pixel(0, 0), [0x33, 0x22, 0x11]);
}

#[test]
fn madctl_ss_and_gs_flip_the_panel_and_leave_memory_alone() {
    // MADCTL D1 "Flip Horizontal" and D0 "Flip Vertical", p.53, applied at
    // scan-out across the module's lit area -- the reading the emu docs argue
    // for, and the one Espressif's panel_spd2010_mirror() implies.
    let geom = Geometry {
        dots_x: 8,
        dots_y: 4,
    };
    let mut r = ram();
    let mut d = ready(&mut r, geom, PixelFormat::Rgb888);
    d.draw(0, 0, 3, 0, &[9, 9, 9, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    d.cmd_with(op::MADCTL, &[madctl::SS]).unwrap();
    let e = d.release();
    // Memory is untouched by the flip.
    assert_eq!(e.gram_pixel(0, 0), [9, 9, 9]);
    // The dot that now shows memory's column 0 is the far side of the panel.
    assert_eq!(e.shown_pixel(7, 0), [9, 9, 9]);
    assert_eq!(e.shown_pixel(0, 0), [0, 0, 0]);
}

#[test]
fn rgb565_unpacks_the_way_the_datasheet_packs_it() {
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb565);
    // RRRRRGGG GGGBBBBB: full red, then full blue, then two black.
    d.draw(
        0,
        0,
        3,
        0,
        &[0xF8, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00, 0x00],
    )
    .unwrap();
    let e = d.release();
    assert_eq!(e.gram_pixel(0, 0), [255, 0, 0]);
    assert_eq!(e.gram_pixel(1, 0), [0, 0, 255]);
}

#[test]
fn rgb888_reaches_ram_exactly_and_565_does_not() {
    // The SH8601Z carries a grey frame bit-exact in one byte a pixel, in a
    // 256-grey mode this chip does not have (p.57 defines DBI = 7, 6, 5 and
    // nothing else). So exactness here costs three bytes a pixel, and the
    // cheap format rounds. Both bounds are checked by exhausting all 256
    // greys rather than by asserting a number someone chose.
    for fmt in [
        PixelFormat::Rgb888,
        PixelFormat::Rgb666,
        PixelFormat::Rgb565,
    ] {
        let bpp = fmt.bytes_per_pixel();
        let mut worst = 0u8;
        let mut buf = [0u8; 3];
        for g in 0..=255u8 {
            fmt.encode([g, g, g], &mut buf);
            let back = fmt.decode(&buf[..bpp]);
            let avg = ((back[0] as u16 + back[1] as u16 + back[2] as u16 + 1) / 3) as u8;
            worst = worst.max(avg.abs_diff(g));
        }
        assert_eq!(
            worst,
            fmt.max_grey_error(),
            "{fmt:?} says its worst grey error is {} and it is {worst}",
            fmt.max_grey_error()
        );
    }
    assert_eq!(PixelFormat::Rgb888.max_grey_error(), 0);
    assert_eq!(PixelFormat::Rgb666.max_grey_error(), 3);
    assert_eq!(PixelFormat::Rgb565.max_grey_error(), 6);

    // A four-grey palette is the case that actually matters, and it behaves
    // differently from the worst case: 262k carries all four exactly,
    // 65k carries none of them exactly. A harness that picked its tolerance
    // from max_grey_error alone would be too loose for one and too tight for
    // no one.
    for (fmt, want_exact) in [
        (PixelFormat::Rgb888, true),
        (PixelFormat::Rgb666, true),
        (PixelFormat::Rgb565, false),
    ] {
        let bpp = fmt.bytes_per_pixel();
        let mut buf = [0u8; 3];
        let exact = [255u8, 170, 85, 0].iter().all(|&g| {
            fmt.encode([g, g, g], &mut buf);
            let back = fmt.decode(&buf[..bpp]);
            back == [g, g, g]
        });
        assert_eq!(exact, want_exact, "{fmt:?} and the four greys");
    }
}

// -- the restrictions the datasheet states --------------------------------

#[test]
fn a_column_start_must_be_a_multiple_of_four() {
    // "SC must be 4M, where M is integer" -- Set Column restriction, p.48.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    assert_eq!(
        d.set_window(2, 0, 7, 0),
        Err(Fault::ColumnStartNotAligned(2))
    );
    assert!(d.set_window(4, 0, 7, 0).is_ok());
}

#[test]
fn a_column_end_must_be_one_less_than_a_multiple_of_four() {
    // "EC must be 4N-1, where N is integer" -- p.48. The end is inclusive, so
    // this makes every legal window a multiple of four columns wide.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    assert_eq!(d.set_window(0, 0, 6, 0), Err(Fault::ColumnEndNotAligned(6)));
    assert!(d.set_window(0, 0, 7, 0).is_ok());
    for ec in 0..64u16 {
        assert_eq!(
            columns_aligned(0, ec),
            (ec + 1) % COLUMN_ALIGNMENT == 0,
            "EC {ec}"
        );
    }
}

#[test]
fn rows_carry_no_alignment_restriction() {
    // Set Row, p.49: its Restriction field is empty, where Set Column's has
    // two lines in it. Copying the column rule onto rows would reject windows
    // the chip accepts.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    assert!(d.set_window(0, 1, 3, 2).is_ok(), "SP = 1, EP = 2 is legal");
}

#[test]
fn centring_a_384_wide_frame_on_a_412_dot_panel_is_the_trap() {
    // This is the whole reason the alignment is worth modelling. The obvious
    // arithmetic for centring a 384-pixel-wide frame on 412 dots gives a start
    // column of 14, and 14 is not a multiple of four. On real hardware this is
    // not an error -- it is a picture in slightly the wrong place, or worse.
    let naive_sc = (412 - 384) / 2;
    assert_eq!(naive_sc, 14);
    assert!(!columns_aligned(naive_sc, naive_sc + 384 - 1));

    let mut r = ram();
    let mut d = ready(&mut r, Geometry::WAVESHARE_1_46, PixelFormat::Rgb565);
    assert_eq!(
        d.set_window(naive_sc, 14, naive_sc + 383, 14 + 255),
        Err(Fault::ColumnStartNotAligned(14))
    );

    // Snapping the start down keeps the frame's own width, which is already a
    // multiple of four, so the window stays exactly 384 columns.
    let sc = naive_sc - naive_sc % COLUMN_ALIGNMENT;
    assert_eq!(sc, 12);
    assert!(columns_aligned(sc, sc + 384 - 1));
    assert!(d.set_window(sc, 14, sc + 383, 14 + 255).is_ok());
}

#[test]
fn align_columns_only_ever_grows_the_window() {
    // Espressif's README asks an LVGL caller to round x1 down and x2 up. A
    // window that grows costs pixels; one that shrinks loses picture.
    for sc in 0..32u16 {
        for w in 1..32u16 {
            let ec = sc + w - 1;
            let (a, b) = align_columns(sc, ec);
            assert!(a <= sc && b >= ec, "({sc},{ec}) -> ({a},{b}) shrank");
            assert!(columns_aligned(a, b), "({sc},{ec}) -> ({a},{b}) unaligned");
            assert!(sc - a < COLUMN_ALIGNMENT && b - ec < COLUMN_ALIGNMENT);
        }
    }
}

#[test]
fn a_memory_write_carries_at_least_four_pixels() {
    // Section 6.5, p.16, and the note under Figure 6-11: "please write 4
    // pixels data or more".
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    d.set_window(0, 0, 7, 0).unwrap();
    let mut e = d.release();
    assert_eq!(
        e.transfer(&Transaction::pixels(op::RAMWR, &[1, 1, 1, 2, 2, 2])),
        Err(Fault::PixelRunTooShort { pixels: 2 })
    );
}

#[test]
fn a_memory_write_carries_a_multiple_of_four_pixels() {
    // "Number of pixel must be in multiple of 4", section 6.5, p.16.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    d.set_window(0, 0, 7, 0).unwrap();
    let mut e = d.release();
    assert_eq!(
        e.transfer(&Transaction::pixels(op::RAMWR, &[0u8; 18])), // 6 pixels
        Err(Fault::PixelCountNotMultipleOfFour { pixels: 6 })
    );
    assert_eq!(PIXEL_RUN_GRANULARITY, 4);
}

#[test]
fn pixel_data_must_be_whole_pixels() {
    // Section 6.5: "RAM must be written as 1 pixel (3 bytes ... 2 bytes ...)".
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    d.set_window(0, 0, 7, 0).unwrap();
    let mut e = d.release();
    assert_eq!(
        e.transfer(&Transaction::pixels(op::RAMWR, &[0u8; 13])),
        Err(Fault::PartialPixel {
            got: 13,
            bytes_per_pixel: 3
        })
    );
}

#[test]
fn a_whole_frame_may_not_go_out_on_one_ramwr() {
    // Write memory start restriction, p.50: "Cannot write whole frame data
    // using 0x2C. Separate whole frame into several segment. Use 0x2C and 0x3C
    // to write whole frame data." Section 6.5 says it again.
    //
    // Espressif's own panel_spd2010_draw_bitmap() sends exactly this and no
    // 3Ch at all, so the vendor's driver contradicts the vendor's datasheet.
    // The model follows the datasheet.
    let geom = Geometry {
        dots_x: 8,
        dots_y: 2,
    };
    let mut r = ram();
    let mut d = ready(&mut r, geom, PixelFormat::Rgb888);
    d.set_window(0, 0, 7, 1).unwrap();
    let mut e = d.release();
    assert_eq!(
        e.transfer(&Transaction::pixels(op::RAMWR, &[0u8; 48])), // all 16 px
        Err(Fault::WholeFrameInOneRamwr { window_pixels: 16 })
    );
}

#[test]
fn the_driver_splits_so_the_whole_frame_rule_holds() {
    // The same frame through the driver instead of by hand. One RAMWR and at
    // least one RAMWRC, each a legal run, and the picture arrives.
    let geom = Geometry {
        dots_x: 8,
        dots_y: 2,
    };
    let mut r = ram();
    let mut d = ready(&mut r, geom, PixelFormat::Rgb888);
    let px: Vec<u8> = (1..=16u8).flat_map(|v| [v, v, v]).collect();
    d.draw(0, 0, 7, 1, &px).expect("the split keeps it legal");
    let e = d.release();
    assert!(
        e.transfers_this_window >= 2,
        "a lone RAMWR would have faulted"
    );
    assert_eq!(e.window_fill(), (16, 16));
    assert_eq!(e.gram_pixel(7, 1), [16, 16, 16]);
}

#[test]
fn a_window_smaller_than_the_data_is_rejected() {
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    d.set_window(0, 0, 3, 0).unwrap(); // four pixels
    let mut e = d.release();
    assert_eq!(
        e.transfer(&Transaction::pixels(op::RAMWR, &[0u8; 24])), // eight
        Err(Fault::WindowOverrun {
            window_pixels: 4,
            got: 5
        })
    );
}

#[test]
fn ramwrc_without_a_ramwr_is_rejected() {
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    d.set_window(0, 0, 7, 0).unwrap();
    let mut e = d.release();
    assert_eq!(
        e.transfer(&Transaction::pixels(op::RAMWRC, &[0u8; 12])),
        Err(Fault::PixelDataWithoutRamwr)
    );
}

#[test]
fn writing_past_the_modules_dots_is_reported() {
    // Not a chip error -- a firmware bug. The Waveshare module lights 412 x
    // 412 of the 454 x 454 the IC can address, so a window past it is picture
    // the device will never show.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::WAVESHARE_1_46, PixelFormat::Rgb888);
    // The registers themselves take it -- the IC can address 454 x 454, and
    // 447 is a legal 4N-1.
    d.set_window(0, 0, 447, 0).unwrap();
    // It is the memory write that has nowhere to go.
    assert_eq!(
        d.write_pixels(&[0u8; 12]),
        Err(Fault::OutsideVisibleArea {
            window: (0, 0, 447, 0),
            dots: (412, 412),
        })
    );
    // The panel's own full width is fine, because 412 is a multiple of four.
    assert!(d.set_window(0, 0, 411, 0).is_ok());
}

#[test]
fn a_memory_write_while_asleep_is_allowed_on_this_chip() {
    // The SH8601Z forbids it outright -- "No access in the frame memory in
    // sleep in mode", p.115 of that datasheet.
    // This part's Register Availability tables under Write memory start (p.50)
    // and Write memory Continue (p.58) both say "Sleep In -- Yes". Copying the
    // other chip's rule across would reject a legal sequence.
    let mut r = ram();
    let mut e = Spd2010Emulator::new(&mut r, Geometry::WAVESHARE_1_46).unwrap();
    assert!(!e.is_displaying(), "still asleep out of reset");
    e.transfer(&Transaction::command(op::CASET, &[0, 0, 0, 3]))
        .unwrap();
    e.transfer(&Transaction::command(op::RASET, &[0, 0, 0, 0]))
        .unwrap();
    assert!(e
        .transfer(&Transaction::pixels(op::RAMWR, &[0xAB; 12]))
        .is_ok());
    assert_eq!(e.gram_pixel(0, 0), [0xAB, 0xAB, 0xAB], "it reached memory");
    assert_eq!(e.shown_pixel(0, 0), [0, 0, 0], "but nothing is displayed");
}

#[test]
fn only_the_three_documented_pixel_formats_are_accepted() {
    // p.57 defines DBI[2:0] = 7, 6, 5 with the high five bits fixed at 01110b.
    let mut r = ram();
    let mut e = Spd2010Emulator::new(&mut r, Geometry::FULL).unwrap();
    for good in [colmod::RGB888, colmod::RGB666, colmod::RGB565] {
        assert!(e
            .transfer(&Transaction::command(op::COLMOD, &[good]))
            .is_ok());
    }
    // 11h is the SH8601Z's 256-grey. There is no grey mode here, so a driver
    // carried across from that chip sets a format this one does not have.
    assert_eq!(
        e.transfer(&Transaction::command(op::COLMOD, &[0x11])),
        Err(Fault::UnsupportedPixelFormat(0x11))
    );
    // DBI = 4 and below are undefined even with the right high bits.
    assert_eq!(
        e.transfer(&Transaction::command(op::COLMOD, &[0x74])),
        Err(Fault::UnsupportedPixelFormat(0x74))
    );
    // The format survived every rejection.
    assert_eq!(e.format(), Some(PixelFormat::Rgb565));
}

#[test]
fn brightness_is_fourteen_bits_with_the_high_part_first() {
    // WRDISBV, p.61: 1st parameter DBV[13:6], 2nd `0 0 DBV[5:0]`. The SH8601Z
    // sends a ten-bit DBV low byte first; reusing those two bytes here sets a
    // brightness about sixty-four times too low.
    let mut d = Spd2010::new(Recorder::default());
    d.set_brightness(1).unwrap();
    let bus = d.release();
    assert_eq!(
        bus.params_of(op::WRDISBV).unwrap(),
        &vec![0x00, 0x01],
        "high part first, so DBV = 1 is 00h 01h and not 01h 00h"
    );

    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    for v in [0u16, 1, 0x1234, 0x3FFF] {
        d.set_brightness(v).unwrap();
        assert_eq!(d.release().brightness(), v, "DBV {v:#06x} round trip");
        d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    }
    // Above the field's range it clamps rather than wrapping.
    d.set_brightness(0xFFFF).unwrap();
    assert_eq!(d.release().brightness(), 0x3FFF);
}

// -- the command sets -----------------------------------------------------

#[test]
fn a_vendor_page_absorbs_user_commands_instead_of_obeying_them() {
    // FFh 20h 10h <page> selects a command set. While a vendor page is live,
    // 36h is not MADCTL -- it is whatever register 36h names on that page.
    // Both real drivers write hundreds of bytes this way.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    d.vendor_page(0x11).unwrap();
    d.cmd_with(op::MADCTL, &[madctl::BGR]).unwrap();
    {
        let e = &mut d;
        let _ = e;
    }
    let mut e = d.release();
    assert_eq!(e.command_set(), 0x11);
    assert_eq!(e.madctl(), 0x00, "MADCTL was not touched on a vendor page");
    assert_eq!(e.vendor_writes_seen, 1);
    // Back to the user set, and the same command now means what it says.
    e.transfer(&Transaction::command(
        op::CMD_SET,
        &[cmdset::MAGIC[0], cmdset::MAGIC[1], cmdset::USER],
    ))
    .unwrap();
    e.transfer(&Transaction::command(op::MADCTL, &[madctl::BGR]))
        .unwrap();
    assert_eq!(e.madctl(), madctl::BGR);
}

#[test]
fn a_memory_write_on_a_vendor_page_is_a_fault() {
    // The one command that cannot plausibly be a vendor register write.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    d.set_window(0, 0, 7, 0).unwrap();
    d.vendor_page(0x2D).unwrap();
    let mut e = d.release();
    assert_eq!(
        e.transfer(&Transaction::pixels(op::RAMWR, &[0u8; 12])),
        Err(Fault::MemoryWriteInVendorSet { set: 0x2D })
    );
}

#[test]
fn the_command_set_select_needs_its_three_bytes() {
    let mut r = ram();
    let mut e = Spd2010Emulator::new(&mut r, Geometry::FULL).unwrap();
    assert_eq!(
        e.transfer(&Transaction::command(op::CMD_SET, &[0x20, 0x10])),
        Err(Fault::BadParameterCount {
            cmd: op::CMD_SET,
            got: 2
        })
    );
}

#[test]
fn display_off_changes_what_is_shown_not_what_is_stored() {
    // DISPOFF p.46, INVON p.45. Note there is no ALLPON/ALLPOFF on this part
    // -- Table 13-1 has no 22h or 23h -- so those SH8601Z modes have no
    // counterpart to test.
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb888);
    d.draw(0, 0, 3, 0, &[200, 200, 200, 0, 0, 0, 0, 0, 0, 0, 0, 0])
        .unwrap();
    let mut e = d.release();
    assert_eq!(e.shown_pixel(0, 0), [200, 200, 200]);

    e.transfer(&Transaction::command(op::INVON, &[])).unwrap();
    assert_eq!(e.shown_pixel(0, 0), [55, 55, 55], "inverted at scan-out");
    assert_eq!(e.gram_pixel(0, 0), [200, 200, 200], "memory untouched");

    e.transfer(&Transaction::command(op::INVOFF, &[])).unwrap();
    e.transfer(&Transaction::command(op::DISPOFF, &[])).unwrap();
    assert_eq!(e.shown_pixel(0, 0), [0, 0, 0], "display off is black");
    assert_eq!(
        e.gram_pixel(0, 0),
        [200, 200, 200],
        "memory still untouched"
    );
}

#[test]
fn swreset_restores_every_documented_default() {
    let mut r = ram();
    let mut d = ready(&mut r, Geometry::FULL, PixelFormat::Rgb565);
    d.set_window(4, 4, 11, 11).unwrap();
    d.cmd_with(op::MADCTL, &[madctl::BGR | madctl::SS]).unwrap();
    d.vendor_page(0x12).unwrap();
    d.user_page().unwrap();
    d.cmd(op::SWRESET).unwrap();
    let e = d.release();
    // p.32: "It resets the commands and parameters to their S/W Reset default
    // values (See default tables in each command description)."
    assert_eq!(e.window(), (0x0000, 0x0000, 0x018F, 0x018F));
    assert_eq!(e.format(), Some(PixelFormat::Rgb888));
    assert_eq!(e.madctl(), 0x00);
    assert_eq!(e.brightness(), 0);
    assert_eq!(e.command_set(), cmdset::USER);
    assert!(!e.is_displaying(), "and back to sleep");
}
