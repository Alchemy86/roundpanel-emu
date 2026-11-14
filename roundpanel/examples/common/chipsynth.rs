//! A board-safe sound engine, small enough to read in one sitting.
//!
//! This is the pattern that fixed a real stutter on this board (Pokewalker's
//! battle music; see `docs/timing-and-audio.md`), rebuilt from scratch around
//! an original tune:
//!
//! * [`Chip`] -- four chip-style voices (two squares, a wavetable, a noise
//!   LFSR) in integer maths only. The per-sample loop has no division, no
//!   floats and no 64-bit arithmetic, skips voices that cannot be heard, and
//!   sums runs of noise steps in one go.
//! * [`Engine`] -- a mix-ahead ring: the "DMA buffer" is played round and
//!   round, and each call mixes one [`SLICE`] at a time just ahead of the read
//!   position, never more than [`LEAD`] frames ahead, and stops when its time
//!   budget is spent.
//! * [`Spsc`] -- a lock-free single-producer/single-consumer command queue.
//!   The drawing code only ever pushes onto it; the engine drains it.
//!
//! # How this maps onto the ESP32-S3
//!
//! Nothing below uses `std` except the atomics, which are `core::sync::atomic`
//! under another name, and every atomic is 32 bits or narrower (the Xtensa
//! LX7 has no 64-bit atomics, which is why frame counters are `u32` and
//! compared with `wrapping_sub`: they wrap after 74 hours at 16 kHz).
//! [`Shared`] is built by a `const fn`, so on the board it is a plain
//! `static`, exactly as the desktop example uses it.
//!
//! On the desktop a `std::thread` stands in for the second core. On the board:
//!
//! * core 0 runs the frame loop and is the queue's only producer
//!   ([`Spsc::push`]);
//! * core 1 owns the [`Engine`] and the I2S peripheral. Start it with
//!   `esp_hal::system::CpuControl::start_app_core`, give it its own stack, and
//!   have it loop: read the I2S DMA descriptor position as `play`, call
//!   [`Engine::poll`] with a clock that reads `esp_hal::time::Instant`, then
//!   wait for the next DMA "half done" event. The ring is the DMA buffer, so
//!   [`Shared::ring`] becomes the circular descriptor chain's memory (as `i16`
//!   in DMA-capable internal RAM, not atomics).
//!
//! Neither side can stall the other: a slow frame does not stop the mixing,
//! and the mixing never runs inside a frame.
#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, AtomicI16, AtomicU32, Ordering};

/// Output frames a second. Small on purpose: the board mixes every one.
pub const RATE: u32 = 16_000;
/// Chip cycles per output frame. A power of two, so a frame's box filter is a
/// sum and never a divide.
pub const CYCLES_PER_FRAME: u32 = 64;
/// The voices' clock: 1.024 MHz.
pub const CLOCK: u32 = RATE * CYCLES_PER_FRAME;
/// Frames mixed per step of the ring (8 ms). Pokewalker uses the same size.
pub const SLICE: u32 = 128;
/// How far ahead of the read position the ring is kept (128 ms): how long the
/// mixer can be starved before anything is heard.
pub const LEAD: u32 = 2048;
/// The ring, in frames. A power of two, longer than `LEAD + SLICE`.
pub const RING: usize = 4096;
const SLOTS: usize = RING / SLICE as usize;
/// A frame holds at most 8 steps of the fastest noise (period 8), which is
/// what lets [`Chip::noise_run`] do a run with shifts instead of a loop.
const _: () = assert!(CYCLES_PER_FRAME / 8 <= 14);
const _: () = assert!(RING.is_power_of_two() && (LEAD + SLICE) as usize <= RING);

/// The envelopes step at 64 Hz.
const ENV_TICK: u32 = RATE / 64;
/// One sixteenth note at 120 bpm.
pub const STEP_FRAMES: u32 = RATE / 8;
/// The tune loops after four bars of sixteen steps: eight seconds.
pub const SONG_FRAMES: u32 = STEP_FRAMES * 64;

