//! Turning an indexed canvas into panel bytes: once per pixel, or once per
//! colour.
//!
//! Shared by the `colour-table` example and `roundpanel/tests/colour_table.rs`.
//! Written as device code: no allocation in either encoder, integer maths
//! only, so it can be lifted into firmware as it stands.
//!
//! The canvas is 103 x 103, one byte a pixel, each byte an index into a
//! 256-entry palette. On the panel each canvas pixel is a 4 x 4 block of dots
//! (103 x 4 = 412). A colour is the palette entry under the frame's fade, which
//! stands in for whatever your own colour lookup does per pixel -- a ramp, a
//! tint, a palette swap.
//!
//! [`encode_per_pixel`] resolves and encodes the colour of every canvas pixel.
//! [`ColourTable::build`] resolves and encodes all 256 once, for this frame's
//! palette and fade, and [`encode_with_table`] is then a copy per pixel. A
//! pixel's bytes depend on its index and nothing else, so the two must agree
//! byte for byte; the test holds them to it.

#![allow(dead_code)]

use roundpanel::{PixelFormat, Rgb};

/// Canvas pixels a side.
pub const CANVAS: usize = 103;
/// Panel dots per canvas pixel, each way.
pub const UPSCALE: usize = 4;
/// Panel dots a side.
pub const DOTS: usize = CANVAS * UPSCALE;
/// The most bytes a dot takes on the wire (16.7M and 262k colour).
pub const MAX_BPP: usize = 3;

/// A palette entry under a fade of `fade / 256`. The per-pixel work the table
/// takes off the hot loop.
#[inline]
pub fn resolve(palette: &[Rgb; 256], index: u8, fade: u16) -> Rgb {
    let [r, g, b] = palette[index as usize];
    let f = fade as u32;
    [
        ((r as u32 * f) >> 8) as u8,
        ((g as u32 * f) >> 8) as u8,
        ((b as u32 * f) >> 8) as u8,
    ]
}

/// Every panel row of `canvas` into `out`, in `format`, resolving and
/// encoding each canvas pixel's colour as it is reached.
pub fn encode_per_pixel(
    canvas: &[u8],
    palette: &[Rgb; 256],
    fade: u16,
    format: PixelFormat,
    out: &mut [u8],
) {
    let bpp = format.bytes_per_pixel();
    let row_bytes = DOTS * bpp;
    for (cy, row) in canvas.chunks_exact(CANVAS).enumerate() {
        let top = cy * UPSCALE * row_bytes;
        let line = &mut out[top..top + row_bytes];
        for (dots, &index) in line.chunks_exact_mut(UPSCALE * bpp).zip(row) {
            let mut px = [0u8; MAX_BPP];
            format.encode(resolve(palette, index, fade), &mut px);
            for dot in dots.chunks_exact_mut(bpp) {
                dot.copy_from_slice(&px[..bpp]);
            }
        }
        repeat_row(out, top, row_bytes);
    }
}

/// Every index's run of [`UPSCALE`] encoded dots, for one frame's palette,
/// fade and format.
pub struct ColourTable {
    bpp: usize,
    runs: [[u8; UPSCALE * MAX_BPP]; 256],
}

impl ColourTable {
    /// 256 resolves and 256 encodes, however many pixels the frame has.
    pub fn build(palette: &[Rgb; 256], fade: u16, format: PixelFormat) -> ColourTable {
        let bpp = format.bytes_per_pixel();
        let mut runs = [[0u8; UPSCALE * MAX_BPP]; 256];
        for (index, run) in runs.iter_mut().enumerate() {
            let mut px = [0u8; MAX_BPP];
            format.encode(resolve(palette, index as u8, fade), &mut px);
            for dot in run[..UPSCALE * bpp].chunks_exact_mut(bpp) {
                dot.copy_from_slice(&px[..bpp]);
            }
        }
        ColourTable { bpp, runs }
    }
}

/// The same bytes as [`encode_per_pixel`], read out of `table`.
pub fn encode_with_table(canvas: &[u8], table: &ColourTable, out: &mut [u8]) {
    let run = UPSCALE * table.bpp;
    let row_bytes = DOTS * table.bpp;
    for (cy, row) in canvas.chunks_exact(CANVAS).enumerate() {
        let top = cy * UPSCALE * row_bytes;
        let line = &mut out[top..top + row_bytes];
        for (dots, &index) in line.chunks_exact_mut(run).zip(row) {
            dots.copy_from_slice(&table.runs[index as usize][..run]);
        }
        repeat_row(out, top, row_bytes);
    }
}

/// The upscale downwards: the three panel rows under a canvas row are copies
/// of the first, not three more encodes.
fn repeat_row(out: &mut [u8], top: usize, row_bytes: usize) {
    for k in 1..UPSCALE {
        out.copy_within(top..top + row_bytes, top + k * row_bytes);
    }
}

/// Bytes a whole frame takes in `format`.
pub fn frame_bytes(format: PixelFormat) -> usize {
    DOTS * DOTS * format.bytes_per_pixel()
}
