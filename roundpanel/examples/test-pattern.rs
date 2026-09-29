//! A test pattern, and nothing else.
//!
//!     cargo run --release --example test-pattern
//!
//! Every mark on it is checkable against the real module: the outer ring rides
//! the last lit dot, the crosshair crosses at the centre dot, the ticks along
//! the horizontal diameter are four dots apart -- the column alignment the
//! SPD2010 insists on -- and the twelve wedges say whether your colour order
//! survived the trip. If it looks like this on the hardware, the pipeline is
//! right.

use roundpanel::{rgb, App, Ctx, Frame, Rgb};

const INK: Rgb = rgb(0xe6_ea_f2);
const DIM: Rgb = rgb(0x5a_63_72);
const ACCENT: Rgb = rgb(0x4e_a3_ff);

/// Twelve wedges, round the clock.
const WEDGES: [Rgb; 12] = [
    rgb(0xff_00_00),
    rgb(0xff_7f_00),
    rgb(0xff_ff_00),
    rgb(0x7f_ff_00),
    rgb(0x00_ff_00),
    rgb(0x00_ff_7f),
    rgb(0x00_ff_ff),
    rgb(0x00_7f_ff),
    rgb(0x00_00_ff),
    rgb(0x7f_00_ff),
    rgb(0xff_00_ff),
    rgb(0xff_00_7f),
];

struct TestPattern;

impl App for TestPattern {
    fn draw(&mut self, f: &mut Frame, _ctx: &Ctx) {
        let (cx, cy) = f.centre();
        let r = f.width() as i32 / 2;
        f.clear(rgb(0x0d_11_17));

        // The wedges, drawn as a ring so the middle stays legible.
        for y in -r..r {
            for x in -r..r {
                let d2 = x * x + y * y;
                if d2 > r * r || d2 < (r - 44) * (r - 44) {
                    continue;
                }
                let a = (y as f32).atan2(x as f32) + std::f32::consts::PI;
                let i = (a / (std::f32::consts::TAU / 12.0)) as usize % 12;
                f.set(cx + x, cy + y, WEDGES[i]);
            }
        }

        // Concentric rings at a round 25 dots, so a ruler laid on a photograph
        // of the real screen lands on them.
        for k in 1..=6 {
            f.ring(cx, cy, k * 25, 1, DIM);
        }

        // The crosshair, and the centre dot it crosses on.
        f.rect(cx, cy - r + 46, 1, 2 * (r - 46), ACCENT);
        f.rect(cx - r + 46, cy, 2 * (r - 46), 1, ACCENT);
        f.disc(cx, cy, 3, INK);

        // Ticks every four dots, centred: the SPD2010's column alignment made
        // visible. Every fourth tick is taller, so the group of four the chip
        // addresses at once is the thing you see.
        let ruler_x = cx - 80;
        for i in 0..41 {
            let tall = i % 4 == 0;
            f.rect(
                ruler_x + i * 4,
                cy + 46,
                1,
                if tall { 9 } else { 5 },
                if tall { INK } else { DIM },
            );
        }

        // A plate under the wording, so the crosshair does not run through it.
        f.rect(cx - 96, cy - 74, 192, 44, rgb(0x0d_11_17));
        f.text_centred(cx, cy - 68, "ROUNDPANEL", 3, INK);
        f.text_centred(cx, cy - 44, "412 X 412 SPD2010", 2, DIM);
        f.rect(cx - 74, cy + 62, 148, 16, rgb(0x0d_11_17));
        f.text_centred(cx, cy + 66, "TICKS ARE 4 DOTS", 2, DIM);
    }
}

fn main() -> Result<(), String> {
    roundpanel::run_cli(TestPattern)
}
