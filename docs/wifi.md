# WiFi on this board: what froze it, and the rules that came out of it

The emulator cannot run WiFi. `esp-radio` drives the ESP32-S3's own radio
through a closed binary blob, so there is nothing to emulate on a desktop. This
page is what [Pokewalker](https://github.com/Alchemy86/pokewalker) learned
putting WiFi on this board with `esp-hal` and `esp-radio` (no ESP-IDF), and
[`board/wifi.rs`](../board/wifi.rs) is the reference code. It is not built by
this repo's CI, and says so.

What it does: the user holds a button, the board opens a hotspot, a phone joins
it, a captive-portal page lists the networks the board scanned, the user picks
one and types the password, and the board joins it. After that it can sync with
a web service over TLS. That was set up on the real board and works
(Pokewalker's `aaa4c3fa`, "WiFi setup from a phone, triggered only, verified on
the board").

## What went wrong

**A boot-time radio froze the device.** The first build brought the radio up
during boot. The board stopped before its first frame. Bringing a radio up
starts a scheduler, allocates driver buffers and runs the blob's own start-up,
and any of it stalling takes the boot down with it.

**PSRAM first starved the radio.** The module has 8 MB of octal PSRAM, and
putting the heap there seemed the obvious way to make room. But `esp-alloc`
hands each allocation to the first region with room, and that includes the C
`malloc` inside the WiFi blob, which needs internal RAM. With PSRAM registered
first the board printed `wifi up` and the radio stalled; an unbounded wait on
the network scan then held the whole device with it.

**The setup page listed no networks.** `esp-radio`'s event channel holds
**two** events by default, and the driver publishes with `try_publish`: when it
is full, a new event is dropped. The events from bringing the radio up filled
it, and the `ScanDone` that the scan was waiting for was thrown away. Fixed by
raising the capacity to 16 with 4 subscribers, and draining the firmware's own
subscriber before and during every wait.

**The first full-size packet to a phone panicked.** A 1514-byte link MTU
overran `esp-radio`'s transmit buffer. 1492 works.

**The radio's RAM came out of the stack.** The WiFi driver keeps about 34 KB of
code in IRAM, which on the S3 is the same SRAM the stack comes from. Left alone
it took the stack from about 104 KB to 45 KB, and 46 KB had already been
overflowed once by the busiest screen. The fix was a 72 KB internal heap and
everything big (pictures, the setup page's buffers) moved to PSRAM on purpose;
the stack ended at about 92 KB.

## The rules

1. **WiFi is something the user starts, never something boot does.** Boot keeps
   the peripherals and the saved network, nothing else. The scheduler and the
   radio start inside the frame loop, one frame after the user asks (so the
   screen saying so is already showing), and are torn down when they leave.
   Pokewalker's build script fails the build if radio start-up code appears in
   `main` before the loop.
2. **Register the internal heap before PSRAM.** Then put large, non-DMA
   allocations in PSRAM explicitly.
3. **Size the event channel and drain it.** In `.cargo/config.toml`:
   `ESP_RADIO_CONFIG_EVENT_CHANNEL_CAPACITY = "16"` and
   `ESP_RADIO_CONFIG_EVENT_CHANNEL_SUBSCRIBERS = "4"`. Read every event off your
   subscriber each frame and inside every wait.
4. **Every wait on the radio has a deadline.** A scan gets 5 s (it normally
   takes about a quarter of a second). A radio that will not start is retried
   30 s later, not every frame, and the device carries on without it.
5. **Use fewer driver buffers than the defaults** (6 static and 16 dynamic
   receive, 16 transmit, against 10 and 32): they come out of the small
   internal heap, and a setup page is not a throughput workload.
6. **No executor needed.** Poll `smoltcp` once a frame over the radio's raw
   packet interfaces, and poll the odd radio future with a no-op waker and a
   deadline (`block_on_for` in `board/wifi.rs`).
7. **Prove the loop is alive.** Print a line on the loop's first frame and every
   ten seconds after, with the free internal and PSRAM heap. When a WiFi change
   hangs the board, that line's absence says where.

## TLS

Pokewalker talks to its web service over TLS 1.3 with `embedded-tls`, trusting
two ECDSA roots (GTS Root R4 and ISRG Root X2). One trap: **never hand the
verifier the chain exactly as the server sent it.** It verifies from the root
down through every certificate sent, and Cloudflare sends the root's RSA
cross-signed certificate on top, so every handshake fails. Cut the chain at the
first certificate a trusted root issued, then verify that path against that
root. Check a real server's chain with `openssl s_client -showcerts` before
picking roots. (That code was built and host-tested, and was not yet confirmed
on the board when this page was written.)

## Staying online without draining the battery

Once WiFi is up, Pokewalker can stay on the network and sync by itself, and
the policy is worth copying:

- **Debounce.** A change worth sending goes out about 25 s after things go
  quiet, at most 2 minutes after the first change, never sooner than 2 minutes
  after the last sync began. Counters that change constantly (steps) are not a
  reason to sync on their own; they ride along.
- **Heartbeat.** Every 10 minutes, to collect anything waiting on the server.
- **Back off.** A failure waits a minute, doubling to half an hour.
- **Doze.** After five minutes untouched, or with the screen asleep, the radio
  goes off and each sync brings it up for its own length only (every 5 minutes
  at the soonest, a heartbeat every 15). A touch or a key wakes it.
- **Never mid-animation.** Nothing starts while something time-critical is on
  the glass.

## Checking it

There is nothing to run here. On the board: flash, trigger WiFi, and read the
serial log for the loop-alive line, the scan's network count and the heap
figures. Pokewalker's desktop window has a model radio and serves the real
setup page on `127.0.0.1`, which is how the page and the decision logic were
built without the board; the radio itself can only be tested on hardware.

## Where to read more

Pokewalker: `software/README.md`, "WiFi, set up with a phone" and "Staying
online, and edits from the site"; `software/pw-pet-firmware/src/wifi.rs` (the
real radio), `software/pw-pet/src/wifi.rs` (every decision, host-tested),
`software/pw-pet/src/portal.rs` (DNS, DHCP and HTTP for the setup hotspot),
`software/pw-pet/src/x509.rs` and `software/pw-pet-firmware/src/portal.rs`
(TLS).
