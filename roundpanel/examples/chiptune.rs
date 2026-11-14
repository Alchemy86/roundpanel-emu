//! Sound the way this board can afford it: a four-voice chip synth mixed ahead
//! on its own "core", fed by a lock-free queue, with the ring's lead and every
//! call's cost drawn round the glass.
//!
//!     cargo run --release --example chiptune
//!     cargo run --release --example chiptune -- --wav tune.wav
//!     cargo run --release --example chiptune -- --board-projection
//!
//! The harness has no speaker, so the window shows the engine rather than
//! playing it: `--wav` writes what it would have played. PWR turns the
//! silent-voice skip off and on (watch the steps and the cost move, while the
//! sound stays bit-identical), BOOT fires a noise burst, and a tap on a ring
//! mutes or unmutes that voice.
//!
//! The engine is `common/chipsynth.rs`, and is the part to copy. Read
//! `docs/timing-and-audio.md` for why it is built this way: the board runs it
//! roughly 150 times slower than your desktop does.

#[path = "common/chipsynth.rs"]
mod chipsynth;

use std::f32::consts::TAU;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use chipsynth::{
    render, Command, Engine, Shared, LEAD, NOISE, RATE, SLICE, SONG_FRAMES, SQ1, SQ2, WAV,
};
use roundpanel::{rgb, App, Cli, Ctx, Frame, Rgb, SideKey};

/// Board time per unit of desktop time for this kind of code: 37.1 us on the
/// board against 248.8 ns on a desktop for one frame of Pokewalker's mixer in
/// the same state (~149x). It is ONE calibration, of a different synth, on one
/// desktop. Re-measure it on your own hardware -- time a known slice on the
/// board, time the same slice here -- before trusting a projection.
const BOARD_RATIO_NUM: u64 = 37_100;
const BOARD_RATIO_DEN: u64 = 249;
/// What one call may cost on the board before it is a stall.
const BOARD_CALL_BUDGET_NS: u64 = 2_000_000;
/// What the engine may take of its core, in percent.
const BOARD_CORE_BUDGET_PCT: f64 = 5.0;
/// The per-call budget the mixer stops at on the board (1.2 ms), scaled down to
/// the desktop so the window behaves as the board would.
const DESKTOP_POLL_BUDGET_NS: u32 = (1_200_000 * BOARD_RATIO_DEN / BOARD_RATIO_NUM) as u32;

/// The board's `static`. Both "cores" see this and nothing else.
static SHARED: Shared = Shared::new();

const BG: Rgb = rgb(0x0b_0e_14);
const INK: Rgb = rgb(0xe6_ea_f2);
const DIM: Rgb = rgb(0x5a_63_72);
const VOICES: [(usize, &str, i32, Rgb); 4] = [
    (SQ1, "LEAD", 172, rgb(0x4e_a3_ff)),
    (SQ2, "ARP", 148, rgb(0x3d_dc_97)),
    (WAV, "BASS", 124, rgb(0xff_b3_4e)),
    (NOISE, "DRUMS", 100, rgb(0xff_5e_7a)),
];
const RING_T: i32 = 10;

fn board_ns(host_ns: u64) -> u64 {
    host_ns * BOARD_RATIO_NUM / BOARD_RATIO_DEN
}

/// The audio side's loop, on a thread standing in for core 1.
///
/// On the board this body is core 1's entry function: `play` comes from the
/// I2S DMA position instead of a wall clock, `clock` from
/// `esp_hal::time::Instant`, and the sleep is a wait for the DMA's next
/// half-buffer event.
fn spawn_audio_core(shared: &'static Shared) {
    std::thread::Builder::new()
        .name("audio core".into())
        .spawn(move || {
            let mut engine = Engine::new(shared);
            let start = Instant::now();
            let mut clock = || start.elapsed().as_nanos() as u32;
            while !shared.stop.load(Ordering::Relaxed) {
                let play = (start.elapsed().as_micros() as u64 * RATE as u64 / 1_000_000) as u32;
                shared.play.store(play, Ordering::Relaxed);
                engine.poll(play, DESKTOP_POLL_BUDGET_NS, &mut clock);
                std::thread::sleep(Duration::from_millis(2));
            }
        })
        .expect("start the audio thread");
}

