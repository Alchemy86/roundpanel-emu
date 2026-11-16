//! One colour table a frame instead of a colour conversion a pixel.
//!
//!     cargo run --release --example colour-table
//!     cargo run --release --example colour-table -- --bench 200
//!     cargo run --release --example colour-table -- --shot table.png --frames 60
//!
//! An indexed 103 x 103 canvas, a 256-colour palette and a fade, put on the
//! panel at 4 x 4 dots a pixel -- the shape of a game that draws in palette
//! indices. Every frame is encoded both ways, `common/colourtable.rs` checks
//! they are the same bytes, and the glass shows what each cost on this
//! machine. On the real board this change took a frame's encode from 18.8 ms
//! to 2.7 ms; see `docs/two-cores.md`.
//!
//! The times on the glass are this desktop's, not the board's. `--bench N`
//! encodes N frames in each of the three pixel formats and prints the
//! averages. BOOT toggles the fade.

#[path = "common/colourtable.rs"]
mod colourtable;

use std::hint::black_box;
use std::time::Instant;

use colourtable::{
    encode_per_pixel, encode_with_table, frame_bytes, resolve, ColourTable, CANVAS, UPSCALE,
};
use roundpanel::{rgb, App, Cli, Ctx, Frame, PixelFormat, Rgb, SideKey};

const INK: Rgb = rgb(0xe6_ea_f2);
const DIM: Rgb = rgb(0x5a_63_72);
const PANEL: Rgb = rgb(0x0b_0e_14);

/// A 256-entry palette: four hues, 64 steps each.
fn palette() -> [Rgb; 256] {
    let hues = [
        rgb(0x4e_a3_ff),
        rgb(0x3d_dc_97),
        rgb(0xff_b3_4e),
        rgb(0xff_5e_7a),
    ];
    let mut p = [[0; 3]; 256];
    for (i, c) in p.iter_mut().enumerate() {
        let hue = hues[i / 64];
        let step = (i % 64) as u32 * 4 + 3;
        *c = hue.map(|ch| (ch as u32 * step / 255) as u8);
    }
    p
}

/// An animated picture in palette indices, integer maths only.
fn paint(canvas: &mut [u8], t: u32) {
    for y in 0..CANVAS {
        for x in 0..CANVAS {
            let (dx, dy) = (x as i32 - 51, y as i32 - 51);
            let d = ((dx * dx + dy * dy) as u32) / 24;
            let ring = (d + t / 2) & 63;
            let band = ((x as u32 + t / 3) / 26 + (y as u32) / 26) & 3;
            canvas[y * CANVAS + x] = (band * 64 + ring) as u8;
        }
    }
}

/// Microseconds, averaged over the frames since the last reset.
#[derive(Default)]
struct Average {
    total_us: f64,
    n: u32,
}

impl Average {
    fn add(&mut self, since: Instant) {
        self.total_us += since.elapsed().as_secs_f64() * 1e6;
        self.n += 1;
    }

    fn us(&self) -> u32 {
        if self.n == 0 {
            0
        } else {
            (self.total_us / self.n as f64) as u32
        }
    }
}

struct Demo {
    palette: [Rgb; 256],
    canvas: Vec<u8>,
    per_pixel_out: Vec<u8>,
    table_out: Vec<u8>,
    format: PixelFormat,
    fading: bool,
    t: u32,
    per_pixel: Average,
    build: Average,
    table: Average,
    mismatches: u32,
}

impl Demo {
    fn new(format: PixelFormat) -> Demo {
        Demo {
            palette: palette(),
            canvas: vec![0; CANVAS * CANVAS],
            per_pixel_out: vec![0; frame_bytes(format)],
            table_out: vec![0; frame_bytes(format)],
            format,
            fading: true,
            t: 0,
            per_pixel: Average::default(),
            build: Average::default(),
            table: Average::default(),
            mismatches: 0,
        }
    }

    fn fade(&self) -> u16 {
        if !self.fading {
            return 256;
        }
        let phase = (self.t % 120) as i32 - 60;
        (160 + 96 * (60 - phase.abs()) / 60) as u16
    }

