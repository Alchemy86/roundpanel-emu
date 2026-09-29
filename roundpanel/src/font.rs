//! A 3x5 pixel font, so the harness can label its own furniture.
//!
//! Five rows a glyph, three columns, uppercase and digits and three marks.
//! It is here because a window with two unlabelled keys on it is a puzzle, and
//! because an example application wants to be able to put a number on the
//! screen without pulling in a text stack.
//!
//! **It is not a typeface and it is not what you should ship.** Anything that
//! wants real text on this panel wants a real font renderer; this is the
//! smallest thing that makes a counter legible.

/// Glyph height, in pixels.
pub const GLYPH_H: usize = 5;
/// Glyph width, in pixels.
pub const GLYPH_W: usize = 3;

/// Five rows per glyph; bits 2..0 are the three columns, left to right.
/// Order: space, A-Z, 0-9, then `:`, `-`, `/`.
const GLYPHS: [[u8; GLYPH_H]; 40] = [
    [0, 0, 0, 0, 0],
    [2, 5, 7, 5, 5],
    [6, 5, 6, 5, 6],
    [3, 4, 4, 4, 3],
    [6, 5, 5, 5, 6],
    [7, 4, 6, 4, 7],
    [7, 4, 6, 4, 4],
    [3, 4, 5, 5, 3],
    [5, 5, 7, 5, 5],
    [7, 2, 2, 2, 7],
    [1, 1, 1, 5, 2],
    [5, 5, 6, 5, 5],
    [4, 4, 4, 4, 7],
    [5, 7, 7, 5, 5],
    [5, 7, 5, 5, 5],
    [2, 5, 5, 5, 2],
    [6, 5, 6, 4, 4],
    [2, 5, 5, 7, 3],
    [6, 5, 6, 5, 5],
    [3, 4, 2, 1, 6],
    [7, 2, 2, 2, 2],
    [5, 5, 5, 5, 2],
    [5, 5, 5, 2, 2],
    [5, 5, 7, 7, 5],
    [5, 5, 2, 5, 5],
    [5, 5, 2, 2, 2],
    [7, 1, 2, 4, 7],
    [7, 5, 5, 5, 7],
    [2, 6, 2, 2, 7],
    [6, 1, 2, 4, 7],
    [6, 1, 2, 1, 6],
    [5, 5, 7, 1, 1],
    [7, 4, 6, 1, 6],
    [3, 4, 6, 5, 2],
    [7, 1, 2, 2, 2],
    [2, 5, 2, 5, 2],
    [2, 5, 3, 1, 6],
    [0, 2, 0, 2, 0],
    [0, 0, 7, 0, 0],
    [1, 1, 2, 4, 4],
];
/// The five-row bitmap for `c`, or the blank glyph for anything unmapped.
/// Lowercase is folded to uppercase; the font has no lowercase forms.
pub fn glyph(c: char) -> &'static [u8; GLYPH_H] {
    let i = match c.to_ascii_uppercase() {
        'A'..='Z' => 1 + (c.to_ascii_uppercase() as usize - 'A' as usize),
        '0'..='9' => 27 + (c as usize - '0' as usize),
        ':' => 37,
        '-' => 38,
        '/' => 39,
        _ => 0,
    };
    &GLYPHS[i]
}

/// Width in pixels of `text` at `scale`, including the one-pixel gaps between
/// glyphs but not after the last one.
pub fn text_width(text: &str, scale: i32) -> i32 {
    let n = text.chars().count() as i32;
    if n == 0 {
        0
    } else {
        n * (GLYPH_W as i32 + 1) * scale - scale
    }
}
