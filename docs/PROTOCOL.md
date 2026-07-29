# Supvan T-series printer protocol

Reverse-engineered notes on the wire protocol the Supvan T50 / T50M Pro
label printers speak. Two transports — Bluetooth RFCOMM (SPP) and USB HID
— share the same command codes but use **different framings and
different response sizes per command**. That asymmetry is the single
biggest gotcha when working with this hardware; this document is the
crib sheet for it.

Sources of ground truth:

- The vendor Android app (Katasymbol v1.4.20, decompiled with jadx) and the
  vendor's Electron desktop app, both reverse-engineered during initial
  bring-up. Key Android sources: `BasePrint.java` (framing/transport/status),
  `T50PlusPrint.java` (print flow + material), `BluetoothUtils.java` /
  `BLEUtils.java` (transports), `LzmaUtils.java` (compression params),
  `{MSTA,FSTA,PAGE}_REG_BITS.java` (status/page bits). Where a field comment
  says "from Electron app" or "byteToString(A,11,21)" it's a direct port of
  that source.
- `supvan-proto/src/{cmd,status,bt_transport,usb_transport}.rs` — the
  Rust implementation. Always more authoritative than this document.
- Live captures from the working katasymbol deployment (the BT side
  was self-validated by ipptool round-trips; the USB side was decoded
  from `cargo test` fixtures and `cargo run` traces).

## Transports

|                       | BT (RFCOMM, SPP)                                | USB HID                                              |
|-----------------------|-------------------------------------------------|------------------------------------------------------|
| Underlying channel    | Bluetooth Classic SPP, RFCOMM channel auto-detected via `RfcommSocket::connect_default` | Hidraw (`/dev/hidrawN`) on a Supvan VID `0x1820` device |
| Framing magic         | `7E 5A` header                                  | `C0 40` header                                       |
| Command size sent     | 16 bytes (always)                               | 8 bytes (most cmds), 10 bytes for two-param cmds      |
| Param byte order      | Little-endian at frame[12..14]                  | Big-endian at frame[2..4] (NB the swap)              |
| Response size         | Variable, status-frame-prefixed                  | **Fixed 8 bytes** for most commands; **64 bytes** for `RETURN_MAT` |
| Carries device name in `RD_DEV_NAME`? | yes, ASCII at frame[22..]              | **no** — the 8-byte response has no string slot     |
| Carries device serial in `RETURN_MAT`? | yes, BCD bytes at frame[51..57]       | **yes**, ASCII null-terminated at offset 40         |

Because the USB response is fixed-size for everything except
`RETURN_MAT`, several BT-only data items (firmware version, device name,
extended version) are stubbed to `None` on the USB transport in
`usb_transport.rs::parse_*_response`. That isn't a bug; the firmware
genuinely can't encode them in 8 bytes.

## BT frame format

### Command frame (16 bytes, sent to printer)

```
[0]  7E             magic1
[1]  5A             magic2
[2]  0C             payload-length low (= 12)
[3]  00             payload-length high
[4]  10             PROTO_ID
[5]  01             PROTO_VER
[6]  AA             marker
[7]  CMD            command byte (see table below)
[8]  chk_lo         checksum low  ⎫
[9]  chk_hi         checksum high ⎭ = LE sum of bytes [10..16]
[10] 00             reserved
[11] 01             DATA_TYPE (?)
[12] param_lo       parameter, little-endian
[13] param_hi
[14] block_lo       block_count for start-trans, else 0
[15] block_hi
```

Built by `cmd::make_cmd(cmd, param)` and `cmd::make_cmd_start_trans(cmd, block_size, block_count)`.

### Response frame (variable size)

```
[0]  7E             magic1
[1]  5A             magic2
[2]  len_lo         payload-length low
[3]  len_hi         payload-length high
[4]  10             PROTO_ID
[5]  03             reply marker (vs 01 on request)
[6]  55             reply marker
[7]  CMD            command byte being acknowledged
[8..9]              checksum
[10..21]            command-specific metadata; for status responses
                     this is the registered state described below
[22..]              command-specific payload (string / material / …)
```

