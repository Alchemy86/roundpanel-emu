// REFERENCE CODE FOR THE REAL BOARD -- not compiled by this workspace, and it
// cannot run in the emulator: esp-radio drives the ESP32-S3's own radio.
// See board/README.md and docs/wifi.md.
//
// The shape that works on this board, trimmed from Pokewalker's firmware
// (pw-pet-firmware/src/wifi.rs). Each rule below is a real failure:
//
//   1. Never bring the radio up at boot. The boot path keeps the peripherals
//      and nothing else; the radio (and the esp-rtos scheduler it needs) start
//      inside the frame loop, when the user asks. A boot-time bring-up froze
//      the real walker before its first frame.
//   2. Internal heap first, PSRAM second. esp-alloc hands an allocation to
//      the first region with room, and that includes the WiFi blob's own C
//      malloc, which needs internal RAM. PSRAM-first booted to "wifi up" and
//      then the radio stalled.
//   3. Size esp-radio's event channel (Cargo env, below). The default holds 2
//      events and the driver DROPS a new one when it is full -- including the
//      ScanDone a scan is waiting for. Drain your subscriber often.
//   4. Every wait on the radio has a deadline. A stalled radio plus an
//      unbounded wait is a frozen device.
//   5. MTU 1492, not 1514: a full-size frame overran esp-radio's tx buffer
//      and panicked on the first large packet to a phone.
//
// Cargo.toml (versions this was written against):
//
//     esp-hal   = { version = "1.2.2", features = ["esp32s3", "unstable"] }
//     esp-alloc = { version = "0.11.0", features = ["esp32s3"] }
//     esp-radio = { version = "=1.0.0-beta.1", features = ["esp32s3", "unstable", "wifi"] }
//     esp-rtos  = { version = "0.4.0", features = ["esp32s3", "esp-radio", "esp-alloc"] }
//     smoltcp   = { version = "0.12.0", default-features = false, features = [
//                   "alloc", "medium-ethernet", "proto-ipv4", "socket-udp",
//                   "socket-tcp", "socket-dhcpv4", "proto-dns", "socket-dns"] }
//
// .cargo/config.toml:
//
//     [env]
//     ESP_RADIO_CONFIG_EVENT_CHANNEL_CAPACITY = "16"
//     ESP_RADIO_CONFIG_EVENT_CHANNEL_SUBSCRIBERS = "4"

use core::future::Future;
use core::pin::pin;
use core::task::{Context, Poll, Waker};

use esp_hal::peripherals::{FROM_CPU_INTR0, TIMG0, WIFI};
use esp_hal::time::Instant;
use esp_radio::wifi::{ControllerConfig, WifiController};

// --- in main(), before anything allocates -----------------------------------
//
//     // 72 KB internal heap FIRST: the radio's buffers and the blob's malloc
//     // land here. Then PSRAM for anything big that does not need to be
//     // internal (pictures, web pages: allocate those with
//     // esp_alloc::ExternalMemory on purpose).
//     esp_alloc::heap_allocator!(size: 72 * 1024);
//     esp_alloc::psram_allocator!(p.PSRAM, esp_hal::psram);
//
//     // The WiFi driver keeps ~34 KB of code in IRAM, which on the S3 is the
//     // same SRAM the stack comes from. Re-check the stack's size
//     // (`readelf -S`, the .stack section) after any RAM change.
//
//     // Keep the peripherals; start nothing.
//     let mut wifi = Wifi::new(p.WIFI, p.TIMG0, p.FROM_CPU_INTR0);
//
//     loop {
//         // ... draw a frame ...
//         wifi.poll(now_us);            // cheap while nothing is asked of it
//         if user_asked_for_wifi { wifi.want(true, now_us); }
//         if user_left_wifi_screen { wifi.want(false, now_us); }
//     }

/// How long a scan may take before it is abandoned. A station-only active
/// scan is about a quarter of a second (13 channels, up to 20 ms each).
const SCAN_LIMIT_US: u64 = 5_000_000;
/// After a radio that would not start, how long before trying again.
const RESTART_US: u64 = 30_000_000;

