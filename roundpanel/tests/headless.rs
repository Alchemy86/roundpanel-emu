//! Running an application with no window anywhere.
//!
//! This is the check a project's CI wants: no display, no event loop, no mouse
//! to synthesise, and the frame still goes through the panel's own command set
//! on its way to the picture. If your application is testable at all, it is
//! testable like this.

use std::time::Instant;

use roundpanel::{rgb, App, Ctx, Frame, PixelFormat};

/// Something with a moving part, so a frame at one moment differs from a frame
/// at another and a test can say which it got.
struct Sweep;

impl App for Sweep {
    fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        let (cx, cy) = f.centre();
        f.clear(rgb(0x0d_11_17));
        f.ring(cx, cy, 200, 4, rgb(0x4e_a3_ff));
        let x = 60 + (ctx.now_ms() as i32 / 10) % 290;
        f.disc(x, cy, 10, rgb(0xff_b3_4e));
    }
}

#[test]
fn a_frame_rendered_headless_comes_back_off_the_panel() {
    let panel = roundpanel::render_at(&mut Sweep, 0, PixelFormat::Rgb888).expect("render");
    let (w, h) = panel.dots_size();
    assert_eq!((w, h), (412, 412));
    assert_eq!(panel.pixels, 412 * 412, "the whole module was written");

    // The dot is at x=60 at t=0, on the centre row.
    let i = (206 * w + 60) * 3;
    assert_eq!(&panel.dots()[i..i + 3], &[0xff, 0xb3, 0x4e]);
    // And the ring is where the ring is.
    let j = (206 * w + 6) * 3;
    assert_eq!(&panel.dots()[j..j + 3], &[0x4e, 0xa3, 0xff]);
}

#[test]
fn a_later_frame_differs_from_an_earlier_one() {
    let a = roundpanel::render_at(&mut Sweep, 0, PixelFormat::Rgb888).unwrap();
    let b = roundpanel::render_at(&mut Sweep, 1_000_000, PixelFormat::Rgb888).unwrap();
    assert_ne!(a.dots(), b.dots());
}

#[test]
fn a_screenshot_is_written_where_it_was_asked_for() {
    let path = std::env::temp_dir().join("roundpanel-headless-test.png");
    let _ = std::fs::remove_file(&path);
    roundpanel::shoot(&mut Sweep, 500_000, 1, PixelFormat::Rgb888, &path).expect("shoot");
    let bytes = std::fs::read(&path).expect("the file");
    assert_eq!(&bytes[1..4], b"PNG");
    let _ = std::fs::remove_file(&path);
}

/// Not a pass/fail threshold -- it is a number this harness should be made to
/// print, because every frame goes through a QSPI decoder sixty times a second
/// and that should be shown to be cheap rather than assumed to be.
#[test]
fn the_cost_of_a_present_is_reported() {
    let mut panel = roundpanel::Panel::new(PixelFormat::Rgb888);
    let mut frame = Frame::new();
    Sweep.draw(&mut frame, &fake_ctx());
    // Once to warm, then a run of them.
    panel.present(&frame).unwrap();
    let n = 60;
    let t0 = Instant::now();
    for _ in 0..n {
        panel.present(&frame).unwrap();
    }
    let us = t0.elapsed().as_micros() as u64 / n;
    println!(
        "present: {us} us/frame at 412x412 Rgb888 ({} fps budget)",
        1_000_000 / us.max(1)
    );
    assert!(
        us < 100_000,
        "a frame took {us} us, which is not a live window"
    );
}

fn fake_ctx() -> Ctx<'static> {
    Ctx {
        now_us: 0,
        frame_no: 1,
        events: &[],
        touch: None,
        keys_down: [false; 2],
    }
}