The first 8 bytes are a "header" — magic + reply markers + echoed command.
Validation happens in `status::validate_response(data, expected_cmd)`.

## USB HID frame format

### Command frame (8 or 10 bytes; written as a 64-byte HID report)

```
[0]  C0             USB_MAGIC1
[1]  40             USB_MAGIC2
[2]  param_hi       parameter, **big-endian** (opposite of BT)
[3]  param_lo
[4]  CMD            command byte
[5]  00             reserved
[6]  08             reserved (looks like a length hint, always 0x08)
[7]  00             reserved
[8]  param2_hi      (only the 10-byte `send_cmd_two` form)
[9]  param2_lo
```

Built by `usb_transport::UsbHidTransport::make_usb_cmd(cmd, param)` and
`make_usb_cmd_two(cmd, param1, param2)`. The HID write transparently
right-pads to 64 bytes (`HID_REPORT_SIZE`).

### Status response frame (8 bytes)

This is the only USB response size for everything *except*
`RETURN_MAT`. It carries no string payload — there's nowhere to put one.

```
[0]  echo / length indicator (varies by command; not the command byte)
[1]  MSTA low   — same bits as BT byte 14
[2]  MSTA high  — same bits as BT byte 15
[3]  FSTA low   — same bits as BT byte 16
[4]  FSTA high  — same bits as BT byte 17
[5]  print count low
[6]  print count high
[7]  reserved
```

Parsed by `usb_transport::UsbHidTransport::parse_usb_status(resp)`.
The bit assignments are the **same** as BT (see PrinterStatus table
below); the frame just packs them at different offsets.

### Material response frame (64 bytes)

Returned by `RETURN_MAT (0x30)` only. The HID descriptor evidently
declares a second, larger feature/output report just for material data.

```
[0]        length / type indicator
[1..8]     status bytes (same shape as INQUIRY_STA response)
[19]       width_mm   (label width, integer millimetres)
[20]       height_mm  (label height)
[21]       gap_mm     (inter-label gap)
[22]       label_type (see vendor docs; not enumerated here)
[31..32]   SN low/high (u16 LE) — vendor "label SN" counter; NOT the
            device serial.
[40..]     device serial as ASCII, null-terminated.
            For the T50M Pro reference unit this is "T0117A2410211517",
            the same string the firmware broadcasts as the BlueZ Name.
```

Parsed by `usb_transport::UsbHidTransport::parse_usb_material(resp)`.

The note from the original Electron app — `byteToString(A,11,21)` —
hinted at a device serial at offsets 11..21 (10 bytes BCD); we don't
currently extract that path because the offset-40 ASCII string is
sufficient and self-validating. **TODO**: cross-check whether the
BCD bytes at 11..21 match the ASCII at 40+. If they do it's a
redundant encoding; if not, one of them is the *device* serial and
the other is the *label* serial.

## Command reference

The leading-zero byte is `cmd::CMD_*` in `cmd.rs`. Italicised entries are
output-only (no parsed response).

