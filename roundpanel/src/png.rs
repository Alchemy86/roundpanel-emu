//! Writing a frame out as a picture.
//!
//! Two writers, because they are pictures of different things. [`write_rect`]
//! is the honest picture of the *buffer*: the whole square dot array, corners
//! included. [`write_glass`] is the honest picture of the *module*: the same
//! square masked to its inscribed circle, transparent outside, because those
//! corners are addresses with no dot behind them.

use std::fs::File;
use std::io::{BufWriter, Result};
use std::path::Path;

/// The dot array as it is: `w` x `h`, three bytes a pixel, no mask.
pub fn write_rect(pixels: &[u8], w: usize, h: usize, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(pixels)?;
    Ok(())
}

/// The mask [`write_glass`] applies, as a buffer: RGBA, opaque inside the
/// inscribed circle and fully transparent outside it.
pub fn glass_rgba(pixels: &[u8], w: usize, h: usize) -> Vec<u8> {
    let d = w.min(h) as i64;
    let mut rgba = vec![0u8; w * h * 4];
    for y in 0..h {
        let dy = 2 * y as i64 - (h as i64 - 1);
        for x in 0..w {
            let dx = 2 * x as i64 - (w as i64 - 1);
            if dx * dx + dy * dy > d * d {
                continue; // off the glass entirely
            }
            let (j, i) = ((y * w + x) * 3, (y * w + x) * 4);
            rgba[i..i + 3].copy_from_slice(&pixels[j..j + 3]);
            rgba[i + 3] = 255;
        }
    }
    rgba
}

/// The module as the glass shows it: masked to the inscribed circle,
/// transparent outside, nothing added.
pub fn write_glass(pixels: &[u8], w: usize, h: usize, path: &Path) -> Result<()> {
    let rgba = glass_rgba(pixels, w, h);
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)?;
        }
    }
    let mut enc = png::Encoder::new(BufWriter::new(File::create(path)?), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mask_drops_the_corners_and_keeps_the_middle() {
        let rgba = glass_rgba(&[255u8; 8 * 8 * 3], 8, 8);
        assert_eq!(rgba[3], 0, "a corner was kept");
        assert_eq!(rgba[((4 * 8) + 4) * 4 + 3], 255, "the middle was dropped");
        assert_eq!(rgba[(4 * 4) + 3], 255, "the top of the circle was dropped");
    }
}
