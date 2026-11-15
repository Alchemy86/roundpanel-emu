//! Accelerometer traces: the file format, and the synthetic walk that ships
//! with the `steps` example.
//!
//! A trace is CSV at 62.5 Hz, one sample a line: `ax,ay,az,state,foot`. The
//! axes are raw QMI8658 counts at +/-4 g (8192 a g), exactly what the data
//! registers from `AX_L` hold. `state` is `.` (still), `W` (walking) or `S`
//! (shaking); `foot` is 1 on the sample a real footfall lands. Lines starting
//! with `#` are comments. A trace logged off a real board needs only the
//! three axes; the other two columns are what a test scores against.
//!
//! The shipped trace is not a recording. It is [`synthesize`]d to look like
//! the board's first real walk: a strong and a weak foot, gaps jittering by a
//! quarter, the odd footfall with no visible bump or a second bump after it,
//! the device turning in the hand -- plus a few seconds of shaking, which must
//! count nothing. `cargo run --example steps -- --make-trace PATH` rewrites it.
#![allow(dead_code)]

use std::f64::consts::TAU;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sample {
    pub a: [i16; 3],
    pub state: u8,
    pub foot: bool,
}

pub fn parse(text: &str) -> Result<Vec<Sample>, String> {
    let mut out = Vec::new();
    for (no, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split(',').collect();
        let bad = || format!("trace line {}: {line:?}", no + 1);
        if f.len() < 3 {
            return Err(bad());
        }
        let mut a = [0i16; 3];
        for (i, v) in a.iter_mut().enumerate() {
            *v = f[i].trim().parse().map_err(|_| bad())?;
        }
        let state = f.get(3).and_then(|s| s.bytes().next()).unwrap_or(b'?');
        let foot = f.get(4).is_some_and(|s| s.trim() == "1");
        out.push(Sample { a, state, foot });
    }
    Ok(out)
}

pub fn to_csv(trace: &[Sample]) -> String {
    let mut s = String::from(
        "# Synthetic QMI8658 accelerometer trace, 62.5 Hz, raw counts at +/-4 g (8192/g).\n\
         # Not a recording: made by `cargo run --example steps -- --make-trace`.\n\
         # ax,ay,az,state(. still, W walking, S shaking),foot(1 = a real footfall)\n",
    );
    for t in trace {
        s += &format!(
            "{},{},{},{},{}\n",
            t.a[0], t.a[1], t.a[2], t.state as char, t.foot as u8
        );
    }
    s
}

struct Rng(u64);

impl Rng {
    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.unit()
    }
    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.unit().max(1e-12), self.unit());
        (-2.0 * u.ln()).sqrt() * (TAU * v).cos()
    }
}

enum Part {
    Still(f64),
    Walk { secs: f64, cadence: f64 },
    Shake { secs: f64, hz: f64 },
}

const DT: f64 = 0.016;

/// The shipped walk: still, 30 s of walking, still, 6 s of shaking, still,
/// 20 s of brisker walking, still. About 71 s.
pub fn synthesize(seed: u64) -> Vec<Sample> {
    let parts = [
        Part::Still(4.0),
        Part::Walk {
            secs: 30.0,
            cadence: 1.8,
        },
        Part::Still(4.0),
        Part::Shake { secs: 6.0, hz: 4.0 },
        Part::Still(3.0),
        Part::Walk {
            secs: 20.0,
            cadence: 2.1,
        },
        Part::Still(4.0),
    ];
    let mut rng = Rng(seed | 1);
    let mut out = Vec::new();
    let mut t0 = 0.0;
    for part in &parts {
        let (secs, state) = match *part {
            Part::Still(s) => (s, b'.'),
            Part::Walk { secs, .. } => (secs, b'W'),
            Part::Shake { secs, .. } => (secs, b'S'),
        };
        // Footfalls: time, size; bumps include the odd echo after a step.
        let mut feet: Vec<f64> = Vec::new();
        let mut bumps: Vec<(f64, f64)> = Vec::new();
        if let Part::Walk { cadence, .. } = *part {
            let mut t = 0.4;
            let mut strong = true;
            while t < secs - 0.3 {
                feet.push(t);
                let size = if strong { 0.32 } else { 0.17 } * rng.range(0.8, 1.2);
                let size = if rng.unit() < 0.05 { 0.03 } else { size };
                bumps.push((t, size));
                if rng.unit() < 0.10 {
                    bumps.push((t + 0.12, size * 0.4));
                }
                strong = !strong;
                t += rng.range(0.75, 1.25) / cadence;
            }
        }
        let n = (secs / DT).round() as usize;
        let mut next_foot = 0;
        for i in 0..n {
            let t = i as f64 * DT;
            let abs = t0 + t;
            // Gravity, slowly turning in the hand.
            let th = 0.6 + 0.3 * (0.1 * abs).sin();
            let ph = 0.05 * abs;
            let up = [th.sin() * ph.cos(), th.sin() * ph.sin(), th.cos()];
            let mut along = 0.0;
            for &(bt, size) in &bumps {
                let d = t - bt;
                if d.abs() < 0.5 {
                    along += size * (-(d / 0.05).powi(2)).exp()
                        - 0.4 * size * (-((d - 0.12) / 0.07).powi(2)).exp();
                }
            }
            let mut v = [
                up[0] * (1.0 + along),
                up[1] * (1.0 + along),
                up[2] * (1.0 + along),
            ];
            if let Part::Shake { hz, .. } = *part {
                let w = 0.6 * (TAU * hz * t).sin();
                for k in 0..3 {
                    v[k] += up[k] * w;
                }
                v[0] += 0.2 * (TAU * hz * t + 1.0).sin();
            }
            for x in &mut v {
                *x += 0.01 * rng.gauss();
            }
            let foot = next_foot < feet.len() && t + DT / 2.0 >= feet[next_foot];
            if foot {
                next_foot += 1;
            }
            out.push(Sample {
                a: v.map(|g| (g * 8192.0).round().clamp(-32768.0, 32767.0) as i16),
                state,
                foot,
            });
        }
        t0 += secs;
    }
    out
}
