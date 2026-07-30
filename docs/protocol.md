# WireView Pro II serial protocol

This document records the protocol subset implemented by `wireviewd` 1.0.0.

## Transport

- USB CDC ACM (`0483:5740`)
- 115200 baud
- 8 data bits
- no parity
- 1 stop bit
- no flow control
- 1000 ms read/write deadline
- one process owns the serial descriptor

Commands are raw byte sequences with no outer header or checksum. For a read,
the host writes the command byte and then reads the exact fixed response size.

## Connection handshake

1. Open the serial device and discard pending input.
2. Assert RTS without writing a command.
3. Read exactly 32 bytes.
4. Deassert RTS.
5. Verify the response is ASCII `Thermal Grizzly WireView Pro II` followed by
   NUL.
6. Send command `1`; read three vendor bytes. Bytes 0 and 1 must be `EF 05`;
   byte 2 is the firmware version.
7. Send command `2`; read the 12-byte UID.
8. Send command `5`; read four bytes. Byte 2 is the raw configuration version
   ordinal (`0`, `1`, or `2` in the recovered driver).
9. Send `0C F1` to resume screen updates.

The four-byte version probe is the prefix of a complete configuration response.
Because the daemon keeps the tty open, it waits for and discards the unread
response tail before issuing the UID or screen command. This prevents residual
configuration bytes from being parsed as later responses.

On Linux, an already initialized device may not emit the greeting again after
the tty is closed and reopened. The daemon still performs the RTS greeting
attempt. If that attempt times out without returning conflicting bytes, it
continues with command `1` and accepts the device only when the exact `EF 05`
vendor/product response is returned. A malformed greeting or unexpected vendor
identity is always rejected.

## Commands

| Name | Byte | Implemented |
|---|---:|---|
| Welcome | `00` | Handshake uses RTS instead |
| Read vendor data | `01` | Yes |
| Read UID | `02` | Yes |
| Read device data | `03` | No |
| Read sensors | `04` | Yes |
| Read configuration | `05` | Yes |
| Write configuration | `06` | Yes, complete validated configuration |
| Read calibration | `07` | No |
| Write calibration | `08` | No |
| SPI write page | `09` | No |
| SPI read page | `0A` | Yes, data-log region only |
| SPI erase sector | `0B` | No |
| Screen change | `0C` | Yes |
| Read build information | `0D` | Yes |
| Clear faults | `0E` | Yes, validated masks |
| Reset | `F0` | Yes, guarded controller reboot |
| Enter bootloader | `F1` | No |
| NVM operation | `F2` | Yes, configuration reload/store/reset only |
| NOP | `FF` | No |

Unsupported calibration, arbitrary write/erase, and bootloader commands are
intentionally absent from the backend API.

Command `0D` returns the 68-byte, pack-4 `BuildStruct`: three vendor bytes,
32 NUL-terminated ANSI bytes for product name, 32 for build information, and a
trailing product-name length byte. The device-info response preserves both
strings.

Fault clear is exactly five bytes:

```text
0E ACTIVE_LO ACTIVE_HI LOGGED_LO LOGGED_HI
```

Masks are little endian. Normal daemon calls permit only the six recovered
fault bits (`0x003F`) and retain unknown bits in telemetry instead of silently
discarding them.

## Device configuration

Command `05` returns the complete active configuration. The response length is
72 bytes for raw version 0 (V1), 74 bytes for version 1 (V2), and 96 bytes for
version 2 (V3). Every integer is little endian and the structures use pack-4
alignment.

Fields common to all versions are:

