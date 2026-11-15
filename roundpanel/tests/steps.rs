//! The `steps` example's detector against its shipped trace.
//!
//! The trace is synthetic (see `examples/common/walktrace.rs`), so this checks
//! the detector does what its docs claim on a walk shaped like the real one,
//! not that it is right about any particular person's walk. That only a walk
//! on the board can tell you.

#[path = "../examples/common/footfall.rs"]
mod footfall;
#[path = "../examples/common/walktrace.rs"]
mod walktrace;

use footfall::{isqrt, Detector, LENIENT, STRICT};

const TRACE: &str = include_str!("../examples/data/walk-trace.csv");

/// What one run of the trace counted.
struct Played {
    counted: u32,
    truth: u32,
    /// Steps counted in the first 1.5 s of shaking, before the shake is
    /// recognised.
    shake_leak: u32,
    /// Steps counted after that, through the shake and the second after it.
    shake_late: u32,
}

fn play(gating: footfall::Gating) -> Played {
    let trace = walktrace::parse(TRACE).expect("the shipped trace parses");
    let mut d = Detector::new(gating);
    let mut p = Played {
        counted: 0,
        truth: 0,
        shake_leak: 0,
        shake_late: 0,
    };
    let (mut in_shake, mut since_shake) = (0u32, u32::MAX);
    for s in &trace {
        let added = d.push(s.a);
        p.truth += s.foot as u32;
        if s.state == b'S' {
            in_shake += 1;
            since_shake = 0;
        } else {
            in_shake = 0;
            since_shake = since_shake.saturating_add(1);
        }
        if in_shake > 0 && in_shake <= 94 {
            p.shake_leak += added;
        } else if since_shake < 62 {
            p.shake_late += added;
        }
    }
    p.counted = d.total();
    p
}

#[test]
fn the_lenient_gates_count_the_walk_and_stop_counting_a_shake() {
    let p = play(LENIENT);
    assert!(p.truth > 80, "the trace has {} footfalls", p.truth);
    let walked = p.counted - p.shake_leak;
    let off = walked.abs_diff(p.truth) * 100 / p.truth;
    assert!(
        off <= 10,
        "lenient counted {walked} of {} true footfalls ({off}% off)",
        p.truth
    );
    // The known gap: a shake's first second looks like a brisk walk until
    // four machine-steady beats have been seen. It must stay that small.
    assert!(
        p.shake_leak <= 6,
        "{} steps leaked from the shake's start",
        p.shake_leak
    );
    assert_eq!(
        p.shake_late, 0,
        "{} steps counted once the shake was recognised",
        p.shake_late
    );
}

#[test]
fn the_strict_gates_drop_much_of_the_same_walk() {
    let (lenient, strict) = (play(LENIENT).counted, play(STRICT));
    let (strict, truth) = (strict.counted, strict.truth);
    assert!(
        strict * 10 < lenient * 8,
        "strict {strict} vs lenient {lenient} of {truth}: the comparison the example draws is gone"
    );
}

#[test]
fn the_integer_square_root_is_exact() {
    for n in (0..200_000u32).chain([u32::MAX, 3 * 32768 * 32768, 8192 * 8192]) {
        let r = isqrt(n) as u64;
        assert!(
            r * r <= n as u64 && (r + 1) * (r + 1) > n as u64,
            "isqrt({n}) = {r}"
        );
    }
}
