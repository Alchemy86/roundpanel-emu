// REFERENCE CODE FOR THE REAL BOARD -- not compiled by this workspace.
// See board/README.md and docs/imu-and-steps.md.
//
// The QMI8658 on the Waveshare ESP32-S3-Touch-LCD-1.46B, brought up the way
// the two vendor-side references do it, and read by polling. On a real board
// this answered `WHO_AM_I 0x05`, `rev 0x7c`, firmware `01030e`, and acked
// every CTRL9 command on STATUSINT -- the QMI8658A command set, although
// Waveshare's schematic names a QMI8658C.
//
// References, cited per line below:
//   [WS]  Waveshare's demo for this board: waveshareteam/ESP32-S3-Touch-LCD-1.46,
//         example/ESP-IDF-5.3.2/ESP32-S3-Touch-LCD-1.46-Test/main/QMI8658/QMI8658.c
//         (identical in the Arduino demo's Gyro_QMI8658.cpp).
//   [SL]  lewisxhe/SensorLib, examples/sensor/qmi8658_pedometer/qmi8658_pedometer.ino,
//         and the calls it makes in src/sensor/imu/qmi8658/SensorQMI8658.cpp.
//   [DS]  QMI8658A datasheet, Rev A.
//
// What did NOT work, so do not reintroduce it without a reference that does:
//   * The FIFO together with the pedometer. It passed a few drains and then
//     froze for good -- the FIFO *and* the data registers -- whenever the step
//     count was also being read. Neither reference combines them.
//   * Accelerometer-only (CTRL7 = 0x01). Both references run the gyro too.
//   * Taking STATUS1 bit 0 as the CTRL9 "done" bit. That is the C part's bit
//     and reserved on the A; accepting it let a read run before the chip was
//     ready, and the sensor died at the first runtime FIFO drain.
//
// And the chip's own step count never moved on a real walk, even with this
// exact sequence. Count steps in software from the samples
// (roundpanel/examples/common/footfall.rs); read STEP_CNT as well if you like,
// and prefer it only once you have seen it move.

use esp_hal::delay::Delay;
use esp_hal::i2c::master::I2c;
use esp_hal::Blocking;

/// I2C: shared with the touch controller and the RTC, GPIO 11 SDA / 10 SCL,
/// 100 kHz. The IMU answers on 0x6B (0x6A if SA0 is strapped low).
pub const ADDRS: [u8; 2] = [0x6B, 0x6A];

const WHO_AM_I: u8 = 0x00; // reads 0x05
const REVISION: u8 = 0x01;
const CTRL1: u8 = 0x02;
const CTRL2: u8 = 0x03;
const CTRL3: u8 = 0x04;
const CTRL5: u8 = 0x06;
const CTRL6: u8 = 0x07;
const CTRL7: u8 = 0x08;
const CTRL8: u8 = 0x09;
const CTRL9: u8 = 0x0A;
const CAL1_L: u8 = 0x0B;
const CAL2_L: u8 = 0x0D;
const CAL2_H: u8 = 0x0E;
const CAL3_L: u8 = 0x0F;
const CAL3_H: u8 = 0x10;
const CAL4_L: u8 = 0x11;
const CAL4_H: u8 = 0x12;
const FIFO_CTRL: u8 = 0x14;
const STATUSINT: u8 = 0x2D;
const AX_L: u8 = 0x35;
const DQW_L: u8 = 0x49; // firmware version after COPY_USID
const RST_RESULT: u8 = 0x4D;
const STEP_CNT_L: u8 = 0x5A;
const RESET: u8 = 0x60;

const CMD_ACK: u8 = 0x00;
const CMD_CONFIGURE_PEDOMETER: u8 = 0x0D;
const CMD_RESET_PEDOMETER: u8 = 0x0F;
const CMD_COPY_USID: u8 = 0x10;

/// Counts per g at +/-4 g. At rest the board reads about 974-1012 mg.
pub const COUNTS_PER_G: u32 = 8192;

pub struct Qmi8658 {
    addr: u8,
    delay: Delay,
}

impl Qmi8658 {
    pub fn new(i2c: &mut I2c<'_, Blocking>, delay: Delay) -> Option<Self> {
        let addr = ADDRS.into_iter().find(|&a| {
            let mut b = [0u8; 1];
            i2c.write_read(a, &[WHO_AM_I], &mut b).is_ok() && b[0] == 0x05
        })?;
        let mut imu = Qmi8658 { addr, delay };
        imu.init(i2c);
        Some(imu)
    }