/// The drawing side: core 0. It reads the shared counters and pushes
/// commands; it never mixes. Headless (`--shot`) it drives an engine inline
/// from the frame clock instead, so a screenshot is reproducible.
struct Visualiser {
    shared: &'static Shared,
    inline: Option<Box<Engine<'static>>>,
    started: bool,
    skip: bool,
    muted: [bool; 4],
    worst_shown: u32,
    worst_at_us: u64,
}

impl Visualiser {
    fn new(shared: &'static Shared, inline: bool) -> Self {
        Visualiser {
            shared,
            inline: inline.then(|| Box::new(Engine::new(shared))),
            started: false,
            skip: true,
            muted: [false; 4],
            worst_shown: 0,
            worst_at_us: 0,
        }
    }

    fn send(&self, cmd: Command) {
        self.shared.queue.push(cmd.pack());
    }
}

impl App for Visualiser {
    fn draw(&mut self, f: &mut Frame, ctx: &Ctx) {
        let (cx, cy) = f.centre();
        if ctx.pressed(SideKey::Pwr) {
            self.skip = !self.skip;
            self.send(Command::SetSkip(self.skip));
        }
        if ctx.pressed(SideKey::Boot) {
            self.send(Command::Zap);
        }
        for (x, y) in ctx.taps() {
            let r = (((x - cx).pow(2) + (y - cy).pow(2)) as f32).sqrt() as i32;
            if let Some(&(i, ..)) = VOICES.iter().find(|v| r >= v.2 - 4 && r < v.2 + RING_T + 4) {
                self.muted[i] = !self.muted[i];
                self.send(Command::ToggleMute(i));
            }
        }

        let play = match &mut self.inline {
            Some(engine) => {
                let play = (ctx.now_us * RATE as u64 / 1_000_000) as u32;
                if !self.started {
                    engine.seek(play);
                    self.started = true;
                }
                let t0 = Instant::now();
                engine.poll(play, DESKTOP_POLL_BUDGET_NS, &mut || {
                    t0.elapsed().as_nanos() as u32
                });
                self.shared.play.store(play, Ordering::Relaxed);
                play
            }
            None => self.shared.play.load(Ordering::Relaxed),
        };

        let s = self.shared;
        let lead = s.lead.load(Ordering::Relaxed);
        let call = s.call_ns.load(Ordering::Relaxed);
        if ctx.now_us >= self.worst_at_us + 500_000 {
            self.worst_shown = s.worst_ns.swap(0, Ordering::Relaxed);
            self.worst_at_us = ctx.now_us;
        }
        let worst = self.worst_shown.max(call);
        let steps = s.steps_x100.load(Ordering::Relaxed);
        let skip = s.skip.load(Ordering::Relaxed);

        f.clear(BG);

        // The ring's lead, as a gauge round the rim: full is LEAD frames
        // (128 ms) of audio already mixed.
        let lead_frac = (lead as f32 / LEAD as f32).min(1.0);
        let gauge = if lead < SLICE * 2 {
            rgb(0xff_5e_7a)
        } else {
            rgb(0x7a_e7_ff)
        };
        arc(f, cx, cy, 194, 6, 0.0, 1.0, mix(gauge, BG, 0.18));
        arc(f, cx, cy, 194, 6, 0.0, lead_frac, gauge);

        // Twelve pitch classes round the clock, C at the top.
        for k in 0..12 {
            let a = k as f32 / 12.0 * TAU;
            let (sn, cs) = a.sin_cos();
            let (r0, r1) = if k == 0 {
                (183.0, 190.0)
            } else {
                (185.0, 189.0)
            };
            f.line(
                cx + (r0 * sn) as i32,
                cy - (r0 * cs) as i32,
                cx + (r1 * sn) as i32,
                cy - (r1 * cs) as i32,
                DIM,
            );
        }

        // Each voice is a ring: lit at its pitch class, as wide as it is loud.
        // The drums ring is lit at the noise rate instead. What is shown is
        // the slice the "hardware" is playing now, not the one being mixed.
        let voices = s.voices_at(play);
        for &(i, _, r, c) in &VOICES {
            let (vol, tag) = voices[i];
            let track = if self.muted[i] { 0.06 } else { 0.16 };
            arc(f, cx, cy, r + 3, RING_T - 6, 0.0, 1.0, mix(c, BG, track));
            if vol == 0 {
                continue;
            }
            let centre = if i == NOISE {
                tag as f32 / 8.0
            } else {
                tag as f32 / 12.0
            };
            let half = 0.012 + vol as f32 / 15.0 * 0.11;
            arc(
                f,
                cx,
                cy,
                r - 2,
                RING_T + 4,
                centre - half * 1.4,
                centre + half * 1.4,
                mix(c, BG, 0.3),
            );
            arc(f, cx, cy, r, RING_T, centre - half, centre + half, c);
        }

        // The middle: the readouts, and the waveform the ring is about to
        // play.
        let ink = |ok: bool| if ok { INK } else { rgb(0xff_5e_7a) };
        f.text_centred(cx, 126, &format!("LEAD {} MS", lead * 1000 / RATE), 2, INK);
        f.text_centred(cx, 144, &format!("CALL {} NS", call), 2, DIM);
        let board = board_ns(worst as u64);
        f.text_centred(
            cx,
            238,
            &format!("BOARD {} US", board / 1000),
            2,
            ink(board <= BOARD_CALL_BUDGET_NS),
        );
        f.text_centred(cx, 256, &format!("STEPS/MS {}", steps * 16 / 100), 2, DIM);
        f.text_centred(
            cx,
            274,
            if skip { "SKIP ON" } else { "SKIP OFF" },
            2,
            ink(skip),
        );
        scope(f, cx, cy - 6, s, play);
    }
}