pub struct Wifi {
    peripheral: WIFI<'static>,
    /// The scheduler esp-radio needs, started once, the first time the radio
    /// is wanted -- never at boot.
    rtos: Option<(TIMG0<'static>, FROM_CPU_INTR0<'static>)>,
    radio: Option<WifiController<'static>>,
    wanted: bool,
    start_after_us: Option<u64>,
}

impl Wifi {
    pub fn new(peripheral: WIFI<'static>, timg0: TIMG0<'static>, intr: FROM_CPU_INTR0<'static>) -> Self {
        Wifi { peripheral, rtos: Some((timg0, intr)), radio: None, wanted: false, start_after_us: None }
    }

    /// The user's decision. Off drops the controller, which tears the radio
    /// down.
    pub fn want(&mut self, on: bool, _now_us: u64) {
        self.wanted = on;
    }

    /// Call every frame. With the radio off and not wanted this is two
    /// comparisons.
    pub fn poll(&mut self, now_us: u64) {
        match (self.wanted, self.radio.is_some()) {
            // Wait one frame after it is first wanted, so the screen saying
            // "starting WiFi" is on the glass before any radio work.
            (true, false) => match self.start_after_us {
                None => self.start_after_us = Some(now_us + 100_000),
                Some(t) if now_us >= t => self.start_radio(now_us),
                Some(_) => {}
            },
            (false, true) => {
                self.radio = None;
                self.start_after_us = None;
            }
            _ => {}
        }
        // With the radio up: drain the event subscriber (every frame, and
        // before anything that waits on an event), poll smoltcp once over the
        // radio's interfaces, serve one request. No async executor needed.
    }

    fn start_radio(&mut self, now_us: u64) {
        if let Some((timg0, intr)) = self.rtos.take() {
            let timg0 = esp_hal::timer::timg::TimerGroup::new(timg0);
            esp_rtos::start(timg0.timer0, intr);
        }
        // Fewer driver buffers than the defaults (10 static, 32 dynamic each
        // way): they come out of the small internal heap, and a setup page or
        // a sync is not a throughput workload.
        let config = ControllerConfig::default()
            .with_static_rx_buf_num(6)
            .with_dynamic_rx_buf_num(16)
            .with_dynamic_tx_buf_num(16);
        // SAFETY: the only live copy; the controller made from it is dropped
        // (radio torn down) before another is taken.
        let peripheral = unsafe { self.peripheral.clone_unchecked() };
        match WifiController::new(peripheral, config) {
            Ok(c) => {
                self.radio = Some(c);
                self.start_after_us = None;
            }
            // Carry on without WiFi; try again later, not every frame.
            Err(_) => self.start_after_us = Some(now_us + RESTART_US),
        }
    }
}

/// Poll a radio future to completion or a deadline, on the frame loop's own
/// thread, with no executor. `drain` runs between polls so the subscriber
/// never fills the event channel while you wait (rule 3).
pub fn block_on_for<F: Future>(fut: F, limit_us: u64, mut drain: impl FnMut()) -> Option<F::Output> {
    let mut f = pin!(fut);
    let mut cx = Context::from_waker(Waker::noop());
    let start = Instant::now();
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return Some(v);
        }
        drain();
        if start.elapsed().as_micros() >= limit_us {
            return None;
        }
    }
}

// A scan, bounded:
//
//     let found = block_on_for(ctrl.scan_async(&ScanConfig::default().with_max(24)),
//                              SCAN_LIMIT_US, || drain_events(&mut events));
//     match found {
//         Some(Ok(aps)) => { /* list them */ }
//         Some(Err(_)) | None => { /* keep the last list; the walker carries on */ }
//     }
//
// TLS (embedded-tls, TLS 1.3, ECDSA roots only): never hand the verifier the
// chain exactly as the server sent it. It verifies from the root down through
// every certificate sent, and Cloudflare sends the root's RSA cross-signed
// certificate on top, so every handshake fails. Cut the chain at the first
// certificate a trusted root issued, then verify that path against that root
// (Pokewalker: pw_pet::x509::anchor_for, and `Anchored` in
// pw-pet-firmware/src/portal.rs). Check a real chain with
// `openssl s_client -showcerts` before choosing roots.
