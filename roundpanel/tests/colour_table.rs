//! The `colour-table` example's two encoders, held to the same bytes.
//!
//! The table is only a faster way to compute the same thing: one encode per
//! palette entry instead of one per pixel. If a frame ever came out different,
//! the table would be a bug, not an optimisation.

#[path = "../examples/common/colourtable.rs"]
mod colourtable;

use colourtable::{
    encode_per_pixel, encode_with_table, frame_bytes, ColourTable, CANVAS, DOTS, UPSCALE,
};
use roundpanel::{PixelFormat, Rgb};

fn palette(seed: u32) -> [Rgb; 256] {
    let mut s = seed;
    let mut p = [[0; 3]; 256];
    for c in p.iter_mut() {
        for ch in c.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *ch = (s >> 24) as u8;
        }
    }
    p
}

fn canvas() -> Vec<u8> {
    (0..CANVAS * CANVAS)
        .map(|i| (i * 7 + i / CANVAS * 13) as u8)
        .collect()
}

#[test]
fn the_table_and_the_per_pixel_encode_give_the_same_bytes() {
    let c = canvas();
    for format in [
        PixelFormat::Rgb565,
        PixelFormat::Rgb666,
        PixelFormat::Rgb888,
    ] {
        for (seed, fade) in [(1, 256), (2, 200), (3, 0), (4, 131)] {
            let p = palette(seed);
            let mut a = vec![0xAA; frame_bytes(format)];
            let mut b = vec![0x55; frame_bytes(format)];
            encode_per_pixel(&c, &p, fade, format, &mut a);
            encode_with_table(&c, &ColourTable::build(&p, fade, format), &mut b);
            assert!(a == b, "{format:?}, fade {fade}: the encoders disagree");
        }
    }
}

#[test]
fn every_dot_of_a_canvas_pixel_is_its_colour() {
    let format = PixelFormat::Rgb565;
    let p = palette(9);
    let c = canvas();
    let mut out = vec![0; frame_bytes(format)];
    encode_with_table(&c, &ColourTable::build(&p, 256, format), &mut out);
    let (cx, cy) = (40, 77);
    let mut want = [0u8; 2];
    format.encode(p[c[cy * CANVAS + cx] as usize], &mut want);
    for dy in 0..UPSCALE {
        for dx in 0..UPSCALE {
            let i = ((cy * UPSCALE + dy) * DOTS + cx * UPSCALE + dx) * 2;
            assert_eq!(out[i..i + 2], want, "dot ({dx},{dy}) of pixel ({cx},{cy})");
        }
    }
}