| Offset | Size | Field | Representation |
|---:|---:|---|---|
| 0 | 2 | CRC | opaque device metadata, preserved and compared |
| 2 | 1 | version | raw ordinal 0, 1, or 2 |
| 3 | 32 | friendly name | printable ASCII, NUL padded |
| 35 | 1 | alignment | zero |
| 36 | 1 | fan mode | 0 curve, 1 fixed |
| 37 | 1 | fan temperature source | input, output, external 1/2, maximum |
| 38 | 1 | minimum fan duty | percent |
| 39 | 1 | maximum fan duty | percent |
| 40 | 2 | fan minimum temperature | signed ÷10 °C |
| 42 | 2 | fan maximum temperature | signed ÷10 °C |
| 44 | 1 | backlight | percent |
| 45 | 1 | alignment | zero |
| 46 | 2 | display fault actions | fault bitmask |
| 48 | 2 | buzzer fault actions | fault bitmask |
| 50 | 2 | soft-power fault actions | fault bitmask |
| 52 | 2 | hard-power fault actions | fault bitmask |
| 54 | 2 | temperature limit | signed ÷10 °C |
| 56 | 1 | total-current limit | A |
| 57 | 1 | per-wire current limit | ÷10 A |
| 58 | 2 | total-power limit | W |
| 60 | 1 | current-imbalance limit | percent |
| 61 | 1 | imbalance minimum load | A |
| 62 | 1 | shutdown wait | seconds |
| 63 | 1 | device logging interval | seconds |

V1 places its eight-byte UI block at offset 64. V2 adds the averaging enum at
offset 64 and places its UI block at 65. Their UI block contains current scale,
power scale, legacy theme, rotation, timeout mode, cycle-screen mask, cycle
time, and timeout. Fields unavailable in these versions use a legacy
preset-theme compatibility conversion.

V3 stores the averaging enum at offset 64, three alignment bytes, then this UI
block:

| Offset | Size | Field |
|---:|---:|---|
| 68 | 1 | default screen |
| 69 | 1 | current scale |
| 70 | 1 | power scale |
| 71 | 1 | rotation |
| 72 | 1 | timeout mode |
| 73 | 1 | cycle-screen bitmask |
| 74 | 1 | cycle time in seconds |
| 75 | 1 | timeout in seconds |
| 76 | 4 | primary ARGB color |
| 80 | 4 | secondary ARGB color |
| 84 | 4 | highlight ARGB color |
| 88 | 4 | background ARGB color |
| 92 | 1 | background preset |
| 93 | 1 | fan theme |
| 94 | 1 | display inversion |
| 95 | 1 | alignment |

The averaging ordinals correspond to 22, 44, 89, 177, 354, 709, and 1417 ms.
Fault bit positions are listed in the sensor-response section below.

Configuration writes are sent in chunks of at most 62 payload bytes. Each
frame is `06 OFFSET PAYLOAD`, where `OFFSET` is the byte offset within the
complete structure. The daemon validates every enum, mask, range, version,
preserved CRC field, and representable scaling before sending any frame, then
reads the active configuration back and requires an exact match. No recovered
CRC algorithm is claimed: the field is preserved and compared as opaque device
metadata. API concurrency uses a separate revision derived from the complete
configuration and hardware session.

If a write/readback/store fails while the same session remains connected, the
daemon rewrites the complete previous configuration, verifies it, and restores
NVM when the failed operation may have persisted. A disconnect returns an
unknown outcome and is never retried automatically. Configuration completion
does not send a screen command.

Configuration NVM operations are six bytes:

```text
F2 55 AA 55 AA OPERATION
```

Operation 1 replaces the active configuration with the permanently stored
settings. Operation 2 permanently stores the active configuration. Operation 3
replaces both the active and permanently stored configuration with firmware
defaults. These are configuration operations and are distinct from command
`F0`, the guarded device reboot command exposed as
`RebootDevice` over Varlink and `wireview debug reboot-device --yes` in the CLI.

Before operation 1, the daemon keeps the validated active configuration in
memory. If the loaded configuration is blank, malformed, or for a different
format version, it immediately writes the previous active configuration back
and reports the rejected reload to the caller.

## Stored telemetry history

Telemetry history occupies the 8 MiB region from `0x00800000` through
`0x00FFFFFF`. The daemon exposes only bounded reads inside that region. A
logical dump pauses display updates once with `0C F0`, sends all SPI-read
requests under a session-bound lease, and attempts to resume the display once
with `0C F1` on success, failure, or graceful CLI cancellation. A 10-second
daemon lease performs the same cleanup after a client crash, forced
termination, or lost connection.

