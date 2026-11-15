//! Counting steps in software from raw accelerometer samples.
//!
//! The QMI8658 on this board has a pedometer of its own, and on Pokewalker's
//! real board its step count never moved, whatever it was configured with
//! (`docs/imu-and-steps.md`). The accelerometer itself was fine, so the steps
//! are counted here instead, from samples the host reads at a fixed rate.
//!
//! Written as device code: no `std`, no allocation, no floats, integer maths
//! only. Copy this file into firmware as it is and hand it samples at
//! [`RATE_MILLI_HZ`].
//!
//! # The front end
//!
//! 1. **Magnitude.** `sqrt(x^2 + y^2 + z^2)` in milli-g, so orientation does
//!    not matter: on a belt, in a pocket or in a swinging hand, a footfall is
//!    a bump in the length of the vector wherever gravity points.
//! 2. **Low pass.** A 12-sample moving average (192 ms). It keeps a walking
//!    cadence (1.4-2.8 Hz) and crushes most of a 4-6 Hz shake, and folds a
//!    heel strike and the push-off after it into one bump.
//! 3. **High pass.** An exponential baseline with a one-second time constant
//!    is subtracted: gravity and slow drift go, the bumps stay.
//! 4. **Peaks.** A peak is the top of an excursion over the threshold, taken
//!    once the signal falls back under half of it; the next one is armed only
//!    once the signal has been below the baseline.
//!
//! # The gates, and why there are two sets
//!
//! What to do with a peak is where the real walk taught us something. The
//! first board build used [`STRICT`]: eight regular steps before anything
//! counts, each gap within 40% of the one before, and a bump that came too
//! soon reset the run. On a real 90-second walk it counted 14 steps -- the
//! weaker foot's bumps fell under the threshold, irregular gaps restarted the
//! run, and the "too soon" rule threw away whole runs.
//!
//! [`LENIENT`] is what worked: a threshold off the weaker foot (a quarter of
//! the second-smallest recent peak, floor 40 mg); three footfalls near the
//! *median* gap to start a walk; then every footfall 0.26-2 s after the last
//! counts, a gap of about twice the usual one counts two (the weaker foot was
//! missed), and a sooner bump is ignored rather than fatal. The one thing
//! rejected outright is shaking: four upward baseline crossings in a row,
//! each under 288 ms after the last and machine-steady (within two samples of
//! the interval before), end the walk and mute every bump for a second.
//!
//! Known gap: until those four beats have been seen, the start of a shake
//! looks like a brisk walk, so its first second can count a few steps (five
//! on the shipped trace). Nothing after that counts.
#![allow(dead_code)]

/// Samples must arrive at 62.5 Hz: the QMI8658's accelerometer ODR, and what
/// a poll between display bands can hold (resample a bunched poll to it).
pub const RATE_MILLI_HZ: u32 = 62_500;
pub const SAMPLE_US: u64 = 16_000;
/// Raw counts per g at the QMI8658's +/-4 g range.
pub const COUNTS_PER_G: u32 = 8192;

const SMOOTH: usize = 12;
const BASE_SHIFT: u32 = 6;
const PEAKS: usize = 6;
const GAPS: usize = 5;
/// Baseline crossings in a row, each machine-steady and under [`SHAKE_GAP`],
/// that mean the device is being shaken.
const SHAKE_STREAK: u32 = 4;
/// 18 samples, 288 ms: faster than about 3.5 bumps a second.
const SHAKE_GAP: u32 = 18;
/// Once shaking is seen, every bump for a second is ignored.
const SHAKE_HOLD: u32 = 62;

/// How a peak becomes a step.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Gating {
    /// Footfalls in a regular run before any of them is counted.
    pub entry_steps: u32,
    /// How far (percent) a gap may be from the usual one while starting.
    pub regularity_pct: u32,
    /// "Usual" is the median of recent gaps (true) or the gap before (false).
    pub against_median: bool,
    /// Shortest and longest gap between footfalls, in samples.
    pub min_gap: u32,
    pub max_gap: u32,
    /// The threshold's floor, milli-g of filtered signal.
    pub min_threshold_mg: i32,
    /// Threshold = (second-smallest recent peak) / 4, so the weaker foot
    /// clears it. Off: two fifths of the mean recent peak.
    pub weak_foot_threshold: bool,
    /// A gap of 1.6-2.5x the usual counts two steps.
    pub count_doubles: bool,
    /// A bump sooner than `min_gap` throws the run away (true) or is ignored.
    pub soon_resets: bool,
}

/// The first board build's gates. Kept to compare against.
pub const STRICT: Gating = Gating {
    entry_steps: 8,
    regularity_pct: 40,
    against_median: false,
    min_gap: 19,
    max_gap: 125,
    min_threshold_mg: 60,
    weak_foot_threshold: false,
    count_doubles: false,
    soon_resets: true,
};

