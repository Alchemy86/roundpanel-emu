//! The SPD2010 user command set.
//!
//! Every constant here is read out of the vendor datasheet -- Solomon Systech,
//! *L-WEA2010: 454x454 TDDI for IOT/Wearable*, Product Preview Rev 0.50, April
//! 2021, 73 pp -- and the page number in each comment is that document's own
//! printed page, so the next reader can check any line of this file against it.
//! Section 13, Table 13-1 "User Command Set Table" (pp.29-30) is the master
//! list; section 14.1 describes each one.
//!
//! The same opcodes appear, unchanged, in two independent implementations that
//! run against real silicon, which is how this file was cross-checked rather
//! than merely transcribed:
//!
//! * Espressif's `esp_lcd_spd2010` component, v2.0.0~1 (`esp_lcd_spd2010.c`),
//!   the vendor's own driver, which is what the datasheet link above is
//!   published alongside.
//! * ESPHome's `mipi_spi` model `WAVESHARE-ESP32-S3-TOUCH-LCD-1.46`
//!   (`esphome/components/mipi_spi/models/spd2010.py`, PR #19056), which
//!   targets the exact board this crate exists for.
//!
//! Where those and the datasheet disagree, the disagreement is written down at
//! the point it matters rather than silently resolved. There are three, and
//! all three are in [`crate::emu`] and [`crate::driver`]: the whole-frame
//! memory-write rule, the SPI clock mode, and the meaning of `MADCTL`'s flip
//! bits.
//!
//! # This is not the SH8601Z with different numbers
//!
//! The SH8601Z -- the AMOLED controller on the other round QSPI modules on
//! sale -- looks superficially alike. Both are QSPI, both frame a command as `cmd << 8` in a 24-bit
//! address phase, both use the DCS-ish `2Ah`/`2Bh`/`2Ch`/`3Ch` quartet. They
//! diverge everywhere it counts:
//!
//! | | SH8601Z | SPD2010 |
//! |---|---|---|
//! | read opcode | `03h` | `0Bh` (Figure 6-12, p.15) |
//! | column window | any `SC <= EC` | `SC` must be `4M`, `EC` must be `4N-1` (p.48) |
//! | pixel formats | 888/666/565/332/256-grey/111 | 888/666/565 only (p.57) |
//! | `MADCTL` | `MX` D6, `BGR` D3 | `BGR` D3, `SS` D1, `GS` D0 (p.53) |
//! | brightness | `DBV[9:0]`, low byte first | `DBV[13:6]` then `DBV[5:0]` (p.61) |
//! | write while asleep | forbidden (p.115) | permitted (p.50, p.58) |
//! | whole frame in one `2Ch` | fine | forbidden (p.50) |
//! | command paging | none | `FFh` selects a command set |
//!
//! Anything built on the assumption that a QSPI AMOLED controller is a QSPI
//! AMOLED controller gets at least the first two of those wrong, and the first
//! two are the ones that produce a picture rather than an error.