/// The next 24 ms of the ring as a trace across the middle of the glass.
fn scope(f: &mut Frame, cx: i32, cy: i32, s: &Shared, play: u32) {
    const W: i32 = 150;
    let mut last = None;
    for x in 0..W {
        let v = s.sample(play.wrapping_add(x as u32 * 3)) as i32;
        let y = cy - v * 30 / 32768;
        let px = cx - W / 2 + x;
        if let Some((lx, ly)) = last {
            f.line(lx, ly, px, y, rgb(0xc9_b8_ff));
        }
        last = Some((px, y));
    }
}

/// An arc of a ring `t` pixels thick from radius `r` outwards, between two
/// angles in turns clockwise from twelve o'clock.
#[allow(clippy::too_many_arguments)]
fn arc(f: &mut Frame, cx: i32, cy: i32, r: i32, t: i32, a0: f32, a1: f32, c: Rgb) {
    let n = ((a1 - a0).abs() * TAU * (r + t) as f32 * 2.5) as i32 + 1;
    for k in 0..=n {
        let (sn, cs) = ((a0 + (a1 - a0) * k as f32 / n as f32) * TAU).sin_cos();
        for d in 0..t {
            let rr = (r + d) as f32;
            f.set(
                cx + (rr * sn).round() as i32,
                cy - (rr * cs).round() as i32,
                c,
            );
        }
    }
}

fn mix(a: Rgb, b: Rgb, k: f32) -> Rgb {
    core::array::from_fn(|i| (a[i] as f32 * k + b[i] as f32 * (1.0 - k)) as u8)
}