    fn wr(&mut self, i2c: &mut I2c<'_, Blocking>, reg: u8, v: u8) {
        let _ = i2c.write(self.addr, &[reg, v]);
    }

    fn rd(&mut self, i2c: &mut I2c<'_, Blocking>, reg: u8) -> u8 {
        let mut b = [0xFF];
        let _ = i2c.write_read(self.addr, &[reg], &mut b);
        b[0]
    }

    fn w16(&mut self, i2c: &mut I2c<'_, Blocking>, lo: u8, v: u16) {
        self.wr(i2c, lo, v as u8);
        self.wr(i2c, lo + 1, (v >> 8) as u8);
    }

    fn init(&mut self, i2c: &mut I2c<'_, Blocking>) {
        // Soft reset, then wait for RST_RESULT bit 7. [SL] reset(), 298-316.
        self.wr(i2c, RESET, 0xB0);
        self.delay.delay_millis(2);
        for _ in 0..50 {
            let r = self.rd(i2c, RST_RESULT);
            if r != 0xFF && r & 0x80 != 0 {
                break;
            }
            self.delay.delay_millis(10);
        }

        // Register auto-increment on, bit 0 clear. [WS] setState, line 212.
        let c1 = self.rd(i2c, CTRL1);
        self.wr(i2c, CTRL1, (c1 & 0xFE) | 0x40);
        let _rev = self.rd(i2c, REVISION); // 0x7c on the board

        // CTRL9 handshake enabled, then COPY_USID makes the firmware version
        // readable at DQW_L. [SL] initImpl, 2456-2458.
        self.wr(i2c, CTRL8, 0x80);
        let _ = self.command(i2c, CMD_COPY_USID);
        let mut _fw = [0u8; 3];
        let _ = i2c.write_read(self.addr, &[DQW_L], &mut _fw); // 01030e on the board

        // FIFO in bypass. FIFO and the data registers are either-or [DS 6.2]:
        // in stream mode AX..AZ freeze, so polling needs bypass.
        self.wr(i2c, FIFO_CTRL, 0x00);
        self.wr(i2c, CTRL6, 0x00); // [WS] line 219
        // +/-4 g, 112.1 Hz with the gyro on. [SL] configAccel(FS_4G, 112), 171-173.
        self.wr(i2c, CTRL2, 0x16);
        // 1024 dps, 112.1 Hz. [SL] configGyro(FS_1000_DPS, 112), 174-176.
        self.wr(i2c, CTRL3, 0x66);
        // Both low-pass filters on, mode 0. [SL] default LpfMode::MODE_0.
        self.wr(i2c, CTRL5, 0x11);
        // Accelerometer AND gyroscope. [WS] line 218 (0x43), [SL] 178-179.
        self.wr(i2c, CTRL7, 0x03);

        // Pedometer: sensors off while it is configured, in two CAL pages.
        // [SL] configPedometer(50, 80, 60, 400, 8, 1, 0, 1), line 186,
        // implemented at SensorQMI8658.cpp 1760-1814.
        self.wr(i2c, CTRL7, 0x00);
        self.w16(i2c, CAL1_L, 50); // ped_sample_cnt
        self.w16(i2c, CAL2_L, 80); // ped_fix_peak2peak
        self.w16(i2c, CAL3_L, 60); // ped_fix_peak
        self.wr(i2c, CAL4_H, 0x01); // first page
        self.wr(i2c, CAL4_L, 0x02);
        let _ = self.command(i2c, CMD_CONFIGURE_PEDOMETER);
        self.w16(i2c, CAL1_L, 400); // ped_time_up
        self.wr(i2c, CAL2_L, 8); // ped_time_low
        self.wr(i2c, CAL2_H, 1); // ped_time_cnt_entry
        self.wr(i2c, CAL3_L, 0); // ped_fix_precision
        self.wr(i2c, CAL3_H, 1); // ped_sig_count
        self.wr(i2c, CAL4_H, 0x02); // second page
        self.wr(i2c, CAL4_L, 0x02);
        let _ = self.command(i2c, CMD_CONFIGURE_PEDOMETER);
        self.wr(i2c, CTRL7, 0x03);

        // Reset the count and enable the pedometer. [SL] resetStepCount (189),
        // enablePedometer (190), SensorQMI8658.cpp 1831-1879.
        let _ = self.command(i2c, CMD_RESET_PEDOMETER);
        self.wr(i2c, CTRL8, 0x80);
        self.wr(i2c, CTRL8, 0x80 | 0x10);
        self.wr(i2c, CTRL7, 0x03);

        // Then check the accelerometer is live: a dozen reads over ~200 ms
        // should not all be identical. Pokewalker falls back to an older,
        // accelerometer-only init if they are, and logs which ran.
    }