/// Commands this crate models.
///
/// Table 13-1 (pp.29-30) is the whole user command set; what is *absent* from
/// it is as informative as what is present. There is no `PTLON` (12h), no
/// `PTLAR` (30h), no `ALLPOFF`/`ALLPON` (22h/23h) and no deep standby -- all of
/// which the SH8601Z has. A partial-display mode does not exist on this part.
pub mod op {
    /// No operation. p.31
    pub const NOP: u8 = 0x00;
    /// Software reset. p.32
    pub const SWRESET: u8 = 0x01;
    /// Read ID: 3 read parameters, ID1..ID3. p.33
    pub const RDID: u8 = 0x04;
    /// Read display mode. p.34
    pub const RDISPMODE: u8 = 0x09;
    /// Read display power mode. p.35
    pub const RDDPM: u8 = 0x0A;
    /// Read display MADCTL. p.36
    pub const RDDMADCTL: u8 = 0x0B;
    /// Read colour format. p.37
    pub const RDPIX: u8 = 0x0C;
    /// Read display image mode. p.38
    pub const RDDIM: u8 = 0x0D;
    /// Read signal mode. p.39
    pub const RDDSM: u8 = 0x0E;
    /// Read self-diagnostic result. p.40
    pub const RDDSDR: u8 = 0x0F;
    /// Sleep in. p.41
    pub const SLPIN: u8 = 0x10;
    /// Sleep out. p.42
    pub const SLPOUT: u8 = 0x11;
    /// Normal display mode on. p.43
    pub const NORON: u8 = 0x13;
    /// Display inversion off. p.44
    pub const INVOFF: u8 = 0x20;
    /// Display inversion on. p.45
    pub const INVON: u8 = 0x21;
    /// Display off. p.46
    pub const DISPOFF: u8 = 0x28;
    /// Display on. p.47
    pub const DISPON: u8 = 0x29;
    /// Set column: 4 parameters, SC[15:0] then EC[15:0]. p.48
    ///
    /// The datasheet calls this `SETCOL`/`SELCOL`; every driver in the wild
    /// calls it CASET, and so does this crate, because that is the name a
    /// reader arrives with.
    pub const CASET: u8 = 0x2A;
    /// Set row: 4 parameters, SP[15:0] then EP[15:0]. p.49
    ///
    /// `SETPAGE`/`SELPAGE` in the datasheet, RASET in Espressif's driver.
    pub const RASET: u8 = 0x2B;
    /// Write memory start: variable-length pixel data. p.50
    pub const RAMWR: u8 = 0x2C;
    /// Tearing effect off. p.51
    pub const TEOFF: u8 = 0x34;
    /// Tearing effect on: 1 parameter, TELOM. p.52
    pub const TEON: u8 = 0x35;
    /// Data access control: 1 parameter, BGR / SS / GS. p.53
    pub const MADCTL: u8 = 0x36;
    /// Idle mode off. p.55
    pub const IDMOFF: u8 = 0x38;
    /// Idle mode on. p.56
    pub const IDMON: u8 = 0x39;
    /// Set colour format: 1 parameter, DBI[2:0]. p.57
    pub const COLMOD: u8 = 0x3A;
    /// Write memory continue: variable-length pixel data. p.58
    pub const RAMWRC: u8 = 0x3C;
    /// Set tear scanline: 2 parameters, STS[15:0]. p.59
    pub const TESCAN: u8 = 0x44;
    /// Get scanline: 2 read parameters, GTS[15:0]. p.60
    pub const RDSCAN: u8 = 0x45;
    /// Write display brightness: 2 parameters, DBV[13:6] then DBV[5:0]. p.61
    pub const WRDISBV: u8 = 0x51;
    /// Set display brightness mode: 1 parameter, BCTRL / DD / BL. p.62
    pub const WRCTRLD: u8 = 0x53;
    /// Read display brightness mode. p.63
    pub const RDCTRLD: u8 = 0x54;
    /// Set CABC control: 1 parameter, PS[2:0]. p.64
    pub const WRCABC: u8 = 0x55;
    /// Read CABC control. p.65
    pub const RDCABC: u8 = 0x56;
    /// Set CABC minimum brightness: 2 parameters, CMB. p.66
    pub const WRCABCMB: u8 = 0x5E;
    /// Read CABC minimum brightness. p.67
    pub const RDCABCMB: u8 = 0x5F;
    /// Read DDB start. p.68
    pub const RDDDBST: u8 = 0xA1;
    /// Read DDB continue. p.69
    pub const RDDDBCON: u8 = 0xA8;

    /// Command-set select: 3 parameters.
    ///
    /// This one is **not** in Table 13-1, because it is not a user command --
    /// it is what selects which command set the user commands above belong to.
    /// Both real drivers open with it and neither documents it beyond the
    /// bytes, so it is recorded here as they send it:
    ///
    /// ```text
    /// esp_lcd_spd2010.c:
    ///     #define SPD2010_CMD_SET       (0xFF)
    ///     #define SPD2010_CMD_SET_BYTE0 (0x20)
    ///     #define SPD2010_CMD_SET_BYTE1 (0x10)
    ///     #define SPD2010_CMD_SET_USER  (0x00)
    ///     tx_param(.., SPD2010_CMD_SET, {BYTE0, BYTE1, USER}, 3)
    ///
    /// esphome/.../models/spd2010.py:
    ///     (0xFF, 0x20, 0x10, 0x10),   # ... a page of vendor registers ...
    ///     (0xFF, 0x20, 0x10, 0x00),   # back to the user set
    /// ```
    ///
    /// The vendor pages (`10h`, `11h`, `12h`, `18h`, `2Dh` and more) carry
    /// gamma, power and GIP timing registers that are panel-specific and
    /// undocumented; both drivers write a long table of them verbatim and say
    /// so in a comment. This crate models the *select* faithfully -- which set
    /// is live, and that a user command means nothing while a vendor page is
    /// -- and models none of the vendor registers themselves, because there is
    /// no public description of what they do.
    pub const CMD_SET: u8 = 0xFF;
}

