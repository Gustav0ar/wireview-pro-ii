# Compatibility

## Qualification matrix

Codec support and physical-device qualification are tracked separately. A
configuration layout can be implemented and fuzz-tested without implying that
it has been exercised on matching hardware.

| Device/configuration | Automated evidence | Physical evidence | Status |
|---|---|---|---|
| WireView Pro II, configuration V3 (raw 2), firmware raw 4, build `TG-WV-PRO2-FW_20260225_1902` | Unit, golden-vector, mock integration, Varlink, parser fuzz, package validation | Read-only identity, telemetry, faults, configuration, history cancellation, and reconnect behavior on Linux | Supported; release candidates still require the attended gates |
| Configuration V2 (raw 1) | Unit, golden-vector, mock integration, parser fuzz | Not yet tested on a V2 device | Implemented, hardware qualification pending |
| Configuration V1 (raw 0) | Unit, golden-vector, mock integration, parser fuzz | Not yet tested on a V1 device | Implemented, hardware qualification pending |
| Unknown configuration layout | Rejection tests | Not applicable | Rejected before configuration access |
| Unknown firmware with known EF05 identity and layout | Capability and identity validation tests | Not qualified | Accepted only when identity, layout, and decoded capabilities remain compatible |

| Distribution target | CI environment | Coverage |
|---|---|---|
| Debian/Ubuntu `.deb` | Ubuntu 24.04 | Build, install prior version, upgrade, mock service restart, socket activation, removal |
| Fedora `.rpm` | Fedora 43 | Build, install prior version, upgrade, execute, removal |
| Arch `.pkg.tar.zst` | Current Arch base-devel image | Build, install prior version, upgrade, execute, removal |

CI results qualify their recorded runner images, not every downstream release.
The exact candidate package must also pass
[`release-qualification.md`](release-qualification.md), including the
uninterrupted 24-hour soak and attended physical/VM detach test. Those gates
must not be inferred from codec tests or an earlier build.

## Implemented

- USB application identity `0483:5740`
- WireView vendor/product bytes `EF:05`
- Firmware version reported as the raw vendor response byte
- Complete read/write codecs for configuration V1, V2, and V3 (raw ordinals
  0, 1, and 2)
- Live telemetry with raw/decoded fault masks
- Public build-string/device identity reads and validated selective fault clear
- Parsed and exact-raw device history with one session-bound pause/resume lease
- Streaming parsed-history export with atomic destination-file replacement
- Strict 1.0.7 measurement validity gates and power-on boundary handling
- Typed exact reads and guarded, verified sector-preserving writes for all
  eight V3 RGB565 theme slots
- Generated API 2 interface/capability fingerprint, capability preflight, and
  matching GUI, CLI, and daemon build identity
- Runtime daemon polling interval from 100 through 5000 ms
- Verified physical-screen commands
- Bounded debug screen pause/resume with independent history-dump ownership
- Temporary configuration apply, reload from saved settings, permanent store,
  and firmware-default reset, with readback, revision conflict detection,
  rollback, and no implicit display change
- Guarded device-controller reboot with automatic daemon reconnection

## Capability policy

The handshake determines capabilities for each host attachment. Version 1.0.0
publishes `telemetry`, `history`, `screen`, `device-info`, `fault-clear`, and the
detected `config-v1`, `config-v2`, or `config-v3` only after the greeting and
device identity match. Raw configuration version 2 additionally publishes
`theme-assets-read` and `theme-assets-write`; older layouts do not.

Calibration, arbitrary SPI writes/erase, and DFU are not advertised.
Configuration reset is narrowly implemented through the recovered
configuration NVM command; it is distinct from the guarded device-reboot
command.

Unknown device identity, sensor enums, and configuration versions are rejected.
The current firmware byte is reported but is not allowlisted: a firmware is
accepted when the EF05 identity, known configuration layout, and decoded
capabilities remain compatible. Documentation must not claim that every unknown
firmware byte is rejected. V1/V2 configuration fields that do not exist in
those layouts are represented using the same compatibility defaults and theme
mapping as the device's legacy layouts.

## Feature boundary

| Device behavior | Status |
|---|---|
| Connect/disconnect, telemetry, identity, build string | Implemented |
| Poll interval | Implemented as runtime daemon setting |
| V1/V2/V3 config read/write and config NVM load/store/reset | Implemented |
| Screen selection | Implemented |
| Screen pause/resume | Implemented as bounded debug leases and internal history ownership |
| Raw log and parsed history | Implemented |
| Fixed V3 RGB565 theme-slot backup and guarded replacement | Implemented; physical write qualification pending |
| Raw and decoded fault state; selective `ClearFaults` | Implemented |
| Calibration NVM actions | Gated: no safe backup/readback recovery evidence |
| Enter bootloader / DFU update | Gated: firmware authenticity, range, identity continuity, and recovery evidence incomplete |
| Raw calibration/SPI read/write/erase/device-data commands | Deliberately not exposed |

The current daemon intentionally manages one active device. It rejects
ambiguous matches rather than choosing one. Explicit multi-device selection is
tracked as a later architecture item in the implementation plan.

The Debian, RPM, and Arch packages also install a generated `wireview(1)` page
and Bash, Zsh, and Fish completions. Tagged releases publish SHA-256 checksums,
a keyless Sigstore bundle for the checksum manifest, and GitHub provenance and
SBOM attestations.
