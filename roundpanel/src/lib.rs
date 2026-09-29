//! A desktop emulator for the **ESP32-S3 Development Board With 1.46 Round
//! Display 412 x 412** -- Waveshare's ESP32-S3-Touch-LCD-1.46B.
//!
//! Write an application against [`App`], call [`run`], and it appears in a
//! window the size and shape of the real board's screen, with the board's two
//! side keys beside it and a mouse standing in for the touch panel.
//!
//! ```no_run
//! use roundpanel::{rgb, App, Ctx, Frame};
//!
//! struct Blob;
//!
//! impl App for Blob {
//!     fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
//!         let (cx, cy) = f.centre();
//!         f.clear(rgb(0x0d1117));
//!         let r = 60 + (ctx.now_ms() as i32 / 12) % 90;
//!         f.ring(cx, cy, r, 6, rgb(0x4ea3ff));
//!     }
//! }
//!
//! fn main() -> Result<(), String> {
//!     roundpanel::run(Blob)
//! }
//! ```
//!
//! # What makes this an emulator rather than a canvas
//!
//! Every frame goes through [`spd2010`]: encoded to the interface format,
//! handed to the real QSPI driver, decoded by a model of the display
//! controller, and read back out of that model's RAM before the window shows
//! it. So the desktop run tells you about the *panel* -- the four-column
//! address alignment it insists on, the whole-frame `RAMWR` it refuses, the
//! colour it cannot carry in 65k mode -- not only about your drawing code. See
//! [`panel`] for the rules that surprise people.
//!
//! # What it is not
//!
//! Not an ESP32-S3 emulator. It runs your drawing code natively as a Rust
//! program on your machine; there is no Xtensa core, no FreeRTOS, no WiFi and
//! no filesystem here. The touch half of the SPD2010 -- it is a TDDI part, so
//! the same die carries the touch controller -- is not modelled either: a
//! mouse press arrives as a touch event, but nothing simulates the I2C
//! transactions that would deliver it on the board.

pub mod font;
pub mod frame;
pub mod panel;
pub mod png;

#[cfg(not(target_arch = "wasm32"))]
mod window;

pub use frame::{rgb, Frame, Rgb, BLACK, HEIGHT, WHITE, WIDTH};
pub use panel::Panel;
pub use spd2010;
pub use spd2010::PixelFormat;

use std::path::PathBuf;

/// The board's two side buttons.
///
/// Two, because the board has two: Waveshare's product page is the count --
/// "Onboard PWR and BOOT two side buttons with customizable functions". The
/// labels are the silkscreen's.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum SideKey {
    Pwr,
    Boot,
}

impl SideKey {
    pub const ALL: [SideKey; 2] = [SideKey::Pwr, SideKey::Boot];

    pub fn label(self) -> &'static str {
        match self {
            SideKey::Pwr => "PWR",
            SideKey::Boot => "BOOT",
        }
    }
}

/// Something the hardware would have told your application about.
///
/// Touch coordinates are panel dots -- the same coordinate space [`Frame`] uses
/// -- and are only delivered for points on the glass, because the round module
/// has no digitiser in the corners of its bounding square.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum Event {
    TouchDown {
        x: i32,
        y: i32,
    },
    TouchMove {
        x: i32,
        y: i32,
    },
    TouchUp {
        x: i32,
        y: i32,
    },
    /// A release that did not travel far from its press: the gesture almost
    /// every UI actually wants. Delivered after the matching `TouchUp`.
    Tap {
        x: i32,
        y: i32,
    },
    KeyDown(SideKey),
    KeyUp(SideKey),
}

/// What the harness knows this frame.
pub struct Ctx<'a> {
    /// Microseconds since the run started.
    pub now_us: u64,
    /// Frames drawn so far, this one included.
    pub frame_no: u64,
    /// What happened since the last frame, in order.
    pub events: &'a [Event],
    /// Where the finger is, if it is down on the glass.
    pub touch: Option<(i32, i32)>,
    /// Which side keys are held, indexed by [`SideKey::ALL`].
    pub keys_down: [bool; 2],
}

