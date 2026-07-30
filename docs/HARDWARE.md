# Supvan T50M Pro — board survey

Chip markings read off the PCB by hand (2026-07-30), one side only. Markings are
verbatim; identifications are inference and each carries its confidence. Nothing
here is from a datasheet we hold — see "Datasheet availability" below.

## Chips

| Marking | Identification | Confidence |
|---------|----------------|-----------|
| `YC3121-L` / `EN1227` / `2406NDGF` | **Yichip YC3121 — Bluetooth 5.0 SoC**, QFN-56-EP(7×7), JLCPCB `C2916799`. Given the printer speaks BT Classic RFCOMM/SPP *and* BLE GATT, and nothing else on this side is a radio, this is the Bluetooth part and very likely the application processor. | medium-high on the part; role inferred |
| `AT8833` / `EC43BAY` | Dual H-bridge motor driver, DRV8833-compatible — the paper-feed stepper. | medium |
| `RU30L15H` / `001 QAE49` | Power MOSFET, likely N-channel ~30 V logic-level (`RU` = Ruichips; `30`/`L` decode as voltage class and logic-level gate on their scheme). Switching a heavy rail: printhead or motor supply. A thermal head draws several amps in bursts. | medium — a *decode of the naming convention*, not a datasheet lookup |
| `YCF5018` / `HQ7552` / `2403NA TD` | **Unidentified.** `YC` prefix hints at Yichip again but `F5018` doesn't place. By role, a battery-powered thermal printer wants a Li-ion charger, a printhead-rail converter, or a printhead driver — reasoning from need, not from the marking. | low — do not rely |
| `AWFGJE` or `AWF6JE` (G/6 unclear) | **Unidentified and not guessable.** Six characters with no vendor prefix reads as an SMD *marking code*, which only resolves via a marking-code database and is reused across vendors. | none |

`2403` / `2406` fit a 2024 date-code pattern, consistent with the unit serial
`T0117A24…` and the `20230412` firmware date.

**5-pad port beside the YC3121** — pad count fits SWD as
`SWDIO / SWCLK / nRST / VCC / GND`. JTAG needs four signals plus power, so five
pads is tight for JTAG and natural for SWD.

## Not yet located

- **Any NFC front-end.** The consumable tags are ISO 14443-A (see
  `PROTOCOL.md`), so something drives them. An earlier revision of these notes
  wrongly attributed that to the YC3121; it may be `YCF5018` or `AWFGJE`, or on
  the other side of the board.
- **External SPI flash** (SOIC-8 / USON-8, `25Q…`-style marking). Its presence or
  absence decides how firmware could be read — see below.
- The reverse side of the board was not surveyed.

## Why the firmware matters

`PROTOCOL.md` flags two things as unverified, and both are answered by firmware:

- the real `PWD`/`PACK` derivation for consumable tags — the vendor's host code
  has `SHA-256(UID ‖ Pwkey)` but commented out, with its author's note that the
  algorithm "has a problem", so the working version exists only in firmware
- whether a genuine tag carries a non-zero signature in `MaterialInfo::code`,
  which `supvan-app::record_is_ours` assumes when it refuses to overwrite a roll

Order of preference for any attempt, cheapest and safest first:

1. **Find external SPI flash and clip it.** Non-destructive, no unlock needed.
2. Failing that, SWD on the YC3121 — after confirming its unlock semantics.
   Read-out protection is likely and on many SoCs a forced unlock mass-erases
   flash. Bricking the printer is a bad trade for a question we can leave open.

Note that Yichip's *other* BLE family, YC16xx, is 32-bit RISC-V and advertises
firmware encryption. Different family, so it says nothing definite about the
3121 — but "probably ARM" is a poor default for this vendor.

## Datasheet availability

**We do not have a YC3121 datasheet.** Every public route was tried and failed:

| Route | Outcome |
|---|---|
| JLCPCB datasheet link for `C2916799` | 403 — Alibaba OSS bucket ACL, with and without referer/UA |
| manuals.plus "YC3121 Bluetooth MCU Product Brief" | 403 — Cloudflare challenge |
| chipspulse / smbom part pages | content-free stubs, no PDF |
| LCSC `C2916799` | 404 — JLC-only part number |
| `yichip.com.cn` | domain parked and for sale; vendor site gone |
| `github.com/Yichip-Microelectronics/YC3121` | empty repo, 8-byte README, untouched since 2021 |

The JLCPCB link is likely reachable from a logged-in session, which is the most
promising way to obtain it:
<https://jlcpcb.com/partdetail/YICHIP-YC3121L/C2916799>

Everything above about the YC3121 therefore rests on distributor metadata
(package, category, part number) plus inference from what the printer does — not
on a datasheet. Treat the core architecture, memory sizes and debug interface as
**unknown**, not as stated facts.
