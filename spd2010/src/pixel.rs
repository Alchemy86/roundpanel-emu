//! Interface pixel formats, and how a byte stream decodes into RAM.
//!
//! There are three, and only three. `COLMOD` (3Ah, p.57) defines
//! `DBI[2:0] = 7, 6, 5` and nothing else; section 6.5 (p.16) says the same
//! thing from the memory side -- "RAM must be written as 1 pixel (3 bytes for
//! 16.7M/262k color format, 2 bytes for 65k color format)".
//!
//! # No grey mode
//!
//! The SH8601Z has a [256-grey][gray] mode that carries a grey frame one byte
//! a pixel, arriving in memory bit-exact. **That mode does not exist on this
//! chip.** The closest thing is 16.7M colour with R = G = B, at three
//! times the bytes, or 65k colour at two thirds of them and some rounding.
//! The rounding is small and bounded -- see [`PixelFormat::max_grey_error`] --
//! but it is not zero, and a harness that asserts a zero round trip on this
//! panel in anything but 16.7M colour is asserting something false.
//!
//! [gray]: crate::cmd::colmod

use crate::cmd::colmod;

/// A pixel format the control interface can be set to.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum PixelFormat {
    /// 24 bit/pixel, 16.7M colour: R, G, B, one byte each. `DBI = 7`.
    Rgb888,
    /// 18 bit/pixel, 262K colour: three bytes, each `C[5:0]` in the high six
    /// bits. `DBI = 6`.
    ///
    /// Section 6.5 counts this as three bytes a pixel, the same as 16.7M, so
    /// the low two bits of each byte are carried on the wire and dropped by
    /// the panel rather than being packed away.
    Rgb666,
    /// 16 bit/pixel, 65K colour: two bytes, `RRRRRGGG GGGBBBBB`, high byte
    /// first. `DBI = 5`.
    Rgb565,
}

impl PixelFormat {
    /// The format a `COLMOD` (3Ah) parameter selects, or `None` if the byte is
    /// not one of p.57's three.
    ///
    /// The high five bits are drawn fixed at `01110b`. A parameter that does
    /// not carry them is rejected rather than masked away: both real drivers
    /// send the full byte (`0x77`, `0x76`, `0x75`), so a host that sends a
    /// bare `DBI` value has a bug worth seeing.
    pub const fn from_colmod(v: u8) -> Option<Self> {
        if v & !colmod::DBI_MASK != colmod::FIXED_HIGH {
            return None;
        }
        match v & colmod::DBI_MASK {
            7 => Some(PixelFormat::Rgb888),
            6 => Some(PixelFormat::Rgb666),
            5 => Some(PixelFormat::Rgb565),
            _ => None,
        }
    }

    /// The `COLMOD` parameter that selects this format.
    pub const fn to_colmod(self) -> u8 {
        match self {
            PixelFormat::Rgb888 => colmod::RGB888,
            PixelFormat::Rgb666 => colmod::RGB666,
            PixelFormat::Rgb565 => colmod::RGB565,
        }
    }

    /// Bytes of interface data per pixel, section 6.5, p.16.
    pub const fn bytes_per_pixel(self) -> usize {
        match self {
            PixelFormat::Rgb888 | PixelFormat::Rgb666 => 3,
            PixelFormat::Rgb565 => 2,
        }
    }

    /// The largest error an 8-bit grey can pick up going out in this format
    /// and coming back as the average of three channels, taken over all 256
    /// greys.
    ///
    /// Only 16.7M colour is lossless. 262k truncates each channel to six bits
    /// and loses at most 3, at grey 3; 65k gives red and blue five bits each
    /// and loses at most 6, at grey 7. Both figures are measured by the
    /// crate's tests, which exhaust all 256 values rather than assert a number
    /// someone chose -- an earlier draft of this method guessed both and got
    /// both wrong, and the test is what said so.
    ///
    /// A four-grey palette is a special case worth knowing: 255, 170, 85 and
    /// 0 all survive 262k *exactly*, because each is already a six-bit value
    /// replicated, and none of them survives 65k. So the worst case here is
    /// not the error a given frame actually sees, and a harness should measure
    /// its own frame rather than reach for this bound.
    pub const fn max_grey_error(self) -> u8 {
        match self {
            PixelFormat::Rgb888 => 0,
            PixelFormat::Rgb666 => 3,
            PixelFormat::Rgb565 => 6,
        }
    }

    /// Decode one pixel's worth of interface bytes into the 24-bit RAM word.
    ///
    /// `bytes.len()` must be [`Self::bytes_per_pixel`]; the caller guarantees
    /// it, so this cannot fail.
    pub fn decode(self, bytes: &[u8]) -> [u8; 3] {
        match self {
            PixelFormat::Rgb888 => [bytes[0], bytes[1], bytes[2]],
            // The two low bits of each byte are dropped by the panel, so the
            // stored word is the top six bits replicated down -- the usual
            // widening that keeps 0x3F mapping to 0xFF rather than 0xFC.
            PixelFormat::Rgb666 => {
                let c = |b: u8| {
                    let v = b >> 2;
                    (v << 2) | (v >> 4)
                };
                [c(bytes[0]), c(bytes[1]), c(bytes[2])]
            }
            PixelFormat::Rgb565 => {
                let v = u16::from_be_bytes([bytes[0], bytes[1]]);
                let r = ((v >> 11) & 0x1F) as u8;
                let g = ((v >> 5) & 0x3F) as u8;
                let b = (v & 0x1F) as u8;
                [
                    (r << 3) | (r >> 2),
                    (g << 2) | (g >> 4),
                    (b << 3) | (b >> 2),
                ]
            }
        }
    }

    /// Encode a 24-bit colour into this format, writing to `out`.
    ///
    /// This is the driver's side of [`Self::decode`]. For 666 and 565 it loses
    /// the bits the interface cannot carry, which is exactly what the real
    /// link does.
    pub fn encode(self, rgb: [u8; 3], out: &mut [u8]) {
        match self {
            PixelFormat::Rgb888 => out[..3].copy_from_slice(&rgb),
            PixelFormat::Rgb666 => {
                out[0] = rgb[0] & 0xFC;
                out[1] = rgb[1] & 0xFC;
                out[2] = rgb[2] & 0xFC;
            }
            PixelFormat::Rgb565 => {
                let v = ((rgb[0] as u16 >> 3) << 11)
                    | ((rgb[1] as u16 >> 2) << 5)
                    | (rgb[2] as u16 >> 3);
                out[..2].copy_from_slice(&v.to_be_bytes());
            }
        }
    }
}