// ---------------------------------------------------------------------------
// Pitch tables, computed by the compiler. The division here happens once, at
// build time; nothing at run time ever divides.

/// A4 up to G#5, in millihertz.
const SEMI_MILLIHZ: [u64; 12] = [
    440_000, 466_164, 493_883, 523_251, 554_365, 587_330, 622_254, 659_255, 698_456, 739_989,
    783_991, 830_609,
];

const fn period_table(steps_per_cycle: u64) -> [u32; 128] {
    let mut t = [1u32; 128];
    let mut m = 0;
    while m < 128 {
        let k = m as i32 - 69;
        let oct = k.div_euclid(12);
        let f = SEMI_MILLIHZ[k.rem_euclid(12) as usize];
        let f = if oct >= 0 { f << oct } else { f >> -oct };
        let p = CLOCK as u64 * 1000 / (steps_per_cycle * f);
        t[m] = if p == 0 { 1 } else { p as u32 };
        m += 1;
    }
    t
}

/// Cycles per duty step (eight a wave cycle) for each MIDI note.
const SQUARE_PERIOD: [u32; 128] = period_table(8);
/// Cycles per wavetable step (32 a wave cycle) for each MIDI note.
const WAVE_PERIOD: [u32; 128] = period_table(32);

/// The four square duties, one bit per step: 12.5%, 25%, 50%, 75%.
const DUTY: [u8; 4] = [0b0000_0001, 0b1000_0001, 0b1000_0111, 0b0111_1110];

/// A triangle, 4 bits a step.
const WAVE: [u8; 32] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4,
    3, 2, 1, 0,
];

pub const SQ1: usize = 0;
pub const SQ2: usize = 1;
pub const WAV: usize = 2;
pub const NOISE: usize = 3;

// ---------------------------------------------------------------------------
// The chip.

/// What a trigger loads into a voice.
#[derive(Clone, Copy, Debug)]
pub struct Note {
    /// Cycles per step. For the noise voice this must be `8 << shift`.
    pub period: u32,
    pub vol: u8,
    pub env_up: bool,
    /// Envelope ticks (1/64 s) per volume step; 0 holds the volume.
    pub env_period: u8,
    /// Square: the duty index. Noise: the period's shift. Wave: unused.
    pub shape: u8,
    /// Noise only: the 7-bit LFSR, which sounds metallic.
    pub narrow: bool,
    /// What the visualiser shows: the pitch class, or the noise shift.
    pub tag: u8,
}