    /// The CTRL9 protocol: write the command, wait for STATUSINT bit 7, write
    /// the acknowledge, wait for bit 7 to clear. STATUSINT only -- see the
    /// header. Every wait has a deadline (100 ms); never spin unbounded on a
    /// sensor that may have wedged.
    fn command(&mut self, i2c: &mut I2c<'_, Blocking>, cmd: u8) -> bool {
        const POLLS: u32 = 1000; // x 100 us
        self.wr(i2c, CTRL9, cmd);
        let done = |s: &mut Self, i2c: &mut I2c<'_, Blocking>| {
            let b = s.rd(i2c, STATUSINT);
            b != 0xFF && b & 0x80 != 0
        };
        let mut seen = false;
        for _ in 0..POLLS {
            if done(self, i2c) {
                seen = true;
                break;
            }
            self.delay.delay_micros(100);
        }
        self.wr(i2c, CTRL9, CMD_ACK);
        if !seen {
            return false;
        }
        for _ in 0..POLLS {
            if !done(self, i2c) {
                return true;
            }
            self.delay.delay_micros(100);
        }
        false
    }

    /// The three axes, raw counts: six bytes from AX_L, little-endian.
    /// [WS] getAccelerometer, lines 263-267.
    pub fn accel(&mut self, i2c: &mut I2c<'_, Blocking>) -> Option<[i16; 3]> {
        let mut b = [0u8; 6];
        i2c.write_read(self.addr, &[AX_L], &mut b).ok()?;
        Some([
            i16::from_le_bytes([b[0], b[1]]),
            i16::from_le_bytes([b[2], b[3]]),
            i16::from_le_bytes([b[4], b[5]]),
        ])
    }

    /// The chip's own 24-bit step total. [SL] getStepCount, 1867-1874.
    pub fn chip_steps(&mut self, i2c: &mut I2c<'_, Blocking>) -> Option<u32> {
        let mut b = [0u8; 3];
        i2c.write_read(self.addr, &[STEP_CNT_L], &mut b).ok()?;
        Some(b[0] as u32 | (b[1] as u32) << 8 | (b[2] as u32) << 16)
    }
}

/// Turns irregular polls into a steady 62.5 Hz stream for the detector.
///
/// The frame loop cannot read the IMU on a timer: it is busy pushing display
/// bands. So it reads once between bands (about 55-70 reads a second on this
/// board) and hands each reading on once for every 16 ms period that has
/// passed since the last, so the detector always sees 62.5 samples a second.
pub struct Resampler {
    next_us: Option<u64>,
}

impl Resampler {
    pub const fn new() -> Self {
        Resampler { next_us: None }
    }

    /// The most periods one call emits: a 2 s stall (a flash write) is
    /// replayed in full, anything longer only as its last 2 s.
    pub const MAX_BURST: u32 = 125;

    /// How many copies of a reading taken at `now_us` the detector gets.
    pub fn periods(&mut self, now_us: u64) -> u32 {
        const PERIOD_US: u64 = 16_000;
        let Some(next) = self.next_us else {
            self.next_us = Some(now_us + PERIOD_US);
            return 1;
        };
        if now_us < next {
            return 0;
        }
        let due = (now_us - next) / PERIOD_US + 1;
        self.next_us = Some(next + due * PERIOD_US);
        due.min(Self::MAX_BURST as u64) as u32
    }
}

// In the frame loop, between display bands:
//
//     if let Some(a) = imu.accel(&mut i2c) {
//         for _ in 0..resampler.periods(now_us) {
//             steps += detector.push(a);   // footfall::Detector::new(LENIENT)
//         }
//     }
//
// and log, every ten seconds: the chip's count, the software count, the read
// rate, and whether the accelerometer is still changing. "acc STALE" is the
// first thing to look for when steps stop.