impl Ctx<'_> {
    pub fn now_ms(&self) -> u64 {
        self.now_us / 1_000
    }

    pub fn now_s(&self) -> f32 {
        self.now_us as f32 / 1_000_000.0
    }

    pub fn is_down(&self, k: SideKey) -> bool {
        self.keys_down[k as usize]
    }

    /// Every tap that landed this frame.
    pub fn taps(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.events.iter().filter_map(|e| match e {
            Event::Tap { x, y } => Some((*x, *y)),
            _ => None,
        })
    }

    /// Whether `k` went down this frame.
    pub fn pressed(&self, k: SideKey) -> bool {
        self.events.contains(&Event::KeyDown(k))
    }
}

/// Your application.
///
/// [`App::draw`] is the only method you have to write. It is called once a
/// frame with the panel's whole framebuffer: draw the frame you want, and the
/// harness sends it to the panel.
pub trait App {
    /// Draw one frame.
    ///
    /// The buffer holds whatever you left in it last time, so clear it if you
    /// are not redrawing everything. The panel's own RAM behaves the same way;
    /// this is not the harness being lazy.
    fn draw(&mut self, frame: &mut Frame, ctx: &Ctx);
}

/// A plain function is an application too, for the cases that have no state.
impl<F: FnMut(&mut Frame, &Ctx)> App for F {
    fn draw(&mut self, frame: &mut Frame, ctx: &Ctx) {
        self(frame, ctx)
    }
}

/// How the window is set up.
#[derive(Clone, Debug)]
pub struct Options {
    /// The window's title bar.
    pub title: String,
    /// Window pixels per panel dot. 1 is the module's real size; 2 doubles it.
    pub scale: usize,
    /// The interface pixel format. `Rgb888` is lossless; the others show you
    /// what the wire really costs.
    pub format: PixelFormat,
    /// Frames a second the window aims for.
    pub target_fps: usize,
    /// Where `S` writes screenshots.
    pub shots_dir: PathBuf,
    /// Draw the board's two side keys and the status strip. Off gives you the
    /// panel and nothing else.
    pub furniture: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            title: "roundpanel-emu".into(),
            scale: 1,
            format: PixelFormat::Rgb888,
            target_fps: 60,
            shots_dir: PathBuf::from("shots"),
            furniture: true,
        }
    }
}

/// Open a window and run `app` in it until it is closed or Escape is pressed.
#[cfg(not(target_arch = "wasm32"))]
pub fn run<A: App>(app: A) -> Result<(), String> {
    run_with(app, Options::default())
}

/// The same, with the window set up your way.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_with<A: App>(app: A, opts: Options) -> Result<(), String> {
    window::run(app, opts)
}

/// The command line every example understands, so an application gets one for
/// free.
///
/// ```text
/// --scale N        window pixels per panel dot (default 1)
/// --format FMT     888 (default, lossless), 666, or 565 -- what the wire costs
/// --fps N          frames a second to aim for (default 60)
/// --bare           just the panel: no side keys, no status strip
/// --shot PATH      render one frame headless, write it, and exit
/// --at MS          which moment --shot starts at (default 0)
/// --frames N       frames to run before --shot captures (default 1)
/// --help
/// ```
///
/// `--shot` is the one that matters on a machine with no display: it needs no
/// window, no event loop and no mouse, and the frame still goes through the
/// panel on its way to the file.
#[derive(Clone, Debug)]
pub struct Cli {
    pub options: Options,
    pub shot: Option<PathBuf>,
    pub at_us: u64,
    pub frames: u64,
}