impl Note {
    pub const fn square(midi: u8, vol: u8, env_period: u8, duty: u8) -> Note {
        Note {
            period: SQUARE_PERIOD[midi as usize],
            vol,
            env_up: false,
            env_period,
            shape: duty,
            narrow: false,
            tag: midi % 12,
        }
    }
    pub const fn wave(midi: u8, vol: u8, env_period: u8) -> Note {
        Note {
            period: WAVE_PERIOD[midi as usize],
            vol,
            env_up: false,
            env_period,
            shape: 0,
            narrow: false,
            tag: midi % 12,
        }
    }
    /// The period is `8 << shift` cycles so that a run of noise steps can be
    /// counted with a shift rather than a divide.
    pub const fn noise(shift: u8, narrow: bool, vol: u8, env_period: u8) -> Note {
        Note {
            period: 8 << shift,
            vol,
            env_up: false,
            env_period,
            shape: shift,
            narrow,
            tag: shift,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Voice {
    pub on: bool,
    pub vol: u8,
    env_up: bool,
    env_period: u8,
    env_count: u8,
    period: u32,
    /// Cycles until the next step.
    timer: u32,
    /// Duty step (0..8) or wavetable position (0..32).
    step: u8,
    shape: u8,
    lfsr: u16,
    narrow: bool,
    pub tag: u8,
}

const VOICE_OFF: Voice = Voice {
    on: false,
    vol: 0,
    env_up: false,
    env_period: 0,
    env_count: 0,
    period: 1,
    timer: 1,
    step: 0,
    shape: 0,
    lfsr: 0x7fff,
    narrow: false,
    tag: 0,
};

pub struct Chip {
    pub v: [Voice; 4],
    skip_silent: bool,
    batch_noise: bool,
    env_clock: u32,
    /// Which voices are running and what each outputs, kept between frames
    /// and recomputed only when a trigger, a release or an envelope step
    /// could have changed them.
    run: [bool; 4],
    lv: [i32; 4],
    dirty: bool,
    /// Voice steps taken, for the "steps a frame" figure. Wraps.
    pub steps: u32,
}

impl Default for Chip {
    fn default() -> Self {
        Chip::new()
    }
}

impl Chip {
    pub const fn new() -> Chip {
        Chip {
            v: [VOICE_OFF; 4],
            skip_silent: true,
            batch_noise: true,
            env_clock: 0,
            run: [false; 4],
            lv: [0; 4],
            dirty: true,
            steps: 0,
        }
    }

    /// Skip voices that cannot be heard. Output-identical either way (see
    /// [`Chip::running`]); off is only here to measure what it saves.
    pub fn set_skip_silent(&mut self, on: bool) {
        self.skip_silent = on;
        self.dirty = true;
    }

    /// Sum runs of noise steps in one go. Output-identical either way.
    pub fn set_batch_noise(&mut self, on: bool) {
        self.batch_noise = on;
    }

    pub fn skip_silent(&self) -> bool {
        self.skip_silent
    }

    fn refresh(&mut self) {
        for i in 0..4 {
            self.run[i] = self.running(i);
            self.lv[i] = self.level(i);
        }
        self.dirty = false;
    }

    /// Start a note. A trigger reloads the voice's whole running state -- its
    /// timer, its step, its LFSR and its envelope counter -- and that is what
    /// makes skipping a silent voice exact (see [`Chip::running`]).
    pub fn trigger(&mut self, i: usize, n: Note) {
        self.v[i] = Voice {
            on: true,
            vol: n.vol,
            env_up: n.env_up,
            env_period: n.env_period,
            env_count: 0,
            period: n.period.max(1),
            timer: n.period.max(1),
            step: 0,
            shape: n.shape,
            lfsr: 0x7fff,
            narrow: n.narrow,
            tag: n.tag,
        };
        self.dirty = true;
    }

    pub fn release(&mut self, i: usize) {
        self.v[i].on = false;
        self.dirty = true;
    }

    /// Whether a voice's timer has to run.
    ///
    /// A voice that is off, or at volume 0 with an envelope that cannot rise,
    /// contributes exactly 0 to every frame whatever its phase -- and it
    /// cannot leave that state except through [`Chip::trigger`], which throws
    /// the phase away. So nothing that is skipped is ever observed, and the
    /// output is bit-identical with or without the skip (the test in
    /// `roundpanel/tests/chiptune.rs` proves it by checksum).
    ///
    /// It would stop being exact if you added a way back to sound *without* a
    /// trigger: a "set volume" command that un-mutes a note mid-way, a rising
    /// envelope that was treated as silent, or anything that reads the phase
    /// of a silent voice (a sync or ring-mod between voices). Then the skipped
    /// voice would resume from a frozen phase instead of the phase it would
    /// have reached. If you add one of those, either make it retrigger or keep
    /// the voice running, and re-run the checksum test.
    ///
    /// This is the board's stutter: a drum hit leaves the noise voice at
    /// volume 0 with its timer firing every 8 cycles -- eight steps a frame,
    /// for good -- unless it is skipped.
    #[inline(always)]
    fn running(&self, i: usize) -> bool {
        let v = &self.v[i];
        let audible_or_rising = v.vol != 0 || (v.env_up && v.env_period != 0);
        v.on && (audible_or_rising || !self.skip_silent)
    }

    /// The voice's output level right now, within +-120.
    #[inline(always)]
    fn level(&self, i: usize) -> i32 {
        let v = &self.v[i];
        if !v.on {
            return 0;
        }
        let vol = v.vol as i32;
        match i {
            SQ1 | SQ2 => {
                if (DUTY[v.shape as usize] >> v.step) & 1 == 1 {
                    vol * 8
                } else {
                    -vol * 8
                }
            }
            WAV => (WAVE[v.step as usize] as i32 - 8) * vol,
            _ => {
                if v.lfsr & 1 == 0 {
                    vol * 8
                } else {
                    -vol * 8
                }
            }
        }
    }

    #[inline(always)]
    fn step_voice(&mut self, i: usize) {
        self.steps = self.steps.wrapping_add(1);
        let v = &mut self.v[i];
        match i {
            SQ1 | SQ2 => v.step = (v.step + 1) & 7,
            WAV => v.step = (v.step + 1) & 31,
            _ => v.lfsr = lfsr_step(v.lfsr, v.narrow),
        }
    }

    /// 64 Hz: every voice that is on moves its volume one step if its
    /// envelope is due.
    #[inline(always)]
    fn envelope(&mut self) {
        self.env_clock += 1;
        if self.env_clock < ENV_TICK {
            return;
        }
        self.env_clock = 0;
        for v in &mut self.v {
            if !v.on || v.env_period == 0 {
                continue;
            }
            v.env_count += 1;
            if v.env_count >= v.env_period {
                v.env_count = 0;
                self.dirty = true;
                if v.env_up {
                    if v.vol < 15 {
                        v.vol += 1;
                    }
                } else if v.vol > 0 {
                    v.vol -= 1;
                }
            }
        }
    }

    /// One output frame: run the voices for [`CYCLES_PER_FRAME`] cycles and
    /// box-filter what they did. Integer only, 32-bit only, no division.
    ///
    /// Event-driven: rather than stepping every cycle, it jumps straight to
    /// whichever running voice steps next, adding `level * cycles` for the
    /// span in between.
    pub fn frame(&mut self) -> i16 {
        self.envelope();
        if self.dirty {
            self.refresh();
        }
        let run = self.run;
        // A voice that is not running is silent, so its level is 0 here.
        let mut lv = self.lv;
        let batch = self.batch_noise && run[NOISE];
        // With batching, the noise voice is left out of the "next event"
        // search and handled as a run.
        let scan = if batch { 3 } else { 4 };
        let mut left = CYCLES_PER_FRAME;
        let mut acc = 0i32;
        while left > 0 {
            let mut w = left;
            for (i, &r) in run.iter().enumerate().take(scan) {
                if r && self.v[i].timer < w {
                    w = self.v[i].timer;
                }
            }
            if batch && self.v[NOISE].timer <= w {
                acc += (lv[SQ1] + lv[SQ2] + lv[WAV]) * w as i32;
                acc += self.noise_run(w, &mut lv[NOISE]);
            } else {
                acc += (lv[SQ1] + lv[SQ2] + lv[WAV] + lv[NOISE]) * w as i32;
                if batch {
                    self.v[NOISE].timer -= w;
                }
            }
            for i in 0..scan {
                if run[i] {
                    self.v[i].timer -= w;
                    if self.v[i].timer == 0 {
                        self.v[i].timer = self.v[i].period;
                        self.step_voice(i);
                        lv[i] = self.level(i);
                    }
                }
            }
            left -= w;
        }
        self.lv = lv;
        // |acc| <= 4 voices * 120 * 64 cycles = 30720: an i16 as it stands.
        acc as i16
    }

    /// Every noise step inside the next `w` cycles, summed in one go.
    ///
    /// Between two other events the only thing that changes is the LFSR, so
    /// the steps are added up exactly as that many trips round
    /// [`Chip::frame`]'s loop would add them: the current level until the
    /// first step, `period` cycles at each level after a step, and the last
    /// level until `w`. Because the period is `8 << shift`, the number of
    /// steps is a shift, not a divide. Returns `sum(level * cycles)`.
    #[inline(always)]
    fn noise_run(&mut self, w: u32, lv: &mut i32) -> i32 {
        let v = &mut self.v[NOISE];
        let log2p = v.shape as u32 + 3;
        let (t0, p) = (v.timer, v.period);
        let n = ((w - t0) >> log2p) + 1;
        let vol8 = v.vol as i32 * 8;
        let mut acc = *lv * t0 as i32;
        let mut l = v.lfsr;
        let last;
        if v.narrow {
            let mut lev = *lv;
            for k in 0..n {
                l = lfsr_step(l, true);
                lev = vol8 - ((l & 1) as i32 * 2 * vol8);
                if k + 1 < n {
                    acc += lev * p as i32;
                }
            }
            last = lev;
        } else {
            // The wide LFSR shifts right and feeds back into bit 14, so for
            // up to 14 steps the level after step k is just bit k of the
            // register as it is now, and the bits fed in are bits 0..n of
            // `l ^ l >> 1`. No loop: a popcount (a few shifts and adds on a
            // core without one) and two shifts.
            let ones = ((l >> 1) & ((1 << (n - 1)) - 1)).count_ones() as i32;
            acc += vol8 * ((n as i32 - 1) - 2 * ones) * p as i32;
            let fed = (l ^ (l >> 1)) & ((1 << n) - 1);
            l = (l >> n) | (fed << (15 - n));
            last = vol8 - ((l & 1) as i32 * 2 * vol8);
        }
        let tail = w - t0 - ((n - 1) << log2p);
        acc += last * tail as i32;
        v.lfsr = l;
        v.timer = p - tail;
        *lv = last;
        self.steps = self.steps.wrapping_add(n);
        acc
    }
}

#[inline(always)]
fn lfsr_step(l: u16, narrow: bool) -> u16 {
    let bit = (l ^ (l >> 1)) & 1;
    let l = (l >> 1) | (bit << 14);
    if narrow {
        (l & !(1 << 6)) | (bit << 6)
    } else {
        l
    }
}

// ---------------------------------------------------------------------------
// The tune: an original four-bar loop in A minor (Am F C G), sixteen steps a
// bar. 0 in the lead line holds the previous note.

const LEAD_LINE: [u8; 64] = [
    76, 0, 0, 0, 72, 0, 74, 0, 76, 0, 0, 0, 69, 0, 0, 0, //
    77, 0, 0, 0, 76, 0, 72, 0, 69, 0, 0, 0, 72, 0, 0, 0, //
    79, 0, 0, 0, 76, 0, 79, 0, 84, 0, 0, 0, 83, 0, 81, 0, //
    79, 0, 0, 0, 74, 0, 0, 0, 71, 0, 74, 0, 79, 0, 0, 0,
];
const CHORDS: [[u8; 3]; 4] = [[69, 72, 76], [65, 69, 72], [60, 64, 67], [67, 71, 74]];
const ARP: [usize; 4] = [0, 1, 2, 1];
const BASS_ROOT: [u8; 4] = [45, 41, 48, 43];
const DRUMS: &[u8; 16] = b"k.h.s.h.k.hks.hh";

const KICK: Note = Note::noise(6, false, 15, 1);
const SNARE: Note = Note::noise(2, false, 12, 1);
const HAT: Note = Note::noise(0, false, 6, 1);
/// BOOT's sound effect: a long white-noise burst, the same shape as the move
/// effect that started the board's stutter.
pub const ZAP: Note = Note::noise(0, false, 15, 3);

/// Where the tune is. Advanced one frame at a time inside the mix.
pub struct Song {
    frame: u32,
    row: usize,
}

impl Song {
    pub const fn new() -> Song {
        Song { frame: 0, row: 0 }
    }

    #[inline(always)]
    fn tick(&mut self, chip: &mut Chip, muted: &[bool; 4]) {
        if self.frame == 0 {
            play_row(chip, self.row, muted);
        }
        self.frame += 1;
        if self.frame == STEP_FRAMES {
            self.frame = 0;
            self.row = (self.row + 1) & 63;
        }
    }
}

fn play_row(chip: &mut Chip, row: usize, muted: &[bool; 4]) {
    let (bar, pos) = (row >> 4, row & 15);
    let lead = LEAD_LINE[row];
    if lead != 0 && !muted[SQ1] {
        chip.trigger(SQ1, Note::square(lead, 12, 3, 2));
    }
    if !muted[SQ2] {
        chip.trigger(SQ2, Note::square(CHORDS[bar][ARP[pos & 3]], 5, 1, 1));
    }
    if pos & 1 == 0 && !muted[WAV] {
        let up = if pos == 6 || pos == 14 { 12 } else { 0 };
        chip.trigger(WAV, Note::wave(BASS_ROOT[bar] + up, 12, 2));
    }
    if !muted[NOISE] {
        match DRUMS[pos] {
            b'k' => chip.trigger(NOISE, KICK),
            b's' => chip.trigger(NOISE, SNARE),
            b'h' => chip.trigger(NOISE, HAT),
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// The queue.

/// A lock-free single-producer/single-consumer queue of `u32` commands.
///
/// Each slot is an atomic, so there is no `unsafe` and nothing to lock: the
/// producer writes a slot then publishes it with a `Release` store of `tail`;
/// the consumer sees it with an `Acquire` load. Exactly one thread may push and
/// exactly one may pop -- on the board, core 0 and core 1.
pub struct Spsc {
    slots: [AtomicU32; 64],
    head: AtomicU32,
    tail: AtomicU32,
}

impl Default for Spsc {
    fn default() -> Self {
        Spsc::new()
    }
}

impl Spsc {
    pub const fn new() -> Spsc {
        Spsc {
            slots: [const { AtomicU32::new(0) }; 64],
            head: AtomicU32::new(0),
            tail: AtomicU32::new(0),
        }
    }

    /// False if the queue is full; the command is dropped, never waited for.
    pub fn push(&self, cmd: u32) -> bool {
        let t = self.tail.load(Ordering::Relaxed);
        let h = self.head.load(Ordering::Acquire);
        if t.wrapping_sub(h) >= self.slots.len() as u32 {
            return false;
        }
        self.slots[t as usize & 63].store(cmd, Ordering::Relaxed);
        self.tail.store(t.wrapping_add(1), Ordering::Release);
        true
    }

    pub fn pop(&self) -> Option<u32> {
        let h = self.head.load(Ordering::Relaxed);
        let t = self.tail.load(Ordering::Acquire);
        if h == t {
            return None;
        }
        let cmd = self.slots[h as usize & 63].load(Ordering::Relaxed);
        self.head.store(h.wrapping_add(1), Ordering::Release);
        Some(cmd)
    }
}

/// What the drawing side can ask for. Packed into a `u32` for the queue.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Zap,
    SetSkip(bool),
    ToggleMute(usize),
}

impl Command {
    pub fn pack(self) -> u32 {
        match self {
            Command::Zap => 1,
            Command::SetSkip(on) => 2 | ((on as u32) << 8),
            Command::ToggleMute(i) => 3 | ((i as u32 & 3) << 8),
        }
    }
    pub fn unpack(w: u32) -> Option<Command> {
        match w & 0xff {
            1 => Some(Command::Zap),
            2 => Some(Command::SetSkip((w >> 8) & 1 == 1)),
            3 => Some(Command::ToggleMute(((w >> 8) & 3) as usize)),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// What the two sides share.

/// Everything both cores touch. All atomics, all at most 32 bits, built by a
/// `const fn` so it can be a `static`.
pub struct Shared {
    /// The DMA buffer: frame `n` lives at `n % RING`.
    pub ring: [AtomicI16; RING],
    /// For the visualiser: each slice's four voices as `vol | tag << 4`, one
    /// byte each, at slot `(frame / SLICE) % SLOTS`.
    vis: [AtomicU32; SLOTS],
    pub queue: Spsc,
    /// Where the "hardware" is reading, in absolute frames.
    pub play: AtomicU32,
    /// `head - play` after the last call, in frames.
    pub lead: AtomicU32,
    /// The last call's cost, in the clock's units.
    pub call_ns: AtomicU32,
    /// The worst call since the reader last took it.
    pub worst_ns: AtomicU32,
    pub underruns: AtomicU32,
    /// Voice steps per output frame in the last slice, times 100.
    pub steps_x100: AtomicU32,
    pub skip: AtomicBool,
    pub stop: AtomicBool,
}

impl Default for Shared {
    fn default() -> Self {
        Shared::new()
    }
}

impl Shared {
    pub const fn new() -> Shared {
        Shared {
            ring: [const { AtomicI16::new(0) }; RING],
            vis: [const { AtomicU32::new(0) }; SLOTS],
            queue: Spsc::new(),
            play: AtomicU32::new(0),
            lead: AtomicU32::new(0),
            call_ns: AtomicU32::new(0),
            worst_ns: AtomicU32::new(0),
            underruns: AtomicU32::new(0),
            steps_x100: AtomicU32::new(0),
            skip: AtomicBool::new(true),
            stop: AtomicBool::new(false),
        }
    }

    pub fn sample(&self, frame: u32) -> i16 {
        self.ring[frame as usize & (RING - 1)].load(Ordering::Relaxed)
    }

    /// The four voices as they were in the slice containing `frame`:
    /// `(volume, tag)` each.
    pub fn voices_at(&self, frame: u32) -> [(u8, u8); 4] {
        let w = self.vis[(frame / SLICE) as usize % SLOTS].load(Ordering::Relaxed);
        core::array::from_fn(|i| {
            let b = (w >> (8 * i)) as u8;
            (b & 15, b >> 4)
        })
    }
}

// ---------------------------------------------------------------------------
// The engine: core 1's half.

pub struct Engine<'a> {
    shared: &'a Shared,
    pub chip: Chip,
    song: Song,
    muted: [bool; 4],
    head: u32,
}

impl<'a> Engine<'a> {
    pub fn new(shared: &'a Shared) -> Engine<'a> {
        Engine {
            shared,
            chip: Chip::new(),
            song: Song::new(),
            muted: [false; 4],
            head: 0,
        }
    }

    /// The next frame to be mixed.
    pub fn head(&self) -> u32 {
        self.head
    }

    pub fn apply(&mut self, cmd: Command) {
        match cmd {
            Command::Zap => self.chip.trigger(NOISE, ZAP),
            Command::SetSkip(on) => {
                self.chip.set_skip_silent(on);
                self.shared.skip.store(on, Ordering::Relaxed);
            }
            Command::ToggleMute(i) => {
                self.muted[i] = !self.muted[i];
                if self.muted[i] {
                    self.chip.release(i);
                }
            }
        }
    }

    /// Mix one slice at the head. Every frame is a song tick plus a chip
    /// frame; nothing else is in the loop.
    pub fn mix_slice(&mut self) {
        let s0 = self.chip.steps;
        let mut vis = 0u32;
        for (i, v) in self.chip.v.iter().enumerate() {
            let vol = if v.on { v.vol } else { 0 };
            vis |= ((vol & 15) as u32 | ((v.tag as u32 & 15) << 4)) << (8 * i);
        }
        self.shared.vis[(self.head / SLICE) as usize % SLOTS].store(vis, Ordering::Relaxed);
        for k in 0..SLICE {
            self.song.tick(&mut self.chip, &self.muted);
            let s = self.chip.frame();
            self.shared.ring[self.head.wrapping_add(k) as usize & (RING - 1)]
                .store(s, Ordering::Relaxed);
        }
        let steps = self.chip.steps.wrapping_sub(s0);
        // SLICE is 128, so "times 100 over 128" is a multiply and a shift.
        self.shared
            .steps_x100
            .store((steps * 100) >> 7, Ordering::Relaxed);
        self.head = self.head.wrapping_add(SLICE);
    }

    /// Mix (and throw away) everything before `play`, for starting part-way
    /// through. Not a real-time path.
    pub fn seek(&mut self, play: u32) {
        while self.head.wrapping_add(SLICE).wrapping_sub(play) as i32 <= 0 {
            self.mix_slice();
        }
    }

    /// One call from the audio loop: drain the queue, then mix slices at the
    /// head until it is [`LEAD`] frames ahead of `play` or `budget` (in
    /// `clock`'s units) is spent. At least one slice is mixed whenever the
    /// ring is short, so the budget can slow the mixer down but never stop it.
    ///
    /// Commands take effect at the head, so a new sound is heard up to
    /// [`LEAD`] frames late (128 ms here). Pokewalker's `mixahead.rs` closes
    /// that gap by snapshotting the chip each slice and rewinding the head to
    /// just past `play` when a sound starts; this example keeps the lead short
    /// instead, to stay small.
    ///
    /// Returns the slices mixed.
    pub fn poll(&mut self, play: u32, budget: u32, clock: &mut impl FnMut() -> u32) -> u32 {
        while let Some(w) = self.shared.queue.pop() {
            if let Some(cmd) = Command::unpack(w) {
                self.apply(cmd);
            }
        }
        if (self.head.wrapping_sub(play) as i32) < 0 {
            // The hardware overtook the mixer and played stale frames. Count
            // it and start again from where it is.
            self.shared.underruns.fetch_add(1, Ordering::Relaxed);
            self.head = play.wrapping_add(SLICE - 1) & !(SLICE - 1);
        }
        let t0 = clock();
        let mut mixed = 0;
        while self.head.wrapping_sub(play) < LEAD {
            self.mix_slice();
            mixed += 1;
            if clock().wrapping_sub(t0) >= budget {
                break;
            }
        }
        if mixed > 0 {
            let cost = clock().wrapping_sub(t0);
            self.shared.call_ns.store(cost, Ordering::Relaxed);
            self.shared.worst_ns.fetch_max(cost, Ordering::Relaxed);
        }
        self.shared
            .lead
            .store(self.head.wrapping_sub(play), Ordering::Relaxed);
        mixed
    }
}

/// Play `frames` of the tune through the ring exactly as the hardware would
/// read it -- the engine mixing ahead, the reader taking a slice at a time --
/// with a [`Command::Zap`] queued at each frame in `zaps`. Returns the frames
/// and the voice steps it took. This is what `--wav` writes and what the
/// checksum test compares.
pub fn render(
    shared: &Shared,
    frames: u32,
    skip_silent: bool,
    batch_noise: bool,
    zaps: &[u32],
) -> (Vec<i16>, u32) {
    let mut e = Engine::new(shared);
    e.chip.set_skip_silent(skip_silent);
    e.chip.set_batch_noise(batch_noise);
    let mut out = Vec::with_capacity(frames as usize);
    let mut play = 0u32;
    let mut next_zap = zaps.iter().peekable();
    while play < frames {
        while next_zap.next_if(|&&z| z <= play).is_some() {
            shared.queue.push(Command::Zap.pack());
        }
        e.poll(play, u32::MAX, &mut || 0);
        for k in 0..SLICE.min(frames - play) {
            out.push(shared.sample(play + k));
        }
        play += SLICE;
    }
    (out, e.chip.steps)
}
