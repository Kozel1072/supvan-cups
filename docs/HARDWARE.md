# Supvan T50M Pro — board survey

Chip markings read off the PCB by hand (2026-07-30), one side only. Markings are
verbatim. The YC3121 identification is now confirmed against its datasheet
(`datasheets/YC3121-L_C2916799.pdf`); the rest remain inference at the stated
confidence.

## Chips

| Marking | Identification | Confidence |
|---------|----------------|-----------|
| `YC3121-L` / `EN1227` / `2406NDGF` | **Yichip YC3121-L — dual-mode Bluetooth 5.0 SoC (BR/EDR + BLE)**, QFN-56-EP(7×7), JLCPCB `C2916799`. **Confirmed by datasheet.** 32-bit RISC core (≤96 MHz, MPU), 64 KB scrambled SRAM, 512 KB/1 MB on-chip **secure** flash, 8 KB OTP, USB, 2×UART, 2×SPI + QSPI, IIC, 40 GPIO, hardware AES/DES/SM4/RSA/SHA/TRNG. This is the application processor and the radio both. | confirmed |
| `AT8833` / `EC43BAY` | Dual H-bridge motor driver, DRV8833-compatible — the paper-feed stepper. | medium |
| `RU30L15H` / `001 QAE49` | Power MOSFET, likely N-channel ~30 V logic-level (`RU` = Ruichips; `30`/`L` decode as voltage class and logic-level gate on their scheme). Switching a heavy rail: printhead or motor supply. A thermal head draws several amps in bursts. | medium — a *decode of the naming convention*, not a datasheet lookup |
| `YCF5018` / `HQ7552` / `2403NA TD` | **Unidentified.** `YC` prefix hints at Yichip again but `F5018` doesn't place. By role, a battery-powered thermal printer wants a Li-ion charger, a printhead-rail converter, or a printhead driver — reasoning from need, not from the marking. | low — do not rely |
| `AWFGJE` or `AWF6JE` (G/6 unclear) | **Unidentified and not guessable.** Six characters with no vendor prefix reads as an SMD *marking code*, which only resolves via a marking-code database and is reused across vendors. | none |

`2403` / `2406` fit a 2024 date-code pattern, consistent with the unit serial
`T0117A24…` and the `20230412` firmware date.

**5-pad port beside the YC3121 is JTAG/SWD.** The datasheet puts the debug pins on
GPIO14 (`JTAG_SW_CLK` / `SWCLK`) and GPIO15 (`JTAG_SW_IO` / `SWDIO`) — a
serial-wire two-wire port, so `SWDIO / SWCLK / nRST / VCC / GND` is the expected
5-pad breakout. But see the security note: getting to the pins is not the hard
part.

## Not yet located

- **Any NFC front-end.** The consumable tags are ISO 14443-A (see
  `PROTOCOL.md`), so something drives them. The YC3121 has no NFC block, so it is
  one of the two unidentified parts (`YCF5018` / `AWFGJE`) or on the reverse side.
- The reverse side of the board was not surveyed. External SPI flash is unlikely
  to be there — the YC3121 has 512 KB/1 MB of on-chip flash, ample for this
  firmware, so there is probably no separate flash chip to clip.

## Firmware is not extractable in practice

`PROTOCOL.md` flags two things that only firmware would settle: the real
`PWD`/`PACK` derivation for consumable tags (the host code's
`SHA-256(UID ‖ Pwkey)` is commented out as broken), and whether a genuine tag
carries a non-zero signature in `MaterialInfo::code`, which
`supvan-app::record_is_ours` assumes.

**The datasheet makes reading that firmware impractical**, and it is worth being
concrete about why so nobody sinks time into it:

- Flash is **on-chip and "secure"** — there is no external flash to clip, which
  was the only cheap, non-destructive route.
- The debug port is a **受控 JTAG ("controlled JTAG")**: access is gated, not
  open. Boot runs from ROM and the SoC does **RSA signature verification of the
  firmware at download and at boot**, with a full crypto block (AES, DES, SM4,
  RSA, SHA, TRNG) and an MPU behind it.
- Protection state and debug-enable live in **8 KB OTP, fuse-based**, locked by
  ROM during the debug/production phase. OTP is one-way: once burned, there is no
  reverting it, and no way to read behind it after lock.

So the realistic options are all poor: the controlled-JTAG port likely refuses a
readout on a production unit, and forcing it — where even possible — risks
tripping OTP/anti-tamper and bricking the printer. This is a much harder target
than the earlier "clip the SPI flash" hope assumed. **Treat both unverified items
as staying unverified**; the `record_is_ours` assumption should be revisited only
if a genuine roll ever misbehaves against it in practice, not by firmware
extraction.

## Datasheet

`datasheets/YC3121-L_C2916799.pdf` — Yichip YC3121-L chip datasheet, V1.0
(2021-04-01), 82 pages, Chinese. Obtained from a logged-in JLCPCB session after
every anonymous route failed (OSS bucket ACL, Cloudflare, dead vendor domain,
empty GitHub repo). Marked "Confidential and Proprietary" by Yichip; kept in-tree
as the authoritative reference for this board.
