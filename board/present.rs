// REFERENCE CODE FOR THE REAL BOARD -- not compiled by this workspace.
// See board/README.md and docs/two-cores.md.
//
// Core 1 sends the frame while core 0 draws the next one. Trimmed from
// Pokewalker's firmware (pw-pet-firmware/src/present.rs and qspi.rs), where it
// took the closed-ball screen from 13.2 to 36.5 frames a second.
//
// The pieces, in the order a frame meets them:
//
//   1. Core 0 finishes drawing into its canvas, waits for the mailbox to be
//      idle, copies the canvas into a snapshot in PSRAM and posts a job. It
//      goes straight on to the next frame.
//   2. Core 1 takes the job and builds one colour table for the frame: all 256
//      palette indices resolved and encoded once, each as a run of four dots.
//   3. It sends the frame in bands of 16 dot rows. Each band is two memory
//      writes (RAMWR, then RAMWRC: the chip forbids a whole window on one
//      RAMWR, p.50), each from its own DMA buffer. There are two pairs of
//      buffers, so core 1 fills the next band's halves while the last half is
//      on the wire.
//   4. It marks the mailbox idle.
//
// On the wire nothing changes: the same CASET, RASET, RAMWR, RAMWRC, in the
// same order, with the same bytes `Spd2010::draw` would send.
//
// Core 1's rules (docs/two-cores.md has the reasons): it never prints or takes
// the console lock, it has a 16 KB stack, it is parked by a flash write
// (`FlashStorage::multicore_auto_park`, board/flash.rs), and nothing here takes
// a lock -- the hand-off is one atomic state byte.
//
// esp-hal 1.2.2 (`unstable`), esp-alloc 0.11.0, allocator-api2. The SPI is
// SPI2 in quad mode at 40 MHz with DMA_CH0; check the signatures against the
// versions you pin.

use core::cell::UnsafeCell;
use core::sync::atomic::Ordering::{Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicU32, AtomicU8};

use esp_hal::dma::DmaTxBuf;
use esp_hal::spi::master::{Address, Command, DataMode, SpiDma, SpiDmaTransfer};
use esp_hal::system::{CpuControl, Stack};
use esp_hal::time::Instant;
use esp_hal::Blocking;
use spd2010::cmd::op::{RAMWR, RAMWRC};
use spd2010::qspi::{opcode, Lanes, QspiBus, Transaction};
use spd2010::{PixelFormat, Spd2010};

/// The canvas your code draws: one palette index a pixel, shown at 4 x 4 dots.
pub const WIDTH: usize = 103;
pub const HEIGHT: usize = 103;
pub const UPSCALE: usize = 4;
/// Panel dots a side: 412.
pub const DOTS: usize = WIDTH * UPSCALE;
/// 65k colour: two bytes a dot, a third less on the wire than 16.7M.
pub const FORMAT: PixelFormat = PixelFormat::Rgb565;

pub struct Canvas {
    pub pixels: [u8; WIDTH * HEIGHT],
    pub palette: [[u8; 3]; 256],
}

/// Dot rows in a band, and bytes in half a band (one memory write).
pub const BAND_ROWS: usize = 16;
pub const HALF_BYTES: usize = DOTS * BAND_ROWS; // 412 * 16 * 2 / 2

// ---------------------------------------------------------------------------
// The bus: a colour write that returns before it is finished.
// ---------------------------------------------------------------------------

/// SPI2 as `spd2010` wants it, plus [`EspQspi::start_colour`]: a memory write
/// from the caller's own DMA buffer that does not wait for the wire.
pub struct EspQspi<'d> {
    spi: Option<SpiDma<'d, Blocking>>,
    flight: Option<SpiDmaTransfer<'d, Blocking, DmaTxBuf>>,
    /// A finished write's buffer, if a command had to wait for it.
    landed: Option<DmaTxBuf>,
}

impl<'d> EspQspi<'d> {
    pub fn new(spi: SpiDma<'d, Blocking>) -> Self {
        EspQspi { spi: Some(spi), flight: None, landed: None }
    }

