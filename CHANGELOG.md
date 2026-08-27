# Changelog

All notable changes to this project will be documented here.

## Unreleased

## 1.2.0 - 2026-08-27

- Add the native Rust and Slint desktop application with overview, conductor,
  graph, fault, history, configuration, theme, and device pages.
- Add bounded full-width telemetry graphs with unit and series selection,
  pause, clear, and 60-second, 5-minute, and 10-minute windows.
- Extract the shared domain model and typed Varlink client contract into
  `wireview-core` and `wireview-ipc` workspace crates.
- Bundle the GUI, CLI, daemon, desktop launcher, and icon in every Debian, RPM,
  and Arch package, with explicit X11 and Wayland runtime dependencies.
- Add deterministic GUI interaction and Xvfb smoke tests for every page, the
  minimum window size, and a live mock-daemon connection.

## 1.1.1 - 2026-08-24

- Fix selective fault clearing by converting daemon bits-to-clear masks into
  the inverted retain masks required by the device firmware.
- Restore the current device screen after fault clearing, matching the Windows
  1.0.7 command sequence.

## 1.1.0 - 2026-08-03

- Align parsed history validity and power-on boundary handling with Windows
  software 1.0.7.
- Retry only transient SPI page-read timeouts while preserving exact-read
  semantics.
- Add exact backup and guarded, sector-preserving, verified replacement for the
  eight fixed V3 RGB565 theme slots.
- Bump the Varlink contract to API 2.
- Retry display-resume cleanup automatically after transient transport errors.
- Make history cancellation interrupt stalled read-only SPI operations.
- Restrict API access to the dedicated `wireview-client` group and bound daemon
  memory, tasks, and file descriptors.

## 1.0.0 - 2026-07-29

- Initial Linux daemon and CLI.
- CLI installed as `wireview`.
- Read-only WireView Pro II telemetry.
- Human-readable one-shot and in-place live telemetry, including lossless raw
  fault masks and freshness/session metadata.
- Complete V1/V2/V3 device configuration through the daemon and CLI, including
  temporary apply, permanent store, safe reload rollback, and firmware-default
  reset.
- Transactional configuration writes with readback verification, stale-revision
  rejection, exact rollback, distinct unknown-outcome/rollback errors, and no
  unrelated display commands.
- Atomic dotted-key configuration reads and changes without requiring JSON
  editing.
- Guarded device-controller reboot and debug factory-reset commands.
- Device identity/build inspection, selective fault clearing, runtime poll
  interval control, bounded debug display-pause leases, and raw 8 MiB
  device-log export with immediate SIGINT/SIGTERM cleanup.
- Streaming table/CSV/JSON history encoding and atomic file replacement.
- API 1 compatibility/capability preflight and packaged build identifiers.
- Attended packaged-release qualification for systemd, reversible
  configuration, history cancellation, USB removal, and VM transfer.
- Read-only device configuration hardware validation; mutating hardware tests
  require a separate explicit opt-in.
- Verified volatile screen selection.
- Varlink API with local-user access and event streaming.
- Line-flushed event monitoring for real-time pipelines.
- USB discovery, disconnect, VM-passthrough, and reconnect lifecycle handling.
- Consecutive transport-failure detection, stale telemetry, descriptor release,
  and new-session enforcement after reconnect.
- Strict bundled CLI/daemon API 1 compatibility preflight.
- Debian, RPM, and Arch Linux packages with systemd integration, SHA-256
  checksums, a signed checksum manifest, SPDX 2.3 SBOM generation,
  tagged-release attestations, a generated manual page, and Bash/Zsh/Fish
  completions.
- Dirty-checkout-safe build provenance, matching CLI/daemon and cross-package
  identities, and full daemon `--version` output.
- A generated Varlink API/capability fingerprint, scheduled parser fuzzing, and
  a bounded 24-hour resource and telemetry soak runner.