impl Cli {
    /// Parse `std::env::args`.
    pub fn from_env() -> Result<Cli, String> {
        Cli::parse(std::env::args().skip(1))
    }

    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Cli, String> {
        let mut cli = Cli {
            options: Options::default(),
            shot: None,
            at_us: 0,
            frames: 1,
        };
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} wants a value"));
            match a.as_str() {
                "--scale" => cli.options.scale = value("--scale")?.parse().map_err(err)?,
                "--fps" => cli.options.target_fps = value("--fps")?.parse().map_err(err)?,
                "--bare" => cli.options.furniture = false,
                "--shot" => cli.shot = Some(PathBuf::from(value("--shot")?)),
                "--at" => cli.at_us = value("--at")?.parse::<u64>().map_err(err)? * 1_000,
                "--frames" => cli.frames = value("--frames")?.parse().map_err(err)?,
                "--title" => cli.options.title = value("--title")?,
                "--format" => {
                    cli.options.format = match value("--format")?.as_str() {
                        "888" | "rgb888" => PixelFormat::Rgb888,
                        "666" | "rgb666" => PixelFormat::Rgb666,
                        "565" | "rgb565" => PixelFormat::Rgb565,
                        other => return Err(format!("--format: 888, 666 or 565, not {other:?}")),
                    }
                }
                "--help" | "-h" => return Err(HELP.into()),
                other => return Err(format!("unknown argument {other:?}\n\n{HELP}")),
            }
        }
        Ok(cli)
    }
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

const HELP: &str = "\
roundpanel-emu -- an ESP32-S3 1.46 inch 412x412 round display, on your desktop

  --scale N      window pixels per panel dot (default 1)
  --format FMT   888 (lossless), 666, or 565 -- what the wire really costs
  --fps N        frames a second to aim for (default 60)
  --bare         just the panel: no side keys, no status strip
  --shot PATH    render one frame headless, write the PNG, and exit
  --at MS        which moment --shot starts at (default 0)
  --frames N     frames to run before --shot captures (default 1)
  --title TEXT   the window title
";

/// Run `app` the way the command line asks: in a window, or once into a PNG.
///
/// This is what an application's `main` should call. It gives you `--shot` on
/// a machine with no display for nothing.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_cli<A: App>(mut app: A) -> Result<(), String> {
    let cli = Cli::from_env()?;
    match cli.shot {
        Some(path) => {
            shoot(&mut app, cli.at_us, cli.frames, cli.options.format, &path)?;
            println!("wrote {}", path.display());
            Ok(())
        }
        None => run_with(app, cli.options),
    }
}

/// Draw one frame at a chosen moment, with no window anywhere.
///
/// This is how an application gets tested: no display, no event loop, no mouse
/// to synthesise -- and the frame still goes through the panel, so what comes
/// back is what the hardware would show. Returns the panel it was drawn on;
/// [`Panel::dots`] is the readback.
pub fn render_at<A: App>(app: &mut A, now_us: u64, format: PixelFormat) -> Result<Panel, String> {
    render_frames(app, now_us, 1, 60, format)
}

/// Run an application for `frames` frames of a `fps` clock and return the
/// panel as it stands at the end.
///
/// Still no window. The framebuffer is reused between frames exactly as the
/// window reuses it, so an application that accumulates -- a trail, a scroll,
/// anything that reads what it drew last time -- comes out the same here as it
/// does live.
pub fn render_frames<A: App>(
    app: &mut A,
    start_us: u64,
    frames: u64,
    fps: u64,
    format: PixelFormat,
) -> Result<Panel, String> {
    let mut panel = Panel::new(format);
    let mut frame = Frame::new();
    let step = 1_000_000 / fps.max(1);
    for i in 0..frames.max(1) {
        let ctx = Ctx {
            now_us: start_us + i * step,
            frame_no: i + 1,
            events: &[],
            touch: None,
            keys_down: [false; 2],
        };
        app.draw(&mut frame, &ctx);
    }
    panel.present(&frame)?;
    Ok(panel)
}

/// Draw one frame and write it out as the round glass shows it.
pub fn shoot<A: App>(
    app: &mut A,
    now_us: u64,
    frames: u64,
    format: PixelFormat,
    path: impl AsRef<std::path::Path>,
) -> Result<(), String> {
    let panel = render_frames(app, now_us, frames, 60, format)?;
    let (w, h) = panel.dots_size();
    png::write_glass(panel.dots(), w, h, path.as_ref()).map_err(|e| e.to_string())
}
