//! A software pedometer, fed an accelerometer trace, drawn on the glass.
//!
//!     cargo run --release --example steps
//!     cargo run --release --example steps -- --trace my-walk.csv
//!     cargo run --release --example steps -- --shot steps.png --at 42000
//!
//! The board's QMI8658 has a pedometer of its own, and on a real board its
//! count never moved. The accelerometer was fine, so the steps are counted in
//! software from the raw samples: `common/footfall.rs` is that detector,
//! written as device code. Here it is fed `data/walk-trace.csv` (synthetic,
//! shaped like a real walk, with a few seconds of shaking in the middle) at
//! the board's 62.5 Hz, and shows the filtered signal, the threshold, every
//! peak and what became of it, against the trace's true footfalls. The small
//! figure beside the total is the first board build's strict gates on the
//! same samples, which is why they were replaced.
//!
//! BOOT starts the trace again. `--trace` plays your own CSV (three columns
//! of raw counts is enough; see `common/walktrace.rs`). `--make-trace PATH`
//! writes the synthetic walk. Read `docs/imu-and-steps.md` for the chip side.

#[path = "common/footfall.rs"]
mod footfall;
#[path = "common/walktrace.rs"]
mod walktrace;

use footfall::{Detector, Verdict, LENIENT, SAMPLE_US, STRICT};
use roundpanel::{rgb, App, Cli, Ctx, Frame, Rgb, SideKey};
use walktrace::Sample;

const TRACE: &str = include_str!("data/walk-trace.csv");
/// Samples of history across the chart: four seconds.
const SPAN: usize = 250;

const BG: Rgb = rgb(0x0b_0e_14);
const INK: Rgb = rgb(0xe6_ea_f2);
const DIM: Rgb = rgb(0x5a_63_72);
const SIGNAL: Rgb = rgb(0x7a_e7_ff);
const THRESH: Rgb = rgb(0xff_b3_4e);
const GOOD: Rgb = rgb(0x3d_dc_97);
const BAD: Rgb = rgb(0xff_5e_7a);

struct Steps {
    trace: Vec<Sample>,
    lenient: Detector,
    strict: Detector,
    fed: usize,
    true_steps: u32,
    /// `(signal, threshold)` for the last [`SPAN`] samples.
    history: Vec<(i32, i32)>,
    events: Vec<footfall::Event>,
    origin_us: u64,
}

impl Steps {
    fn new(trace: Vec<Sample>) -> Steps {
        Steps {
            trace,
            lenient: Detector::new(LENIENT),
            strict: Detector::new(STRICT),
            fed: 0,
            true_steps: 0,
            history: Vec::new(),
            events: Vec::new(),
            origin_us: 0,
        }
    }

    fn restart(&mut self, now_us: u64) {
        let trace = std::mem::take(&mut self.trace);
        *self = Steps::new(trace);
        self.origin_us = now_us;
    }

    fn feed(&mut self, s: Sample) {
        self.lenient.push(s.a);
        self.strict.push(s.a);
        if s.foot {
            self.true_steps += 1;
        }
        if let Some(e) = self.lenient.take_event() {
            self.events.push(e);
        }
        self.strict.take_event();
        self.history
            .push((self.lenient.signal_mg(), self.lenient.threshold_mg()));
        if self.history.len() > SPAN {
            self.history.remove(0);
        }
        let oldest = self.lenient.n.saturating_sub(SPAN as u32);
        self.events.retain(|e| e.at > oldest);
        self.fed += 1;
    }
}

impl App for Steps {
    fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        if ctx.pressed(SideKey::Boot) {
            self.restart(ctx.now_us);
        }
        let len = self.trace.len();
        if ctx.now_us < self.origin_us
            || ((ctx.now_us - self.origin_us) / SAMPLE_US) as usize >= len
        {
            self.restart(ctx.now_us);
        }
        let target = ((ctx.now_us - self.origin_us) / SAMPLE_US) as usize;
        while self.fed <= target {
            self.feed(self.trace[self.fed]);
        }
        let now = self.trace[self.fed - 1];

        let (cx, cy) = f.centre();
        f.clear(BG);
        let state_colour = match now.state {
            b'W' => GOOD,
            b'S' => BAD,
            _ => DIM,
        };
        f.ring(cx, cy, 204, 4, state_colour);

        f.text_centred(cx, 52, "STEPS", 2, DIM);
        f.text_centred(cx, 70, &self.lenient.total().to_string(), 8, INK);
        f.text_centred(
            cx,
            118,
            &format!("TRUE {}   STRICT {}", self.true_steps, self.strict.total()),
            2,
            DIM,
        );

