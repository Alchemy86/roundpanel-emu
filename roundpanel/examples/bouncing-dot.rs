//! The smallest application that uses everything the harness offers: it moves,
//! it takes touch, and it reads both side keys.
//!
//!     cargo run --release --example bouncing-dot
//!
//! Drag anywhere on the glass to throw the dot at your finger. PWR cycles the
//! colour, BOOT turns the trail on and off. Start here when you are writing
//! your own.

use roundpanel::{rgb, App, Ctx, Frame, Rgb, SideKey};

const PALETTE: [Rgb; 4] = [
    rgb(0x4e_a3_ff),
    rgb(0x3d_dc_97),
    rgb(0xff_b3_4e),
    rgb(0xff_5e_7a),
];

struct Dot {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
    colour: usize,
    trail: bool,
}

impl Default for Dot {
    fn default() -> Self {
        Dot {
            x: 206.0,
            y: 140.0,
            vx: 130.0,
            vy: 95.0,
            colour: 0,
            trail: true,
        }
    }
}

impl App for Dot {
    fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        // The events the hardware would have raised, in the order it raised
        // them.
        if ctx.pressed(SideKey::Pwr) {
            self.colour = (self.colour + 1) % PALETTE.len();
        }
        if ctx.pressed(SideKey::Boot) {
            self.trail = !self.trail;
        }
        // A finger on the glass takes the dot with it.
        if let Some((tx, ty)) = ctx.touch {
            self.vx += (tx as f32 - self.x) * 0.20;
            self.vy += (ty as f32 - self.y) * 0.20;
        }

        let dt = 1.0 / 60.0;
        self.x += self.vx * dt;
        self.y += self.vy * dt;

        // The wall is the glass, not the buffer: this screen is a circle, and
        // bouncing off the square would leave the dot in a corner no dot of
        // the real module can light.
        let (cx, cy) = f.centre();
        let r = f.width() as f32 / 2.0 - 16.0;
        let (dx, dy) = (self.x - cx as f32, self.y - cy as f32);
        let d = (dx * dx + dy * dy).sqrt();
        if d > r {
            let (nx, ny) = (dx / d, dy / d);
            let dot = self.vx * nx + self.vy * ny;
            self.vx -= 2.0 * dot * nx;
            self.vy -= 2.0 * dot * ny;
            self.x = cx as f32 + nx * r;
            self.y = cy as f32 + ny * r;
        }
        // Friction, so a throw settles instead of running away.
        self.vx *= 0.995;
        self.vy *= 0.995;

        if self.trail {
            fade(f);
        } else {
            f.clear(rgb(0x0d_11_17));
        }
        f.ring(cx, cy, f.width() as i32 / 2 - 2, 2, rgb(0x23_2a_36));
        f.disc(self.x as i32, self.y as i32, 12, PALETTE[self.colour]);
        f.text_centred(cx, 30, "DRAG ME", 2, rgb(0x5a_63_72));
        f.text_centred(
            cx,
            f.height() as i32 - 46,
            "1 PWR COLOUR  2 BOOT TRAIL",
            2,
            rgb(0x5a_63_72),
        );
    }
}

/// Darken the whole buffer a little. The framebuffer keeps what you left in
/// it, so this is all a trail takes.
fn fade(f: &mut Frame) {
    for px in f.as_rgb8_mut() {
        *px = (*px as u16 * 15 / 16) as u8;
    }
}

fn main() -> Result<(), String> {
    roundpanel::run_cli(Dot::default())
}
