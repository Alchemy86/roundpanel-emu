// REFERENCE CODE FOR THE REAL BOARD -- not compiled by this workspace.
// See board/README.md and docs/two-cores.md.
//
// Both cores take rows of one picture from a shared counter. Trimmed from
// Pokewalker's firmware (pw-pet-firmware/src/present.rs, Core1Assist) and its
// engine (pw-world's `Assist`, pw-pet's `twocore::Rows`).
//
// Use it only where core 1 is idle while core 0 draws. On this board core 1
// is usually sending the last frame, and rows offered to it then are rows it
// takes from the send: Pokewalker measured its closed-ball screen SLOWER with
// shared rows and took them out. They paid on the 3D ground and the battle
// floor, where one picture is longer than a whole send.
//
// The rule that keeps the picture the same: rows are MADE on whichever core
// takes them, into a buffer, and then SET into the canvas on core 0, row by
// row and left to right -- the same calls in the same order the one-core
// drawing makes. A row must read only what was there before the picture
// started (never a row another core is writing). Then the output cannot
// change, only how soon it is ready; check that with a frame hash, one core
// against two.

use core::cell::UnsafeCell;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU8};

/// Whoever owns a second core: run `work(y)` for every `y` in `0..rows`
/// exactly once, split however it likes, and return when all are done.
/// On a desktop test, two `std::thread::scope` threads on the same counter.
pub trait Assist {
    fn share(&self, rows: i32, work: &(dyn Fn(i32) + Sync));
}

const IDLE: u8 = 0;
const POSTED: u8 = 1;
const TAKEN: u8 = 2;
const DONE: u8 = 3;

struct Share {
    state: AtomicU8,
    work: UnsafeCell<Option<*const (dyn Fn(i32) + Sync + 'static)>>,
    rows: AtomicI32,
    next: AtomicI32,
}

// SAFETY: core 0 writes `work` only while IDLE; core 1 reads it only after
// moving POSTED to TAKEN; core 0 does not return from `share` -- ending the
// borrow `work` points at -- until the state is IDLE again, which needs core 1
// to have finished.
unsafe impl Sync for Share {}

static SHARE: Share = Share {
    state: AtomicU8::new(IDLE),
    work: UnsafeCell::new(None),
    rows: AtomicI32::new(0),
    next: AtomicI32::new(0),
};

/// Rows core 1 drew, for the heartbeat. If it stays near zero, core 1 was
/// busy and the sharing is only overhead.
pub static HELPED_ROWS: AtomicU32 = AtomicU32::new(0);

/// Core 0's side.
pub struct Core1Assist;

impl Assist for Core1Assist {
    fn share(&self, rows: i32, work: &(dyn Fn(i32) + Sync)) {
        let erased: *const (dyn Fn(i32) + Sync + '_) = work;
        // SAFETY: only the lifetime is erased; see `impl Sync for Share`.
        let erased: *const (dyn Fn(i32) + Sync + 'static) = unsafe { core::mem::transmute(erased) };
        SHARE.next.store(0, Relaxed);
        SHARE.rows.store(rows, Relaxed);
        unsafe { *SHARE.work.get() = Some(erased) };
        SHARE.state.store(POSTED, Release);
        // Core 0 draws too, from the same counter.
        loop {
            let y = SHARE.next.fetch_add(1, Relaxed);
            if y >= rows {
                break;
            }
            work(y);
        }
        // If core 1 never took the offer, withdraw it; otherwise wait for it.
        if SHARE.state.compare_exchange(POSTED, IDLE, Acquire, Relaxed).is_err() {
            while SHARE.state.load(Acquire) != DONE {
                core::hint::spin_loop();
            }
            SHARE.state.store(IDLE, Relaxed);
        }
        unsafe { *SHARE.work.get() = None };
    }
}

/// Core 1's side: if rows are on offer, draw them until none are left, calling
/// `between` after each (the sound engine, `lend::tend`). Whether there were
/// any. Call it from core 1's loop and between bands of a send.
pub fn help(between: &mut impl FnMut()) -> bool {
    if SHARE.state.compare_exchange(POSTED, TAKEN, Acquire, Relaxed).is_err() {
        return false;
    }
    let work = unsafe { &*(*SHARE.work.get()).unwrap() };
    let rows = SHARE.rows.load(Relaxed);
    let mut n = 0;
    loop {
        let y = SHARE.next.fetch_add(1, Relaxed);
        if y >= rows {
            break;
        }
        work(y);
        n += 1;
        between();
    }
    HELPED_ROWS.fetch_add(n, Relaxed);
    SHARE.state.store(DONE, Release);
    true
}

/// Whether rows are waiting, so core 1's idle wait can end early.
pub fn offered() -> bool {
    SHARE.state.load(Acquire) == POSTED
}

// Using it: make each row into a buffer of atomics, then set the canvas in
// one-core order.
//
//     static ROWS: [AtomicU16; WIDTH * HEIGHT]   // PSRAM in practice
//     const SET: u16 = 0x100;                    // "this pixel was drawn"
//
//     Core1Assist.share(h, &|y| {
//         let line = &ROWS[y as usize * WIDTH..][..WIDTH];
//         for p in line { p.store(0, Relaxed) }
//         draw_row(&canvas_as_it_was, y, |x, index| line[x].store(SET | index as u16, Relaxed));
//     });
//     for y in 0..h { for x in 0..WIDTH {
//         let v = ROWS[y * WIDTH + x].load(Relaxed);
//         if v & SET != 0 { canvas.set(x, y, v as u8) }
//     }}
