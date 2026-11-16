//! Where a frame's time goes on the board, with one core and with two.
//!
//!     cargo run --release --example frame-budget
//!     cargo run --release --example frame-budget -- --draw 20 --encode 2.7 --print
//!     cargo run --release --example frame-budget -- --shot budget.png
//!
//! A calculator, not a measurement. The wire time is computed from what the
//! driver really sends for a frame -- the emulated panel is presented once and
//! its transfers are counted -- at the board's 40 MHz on four lanes. The draw
//! and encode times are yours to give: the defaults are the encode times
//! measured on the real board before and after the colour table (18.8 ms and
//! 2.7 ms), and a draw time you should replace with your own.
//!
//! Three passes are drawn as timelines, core 0 above core 1:
//!
//! - one core: draw, encode, send, one after the other;
//! - core 1 sends: core 0 draws the next frame while core 1 encodes and sends
//!   this one, so a pass is the longer of the two;
//! - and pipelined: core 1 encodes the next half band while the last is on the
//!   wire, so its side is the longer of encode and send, not their sum.
//!
//! PWR steps the draw time, BOOT swaps per-pixel and table encode. Read
//! `docs/two-cores.md` for the code that does each step on the board.

use roundpanel::{rgb, App, Cli, Ctx, Frame, Panel, PixelFormat, Rgb, SideKey};

/// The board's panel clock, and bits a clock on four lanes.
const SPI_HZ: f64 = 40_000_000.0;
const QUAD_BITS: f64 = 4.0;
/// Clocks a command costs besides its pixel data: an 8-bit opcode and a 24-bit
/// address on one lane, plus a few parameter bytes. An estimate; the pixel
/// data is nearly all of the frame.
const COMMAND_CLOCKS: f64 = 64.0;
/// Bands a frame is sent in on the board (16 dot rows each).
const BANDS: f64 = 26.0;

const DRAW_STEPS: [f64; 6] = [8.0, 15.0, 20.0, 30.0, 45.0, 60.0];
/// Encode times measured on the board: per-pixel conversion, then the table.
const ENCODE_PER_PIXEL_MS: f64 = 18.8;
const ENCODE_TABLE_MS: f64 = 2.7;

const BG: Rgb = rgb(0x0b_0e_14);
const INK: Rgb = rgb(0xe6_ea_f2);
const DIM: Rgb = rgb(0x5a_63_72);
const DRAW: Rgb = rgb(0x4e_a3_ff);
const ENCODE: Rgb = rgb(0xff_b3_4e);
const WIRE: Rgb = rgb(0x3d_dc_97);
const IDLE: Rgb = rgb(0x1c_22_2c);

/// The wire time of one whole frame, from the transfers the driver issued.
fn wire_ms(format: PixelFormat) -> Result<(f64, usize, usize), String> {
    let mut panel = Panel::new(format);
    panel.present(&Frame::new())?;
    let data_clocks = panel.pixels as f64 * format.bytes_per_pixel() as f64 * 8.0 / QUAD_BITS;
    let command_clocks = panel.commands as f64 * COMMAND_CLOCKS;
    Ok((
        (data_clocks + command_clocks) / SPI_HZ * 1e3,
        panel.pixels,
        panel.commands,
    ))
}

/// One pass's segments on each core: `(start ms, length ms, colour)`.
struct Pass {
    name: &'static str,
    core0: Vec<(f64, f64, Rgb)>,
    core1: Vec<(f64, f64, Rgb)>,
    ms: f64,
}

fn passes(draw: f64, encode: f64, wire: f64) -> [Pass; 3] {
    let one = Pass {
        name: "ONE CORE",
        core0: vec![
            (0.0, draw, DRAW),
            (draw, encode, ENCODE),
            (draw + encode, wire, WIRE),
        ],
        core1: vec![],
        ms: draw + encode + wire,
    };
    let sent = Pass {
        name: "CORE 1 SENDS",
        core0: vec![(0.0, draw, DRAW)],
        core1: vec![(0.0, encode, ENCODE), (encode, wire, WIRE)],
        ms: draw.max(encode + wire),
    };
    let first_band = encode / BANDS;
    let side = first_band + encode.max(wire);
    let piped = Pass {
        name: "PIPELINED",
        core0: vec![(0.0, draw, DRAW)],
        core1: vec![(first_band, side - first_band, WIRE), (0.0, encode, ENCODE)],
        ms: draw.max(side),
    };
    [one, sent, piped]
}

struct Budget {
    wire: f64,
    draw_step: usize,
    table: bool,
}

impl Budget {
    fn encode(&self) -> f64 {
        if self.table {
            ENCODE_TABLE_MS
        } else {
            ENCODE_PER_PIXEL_MS
        }
    }
}

