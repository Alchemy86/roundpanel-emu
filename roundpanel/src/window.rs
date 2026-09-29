//! The window: the panel, the board's two side keys, and a mouse for a finger.
//!
//! Everything drawn here is furniture -- a picture of the *hardware*, not of
//! the screen. None of it goes through [`crate::panel`], so the round area in
//! the middle still shows only what your application drew and what the chip
//! model handed back. The two buffers are composited here and nowhere earlier,
//! which is what keeps the emulated screen honest.
//!
//! # Where the keys are drawn is not a measurement
//!
//! Waveshare document that both buttons are on the side. They publish no
//! azimuth, side or spacing, and none has been measured here, so the two caps
//! are simply placed symmetrically on the right, far enough apart to be
//! separate targets. The count and the labels are the hardware; the placement
//! is a usable window.

use std::path::Path;
use std::time::Instant;

use minifb::{Key, KeyRepeat, MouseButton, MouseMode, Window, WindowOptions};

use crate::font::{text_width, GLYPH_H, GLYPH_W};
use crate::frame::Frame;
use crate::panel::Panel;
use crate::{App, Ctx, Event, Options, SideKey};

const SHELL: u32 = 0x00_14_16_1a;
const SHELL_EDGE: u32 = 0x00_2c_30_38;
const BEZEL: u32 = 0x00_2a_2e_36;
const KEY: u32 = 0x00_4a_51_5e;
const KEY_LIT: u32 = 0x00_4e_a3_ff;
const LABEL: u32 = 0x00_c8_cd_d8;
const DIM: u32 = 0x00_78_80_90;

/// Radius of a drawn key cap, in window pixels.
const KEY_R: i32 = 24;
/// Gap between the panel's bounding box and the nearest point of a cap.
const KEY_GAP: i32 = 16;
/// How far a press may travel and still count as a tap, in panel dots.
const TAP_SLOP: i32 = 8;

struct Layout {
    scale: usize,
    width: usize,
    height: usize,
    screen_x: i32,
    screen_y: i32,
    side: i32,
    status_y: i32,
    furniture: bool,
}

impl Layout {
    fn new(dots: usize, scale: usize, furniture: bool) -> Layout {
        let side = (dots * scale) as i32;
        let margin = if furniture { 20 } else { 0 };
        let gutter = if furniture {
            KEY_GAP + KEY_R * 2 + 16
        } else {
            0
        };
        let status_h = if furniture { 74 } else { 0 };
        Layout {
            scale,
            width: (margin + side + gutter) as usize,
            height: (margin + side + status_h) as usize,
            screen_x: margin,
            screen_y: margin,
            side,
            status_y: margin + side + 14,
            furniture,
        }
    }

    fn screen_centre(&self) -> (f32, f32) {
        (
            self.screen_x as f32 + self.side as f32 / 2.0,
            self.screen_y as f32 + self.side as f32 / 2.0,
        )
    }

    fn key_centre(&self, i: usize) -> (f32, f32) {
        let (cx, cy) = self.screen_centre();
        let off = [-0.45f32, 0.45][i];
        (
            cx + self.side as f32 / 2.0 + KEY_GAP as f32 + KEY_R as f32,
            cy + off * self.side as f32 / 2.0,
        )
    }

    fn key_at(&self, mx: f32, my: f32) -> Option<SideKey> {
        if !self.furniture {
            return None;
        }
        SideKey::ALL.iter().enumerate().find_map(|(i, k)| {
            let (kx, ky) = self.key_centre(i);
            let (dx, dy) = (mx - kx, my - ky);
            (dx * dx + dy * dy <= (KEY_R * KEY_R) as f32).then_some(*k)
        })
    }

    /// Which panel dot a window pixel is, if it is on the panel at all.
    fn dot_at(&self, mx: f32, my: f32) -> Option<(i32, i32)> {
        let x = (mx as i32 - self.screen_x) / self.scale as i32;
        let y = (my as i32 - self.screen_y) / self.scale as i32;
        let on = mx as i32 >= self.screen_x
            && my as i32 >= self.screen_y
            && x < (self.side / self.scale as i32)
            && y < (self.side / self.scale as i32);
        on.then_some((x, y))
    }
}

/// The window's own pixel buffer: `0x00RRGGBB`, nothing to do with the panel's.
struct Surface<'a> {
    px: &'a mut [u32],
    w: usize,
    h: usize,
}