    /// Start `RAMWR` or `RAMWRC` with the first `len` bytes of `buf`. Gives the
    /// buffer back if the bus would not start.
    pub fn start_colour(&mut self, ram: u8, mut buf: DmaTxBuf, len: usize) -> Result<(), DmaTxBuf> {
        if let Some(b) = self.finish() {
            self.landed = Some(b);
        }
        let Some(spi) = self.spi.take() else { return Err(buf) };
        buf.set_length(len);
        let cmd = Command::_8Bit(opcode::WRITE_COLOUR as u16, DataMode::SingleTwoDataLines);
        let addr = Address::_24Bit((ram as u32) << 8, DataMode::SingleTwoDataLines);
        match spi.half_duplex_write_buffer(DataMode::Quad, cmd, addr, 0, len, buf) {
            Ok(t) => {
                self.flight = Some(t);
                Ok(())
            }
            Err((_, spi, buf)) => {
                self.spi = Some(spi);
                Err(buf)
            }
        }
    }

    /// Wait for the write on the wire, if any, and have its buffer back.
    pub fn finish(&mut self) -> Option<DmaTxBuf> {
        if let Some(t) = self.flight.take() {
            let (spi, buf) = t.wait();
            self.spi = Some(spi);
            return Some(buf);
        }
        self.landed.take()
    }
}

impl QspiBus for EspQspi<'_> {
    type Error = esp_hal::spi::Error;

    fn transfer(&mut self, t: &Transaction<'_>) -> Result<(), Self::Error> {
        // A command waits for any colour write still on the wire.
        if let Some(b) = self.finish() {
            self.landed = Some(b);
        }
        let Some(spi) = self.spi.as_mut() else { return Ok(()) };
        let cmd = Command::_8Bit(t.opcode as u16, DataMode::SingleTwoDataLines);
        let addr = Address::_24Bit(t.address & 0x00FF_FFFF, DataMode::SingleTwoDataLines);
        let mode = match t.lanes {
            Lanes::Single => DataMode::SingleTwoDataLines,
            Lanes::Quad => DataMode::Quad,
        };
        spi.half_duplex_write(mode, cmd, addr, 0, t.data)
    }
}

// ---------------------------------------------------------------------------
// The mailbox: one frame in flight, no lock.
// ---------------------------------------------------------------------------

const IDLE: u8 = 0;
const POSTED: u8 = 1;

pub struct Mailbox {
    state: AtomicU8,
    job: UnsafeCell<Option<*const Canvas>>,
}

// SAFETY: core 0 writes `job` only while IDLE, core 1 reads it only while
// POSTED; the state is stored with Release and loaded with Acquire.
unsafe impl Sync for Mailbox {}

impl Mailbox {
    const fn new() -> Self {
        Mailbox { state: AtomicU8::new(IDLE), job: UnsafeCell::new(None) }
    }

    pub fn idle(&self) -> bool {
        self.state.load(Acquire) == IDLE
    }

    pub fn posted(&self) -> bool {
        self.state.load(Acquire) == POSTED
    }

    /// Core 0: hand over a snapshot. Only when [`Mailbox::idle`].
    pub fn post(&self, canvas: *const Canvas) {
        unsafe { *self.job.get() = Some(canvas) };
        self.state.store(POSTED, Release);
    }

    fn take(&self) -> Option<*const Canvas> {
        if self.state.load(Acquire) != POSTED {
            return None;
        }
        unsafe { *self.job.get() }
    }

    fn done(&self) {
        unsafe { *self.job.get() = None };
        self.state.store(IDLE, Release);
    }
}

pub static MAILBOX: Mailbox = Mailbox::new();

/// Core 1's time, microseconds, read by core 0's heartbeat (docs/two-cores.md).
pub static ENCODE_US: AtomicU32 = AtomicU32::new(0);
pub static TX_US: AtomicU32 = AtomicU32::new(0);
pub static SENT: AtomicU32 = AtomicU32::new(0);

fn add_us(to: &AtomicU32, since: Instant) {
    to.fetch_add(since.elapsed().as_micros() as u32, Relaxed);
}

// ---------------------------------------------------------------------------
// The colour table: 256 encodes a frame instead of 10,609.
// ---------------------------------------------------------------------------

