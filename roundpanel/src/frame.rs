//! The framebuffer your application draws into.
//!
//! One frame is the panel's whole dot array: 412 x 412, three bytes a pixel,
//! R, G, B, row-major. That is the module's real resolution and there is no
//! scaling anywhere between here and the chip model, so what you write at
//! `(x, y)` is the dot at `(x, y)`.
//!
//! # The buffer is square and the glass is round
//!
//! The SPD2010 addresses a rectangle, so the framebuffer is one. The module
//! only lights the dots inside the inscribed circle, and its four corners are
//! addresses with no dot behind them. Drawing there is not an error -- the
//! chip accepts it, and this crate's own window draws the circle only -- but
//! nothing you put in a corner will ever be seen on the hardware.
//! [`Frame::on_glass`] is the test, and [`Frame::clip_to_glass`] is the
//! blunt way to be sure.

use crate::font::{glyph, GLYPH_H, GLYPH_W};

/// The module's lit columns.
pub const WIDTH: usize = 412;
/// The module's lit rows.
pub const HEIGHT: usize = 412;

/// One pixel: red, green, blue.
pub type Rgb = [u8; 3];

pub const BLACK: Rgb = [0, 0, 0];
pub const WHITE: Rgb = [255, 255, 255];

/// A colour from the `0xRRGGBB` form a designer will hand you.
pub const fn rgb(hex: u32) -> Rgb {
    [(hex >> 16) as u8, (hex >> 8) as u8, hex as u8]
}

/// A 412 x 412 RGB framebuffer.
#[derive(Clone)]
pub struct Frame {
    px: Vec<u8>,
    w: usize,
    h: usize,
    clip_glass: bool,
}

impl Default for Frame {
    fn default() -> Self {
        Frame::new()
    }
}

impl Frame {
    /// A black frame the size of the real panel.
    pub fn new() -> Frame {
        Frame {
            px: vec![0; WIDTH * HEIGHT * 3],
            w: WIDTH,
            h: HEIGHT,
            clip_glass: false,
        }
    }

    pub fn width(&self) -> usize {
        self.w
    }

    pub fn height(&self) -> usize {
        self.h
    }

    /// The centre dot, which is also the centre of the glass.
    pub fn centre(&self) -> (i32, i32) {
        (self.w as i32 / 2, self.h as i32 / 2)
    }

    /// The raw buffer: `width * height * 3` bytes, R, G, B, row-major. This is
    /// what goes to the panel.
    pub fn as_rgb8(&self) -> &[u8] {
        &self.px
    }

    pub fn as_rgb8_mut(&mut self) -> &mut [u8] {
        &mut self.px
    }