        chart(f, self);

        let (word, colour) = if self.lenient.shaking() {
            ("SHAKE REJECTED".to_string(), BAD)
        } else if self.lenient.walking() {
            ("WALKING".to_string(), GOOD)
        } else if self.lenient.run_progress() > 0 {
            (
                format!(
                    "STARTING {}/{}",
                    self.lenient.run_progress(),
                    LENIENT.entry_steps
                ),
                THRESH,
            )
        } else {
            ("STILL".to_string(), DIM)
        };
        f.text_centred(cx, 300, &word, 3, colour);
        f.text_centred(
            cx,
            332,
            &format!(
                "{} MG  THRESHOLD {}",
                self.lenient.magnitude_mg(),
                self.lenient.threshold_mg()
            ),
            2,
            DIM,
        );
        let trace_word = match now.state {
            b'W' => "TRACE: WALKING",
            b'S' => "TRACE: SHAKING",
            b'.' => "TRACE: STILL",
            _ => "YOUR TRACE",
        };
        f.text_centred(cx, 354, trace_word, 2, state_colour);
    }
}

/// The last four seconds: filtered signal, threshold, peaks by verdict, and
/// the true footfalls as ticks underneath.
fn chart(f: &mut Frame, s: &Steps) {
    const X0: i32 = 56;
    const W: i32 = 300;
    const Y0: i32 = 220;
    const SCALE: i32 = 3;
    let x_of = |k: usize| X0 + k as i32 * W / SPAN as i32;
    let y_of = |mg: i32| (Y0 - mg / SCALE).clamp(Y0 - 70, Y0 + 50);
    let n = s.history.len();
    let first = s.lenient.n as usize - n;

    f.line(X0, Y0, X0 + W, Y0, rgb(0x23_2a_36));
    for (k, w) in s.history.windows(2).enumerate() {
        let (x0, x1) = (x_of(k), x_of(k + 1));
        if k % 4 < 2 {
            f.line(x0, y_of(w[0].1), x1, y_of(w[1].1), THRESH);
        }
        f.line(x0, y_of(w[0].0), x1, y_of(w[1].0), SIGNAL);
    }
    for k in 0..n {
        if s.trace.get(first + k).is_some_and(|t| t.foot) {
            f.rect(x_of(k), Y0 + 56, 2, 8, INK);
        }
    }
    for e in &s.events {
        let k = (e.at as usize).saturating_sub(first + 1);
        let c = match e.verdict {
            Verdict::Step | Verdict::Start => GOOD,
            Verdict::Double => THRESH,
            Verdict::Shake => BAD,
            Verdict::Secondary | Verdict::Irregular => DIM,
        };
        f.disc(x_of(k), y_of(e.mg), 4, c);
    }
    f.text(X0, Y0 + 68, "FOOTFALLS", 1, DIM);
}

fn main() -> Result<(), String> {
    let mut rest = Vec::new();
    let mut trace_path = None;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--trace" => trace_path = Some(args.next().ok_or("--trace wants a path")?),
            "--make-trace" => {
                let path = args.next().ok_or("--make-trace wants a path")?;
                let csv = walktrace::to_csv(&walktrace::synthesize(0x5eed_f007));
                std::fs::write(&path, csv).map_err(|e| format!("{path}: {e}"))?;
                println!("wrote {path}");
                return Ok(());
            }
            _ => rest.push(a),
        }
    }
    let text = match &trace_path {
        Some(p) => std::fs::read_to_string(p).map_err(|e| format!("{p}: {e}"))?,
        None => TRACE.to_string(),
    };
    let trace = walktrace::parse(&text)?;
    if trace.is_empty() {
        return Err("the trace has no samples".into());
    }
    let mut cli = Cli::parse(rest).map_err(|e| {
        format!("{e}\n  --trace PATH       play your own accelerometer CSV\n  --make-trace PATH  write the synthetic walk")
    })?;
    if cli.options.title == "roundpanel-emu" {
        cli.options.title = "steps".into();
    }
    let mut app = Steps::new(trace);
    match cli.shot {
        Some(path) => {
            roundpanel::shoot(&mut app, cli.at_us, cli.frames, cli.options.format, &path)?;
            println!("wrote {}", path.display());
            Ok(())
        }
        None => roundpanel::run_with(app, cli.options),
    }
}