/// `FFh` command-set select parameters, as both real drivers send them.
pub mod cmdset {
    /// First two parameter bytes, fixed in both drivers.
    pub const MAGIC: [u8; 2] = [0x20, 0x10];
    /// Third byte: the user command set, i.e. Table 13-1.
    pub const USER: u8 = 0x00;
}

/// `MADCTL` (36h) bit assignments, p.53.
///
/// Note what is *not* here. This chip's `MADCTL` defines D3, D1 and D0 only;
/// D7:D4 and D2 are printed "Revered" [sic] with a value of 0. So there is no
/// row-column exchange bit and the SPD2010 cannot transpose an image -- the
/// same conclusion the SH8601Z invites, and Espressif's driver states it
/// outright for this part:
///
/// ```text
/// esp_lcd_spd2010.c, panel_spd2010_swap_xy():
///     ESP_LOGE(TAG, "swap_xy is not supported by this panel");
///     return ESP_ERR_NOT_SUPPORTED;
/// ```
///
/// A frame that needs turning must be turned by the host before it is sent, or
/// by how the module is physically mounted.
pub mod madctl {
    /// D3, BGR: '0' = RGB, '1' = BGR.
    pub const BGR: u8 = 0x08;
    /// D1, SS: "Flip Horizontal". '0' = normal, '1' = flipped horizontally.
    ///
    /// Espressif's driver maps `mirror_x` onto this bit (`madctl_val |=
    /// BIT(1)`), and ESPHome's model declares `use_axis_flips=True` with
    /// `{CONF_MIRROR_X, CONF_MIRROR_Y}` for the same pair. See
    /// [`crate::emu`] for what "flip" is taken to mean here and why that is
    /// not the same reading the SH8601Z's `MX` invites.
    pub const SS: u8 = 0x02;
    /// D0, GS: "Flip Vertical". '0' = normal, '1' = flipped vertically.
    ///
    /// Espressif's `mirror_y` (`madctl_val |= BIT(0)`).
    pub const GS: u8 = 0x01;

    /// Power-on and S/W reset default, p.53.
    pub const RESET_DEFAULT: u8 = 0x00;
}

/// `COLMOD` (3Ah) parameter, p.57.
///
/// The byte is drawn as `0 1 1 1 0 DBI[2:0]`, so the high five bits are fixed
/// at `01110b` and only `DBI[2:0]` varies. Three values are defined and there
/// are no others:
///
/// ```text
/// DBI[2:0] = 7: 16.7M color
/// DBI[2:0] = 6: 262k color
/// DBI[2:0] = 5: 65k color
/// ```
///
/// This is the single biggest practical difference from the SH8601Z, which
/// offers 3-3-2, 1-1-1 and a 256-grey mode on top of these. An application
/// drawing in greys gets them one byte each, exactly, on an SH8601Z;
/// **this chip cannot**. On an SPD2010 a grey frame goes out as colour, two or
/// three bytes a pixel, and 65k colour does not carry an 8-bit grey without
/// loss. [`crate::pixel::PixelFormat`] is where that lands.
pub mod colmod {
    /// The fixed high bits every legal parameter carries: `01110b`.
    pub const FIXED_HIGH: u8 = 0x70;
    /// The `DBI[2:0]` field mask.
    pub const DBI_MASK: u8 = 0x07;

    /// 24 bit/pixel, 16.7M colour. DBI = 7.
    pub const RGB888: u8 = 0x77;
    /// 18 bit/pixel, 262K colour. DBI = 6.
    pub const RGB666: u8 = 0x76;
    /// 16 bit/pixel, 65K colour. DBI = 5.
    pub const RGB565: u8 = 0x75;
    /// Power-on and S/W reset default, p.57.
    pub const RESET_DEFAULT: u8 = RGB888;
}

/// `WRCTRLD` (53h) bit assignments, p.62.
pub mod wrctrld {
    /// D5, BCTRL: brightness control block on/off. With it off "the setting on
    /// DBV(51h) will be ignored".
    pub const BCTRL: u8 = 0x20;
    /// D3, DD: display dimming, manual brightness only.
    pub const DD: u8 = 0x08;
    /// D2, BL: backlight control on/off.
    pub const BL: u8 = 0x04;
    /// Power-on default, p.62.
    pub const RESET_DEFAULT: u8 = 0x00;
}