    /// Whether a dot is inside the round glass.
    ///
    /// Doubled units, so the centre of an even-sided panel -- which falls
    /// between dots -- comes out exact rather than half a dot off.
    pub fn on_glass(&self, x: i32, y: i32) -> bool {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return false;
        }
        let d = self.w.min(self.h) as i32;
        let dx = 2 * x - (self.w as i32 - 1);
        let dy = 2 * y - (self.h as i32 - 1);
        dx * dx + dy * dy <= d * d
    }

    /// Drop everything drawn outside the glass from now on.
    ///
    /// Off by default, because the honest default is to let you write what the
    /// chip would accept. Turn it on and the corners simply stop taking ink,
    /// which is the cheapest way to stop a rectangular layout from lying to
    /// you about how much room there is. [`Frame::clear`] is the one call it
    /// does not apply to.
    pub fn clip_to_glass(&mut self, on: bool) {
        self.clip_glass = on;
    }

    /// Fill the whole buffer, [`Frame::clip_to_glass`] or not: a frame you
    /// cannot blank is a frame with the last one still under it.
    pub fn clear(&mut self, c: Rgb) {
        for i in (0..self.px.len()).step_by(3) {
            self.px[i..i + 3].copy_from_slice(&c);
        }
    }

    pub fn set(&mut self, x: i32, y: i32, c: Rgb) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        if self.clip_glass && !self.on_glass(x, y) {
            return;
        }
        let i = (y as usize * self.w + x as usize) * 3;
        self.px[i..i + 3].copy_from_slice(&c);
    }

    pub fn get(&self, x: i32, y: i32) -> Rgb {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return BLACK;
        }
        let i = (y as usize * self.w + x as usize) * 3;
        [self.px[i], self.px[i + 1], self.px[i + 2]]
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgb) {
        for dy in 0..h {
            for dx in 0..w {
                self.set(x + dx, y + dy, c);
            }
        }
    }

    /// A rectangle's outline, `t` pixels thick, drawn inside the bounds given.
    pub fn rect_outline(&mut self, x: i32, y: i32, w: i32, h: i32, t: i32, c: Rgb) {
        self.rect(x, y, w, t, c);
        self.rect(x, y + h - t, w, t, c);
        self.rect(x, y, t, h, c);
        self.rect(x + w - t, y, t, h, c);
    }

    pub fn disc(&mut self, cx: i32, cy: i32, r: i32, c: Rgb) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    self.set(cx + dx, cy + dy, c);
                }
            }
        }
    }

    /// A ring of outer radius `r`, `t` pixels thick, drawn inwards.
    pub fn ring(&mut self, cx: i32, cy: i32, r: i32, t: i32, c: Rgb) {
        let inner = (r - t).max(0);
        for dy in -r..=r {
            for dx in -r..=r {
                let d2 = dx * dx + dy * dy;
                if d2 <= r * r && d2 > inner * inner {
                    self.set(cx + dx, cy + dy, c);
                }
            }
        }
    }

    /// A one-pixel line, Bresenham.
    pub fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgb) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        loop {
            self.set(x, y, c);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Draw `text` in the built-in 3x5 font, `scale` pixels to the glyph dot.
    /// Returns the x just past the last glyph.
    pub fn text(&mut self, x: i32, y: i32, s: &str, scale: i32, c: Rgb) -> i32 {
        let pitch = (GLYPH_W as i32 + 1) * scale;
        let mut cx = x;
        for ch in s.chars() {
            for (row, bits) in glyph(ch).iter().enumerate() {
                for col in 0..GLYPH_W {
                    if (bits >> (GLYPH_W - 1 - col)) & 1 == 1 {
                        self.rect(
                            cx + col as i32 * scale,
                            y + row as i32 * scale,
                            scale,
                            scale,
                            c,
                        );
                    }
                }
            }
            cx += pitch;
        }
        cx - scale
    }

    /// The same string, centred on `cx`.
    pub fn text_centred(&mut self, cx: i32, y: i32, s: &str, scale: i32, c: Rgb) {
        let w = crate::font::text_width(s, scale);
        self.text(cx - w / 2, y, s, scale, c);
    }

    /// How tall a line of [`Frame::text`] is, gap included.
    pub fn line_height(scale: i32) -> i32 {
        (GLYPH_H as i32 + 3) * scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_frame_is_the_panels_own_size_and_black() {
        let f = Frame::new();
        assert_eq!((f.width(), f.height()), (412, 412));
        assert_eq!(f.as_rgb8().len(), 412 * 412 * 3);
        assert!(f.as_rgb8().iter().all(|&b| b == 0));
    }

    #[test]
    fn drawing_off_the_edge_is_dropped_rather_than_wrapped() {
        let mut f = Frame::new();
        f.set(-1, 0, WHITE);
        f.set(0, -1, WHITE);
        f.set(412, 0, WHITE);
        f.set(0, 412, WHITE);
        assert!(f.as_rgb8().iter().all(|&b| b == 0), "something wrapped");
    }

    /// The corners of the buffer are addresses with no dot behind them, and
    /// the middle of every edge is on the glass. Get this backwards and a
    /// layout is wrong by a whole quadrant.
    #[test]
    fn the_glass_is_the_inscribed_circle() {
        let f = Frame::new();
        assert!(!f.on_glass(0, 0));
        assert!(!f.on_glass(411, 411));
        assert!(f.on_glass(206, 206));
        assert!(f.on_glass(206, 0), "the top of the circle");
        assert!(f.on_glass(0, 206), "the left of the circle");
    }

    #[test]
    fn clipping_to_the_glass_drops_the_corners_and_keeps_the_middle() {
        let mut f = Frame::new();
        f.clip_to_glass(true);
        f.rect(0, 0, 412, 412, WHITE);
        assert_eq!(f.get(0, 0), BLACK, "a corner took ink");
        assert_eq!(f.get(206, 206), WHITE, "the middle did not");
    }

    /// `clear` is the one call that is allowed past the clip: a frame you
    /// cannot blank is a frame with last frame's picture still under it.
    #[test]
    fn clear_fills_the_whole_buffer_even_when_clipping() {
        let mut f = Frame::new();
        f.clip_to_glass(true);
        f.clear(WHITE);
        assert_eq!(f.get(0, 0), WHITE);
    }

    #[test]
    fn text_reports_the_width_it_draws() {
        let mut f = Frame::new();
        let end = f.text(10, 10, "ROUND", 2, WHITE);
        assert_eq!(end - 10, crate::font::text_width("ROUND", 2));
        assert!(f.as_rgb8().iter().any(|&b| b != 0), "nothing was drawn");
    }
}