impl App for Budget {
    fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        if ctx.pressed(SideKey::Pwr) {
            self.draw_step = (self.draw_step + 1) % DRAW_STEPS.len();
        }
        if ctx.pressed(SideKey::Boot) {
            self.table = !self.table;
        }
        let draw = DRAW_STEPS[self.draw_step];
        let encode = self.encode();
        let all = passes(draw, encode, self.wire);
        let longest = all.iter().map(|p| p.ms).fold(1.0, f64::max);

        let (cx, _) = f.centre();
        f.clear(BG);
        f.text_centred(cx, 48, "FRAME BUDGET - COMPUTED", 2, DIM);
        f.text_centred(
            cx,
            68,
            &format!(
                "DRAW {} ENCODE {} WIRE {} MS",
                draw.round(),
                encode.round(),
                self.wire.round()
            ),
            2,
            INK,
        );

        let (left, width) = (66, 280);
        let scale = width as f64 / longest;
        for (k, pass) in all.iter().enumerate() {
            let top = 104 + k as i32 * 82;
            f.text(
                left,
                top,
                &format!("{} {} FPS", pass.name, (1000.0 / pass.ms).round()),
                2,
                INK,
            );
            for (lane, segs) in [&pass.core0, &pass.core1].into_iter().enumerate() {
                let y = top + 16 + lane as i32 * 20;
                let pass_w = (pass.ms * scale) as i32;
                f.rect(left, y, pass_w, 14, IDLE);
                for &(start, len, colour) in segs {
                    let x = left + (start * scale) as i32;
                    let w = ((len * scale) as i32).max(1);
                    // Pipelined, core 1 encodes while the wire runs: the
                    // encode is the thin bar over the send.
                    let h = if colour == ENCODE && k == 2 { 4 } else { 14 };
                    f.rect(x, y, w, h, colour);
                }
            }
        }
        f.text_centred(cx, 356, "1 PWR DRAW  2 BOOT ENCODE", 2, DIM);
    }
}

fn print(draw: f64, encode: f64, wire: f64, pixels: usize, commands: usize) {
    println!(
        "a frame: {pixels} pixels, {commands} commands -> {wire:.1} ms on the wire at 40 MHz x 4 (computed)"
    );
    println!("draw {draw:.1} ms, encode {encode:.1} ms (given)");
    for p in passes(draw, encode, wire) {
        println!(
            "  {:<13} {:>6.1} ms a pass  {:>5.1} fps",
            p.name.to_lowercase(),
            p.ms,
            1000.0 / p.ms
        );
    }
}

fn main() -> Result<(), String> {
    let mut rest = Vec::new();
    let mut draw = None;
    let mut encode = None;
    let mut want_print = false;
    let mut args = std::env::args().skip(1);
    let ms = |v: Option<String>, name: &str| -> Result<f64, String> {
        v.ok_or(format!("{name} wants milliseconds"))?
            .parse::<f64>()
            .map_err(|e| format!("{name}: {e}"))
    };
    while let Some(a) = args.next() {
        match a.as_str() {
            "--draw" => draw = Some(ms(args.next(), "--draw")?),
            "--encode" => encode = Some(ms(args.next(), "--encode")?),
            "--print" => want_print = true,
            _ => rest.push(a),
        }
    }
    let mut cli = Cli::parse(rest).map_err(|e| {
        format!("{e}\n  --draw MS     core 0's draw time\n  --encode MS   the encode time\n  --print       print the three passes and exit")
    })?;
    // The board sends 65k colour: two bytes a dot.
    let format = PixelFormat::Rgb565;
    let (wire, pixels, commands) = wire_ms(format)?;
    if want_print {
        print(
            draw.unwrap_or(20.0),
            encode.unwrap_or(ENCODE_TABLE_MS),
            wire,
            pixels,
            commands,
        );
        return Ok(());
    }
    if cli.options.title == "roundpanel-emu" {
        cli.options.title = "frame-budget".into();
    }
    let draw_step = draw
        .map(|d| {
            DRAW_STEPS
                .iter()
                .position(|&s| s >= d)
                .unwrap_or(DRAW_STEPS.len() - 1)
        })
        .unwrap_or(2);
    let mut app = Budget {
        wire,
        draw_step,
        table: encode.is_none_or(|e| e < ENCODE_PER_PIXEL_MS),
    };
    match cli.shot {
        Some(path) => {
            roundpanel::shoot(&mut app, cli.at_us, cli.frames, cli.options.format, &path)?;
            println!("wrote {}", path.display());
            Ok(())
        }
        None => roundpanel::run_with(app, cli.options),
    }
}