| Code | Name                | Direction       | BT response shape         | USB response shape           | Parser                             |
|------|---------------------|-----------------|---------------------------|------------------------------|------------------------------------|
| 0x10 | BUF_FULL            | host → device   | *control / flow only*     | *control / flow only*        | —                                  |
| 0x11 | INQUIRY_STA         | host ↔ device   | 20-byte status frame      | 8-byte status frame          | `parse_status` / `parse_usb_status` |
| 0x12 | CHECK_DEVICE        | host ↔ device   | 8-byte ack (non-empty)    | 8-byte ack (non-empty)       | `validate_response`                |
| 0x13 | START_PRINT         | host → device   | 8-byte ack                | 8-byte ack                   | —                                  |
| 0x14 | STOP_PRINT          | host → device   | 8-byte ack                | 8-byte ack                   | —                                  |
| 0x16 | RD_DEV_NAME         | host ↔ device   | ≥22-byte frame + ASCII    | **stub** (8-byte; no string) | `parse_device_name`; USB returns None |
| 0x17 | READ_REV            | host ↔ device   | ≥25-byte frame + ASCII    | **stub** (8-byte)            | `parse_version`                    |
| 0x2E | PAPER_SKIP          | host → device   | 8-byte ack                | 8-byte ack                   | —                                  |
| 0x30 | RETURN_MAT          | host ↔ device   | ≥57-byte material frame   | **64-byte** material frame   | `parse_material` / `parse_usb_material` |
| 0x5C | NEXT_ZIPPEDBULK     | host → device   | uses `make_cmd_start_trans`; signals next block of zipped raster | same | — |
| 0x5D | SET_RFID_DATA       | host → device   | 8-byte ack, then bulk payload | 8-byte ack, then bulk payload | `rfid::RfidMaterial::encode`; see below |
| 0xC5 | READ_FWVER          | host ↔ device   | ≥23-byte frame; firmware byte at [22] | **stub** (8-byte)  | `parse_firmware_version`           |
| 0xC6 | UPDATE_FW           | host → device   | firmware-transfer start (`0xAA 0xC7` packets follow) | same | `build_firmware_frames`; see docs/FIRMWARE.md |

### Extended opcodes (from the vendor Linux tool)

The vendor Linux editor (`com.supvan.supvaneditor` 1.1.4, Electron) ships an
un-minified **source map** that confirms the vocabulary above and adds the codes
below. We keep them as `cmd::CMD_*` constants but do not drive them yet —
response parsing needs on-device verification (the tool's byte offsets are for
its own USB/serial framing, not our 22-byte-header BT frames).

| Code | Name | Notes |
|------|------|-------|
| 0x19 | CHECK_RIB   | check ribbon; defined but no active call site |
| 0x22 | RD_LAB_DPI  | read label DPI (response = DPI×100 as LE u16); G/TP/MP50 |
| 0x24 / 0x25 | RD_LAB_DPI_24/25 | per-material DPI read variants (sp plugin) |
| 0x33 | SET_PRTMODE | set print mode; MP50/P70 only |
| 0x35 | SEND_INF    | set print density; MP50/P70 only |
| 0xF0 | TRANSFER    | "传输字模" (dot-pattern transfer); **reserved** — defined but never sent (the live bitmap path is `NEXT_ZIPPEDBULK` 0x5C). `字模` here is the raster dot-pattern, not typographic fonts. |

`0x11` doubles as `INQUIRY_STA` and a `FINISH_PRINT` marker; `0x14` doubles as
`STOP_PRINT` / `RESET_PRINT`.

### SET_RFID_DATA (0x5D) — synthetic material records

Genuine consumables carry an RFID tag the printer reads via `RETURN_MAT`. Stock
whose tag the printer can't read — third-party, or another brand's — returns an
all-zero material record and raises `label_rw_error`. The vendor's answer is to
synthesise the record host-side and push it over 0x5D, gated on exactly that
all-zero UUID (`MatCtrlFunc.sendRfid()`, for T50/T50S/T50Plus/T50Pro/T80Pro):

    if (UUID.indexOf("00000000") == -1)  isRFIDLabel = false   // real tag, leave it
    else                                 isRFIDLabel = true    // blank, send one

Two steps on the wire: announce the payload length under 0x5D, then bulk-write
the record. The vendor's `CMD_SET_RFID_DATA_WRITE: 999` is an internal step
marker, not a second opcode. `printer::Printer::set_rfid_data` implements this;
`supvan-cli provision` drives it.