/// The gates that counted a real walk.
pub const LENIENT: Gating = Gating {
    entry_steps: 3,
    regularity_pct: 60,
    against_median: true,
    min_gap: 16,
    max_gap: 125,
    min_threshold_mg: 40,
    weak_foot_threshold: true,
    count_doubles: true,
    soon_resets: false,
};

/// What became of one peak.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Counted, or held towards the run that starts a walk.
    Step,
    /// Counted as two: the other foot's bump was missed.
    Double,
    /// The first footfall after a pause.
    Start,
    /// Too soon after the last: a second bump inside a step.
    Secondary,
    /// While shaking: ignored, and the run thrown away.
    Shake,
    /// Off the usual gap while a walk was starting: start again here.
    Irregular,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Event {
    /// Sample index of the peak.
    pub at: u32,
    pub mg: i32,
    pub verdict: Verdict,
}

/// Integer square root, bit by bit: no division, no 64-bit.
pub fn isqrt(n: u32) -> u32 {
    let (mut n, mut root, mut bit) = (n, 0u32, 1u32 << 30);
    while bit > n {
        bit >>= 2;
    }
    while bit != 0 {
        if n >= root + bit {
            n -= root + bit;
            root = (root >> 1) + bit;
        } else {
            root >>= 1;
        }
        bit >>= 2;
    }
    root
}

/// A raw sample's length in milli-g.
pub fn magnitude_mg(s: [i16; 3]) -> i32 {
    let sq = |v: i16| (v.unsigned_abs() as u32).pow(2);
    let len = isqrt(sq(s[0]).saturating_add(sq(s[1])).saturating_add(sq(s[2])));
    (len * 1000 / COUNTS_PER_G) as i32
}

pub struct Detector {
    pub gating: Gating,
    ring: [i32; SMOOTH],
    ring_sum: i32,
    ring_at: usize,
    base_fp: i32,
    started: bool,
    /// Samples so far.
    pub n: u32,
    armed: bool,
    in_peak: bool,
    peak_val: i32,
    peak_at: u32,
    peaks: [i32; PEAKS],
    peaks_len: usize,
    peaks_at: usize,
    gaps: [u32; GAPS],
    gaps_len: usize,
    gaps_at: usize,
    last_gap: u32,
    last_step: Option<u32>,
    last_crossing: Option<u32>,
    last_interval: Option<u32>,
    steady: u32,
    shake_left: u32,
    run: u32,
    pending: u32,
    walking: bool,
    sig: i32,
    mag: i32,
    total: u32,
    event: Option<Event>,
}

impl Detector {
    pub const fn new(gating: Gating) -> Detector {
        Detector {
            gating,
            ring: [0; SMOOTH],
            ring_sum: 0,
            ring_at: 0,
            base_fp: 0,
            started: false,
            n: 0,
            armed: false,
            in_peak: false,
            peak_val: 0,
            peak_at: 0,
            peaks: [0; PEAKS],
            peaks_len: 0,
            peaks_at: 0,
            gaps: [0; GAPS],
            gaps_len: 0,
            gaps_at: 0,
            last_gap: 0,
            last_step: None,
            last_crossing: None,
            last_interval: None,
            steady: 0,
            shake_left: 0,
            run: 0,
            pending: 0,
            walking: false,
            sig: 0,
            mag: 0,
            total: 0,
            event: None,
        }
    }

    pub fn total(&self) -> u32 {
        self.total
    }
    pub fn walking(&self) -> bool {
        self.walking
    }
    pub fn shaking(&self) -> bool {
        self.shake_left > 0
    }
    /// Footfalls held towards the run that starts a walk (0 once walking).
    pub fn run_progress(&self) -> u32 {
        if self.walking {
            0
        } else {
            self.run
        }
    }
    /// The filtered signal (gravity removed), milli-g.
    pub fn signal_mg(&self) -> i32 {
        self.sig
    }
    /// The raw magnitude, milli-g.
    pub fn magnitude_mg(&self) -> i32 {
        self.mag
    }
    pub fn take_event(&mut self) -> Option<Event> {
        self.event.take()
    }

    fn usual_gap(&self) -> Option<u32> {
        if self.gating.against_median {
            if self.gaps_len < 2 {
                return None;
            }
            let mut g = self.gaps;
            let s = &mut g[..self.gaps_len];
            s.sort_unstable();
            Some(s[self.gaps_len / 2])
        } else {
            (self.last_gap > 0).then_some(self.last_gap)
        }
    }

    pub fn threshold_mg(&self) -> i32 {
        let floor = self.gating.min_threshold_mg;
        if self.peaks_len == 0 {
            return floor;
        }
        let mut p = self.peaks;
        let s = &mut p[..self.peaks_len];
        let t = if self.gating.weak_foot_threshold {
            s.sort_unstable();
            s[if self.peaks_len > 2 { 1 } else { 0 }] / 4
        } else {
            s.iter().sum::<i32>() / self.peaks_len as i32 * 2 / 5
        };
        t.max(floor)
    }

