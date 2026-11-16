# The battery: check the polarity first

The board runs from a single LiPo cell on its **BAT** socket, a 2-pin 1.25 mm
(MX1.25 / PicoBlade) connector on the back. We used The Pi Hut's
[150 mAh 3.7 V LiPo with the 1.25 mm PicoBlade connector](https://thepihut.com/products/150mah-3-7v-lipo-battery-1-25mm-picoblade-connector)
(26 × 21 × 4 mm, built-in protection), which is small enough to sit behind the
board in a round case.

## Its leads came wired the other way round

Out of the bag, that cell's plug is wired **opposite to the board's BAT
socket**. Plugged in as it comes, nothing powers from it: the cell's own
protection blocks the reversed connection, so the board only ever runs from
USB. Pull the USB-C cable and it switches off, and holding PWR on battery does
nothing at all.

Swap the two wires in the plug (lift each latch with a needle, swap, push back
in) so the **red lead lands on the `+` marked beside the socket**, or use a
reversing lead. After that the board runs from the cell straight away. The
Pi Hut's own page says it too: battery port polarity is not the same on every
device, so check before you plug in.

## Things that look like proof and are not

We lost an afternoon to these, so here they are:

- **The lights.** The red and green LEDs stay lit on USB with **no cell
  attached at all**. They say nothing about the battery.
- **The battery voltage on USB.** `BAT_ADC` reads the charger's output while
  USB is in, cell or no cell: about 4.18 V whether the cell is connected,
  reversed or missing. Unplugging the cell on USB does not move the reading.
  A reading on USB is not evidence the cell is connected.
- **Waveshare's own demo switching off when you unplug.** Their demo
  (`PWR_Key.c`) only closes the battery path if the **PWR key is held while it
  boots**. Started on USB without the key, it leaves the path open and dies
  on unplug even with a perfect cell. To test the cell with it: USB out, hold
  PWR for about three seconds, let go.

The only honest test is: cell in, USB out, hold PWR. If the board comes on,
the cell works.

## Keeping it on

- **`BAT_Control` (GPIO7)** gates the MOSFET (`Q3`) between the cell and the
  regulator. Out of reset it is not driven and the path is open. Firmware must
  drive it **high** as early as it can, or the board dies the moment USB is
  pulled. Driving it low again is a deliberate power-off.
- **`Key_BAT` (GPIO6)** is the PWR key. Holding it lets the cell power the
  board long enough to boot, which is how it starts on battery alone.
- **`BAT_ADC` (GPIO8, ADC1 channel 7)** reads the cell through a divider:
  cell mV = pin mV × 3 / 0.990476 (Waveshare's `BAT_Driver.c`; a 1382 mV pin
  reading is 4.18 V). Read it at 12 dB attenuation, average a few samples.
- A LiPo reads ≥ 4.20 V only on charge, so treat that as "on external power",
  and below about 2.6 V as "no cell". In between, a standard LiPo open-circuit
  curve (4.19 V full to 3.10 V empty) gives a usable percentage.

The emulator has no battery, so none of this shows up on the desktop. Write
your power-on path for the board, and test it with the cable out.