/// Every index's encoded colour, repeated across the four dots it covers.
/// `roundpanel/examples/common/colourtable.rs` is the same thing on the
/// desktop, with a test that it gives the same bytes as encoding per pixel.
#[inline(never)]
fn colour_table(canvas: &Canvas) -> [[u8; UPSCALE * 2]; 256] {
    let mut table = [[0u8; UPSCALE * 2]; 256];
    for (rgb, run) in canvas.palette.iter().zip(table.iter_mut()) {
        let mut px = [0u8; 2];
        FORMAT.encode(*rgb, &mut px);
        for dot in run.chunks_exact_mut(2) {
            dot.copy_from_slice(&px);
        }
    }
    table
}

/// Dot rows `y0..y0 + rows` of the canvas into `out`.
fn fill(canvas: &Canvas, table: &[[u8; UPSCALE * 2]; 256], y0: usize, rows: usize, out: &mut [u8]) {
    const ROW_BYTES: usize = DOTS * 2;
    for r in 0..rows {
        let cy = (y0 + r) / UPSCALE;
        // A dot row under the same canvas row as the one above is a copy.
        if r > 0 && (y0 + r - 1) / UPSCALE == cy {
            out.copy_within((r - 1) * ROW_BYTES..r * ROW_BYTES, r * ROW_BYTES);
            continue;
        }
        let row = &canvas.pixels[cy * WIDTH..(cy + 1) * WIDTH];
        let line = &mut out[r * ROW_BYTES..(r + 1) * ROW_BYTES];
        for (dots, &index) in line.chunks_exact_mut(UPSCALE * 2).zip(row) {
            dots.copy_from_slice(&table[index as usize]);
        }
    }
}

// ---------------------------------------------------------------------------
// The presenter: core 1's side.
// ---------------------------------------------------------------------------

/// Dot rows in the first memory write of a band of `rows`: half its pixels,
/// rounded down to the four the chip writes at a time -- the split
/// `Spd2010::write_pixels` makes. For 16 rows of 412 that is exactly 8 rows,
/// so each half is whole rows and can be made into its own buffer.
const fn first_rows(rows: usize) -> usize {
    ((rows * DOTS / 2) & !3) / DOTS
}

const _: () = assert!(((BAND_ROWS * DOTS / 2) & !3) % DOTS == 0);
const _: () = assert!(DOTS % BAND_ROWS == 12); // the last band is 12 rows
const _: () = assert!(((12 * DOTS / 2) & !3) % DOTS == 0);

pub struct Presenter<'d> {
    panel: Spd2010<EspQspi<'d>>,
    /// Two pairs of half-band buffers: one pair filled while the other sends.
    halves: [[Option<DmaTxBuf>; 2]; 2],
    flying: Option<(usize, usize)>,
}

impl<'d> Presenter<'d> {
    /// `panel` already initialised; four `dma_tx_buffer!(HALF_BYTES)`s.
    pub fn new(panel: Spd2010<EspQspi<'d>>, halves: [DmaTxBuf; 4]) -> Self {
        let [a, b, c, d] = halves;
        Presenter { panel, halves: [[Some(a), Some(b)], [Some(c), Some(d)]], flying: None }
    }

    /// Send the posted frame, if there is one, calling `between` after each
    /// band (the sound engine, the sensor loan). Whether there was one.
    pub fn serve(&mut self, between: &mut impl FnMut()) -> bool {
        let Some(canvas) = MAILBOX.take() else { return false };
        // SAFETY: core 0 does not touch the snapshot while the job is posted.
        let canvas = unsafe { &*canvas };
        let t = Instant::now();
        let table = colour_table(canvas);
        add_us(&ENCODE_US, t);

        let bands = DOTS.div_ceil(BAND_ROWS);
        let rows_of = |k: usize| BAND_ROWS.min(DOTS - k * BAND_ROWS);
        let mut make = |halves: &mut [Option<DmaTxBuf>; 2], k: usize| {
            let t = Instant::now();
            let (top, rows) = (k * BAND_ROWS, rows_of(k));
            let first = first_rows(rows);
            for (h, (y0, n)) in [(top, first), (top + first, rows - first)].into_iter().enumerate() {
                if let Some(buf) = halves[h].as_mut() {
                    fill(canvas, &table, y0, n, &mut buf.as_mut_slice()[..n * DOTS * 2]);
                }
            }
            add_us(&ENCODE_US, t);
        };

        make(&mut self.halves[0], 0);
        for k in 0..bands {
            let set = k % 2;
            let (top, rows) = (k * BAND_ROWS, rows_of(k));
            let first = first_rows(rows);
            let t = Instant::now();
            self.settle();
            let _ = self.panel.set_window(0, top as u16, DOTS as u16 - 1, (top + rows - 1) as u16);
            self.send(set, 0, RAMWR, first);
            add_us(&TX_US, t);
            // The overlap: the next band is made while this one's first half
            // is on the wire.
            if k + 1 < bands {
                make(&mut self.halves[(k + 1) % 2], k + 1);
            }
            let t = Instant::now();
            self.settle();
            self.send(set, 1, RAMWRC, rows - first);
            add_us(&TX_US, t);
            between();
        }
        let t = Instant::now();
        self.settle();
        add_us(&TX_US, t);

        SENT.fetch_add(1, Relaxed);
        MAILBOX.done();
        true
    }