    /// One raw sample. Returns the steps it added to [`Detector::total`].
    pub fn push(&mut self, s: [i16; 3]) -> u32 {
        let mag = magnitude_mg(s);
        self.mag = mag;
        if !self.started {
            self.started = true;
            self.ring = [mag; SMOOTH];
            self.ring_sum = mag * SMOOTH as i32;
            self.base_fp = mag << BASE_SHIFT;
        }
        self.ring_sum += mag - self.ring[self.ring_at];
        self.ring[self.ring_at] = mag;
        self.ring_at = (self.ring_at + 1) % SMOOTH;
        let smooth = self.ring_sum / SMOOTH as i32;
        self.base_fp += smooth - (self.base_fp >> BASE_SHIFT);
        let sig = smooth - (self.base_fp >> BASE_SHIFT);
        let rising = sig > 0 && self.sig <= 0;
        self.sig = sig;
        self.n = self.n.wrapping_add(1);
        self.shake_left = self.shake_left.saturating_sub(1);

        // Shaking: the filtered signal crosses its baseline on every beat of
        // a shake even when the threshold skips some, so count crossings.
        if rising {
            let interval = self.last_crossing.map(|c| self.n.wrapping_sub(c));
            self.last_crossing = Some(self.n);
            let machine = matches!(
                (interval, self.last_interval),
                (Some(i), Some(p)) if i < SHAKE_GAP && i.abs_diff(p) <= 2
            );
            self.last_interval = interval;
            self.steady = if machine { self.steady + 1 } else { 0 };
            if self.steady >= SHAKE_STREAK {
                self.shake_left = SHAKE_HOLD;
            }
        }

        if let Some(at) = self.last_step {
            if self.n.wrapping_sub(at) > self.gating.max_gap {
                self.forget();
            }
        }

        let thr = self.threshold_mg();
        if self.in_peak {
            if sig > self.peak_val {
                self.peak_val = sig;
                self.peak_at = self.n;
            }
            if sig < thr / 2 {
                self.in_peak = false;
                self.armed = false;
                return self.candidate(self.peak_at, self.peak_val);
            }
        } else if sig < 0 {
            self.armed = true;
        } else if self.armed && sig > thr {
            self.in_peak = true;
            self.peak_val = sig;
            self.peak_at = self.n;
        }
        0
    }

    fn forget(&mut self) {
        self.last_step = None;
        self.gaps_len = 0;
        self.gaps_at = 0;
        self.peaks_len = 0;
        self.peaks_at = 0;
        self.last_gap = 0;
        self.run = 0;
        self.pending = 0;
        self.walking = false;
    }

    fn push_peak(&mut self, v: i32) {
        self.peaks[self.peaks_at] = v;
        self.peaks_at = (self.peaks_at + 1) % PEAKS;
        self.peaks_len = (self.peaks_len + 1).min(PEAKS);
    }

    fn push_gap(&mut self, g: u32) {
        self.gaps[self.gaps_at] = g;
        self.gaps_at = (self.gaps_at + 1) % GAPS;
        self.gaps_len = (self.gaps_len + 1).min(GAPS);
        self.last_gap = g;
    }

    fn note(&mut self, at: u32, mg: i32, verdict: Verdict) {
        self.event = Some(Event { at, mg, verdict });
    }

    fn candidate(&mut self, at: u32, val: i32) -> u32 {
        let g = self.gating;
        if self.shake_left > 0 {
            self.note(at, val, Verdict::Shake);
            self.forget();
            return 0;
        }
        let Some(prev) = self.last_step else {
            self.last_step = Some(at);
            self.run = 1;
            self.pending = 1;
            self.push_peak(val);
            self.note(at, val, Verdict::Start);
            return 0;
        };
        let gap = at.wrapping_sub(prev);
        if gap < g.min_gap {
            if g.soon_resets && !self.walking {
                self.forget();
            }
            self.note(at, val, Verdict::Secondary);
            return 0;
        }
        let (steps, regular) = match self.usual_gap() {
            Some(m)
                if g.count_doubles
                    && gap * 100 >= m * 160
                    && gap * 100 <= m * 250
                    && gap >= 2 * g.min_gap =>
            {
                (2, true)
            }
            Some(m) => (1, gap.abs_diff(m) * 100 <= g.regularity_pct * m),
            None => (1, true),
        };
        self.last_step = Some(at);
        self.push_peak(val);
        if regular {
            self.push_gap(gap / steps);
        }
        let verdict = if steps == 2 {
            Verdict::Double
        } else {
            Verdict::Step
        };
        if self.walking {
            self.note(at, val, verdict);
            self.total = self.total.wrapping_add(steps);
            return steps;
        }
        if !regular {
            self.note(at, val, Verdict::Irregular);
            self.run = 1;
            self.pending = 1;
            self.gaps_len = 0;
            self.gaps_at = 0;
            self.push_gap(gap);
            return 0;
        }
        self.note(at, val, verdict);
        self.run += steps;
        self.pending += steps;
        if self.run >= g.entry_steps {
            let n = self.pending;
            self.total = self.total.wrapping_add(n);
            self.pending = 0;
            self.walking = true;
            return n;
        }
        0
    }
}
