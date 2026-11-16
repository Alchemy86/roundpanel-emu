// REFERENCE CODE FOR THE REAL BOARD -- not compiled by this workspace.
// See board/README.md and docs/two-cores.md.
//
// Lend core 1 the I2C bus while core 0 draws. Trimmed from Pokewalker's
// firmware (pw-pet-firmware/src/lend.rs).
//
// The touch digitiser and the QMI8658 share one I2C bus. Once core 1 sends
// the frame (board/present.rs), core 0 only gets to read them between
// pictures -- and a long picture (50 ms and more for an open-world scene) is a
// long time without a reading. A software pedometer wants a sample at least
// 25 times a second (docs/imu-and-steps.md); a finger wants to be seen sooner.
//
// So for the length of a picture core 0 lends core 1 the bus and both devices.
// Core 1 reads them on the same cadence between its own bands and keeps each
// reading with the time it was taken. When the picture is done core 0 takes
// the bus back and replays the readings, oldest first, through exactly the
// code that would have handled them had it read them itself at those times.
// Only the reading moves. What a reading means does not.
//
// Nothing here takes a lock: the loan is one atomic state byte and the
// readings are a single-producer, single-consumer ring.
//
// esp-hal 1.2.2 (`unstable`). `Touch` and `Imu` are your drivers (the IMU's is
// board/qmi8658.rs); each needs only `&mut I2c` to make a reading.

use core::cell::UnsafeCell;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicU32, AtomicU8, AtomicUsize};

use esp_hal::i2c::master::I2c;
use esp_hal::time::Instant;
use esp_hal::Blocking;

use crate::imu::Imu;
use crate::touch::{Point, Touch};

/// One reading made on core 1, each half stamped with when it was taken.
#[derive(Clone, Copy)]
pub struct Reading {
    pub touch_us: u64,
    pub touch: Result<Option<Point>, ()>,
    pub accel: Option<(u64, (i16, i16, i16))>,
}

/// Core 0 owns the bus.
const OWNED: u8 = 0;
/// Lent: core 1 may take it for a reading.
const LENT: u8 = 1;
/// Core 1 is in the middle of a reading.
const BUSY: u8 = 2;

struct Loan {
    i2c: *mut I2c<'static, Blocking>,
    touch: *mut Touch,
    imu: *mut Option<Imu>,
    boot: Instant,
    every_us: u32,
}

const RING: usize = 16;

struct Shared {
    state: AtomicU8,
    loan: UnsafeCell<Option<Loan>>,
    /// When either core last read the bus, microseconds since boot (wrapping).
    last_us: AtomicU32,
    ring: UnsafeCell<[Option<Reading>; RING]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

// SAFETY: core 0 writes the loan only while OWNED; core 1 uses it only while
// it holds BUSY, which it can only take from LENT; core 0 reclaims only from
// LENT, so never mid-read. The ring has one producer (core 1) and one consumer
// (core 0), its head and tail stored with Release and loaded with Acquire.
unsafe impl Sync for Shared {}

static SHARED: Shared = Shared {
    state: AtomicU8::new(OWNED),
    loan: UnsafeCell::new(None),
    last_us: AtomicU32::new(0),
    ring: UnsafeCell::new([None; RING]),
    head: AtomicUsize::new(0),
    tail: AtomicUsize::new(0),
};

/// Readings core 1 made, for the heartbeat.
pub static READS: AtomicU32 = AtomicU32::new(0);

/// Core 0 read the bus itself at `now_us`.
pub fn read_here(now_us: u64) {
    SHARED.last_us.store(now_us as u32, Relaxed);
}

/// Whether a reading is due, `every_us` after the last one by either core.
pub fn due(now_us: u64, every_us: u64) -> bool {
    (now_us as u32).wrapping_sub(SHARED.last_us.load(Relaxed)) as u64 >= every_us
}

/// Core 0, before drawing: lend the bus and both devices until [`reclaim`].
pub fn lend(i2c: &mut I2c<'_, Blocking>, touch: &mut Touch, imu: &mut Option<Imu>, boot: Instant, every_us: u64) {
    let loan = Loan {
        i2c: i2c as *mut I2c<'_, Blocking> as *mut I2c<'static, Blocking>,
        touch,
        imu,
        boot,
        every_us: every_us as u32,
    };
    unsafe { *SHARED.loan.get() = Some(loan) };
    SHARED.state.store(LENT, Release);
}

/// Core 0, after drawing: take the bus back, waiting out a reading in flight.
pub fn reclaim() {
    while SHARED.state.compare_exchange(LENT, OWNED, Acquire, Relaxed).is_err() {
        core::hint::spin_loop();
    }
    unsafe { *SHARED.loan.get() = None };
}

/// Core 0: the next reading core 1 made, oldest first.
pub fn take() -> Option<Reading> {
    let tail = SHARED.tail.load(Relaxed);
    if tail == SHARED.head.load(Acquire) {
        return None;
    }
    let r = unsafe { (*SHARED.ring.get())[tail % RING].take() };
    SHARED.tail.store(tail.wrapping_add(1), Release);
    r
}

/// Core 1, between bands: read both devices if they are lent and a reading is
/// due. A load and a return when they are not.
pub fn tend() {
    if SHARED.state.load(Relaxed) != LENT {
        return;
    }
    if SHARED.state.compare_exchange(LENT, BUSY, Acquire, Relaxed).is_err() {
        return;
    }
    if let Some(loan) = unsafe { (*SHARED.loan.get()).as_ref() } {
        let now_us = loan.boot.elapsed().as_micros();
        let head = SHARED.head.load(Relaxed);
        let room = head.wrapping_sub(SHARED.tail.load(Acquire)) < RING;
        if room && due(now_us, loan.every_us as u64) {
            SHARED.last_us.store(now_us as u32, Relaxed);
            let (i2c, touch, imu) = unsafe { (&mut *loan.i2c, &mut *loan.touch, &mut *loan.imu) };
            let touch = touch.poll(i2c);
            let accel = imu.as_mut().map(|m| (loan.boot.elapsed().as_micros(), m.accel(i2c)));
            unsafe { (*SHARED.ring.get())[head % RING] = Some(Reading { touch_us: now_us, touch, accel }) };
            SHARED.head.store(head.wrapping_add(1), Release);
            READS.fetch_add(1, Relaxed);
        }
    }
    SHARED.state.store(LENT, Release);
}

// Core 0's frame, in outline:
//
//     lend::lend(&mut i2c, &mut touch, &mut imu, boot, 15_000);
//     draw(&mut canvas);                       // core 1 reads meanwhile
//     lend::reclaim();
//     while let Some(r) = lend::take() {
//         on_touch(r.touch_us, r.touch);       // the same handlers, the
//         if let Some((at, xyz)) = r.accel {   // same timestamps, as if
//             pedometer.accept(xyz, at);       // core 0 had read them
//         }
//     }
//     present::hand_over(&canvas, snapshot, || {
//         if lend::due(now_us(), 15_000) { poll_here(); lend::read_here(now_us()); }
//     });