    fn send(&mut self, set: usize, h: usize, ram: u8, rows: usize) {
        if let Some(buf) = self.halves[set][h].take() {
            match self.panel.bus_mut().start_colour(ram, buf, rows * DOTS * 2) {
                Ok(()) => self.flying = Some((set, h)),
                Err(buf) => self.halves[set][h] = Some(buf),
            }
        }
    }

    fn settle(&mut self) {
        if let Some((set, h)) = self.flying.take() {
            self.halves[set][h] = self.panel.bus_mut().finish();
        }
    }
}

// ---------------------------------------------------------------------------
// Starting core 1, and core 0's side of a frame.
// ---------------------------------------------------------------------------

/// Core 1's stack. 16 KB, in internal RAM; everything big it touches is a
/// static or a box, never a local.
static mut CORE1_STACK: Stack<16_384> = Stack::new();

struct OnCore1<'d>(Presenter<'d>);
// SAFETY: moved to core 1 whole and only touched there.
unsafe impl Send for OnCore1<'_> {}

/// Boot: move the presenter to core 1 for good.
pub fn start(cpu: esp_hal::peripherals::CPU_CTRL<'static>, presenter: Presenter<'static>) {
    let mut ctrl = CpuControl::new(cpu);
    let moved = OnCore1(presenter);
    let stack = unsafe { &mut *core::ptr::addr_of_mut!(CORE1_STACK) };
    let guard = ctrl
        .start_app_core(stack, move || {
            let mut whole = moved;
            core1_main(&mut whole.0)
        })
        .expect("core 1");
    // Dropping the guard would park the core.
    core::mem::forget(guard);
}

/// Core 1's whole life. No `println!` anywhere in here or below it.
fn core1_main(presenter: &mut Presenter<'_>) -> ! {
    let mut chores = || {
        // Your audio engine's poll goes here, and the sensor loan:
        crate::lend::tend();
    };
    loop {
        chores();
        // Rows of a picture core 0 offered (board/rows.rs). Pokewalker also
        // took them between bands of a send; that only pays where a picture
        // is longer than a send.
        if crate::rows::help(&mut chores) {
            continue;
        }
        if presenter.serve(&mut chores) {
            continue;
        }
        let t = Instant::now();
        while t.elapsed().as_micros() < 500 && !MAILBOX.posted() && !crate::rows::offered() {
            crate::lend::tend();
            core::hint::spin_loop();
        }
    }
}

/// The snapshot core 1 reads, in PSRAM, so core 0 keeps its own canvas to draw
/// the next frame into. 10.6 KB here; a bigger canvas is the same.
pub fn snapshot_buffer() -> &'static mut Canvas {
    allocator_api2::boxed::Box::leak(allocator_api2::boxed::Box::new_in(
        Canvas { pixels: [0; WIDTH * HEIGHT], palette: [[0; 3]; 256] },
        esp_alloc::ExternalMemory,
    ))
}

/// Core 0, once a frame, after drawing into `canvas`.
pub fn hand_over(canvas: &Canvas, snapshot: &mut Canvas, mut meanwhile: impl FnMut()) {
    // Core 1 may still be sending the last frame. Waiting here is the only
    // place core 0 ever waits for it; the heartbeat's `wait` is this loop.
    while !MAILBOX.idle() {
        meanwhile();
    }
    snapshot.pixels.copy_from_slice(&canvas.pixels);
    snapshot.palette = canvas.palette;
    MAILBOX.post(snapshot as *const Canvas);
}