impl Surface<'_> {
    fn set(&mut self, x: i32, y: i32, c: u32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        self.px[y as usize * self.w + x as usize] = c;
    }

    fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: u32) {
        for dy in 0..h {
            for dx in 0..w {
                self.set(x + dx, y + dy, c);
            }
        }
    }

    fn disc(&mut self, cx: i32, cy: i32, r: i32, c: u32) {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy <= r * r {
                    self.set(cx + dx, cy + dy, c);
                }
            }
        }
    }

    fn ring(&mut self, cx: i32, cy: i32, r: i32, t: i32, c: u32) {
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

    fn text(&mut self, x: i32, y: i32, s: &str, scale: i32, c: u32) {
        let pitch = (GLYPH_W as i32 + 1) * scale;
        let mut cx = x;
        for ch in s.chars() {
            for (row, bits) in crate::font::glyph(ch).iter().enumerate() {
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
    }

    /// Blit the panel's readback, masked to the round glass.
    ///
    /// The mask is derived from the readback's own size, not from a constant,
    /// so this stays a blitter.
    fn blit_panel(&mut self, dots: &[u8], w: usize, h: usize, x: i32, y: i32, scale: usize) {
        let d = w.min(h) as i64;
        for py in 0..h {
            let dy = 2 * py as i64 - (h as i64 - 1);
            for px in 0..w {
                let dx = 2 * px as i64 - (w as i64 - 1);
                if dx * dx + dy * dy > d * d {
                    continue;
                }
                let i = (py * w + px) * 3;
                let c = ((dots[i] as u32) << 16) | ((dots[i + 1] as u32) << 8) | dots[i + 2] as u32;
                if scale == 1 {
                    self.set(x + px as i32, y + py as i32, c);
                } else {
                    self.rect(
                        x + (px * scale) as i32,
                        y + (py * scale) as i32,
                        scale as i32,
                        scale as i32,
                        c,
                    );
                }
            }
        }
    }
}

pub fn run<A: App>(mut app: A, opts: Options) -> Result<(), String> {
    let mut panel = Panel::new(opts.format);
    let (dots_w, dots_h) = panel.dots_size();
    if dots_w != dots_h {
        return Err("this harness draws square modules only".into());
    }
    let layout = Layout::new(dots_w, opts.scale.max(1), opts.furniture);

    let mut win = Window::new(
        &opts.title,
        layout.width,
        layout.height,
        WindowOptions::default(),
    )
    .map_err(|e| format!("could not open a window: {e}"))?;
    win.set_target_fps(opts.target_fps);

    println!(
        "roundpanel-emu: {dots_w}x{dots_h} dots at x{}, {:?}",
        layout.scale, opts.format
    );
    println!("  mouse = touch   1 = PWR   2 = BOOT   S = screenshot   Esc = quit");

    let mut buf = vec![0u32; layout.width * layout.height];
    let mut frame = Frame::new();
    let mut events: Vec<Event> = Vec::new();
    let mut keys_down = [false; 2];
    let mut key_lit = [0u64; 2];
    let mut mouse_was_down = false;
    let mut touch = None;
    let mut touch_origin = (0, 0);
    let mut cap_held: Option<SideKey> = None;
    let mut frame_no = 0u64;
    let mut shots = 0usize;
    let epoch = Instant::now();

    while win.is_open() && !win.is_key_down(Key::Escape) {
        let now_us = epoch.elapsed().as_micros() as u64;
        frame_no += 1;
        events.clear();

        for k in win.get_keys_pressed(KeyRepeat::No) {
            match k {
                // Positional: the board's keys have no natural letter, and PWR
                // and BOOT both start with one this window uses already.
                Key::Key1 => press(
                    &mut events,
                    &mut keys_down,
                    &mut key_lit,
                    SideKey::Pwr,
                    now_us,
                ),
                Key::Key2 => press(
                    &mut events,
                    &mut keys_down,
                    &mut key_lit,
                    SideKey::Boot,
                    now_us,
                ),
                Key::S => shots += 1,
                _ => {}
            }
        }
        for k in win.get_keys_released() {
            match k {
                Key::Key1 => release(&mut events, &mut keys_down, SideKey::Pwr),
                Key::Key2 => release(&mut events, &mut keys_down, SideKey::Boot),
                _ => {}
            }
        }

        // The mouse is the finger: down, drag, up -- not one event on the press
        // edge. Without the middle of that there is no way to drag anything.
        let down = win.get_mouse_down(MouseButton::Left);
        let at = win.get_mouse_pos(MouseMode::Discard);
        match (down, mouse_was_down, at) {
            (true, false, Some((mx, my))) => {
                if let Some(k) = layout.key_at(mx, my) {
                    press(&mut events, &mut keys_down, &mut key_lit, k, now_us);
                    cap_held = Some(k);
                } else if let Some((x, y)) = layout.dot_at(mx, my) {
                    if frame.on_glass(x, y) {
                        events.push(Event::TouchDown { x, y });
                        touch = Some((x, y));
                        touch_origin = (x, y);
                    }
                }
            }
            (true, true, Some((mx, my))) if touch.is_some() => {
                if let Some((x, y)) = layout.dot_at(mx, my) {
                    if touch != Some((x, y)) {
                        events.push(Event::TouchMove { x, y });
                        touch = Some((x, y));
                    }
                }
            }
            (false, true, _) => {
                if let Some(k) = cap_held.take() {
                    release(&mut events, &mut keys_down, k);
                }
                if let Some((x, y)) = touch.take() {
                    events.push(Event::TouchUp { x, y });
                    let (ox, oy) = touch_origin;
                    if (x - ox).abs() <= TAP_SLOP && (y - oy).abs() <= TAP_SLOP {
                        events.push(Event::Tap { x, y });
                    }
                }
            }
            _ => {}
        }
        mouse_was_down = down;

        // -- the application ------------------------------------------------
        let ctx = Ctx {
            now_us,
            frame_no,
            events: &events,
            touch,
            keys_down,
        };
        app.draw(&mut frame, &ctx);

        // -- the panel ------------------------------------------------------
        let t0 = Instant::now();
        panel.present(&frame)?;
        let present_us = t0.elapsed().as_micros() as u64;

        // -- the window -----------------------------------------------------
        let mut s = Surface {
            px: &mut buf,
            w: layout.width,
            h: layout.height,
        };
        draw_window(
            &mut s, &layout, &panel, &keys_down, &key_lit, now_us, present_us,
        );

        if shots > 0 {
            shots = 0;
            let stem = opts.shots_dir.join(format!("shot-{}", now_us / 1000));
            let screen = stem.with_extension("png");
            crate::png::write_glass(panel.dots(), dots_w, dots_h, &screen)
                .map_err(|e| format!("{}: {e}", screen.display()))?;
            let whole = Path::new(&stem).with_file_name(format!(
                "{}-window.png",
                stem.file_name().unwrap_or_default().to_string_lossy()
            ));
            write_window_png(&buf, layout.width, layout.height, &whole)
                .map_err(|e| format!("{}: {e}", whole.display()))?;
            println!("wrote {} and {}", screen.display(), whole.display());
        }

        win.update_with_buffer(&buf, layout.width, layout.height)
            .map_err(|e| format!("could not draw: {e}"))?;
    }
    Ok(())
}

fn press(
    events: &mut Vec<Event>,
    down: &mut [bool; 2],
    lit: &mut [u64; 2],
    k: SideKey,
    now_us: u64,
) {
    if down[k as usize] {
        return;
    }
    down[k as usize] = true;
    lit[k as usize] = now_us;
    events.push(Event::KeyDown(k));
}

fn release(events: &mut Vec<Event>, down: &mut [bool; 2], k: SideKey) {
    if !down[k as usize] {
        return;
    }
    down[k as usize] = false;
    events.push(Event::KeyUp(k));
}

#[allow(clippy::too_many_arguments)]
fn draw_window(
    s: &mut Surface<'_>,
    layout: &Layout,
    panel: &Panel,
    keys_down: &[bool; 2],
    key_lit: &[u64; 2],
    now_us: u64,
    present_us: u64,
) {
    s.px.fill(SHELL);
    let (dots_w, dots_h) = panel.dots_size();
    let (cx, cy) = layout.screen_centre();
    let r = layout.side / 2;

    if layout.furniture {
        // The module's own rim, so the round screen reads as a part rather than
        // as a hole in the background.
        s.ring(cx as i32, cy as i32, r + 5, 5, BEZEL);
        s.ring(cx as i32, cy as i32, r + 10, 2, SHELL_EDGE);
    }
    s.blit_panel(
        panel.dots(),
        dots_w,
        dots_h,
        layout.screen_x,
        layout.screen_y,
        layout.scale,
    );

    if !layout.furniture {
        return;
    }

    for (i, k) in SideKey::ALL.iter().enumerate() {
        let (kx, ky) = layout.key_centre(i);
        let recent = now_us.saturating_sub(key_lit[i]) < 120_000;
        let c = if keys_down[i] || recent { KEY_LIT } else { KEY };
        s.disc(kx as i32, ky as i32, KEY_R, c);
        s.ring(kx as i32, ky as i32, KEY_R, 2, SHELL_EDGE);
        let label = k.label();
        s.text(
            kx as i32 - text_width(label, 2) / 2,
            ky as i32 - GLYPH_H as i32,
            label,
            2,
            SHELL,
        );
        s.text(
            kx as i32 - text_width(&(i + 1).to_string(), 2) / 2,
            ky as i32 + KEY_R + 6,
            &(i + 1).to_string(),
            2,
            DIM,
        );
    }

    let x = layout.screen_x;
    let lh = (GLYPH_H as i32 + 4) * 2;
    s.text(
        x,
        layout.status_y,
        &format!("{dots_w}X{dots_h} SPD2010 {:?}", panel.format()).to_uppercase(),
        2,
        LABEL,
    );
    s.text(
        x,
        layout.status_y + lh,
        &format!(
            "PANEL {}US  {} CMDS  {} WRITES",
            present_us, panel.commands, panel.memory_writes
        ),
        2,
        DIM,
    );
    s.text(
        x,
        layout.status_y + lh * 2,
        "MOUSE-TOUCH  1-PWR  2-BOOT  S-SHOT  ESC-QUIT",
        2,
        DIM,
    );
}

/// The window's own buffer as a colour PNG: a picture of the whole device,
/// keys included, where `png::write_glass` is a picture of just the screen.
fn write_window_png(buf: &[u32], w: usize, h: usize, path: &Path) -> std::io::Result<()> {
    let mut rgb = vec![0u8; w * h * 3];
    for (i, px) in buf.iter().enumerate() {
        rgb[i * 3] = (px >> 16) as u8;
        rgb[i * 3 + 1] = (px >> 8) as u8;
        rgb[i * 3 + 2] = *px as u8;
    }
    crate::png::write_rect(&rgb, w, h, path)
}