An SPI-read request is:

| Offset | Size | Field |
|---:|---:|---|
| 0 | 1 | command `0A` |
| 1 | 4 | absolute flash address, little endian |
| 5 | 4 | response length, little endian; at most 256 bytes |

The response is exactly the requested number of raw bytes. History entries use
pack-1 layout and are 21 bytes:

| Offset | Size | Field | Scaling |
|---:|---:|---|---|
| 0 | 4 | low 2 bits: entry type; high 30 bits: wrapping MCU tick | milliseconds |
| 4 | 4 | input, output, external 1, external 2 temperatures | signed °C; zero external value means absent |
| 8 | 6 | per-pin voltages | ÷10 V |
| 14 | 6 | per-pin currents | ÷10 A |
| 20 | 1 | cable capability | same enum as live telemetry |

Entry type 0 is a measurement and type 2 is a power-on marker. Types 1 and 3
are skipped. The firmware pads entries that would cross a 256-byte page,
sectors are 4096 bytes, and 32 consecutive erased entries (`FF FF FF FF` in
the data field) after the first valid record mark the logical end of history.

The 30-bit value is a wrapping MCU millisecond counter. It is deliberately not
rendered as a Unix timestamp or calendar date.

The CLI retains only the exact 8 MiB raw region. Table, CSV, and JSON exports
visit decoded records one at a time rather than accumulating the expanded
records. A named output is written to a same-directory temporary file, flushed,
and atomically renamed only after successful completion.

## Screen subcommands

Screen commands are two bytes: command `0C` followed by:

| Screen | Byte |
|---|---:|
| Main | `E0` |
| Simple | `E1` |
| Current | `E2` |
| Temperature | `E3` |
| Status | `E4` |
| Same | `EF` |
| Pause updates | `F0` |
| Resume updates | `F1` |

## Sensor response

Command `04` returns exactly 100 bytes. The source structure uses sequential
layout with pack 4 and little-endian values.

| Offset | Size | Type | Field | Scaling |
|---:|---:|---|---|---|
| 0 | 2 | i16 | input temperature | ÷10 °C |
| 2 | 2 | i16 | output temperature | ÷10 °C |
| 4 | 2 | i16 | external temperature 1 | ÷10 °C; -100 °C means absent |
| 6 | 2 | i16 | external temperature 2 | ÷10 °C; -100 °C means absent |
| 8 | 2 | u16 | VDD | ÷1000 V |
| 10 | 1 | u8 | fan duty | percent |
| 11 | 1 | — | alignment padding | — |
| 12 | 72 | 6 × pin structure | per-pin readings | see below |
| 84 | 4 | u32 | device total power | not exposed; client computes from pins |
| 88 | 4 | u32 | device total current | not exposed; client computes from pins |
| 92 | 2 | u16 | average voltage | ÷1000 V |
| 94 | 1 | u8 | cable capability | enum |
| 95 | 1 | — | alignment padding | — |
| 96 | 2 | u16 | active fault bitmask | enum ordinal is bit position |
| 98 | 2 | u16 | logged fault bitmask | enum ordinal is bit position |

Each 12-byte pin structure is:

| Relative offset | Size | Type | Field | Scaling |
|---:|---:|---|---|---|
| 0 | 2 | i16 | voltage | ÷1000 V |
| 2 | 2 | — | alignment padding | — |
| 4 | 4 | u32 | current | ÷1000 A |
| 8 | 4 | u32 | device power | currently ignored; normalized power is V × A |

Cable capability ordinals are 0=600 W, 1=450 W, 2=300 W, and 3=150 W.

Fault bit positions are:

0. chip over-temperature
1. sensor over-temperature
2. over-current
3. per-wire over-current
4. over-power
5. current imbalance

## Failure behavior

- A short response, timeout, EOF, broken pipe, or removed tty fails the
  transaction.
- EOF, broken pipe, and missing device are classified as connection loss.
- Unknown cable capability ordinals reject the complete snapshot.
- No read or screen mutation is automatically retried after an ambiguous
  connection loss.
- Reconnection repeats the complete handshake and creates a new daemon session.