**Record layout** (80 bytes; every field but the UUID sits at base offset 16,
matching the vendor's `databuf[N + offset]`):

| Offset | Size | Field |
|-------:|-----:|-------|
| 0  | 7  | UUID — decimal catalogue code, right-padded with `'0'` to 14 hex chars |
| 16 | 8  | MatCode (left zero for synthetic records) |
| 24 | 2  | MatSn, LE u16 |
| 26 | 1  | MatType (`PaperTypeEnum`) |
| 27 | 1  | width across the printhead, mm (≤50) |
| 28 | 1  | length along the feed, mm (≤120) |
| 29 | 1  | CustomID (0 = Supvan, 1 = 贴博士) |
| 30 | 1  | GapDistance, mm |
| 31 | 1  | TailLength, mm |
| 32 | 4  | RemainUsage (labels), LE u32 |
| 36 | 2  | **HeatTime5**, LE u16 |
| 38 | 2  | **HeatTime40**, LE u16 |
| 40 | 6  | TimeStamp (zero) |
| 48 | 16 | Cipertext (zero) |
| 64 | 2  | MinOptThreshold, LE u16 |
| 66 | 2  | MaxOptThreshold, LE u16 |
| 68 | 1  | OptIndex |
| 69 | 1  | HeatIndex |

The anti-counterfeit fields are zero because the vendor ships them that way:
`GeneralCipertext1..5()` are commented out and `Cipertext` stays `null`, which
its `deepCopy()` turns into `{}` — so `Cipertext[i]` is `undefined` and lands in
the `Uint8Array` as 0. The `Password` that `generalPassword()` computes is never
serialised. Reimplementing that crypto would be dead work.

**HeatTime5 / HeatTime40** are printhead pulse widths at 5 °C and 40 °C ambient,
which the firmware interpolates between — the only *absolute* energy control in
the protocol. Vendor presets, selected by catalogue code:

| Profile | HeatTime5 | HeatTime40 |
|---------|----------:|-----------:|
| standard label stock          | 1700 | 1200 |
| index / transparent black-mark | 1800 | 1300 |
| black-mark cardstock          | 2500 | 2000 |

### Energy control and its limits

Two knobs, in series: the material record's heat times set absolute pulse width,
and the per-buffer density (0–15) trims it.

Density is **two independent trims**, not one — `Density { black, red }`. The
Android app keeps them as `mDeepness` / `mRedDeepness` and packs them for
transport as `(black << 8) | red`, unpacking whenever the value exceeds 255
(`T50PlusPrint.java:107-112`). They land in different header fields:

| trim | header field |
|------|--------------|
| black | PAGE_REG_BITS `nodu` (byte 1, bits 2-5) |
| red   | `buf[12]` |

Because density lives in the buffer header and buffers tile the label along the
**feed** axis, energy can differ from one printhead line to the next, down to a
single-column buffer (0.125 mm at 8 dots/mm). Within a line it can differ only by
*colour*, via the two-colour mode below.
`buffer::split_into_banded_buffers` exposes the feed-axis case as a list of
`DensityBand`s.

`PaperTypeEnum` has 16 entries and none is two-colour, but that does not mean the
firmware has no colour concept — it has a dedicated mode, described next.

### Two-colour mode

Two interleaved bitplanes per printhead line, each burned at its own trim, giving
**per-dot** colour selection. Recovered from
`ImgConverter.GetBytes(int, int, int, List<Color>)` and the `i2 == 3` branch of
`T50PlusPrint`; implemented in `twocolor.rs` and selected by
`buffer::ColourMode::TwoColour`.

Wire differences from mono:

| aspect | mono | two-colour |
|--------|------|------------|
| PAGE_REG_BITS `first_cut` | 0 | **2** |
| header column count | printed columns | **printed columns × 2** |
| bytes per column | `per_line_byte` | `per_line_byte × 2` |
| max columns per buffer | `MAX_BUF_DATA / bpl` | **halved** |

Each column ships its red line immediately followed by its black line. Pixels are
sorted by luma `0.3 R + 0.59 G + 0.11 B`:

| luma | destination |
|------|-------------|
| ≥ 125 | no ink |
| 48 … 125 | plane 0 (red) |
| ≤ 48 | plane 1 (black) |

The `48` cut is placed so pure red — luma `255 × 0.3 = 76` — lands on the red
side. It is a **luminance cut, not a hue test**: any mid-grey in the 48–125 window
becomes red, so quantise to two ink colours before encoding rather than feeding an
arbitrary image.

**Availability is gated, and not every unit qualifies.**
`DeviceManager.isTwoColorDevice()` returns 2 for T80 Pro / T50 Plus, and 3 for a
T50 Pro *only* when its Bluetooth name contains neither `A` nor `B`. The shipped
app never enables the path regardless: `PrintPageData.colors` is read but never
populated. `MaterialManager.isTwoColorConsumable()` additionally requires a
catalogue code in `{5602, 5618-5621, 5686-5692}` — settable via
`supvan-cli provision --code`.

**Measured on a T50M Pro (`T0117A2410211517`, USB HID): the mode is accepted but
ignored.** The buffers are taken without error and the print completes, but a card
of vertical red/black stripes — both colours on every printhead line, which only
genuine two-plane support can produce — came out uniformly one colour. The
interleave appears to be consumed as plain mono columns, rendering each stripe at
half vertical fill (hence unusually light output). This matches
`isTwoColorDevice()` excluding this serial. The encoder is retained for the
hardware the vendor does list as capable.

**On such a unit, colour is per-label, not per-area.** Heat time is the only
control that moves it; the 0-15 density trim showed no discernible effect across
its full range at the transition heats. Measured on Niimbot two-colour stock:

| heat5:heat40 | result |
|--------------|--------|
| 300:150 | faint red |
| **500:300** | **clean red** |
| 700:450 | red, black appearing at band leading edges |
| 800:500 | red + ~50% black |
| **1700:1200** | **solid black** |

Set the pair with `supvan-cli provision --heat`, or walk a range with
`supvan-cli heat-sweep`. Darker bands at the *leading edge* of each printed run
are a printhead history-compensation artefact, not a density effect.

**No host-commanded backfeed.** `PAPER_SKIP` (0x2E) feeds forward only and the
Linux editor never sends it. The editor's 标签对齐 ("label align") button is
canvas centring (`objCenterClick` sets `scale = 1`), and `keepOnPrintPosition` is
a host-side resume index for a paused queue, not paper motion.

`Savepaper` (PAGE_REG_BITS byte 0, bit 7 — 省纸) is defined and plumbed through
`mSavePaper` in the Android app, but never actually set by either vendor tool.
**Tested and inert**: two consecutive prints with the bit set produced two
normally-advanced labels, indistinguishable from the baseline, with no error.
Exposed as `PageOptions::save_paper` and `supvan-cli test-print --save-paper` so
the negative result can be re-checked on other models. The 2-bit `Cut` field is
likewise never set by the vendor and remains untested.

So a true two-pass over one label has no path: no backfeed opcode, and no way to
suppress the advance.

### Grayscale: there is none

The raster is strictly 1bpp. Two-colour doubles the *planes*, not the bit depth,
and there is no 4bpp analogue anywhere in the format. Every intermediate tone is a
host-side halftone.

The vendor does this in `BitmapUtil` — Floyd–Steinberg, Atkinson, Bayer, Stucki,
and `convertTo16GrayWithDithering`, which quantises to 16 levels
(`GRAY_16_LUT = i * 17`) and then relies on `ImgConverter`'s threshold of 125 to
get back to 1bpp — with a `ditherSwitch` in its print-setup UI.

We halftone in `dither::Ditherer`, selected by `SUPVAN_DITHER` for the IPP app or
`--dither` on `supvan-cli gray-ramp`:

| mode | notes |
|------|-------|
| `bayer` | 4×4 ordered, stateless. **Default** — `SRGB_TO_LINEAR` compensates this hardware's dot spread |
| `floyd-steinberg` | 7/16, 3/16, 5/16, 1/16 |
| `atkinson` | spreads 6/8, higher local contrast |
| `vendor16` | the vendor pipeline reproduced: 16-level quantise + their kernel + threshold 125 |

`vendor16` deliberately skips `SRGB_TO_LINEAR` — the vendor applies no thermal
compensation, and adding ours would stop it being their pipeline. Their weights
(0.3125, 0.1875, 0.375, 0.0625) sum to 15/16 rather than 1, dropping a little
error per pixel; reproduced as-is.

### Opcode coverage and probe results

`cmd.rs` carried 20 opcodes; the Android `BasePrint.java` declares 54, and
`AdjustManager` a further 5 in what looked like empty space. An earlier reading
of the Electron editor alone concluded the vocabulary was complete — it is not,
and that conclusion was wrong. All are now present in `cmd.rs`.

**Almost none have a call site.** The app declares and never sends
`PAPER_BACK`, `HTIME_RD`/`HTIME_SET`, every `YINWEI` setter, `SET_OPTLEVEL`,
`MAT_AUTHEN_RESULT`, `READ_RANDOM`/`VERIFY_RANDOM`, `SET_TIMESTAMP`,
`FORCEUPDATE`, `SET_DENSITY`, `SET_HEADRATE`. Exercised ones are
`0x39 SET_TB_YINWEI` (G-series, `sendCmd(cmd, value, [0u8; 64])`),
`0xBC CHECK_OPTLEVEL` (**`T50PlusPrint`**, `sendCmdStartTrans(cmd, i, i2, [0u8; 64])`),
`0x18 STRD_MAT`, and `0x67`/`0x68 RD`/`WR_DEV_OPT`.

**Unallocated numbers: 171** inside `0x10`–`0xF0`, the largest holes being
`0x6B-0xAF` (69), `0xDA-0xEF` (22), `0x45-0x57` (19). AdjustManager filling
`0x60-0x62`/`0x6F`/`0xB2` is the standing warning that these are *unknown*, not
unused.

**Probing.** USB HID replies do not echo the command byte
(`usb_transport::validate_response` — any non-empty reply is treated as an ack),
so presence of a reply proves nothing. The discriminator is reply *content*
against a baseline captured from deliberately unallocated opcodes. Not-implemented
answers `08 00 00 10 00 00 00 b0 04 …`.

`supvan-cli probe-reads` does this, with a deny-list refusing anything that
writes, moves paper, or falls in `0xC0-0xEF` — an unrecognised opcode there
risks leaving the unit in a bootloader awaiting an image.

Result on a T50M Pro (`T0117A2410211517`), 15 read-only opcodes:

| opcode | result |
|--------|--------|
| `0xB1 RD_TIMESTAMP` | **implemented** — ASCII `"20230412"`, length-prefixed `0x0a` |
| the other 14 | identical to the unallocated baseline — not implemented |

Notably `0x2B HTIME_RD` is **not** implemented, so `0x2C HTIME_SET` almost
certainly is not either: setting heat time directly, bypassing the RFID record,
is not available on this unit. `0xBA PAPER_BACK` remains untested — it moves
paper, so it is deny-listed from the read sweep.

**Status flag — `FirmwareNeedUpgrade`.** The Linux tool decodes a "firmware
needs upgrade" flag from status byte `[3] & 0x20` (G-series `gPrintFlag.js`) —
the printer itself signals stale firmware, a natural trigger for a future
updater. The exact byte offset in our BT/USB status frames is unverified, so
`status.rs` does not decode it yet.

### Print pipeline glue

The above list is the per-command vocabulary. A real print job uses
them in this order (BT, simplified):

1. `CHECK_DEVICE` to confirm liveness.
2. `INQUIRY_STA` to verify no error flags before committing.
3. `START_PRINT` with the speed/darkness param.
4. For each compressed-raster block:
   - `NEXT_ZIPPEDBULK` (start-trans framing carrying `block_size`,
     `block_count`).
   - `BUF_FULL` (start-trans framing carrying `compressed_len`,
     `speed`) → upload raw bytes with the BT transport's
     `send_bulk_data` per-packet ack loop.
5. Poll `INQUIRY_STA.printing` until it clears.
6. `STOP_PRINT`.

The KsJob raster pipeline in `supvan-app/src/job.rs::transfer_page`
implements exactly this sequence.

## PrinterStatus bit layout

Same bit assignments on both transports. Differs only in *where* the
bytes live inside the response frame (BT at offsets 14..20, USB at
offsets 1..7).

| Field                | Reg byte | Bit mask | Source     |
|----------------------|----------|----------|------------|
| `buf_full`           | MSTA low (b14 / r1) | 0x01 | b0 |
| `label_rw_error`     | MSTA low | 0x02     | b0         |
| `label_end`          | MSTA low | 0x04     | b0         |
| `label_mode_error`   | MSTA low | 0x08     | b0         |
| `ribbon_rw_error`    | MSTA low | 0x10     | b0         |
| `ribbon_end`         | MSTA low | 0x20     | b0         |
| `low_battery`        | MSTA low | 0x40     | b0         |
| `device_busy`        | MSTA high (b15 / r2) | 0x04 | b1   |
| `head_temp_high`     | MSTA high | 0x08    | b1         |
| `cover_open`         | FSTA low (b16 / r3) | 0x08  | b2         |
| `insert_usb`         | FSTA low | 0x10     | b2         |
| `printing`           | FSTA low | 0x40     | b2         |
| `label_not_installed`| FSTA high (b17 / r4) | 0x01 | b3        |
| `print_count` (u16)  | b18..19  | full byte LE | resp[5..7] |

Mnemonic: **M**aster state for jam/empty/buffer, **F**lag state for
cover/operator/job state.

## MaterialInfo layout

```rust
pub struct MaterialInfo {
    pub uuid: String,          // BT only (hex-uppercase from frame[22..29])
    pub code: String,          // BT only (hex-uppercase from frame[29..37])
    pub sn: u16,               // both transports; "label SN" counter
    pub label_type: u8,        // both transports
    pub width_mm: u8,          // both transports
    pub height_mm: u8,         // both transports
    pub gap_mm: u8,            // both transports
    pub remaining: Option<u32>, // BT only (frame[43..47] LE); USB stubs None
    pub device_sn: Option<String>, // both transports; THIS is the cross-transport join key
}
```

### Cross-transport correlation

`MaterialInfo.device_sn` is the only field we've verified to carry the
same string over both transports:

- BT: parsed from BCD bytes at `frame[51..57]` (6 bytes of BCD → 12 ASCII
  digits, but the BCD parser concatenates `{byte:02}` per byte, so what we
  return is `"AABBCCDDEEFF"`-style not the printer's literal label).
- USB: parsed as null-terminated ASCII starting at `frame[40]`.

For the T50M Pro reference unit both encode the same printer's
`T0117A2410211517` serial — though after the BT BCD parser passes it
through `format!("{:02}", byte)` formatting, the BT-side value is
**not** literally the same string as the USB value. The discovery code
in `supvan-app/src/ipp_server.rs::SupvanDeviceBackend::list` uses the
BlueZ `Device1.Name` property (an independent path that *does* yield
the ASCII serial) rather than `MaterialInfo.device_sn` for the BT side.

**TODO**: align the BT `device_sn` parser to produce the same ASCII
serial as USB, so a future `MaterialInfo.device_sn`-only correlation
works cleanly without leaning on BlueZ properties.

### Remaining labels

Both transports physically carry this — the firmware updates it every
print — but only the BT parser extracts it today. The USB parser
returns `Some(None)` for `remaining`. From the comment at offset 31..32
("SN" counter), it's plausible the USB-side counter at that offset is
the same value, but the field is currently used for the label SN, not
the remaining count.

**TODO**: figure out where the USB 64-byte report carries the
remaining-label counter (probably also somewhere in 30..50). The
`remaining=None` we observe live confirms it's not at the BT offsets.

## Known gaps and open questions

| Gap | Where | Impact |
|---|---|---|
| `RD_DEV_NAME` over USB | `usb_transport::parse_device_name_response` | Can't get the printer name from a status query; have to issue `RETURN_MAT` instead. |
| `READ_REV` over USB | `parse_version_response` stub | We can't read the protocol version string over USB. Probably fine; same firmware on both ends. |
| `READ_FWVER` over USB | `parse_firmware_version_response` stub | No way to read firmware version over USB without bigger HID report. |
| `MaterialInfo.remaining` over USB | parse_usb_material returns None | Label-counter UX broken for USB-only setups. |
| BT `device_sn` BCD vs USB ASCII | `status::parse_material` | The two transports report the same physical value but in different encodings; downstream code can't naïvely string-compare. |
| `SET_RFID_DATA` (0x5D) | not exercised by any code path | We've never sent it. Firmware support unknown. |
| `BUF_FULL` (0x10) handling | request side is implemented; what the device sends back when its buffer fills mid-print isn't fully decoded. | KsJob's per-packet ack loop handles the timing but doesn't surface a typed status. |

## Appendix: BLE GATT transport (implemented behind the `ble` feature, unverified)

We reach the printers over three transports, all sharing the same 16-byte
command framing and 512-byte data frames:

- **Classic SPP** (RFCOMM): UUID `00001101-0000-1000-8000-00805F9B34FB`,
  channel auto-detected; 512-byte write chunks, ~10 ms inter-chunk drain.
- **USB HID**: `0xC0/0x40` framing, 64-byte reports.
- **BLE GATT**: for BLE-only hardware (E11/E12-class — the vendor gates BLE by
  `printingProcess == 4`). Implemented in `crates/supvan-proto/src/ble.rs` as a
  `BlePipe` driven by the shared [`spp_pipe::SppCodec`]; built only with
  `--features ble` (pulls `bluer` + BlueZ). **Unverified against hardware** — we
  own no BLE printer, so an E11/E12 reporter must validate the live round-trip.

BLE wire details (from the vendor `BLEUtils.java`, as implemented):

- Connect TRANSPORT_LE; BlueZ negotiates the ATT MTU (the app requests 200).
- Auto-detect one of three service/characteristic patterns, first match wins:
  `0000fee7-…` (notify == write `0000FEC1-…`), service
  `0000e0ff-3c17-d293-8e48-14fe2e4da212` (notify `0000ffe1-…`, write
  `0000ffe9-…`), or `0000ff00-…` (notify `0000ff01-…`, write `0000ff02-…`).
- Commands/status: write-with-response, then wait for a notification echoing the
  command byte at **offset 7**, polled up to ~4 s (the app loops 200 × 20 ms).
- Bulk image data: write-without-response, fragmented to ~MTU.
- Discovery (`crates/supvan-app/src/ble_discover.rs`): unfiltered LE scan, keep
  advertisers matching name `^[TGD]\d{2}` and MAC OUI `A4:93:40`.

Known unknowns to confirm on real hardware: the 512-byte SPP frame fragmentation
across BLE's smaller MTU, and whether the per-packet-ack drain behaves the same
over GATT notifications as over the RFCOMM stream.

## See also

- `crates/supvan-proto/src/cmd.rs` — command constants + frame builders.
- `crates/supvan-proto/src/status.rs` — BT response parsers + bit
  assignments.
- `crates/supvan-proto/src/spp_pipe.rs` — the `SppPipe` byte-pipe trait and the
  shared `SppCodec` that drives Classic-BT (`rfcomm.rs`) and BLE (`ble.rs`).
- `crates/supvan-proto/src/usb_transport.rs` — the USB HID `Transport` impl;
  the place where per-transport quirks land.
- `crates/supvan-proto/src/printer.rs` — high-level Printer interface;
  one method per command code, returning the parsed shape.
- `crates/supvan-app/src/job.rs::transfer_page` — the real-world
  ordering of these commands for a one-page label print.
