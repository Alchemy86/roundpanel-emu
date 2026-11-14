//! The chiptune example's engine, checked without a window or a speaker.
//!
//! The claim that matters: skipping silent voices and batching noise runs make
//! the mixer cheaper and change nothing it outputs. A render checksum with and
//! without each is the proof, the same check that guarded the fix on the real
//! board.

#[path = "../examples/common/chipsynth.rs"]
mod chipsynth;

use chipsynth::{render, Command, Shared, Spsc, RATE};

fn fnv1a(frames: &[i16]) -> u64 {
    frames.iter().fold(0xcbf2_9ce4_8422_2325, |h, &s| {
        s.to_le_bytes()
            .iter()
            .fold(h, |h, &b| (h ^ b as u64).wrapping_mul(0x0100_0000_01b3))
    })
}

#[test]
fn skipping_silent_voices_and_batching_noise_change_no_output() {
    let frames = RATE * 10;
    let zaps = [RATE, RATE * 5 / 2, RATE * 6];
    let run = |skip, batch| render(&Shared::new(), frames, skip, batch, &zaps);

    let (reference, steps_naive) = run(false, false);
    let peak = reference.iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert!(peak > 4000, "the tune is close to silent: peak {peak}");

    for (skip, batch) in [(true, false), (false, true), (true, true)] {
        let (out, _) = run(skip, batch);
        assert_eq!(
            fnv1a(&out),
            fnv1a(&reference),
            "skip {skip} batch {batch} changed the output"
        );
    }

    // And the skip is not a no-op: the drums leave the noise voice silent and
    // still stepping every 8 cycles.
    let (_, steps_skipped) = run(true, true);
    assert!(
        steps_skipped * 5 < steps_naive * 4,
        "skip saved too little: {steps_skipped} steps against {steps_naive}"
    );
}

#[test]
fn the_queue_is_first_in_first_out_and_refuses_when_full() {
    let q = Spsc::new();
    assert_eq!(q.pop(), None);
    for i in 0..64 {
        assert!(q.push(i));
    }
    assert!(!q.push(99), "a full queue accepted a command");
    for i in 0..64 {
        assert_eq!(q.pop(), Some(i));
    }
    assert_eq!(q.pop(), None);
    for cmd in [
        Command::Zap,
        Command::SetSkip(true),
        Command::SetSkip(false),
        Command::ToggleMute(3),
    ] {
        assert_eq!(Command::unpack(cmd.pack()), Some(cmd));
    }
}