    /// One frame: paint it, then encode it both ways and time each.
    fn step(&mut self) {
        self.t += 1;
        paint(&mut self.canvas, self.t);
        let fade = self.fade();

        let t = Instant::now();
        encode_per_pixel(
            black_box(&self.canvas),
            &self.palette,
            fade,
            self.format,
            &mut self.per_pixel_out,
        );
        black_box(&self.per_pixel_out);
        self.per_pixel.add(t);

        let t = Instant::now();
        let table = ColourTable::build(&self.palette, fade, self.format);
        black_box(&table);
        self.build.add(t);
        let t = Instant::now();
        encode_with_table(black_box(&self.canvas), &table, &mut self.table_out);
        black_box(&self.table_out);
        self.table.add(t);

        if self.per_pixel_out != self.table_out {
            self.mismatches += 1;
        }
    }
}

impl App for Demo {
    fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        if ctx.pressed(SideKey::Boot) {
            self.fading = !self.fading;
        }
        self.step();
        let fade = self.fade();
        for (i, &index) in self.canvas.iter().enumerate() {
            let (x, y) = ((i % CANVAS * UPSCALE) as i32, (i / CANVAS * UPSCALE) as i32);
            let s = UPSCALE as i32;
            f.rect(x, y, s, s, resolve(&self.palette, index, fade));
        }

        let (cx, _) = f.centre();
        f.rect(cx - 120, 150, 240, 112, PANEL);
        f.text_centred(cx, 160, "ENCODE A FRAME - THIS PC", 2, DIM);
        let per = self.per_pixel.us();
        let tab = self.build.us() + self.table.us();
        f.text_centred(cx, 184, &format!("PER PIXEL {per} US"), 2, INK);
        f.text_centred(cx, 204, &format!("ONE TABLE {tab} US"), 2, INK);
        let same = if self.mismatches == 0 {
            "SAME BYTES"
        } else {
            "BYTES DIFFER"
        };
        f.text_centred(cx, 230, same, 2, DIM);
        f.text_centred(cx, 244, "2 BOOT FADE", 2, DIM);

        if self.per_pixel.n >= 120 {
            self.per_pixel = Average::default();
            self.build = Average::default();
            self.table = Average::default();
        }
    }
}

fn bench(frames: u32) -> Result<(), String> {
    println!(
        "encoding {frames} frames of 412 x 412 from a 103 x 103 indexed canvas, on this machine"
    );
    println!(
        "{:<8} {:>14} {:>12} {:>12} {:>8}",
        "format", "per pixel us", "table us", "build us", "faster"
    );
    for format in [
        PixelFormat::Rgb565,
        PixelFormat::Rgb666,
        PixelFormat::Rgb888,
    ] {
        let mut demo = Demo::new(format);
        for _ in 0..frames {
            demo.step();
        }
        if demo.mismatches > 0 {
            return Err(format!(
                "{format:?}: the two encoders disagreed on {} frames",
                demo.mismatches
            ));
        }
        let per = demo.per_pixel.us();
        let build = demo.build.us();
        let table = demo.table.us() + build;
        println!(
            "{:<8} {:>14} {:>12} {:>12} {:>7.1}x",
            format!("{format:?}"),
            per,
            table,
            build,
            per as f64 / table.max(1) as f64
        );
    }
    println!(
        "times are this desktop's; the board is far slower and the ratio is not the same there"
    );
    Ok(())
}

fn main() -> Result<(), String> {
    let mut rest = Vec::new();
    let mut bench_frames = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--bench" => {
                let n = args.next().ok_or("--bench wants a frame count")?;
                bench_frames = Some(n.parse::<u32>().map_err(|e| e.to_string())?);
            }
            _ => rest.push(a),
        }
    }
    if let Some(n) = bench_frames {
        return bench(n.max(1));
    }
    let mut cli = Cli::parse(rest)
        .map_err(|e| format!("{e}\n  --bench N    encode N frames each way, print the averages"))?;
    if cli.options.title == "roundpanel-emu" {
        cli.options.title = "colour-table".into();
    }
    let mut app = Demo::new(PixelFormat::Rgb565);
    match cli.shot {
        Some(path) => {
            roundpanel::shoot(&mut app, cli.at_us, cli.frames, cli.options.format, &path)?;
            println!("wrote {}", path.display());
            Ok(())
        }
        None => roundpanel::run_with(app, cli.options),
    }
}