/// Time the exact mixer on this machine and project it onto the board.
fn board_projection() -> Result<(), String> {
    const SECONDS: u32 = 60;
    const RUNS: usize = 5;
    let slices = (RATE * SECONDS / SLICE) as usize;
    let zap_every = RATE * 3 / 2;
    println!(
        "{SECONDS} s of the tune, a noise burst every 1.5 s, best of {RUNS} runs a slice;\n\
         board = desktop x {:.1} (Pokewalker's calibration -- re-measure on your board)\n",
        BOARD_RATIO_NUM as f64 / BOARD_RATIO_DEN as f64
    );
    let mut verdict = Ok(());
    for (label, skip, batch) in [
        (
            "as shipped: silent voices skipped, noise runs batched",
            true,
            true,
        ),
        ("silent voices ticking, noise runs batched", false, true),
        (
            "silent voices skipped, noise stepped one event at a time",
            true,
            false,
        ),
        (
            "neither: every voice steps one event at a time",
            false,
            false,
        ),
    ] {
        let mut best = vec![u64::MAX; slices];
        let mut steps = 0;
        for _ in 0..RUNS {
            let shared = Box::new(Shared::new());
            let mut e = Engine::new(&shared);
            e.chip.set_skip_silent(skip);
            e.chip.set_batch_noise(batch);
            let s0 = e.chip.steps;
            for (k, b) in best.iter_mut().enumerate() {
                let at = k as u32 * SLICE;
                if at / zap_every != (at + SLICE) / zap_every {
                    e.apply(Command::Zap);
                }
                let t0 = Instant::now();
                e.mix_slice();
                *b = (*b).min(t0.elapsed().as_nanos() as u64);
            }
            steps = e.chip.steps.wrapping_sub(s0);
        }
        let total: u64 = best.iter().sum();
        let worst = *best.iter().max().unwrap();
        let frames = slices as u64 * SLICE as u64;
        let share = board_ns(total) as f64 / (SECONDS as f64 * 1e9) * 100.0;
        // A steady poll mixes one slice. One after a stall would mix up to a
        // whole lead's worth, but stops once 1.2 ms is spent, so the worst
        // call is that budget plus at most one slice.
        let fits = board_ns(worst) <= BOARD_CALL_BUDGET_NS && share <= BOARD_CORE_BUDGET_PCT;
        println!("{label}:");
        println!(
            "  voice steps a frame   {:.2}",
            steps as f64 / frames as f64
        );
        println!(
            "  desktop               {:.1} ns a frame, worst slice {:.1} us",
            total as f64 / frames as f64,
            worst as f64 / 1000.0
        );
        println!(
            "  board (projected)     {:.2} us a frame, worst slice {:.0} us, {:.1}% of the audio core",
            board_ns(total) as f64 / frames as f64 / 1000.0,
            board_ns(worst) as f64 / 1000.0,
            share
        );
        println!(
            "  budget                {} us a slice, {BOARD_CORE_BUDGET_PCT}% of the core: {}\n",
            BOARD_CALL_BUDGET_NS / 1000,
            if fits { "fits" } else { "DOES NOT FIT" }
        );
        if skip && batch && !fits {
            verdict = Err("the shipped configuration does not fit the board budget".into());
        }
    }
    println!(
        "Only the board's own numbers count: flash it and log underruns, lead and the worst call."
    );
    verdict
}

/// Two loops of the tune, with a noise burst in each, as 16-bit mono PCM.
fn write_wav(path: &str) -> Result<(), String> {
    let frames = SONG_FRAMES * 2;
    let zaps = [RATE * 2, RATE * 13 / 2, RATE * 11];
    let (pcm, _) = render(&Shared::new(), frames, true, true, &zaps);
    let data = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in &pcm {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).map_err(|e| format!("{path}: {e}"))?;
    let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    println!(
        "wrote {path}: {:.1} s at {RATE} Hz, peak {peak}",
        pcm.len() as f32 / RATE as f32
    );
    Ok(())
}

fn main() -> Result<(), String> {
    let mut rest = Vec::new();
    let mut wav = None;
    let mut projection = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--wav" => wav = Some(args.next().ok_or("--wav wants a path")?),
            "--board-projection" => projection = true,
            _ => rest.push(a),
        }
    }
    if let Some(path) = wav {
        return write_wav(&path);
    }
    if projection {
        return board_projection();
    }
    let mut cli = Cli::parse(rest).map_err(|e| {
        format!("{e}\n  --wav PATH          render two loops of the tune to a WAV and exit\n  --board-projection  time the mixer and project its cost on the board")
    })?;
    if cli.options.title == "roundpanel-emu" {
        cli.options.title = "chiptune".into();
    }
    match cli.shot {
        Some(path) => {
            let mut app = Visualiser::new(&SHARED, true);
            roundpanel::shoot(&mut app, cli.at_us, cli.frames, cli.options.format, &path)?;
            println!("wrote {}", path.display());
            Ok(())
        }
        None => {
            spawn_audio_core(&SHARED);
            let result = roundpanel::run_with(Visualiser::new(&SHARED, false), cli.options);
            SHARED.stop.store(true, Ordering::Relaxed);
            result
        }
    }
}
