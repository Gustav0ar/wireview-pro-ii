# Attended release qualification

Run this checklist against the exact package intended for release. It is an
operator-attended hardware gate, not a CI replacement.

## Preconditions

- Install the candidate package and start `wireviewd.socket`.
- Add the operator to `wireview-client`, then log out and back in:

  ```bash
  sudo usermod --append --groups wireview-client "$USER"
  ```

- Run the checklist as that operator.
- Confirm the device is behaving normally and its desired configuration is
  stored.
- Close any other application or VM currently holding the USB device.
- Record the package checksum and expected build ID.

## Automated read-only core

```bash
WIREVIEW_RELEASE_HIL=1 \
WIREVIEW_EXPECT_BUILD_ID=git-0123456789ab-20260730122000 \
bash scripts/qualify-release.sh
```

The core captures package/binary identity, device identity, telemetry, faults,
configuration, service state, and journal evidence. It interrupts an active
history dump and requires immediate daemon cleanup. It performs no persistent
device writes.

Evidence is retained under `target/release-qualification-TIMESTAMP/`.

## Soak gate

Run the packaged daemon continuously before the attended gates:

```bash
bash scripts/soak-test.sh
```

The default duration is 24 hours with one sample per minute. The runner fails
on stale/unavailable telemetry, daemon loss, event-publisher lag, or more than
32 MiB of observed RSS growth. It records every sample plus a summary under
`target/soak-TIMESTAMP/`. Thresholds and intervals can be adjusted with the
`WIREVIEW_SOAK_*` environment variables documented in
[`development.md`](development.md), but the release sign-off must record any
deviation from the defaults.

## Optional attended gates

Temporary configuration and reload, with restoration of the exact original
active settings:

```bash
WIREVIEW_RELEASE_HIL=1 \
WIREVIEW_RELEASE_CONFIG=1 \
bash scripts/qualify-release.sh
```

An explicitly attended theme flash transaction can be exercised separately
with the lower-level hardware smoke test. It reads `fan-dark-1`, writes those
exact same bytes through the erase/rewrite path, reads them back, and requires
an exact comparison:

```bash
WIREVIEW_HIL=1 \
WIREVIEW_HIL_THEME_MUTATION=1 \
WIREVIEW_HIL_THEME_SLOT=fan-dark-1 \
bash scripts/smoke-hardware.sh
```

This gate is off by default and requires an explicit named slot because it
mutates SPI flash even though the payload is unchanged. On failure it retains
the backup, hashes, daemon log, and readback evidence and prints a manual
recovery command. Inspect device behavior before attempting recovery. It does
not exercise firmware update or arbitrary flash access.

The runner validates the slot name before starting the daemon and enables
recovery guidance only after the backup and its digest have been created.

Systemd restart and socket activation:

```bash
WIREVIEW_RELEASE_HIL=1 \
WIREVIEW_RELEASE_SYSTEMD=1 \
bash scripts/qualify-release.sh
```

Physical removal, VM transfer, and return to the host:

```bash
WIREVIEW_RELEASE_HIL=1 \
WIREVIEW_RELEASE_DISCONNECT=1 \
bash scripts/qualify-release.sh
```

The disconnect gate pauses for the operator to move the device. It requires a
new daemon session after reconnection and verifies that the UID is unchanged.

Options can be combined for the complete gate:

```bash
WIREVIEW_RELEASE_HIL=1 \
WIREVIEW_RELEASE_CONFIG=1 \
WIREVIEW_RELEASE_SYSTEMD=1 \
WIREVIEW_RELEASE_DISCONNECT=1 \
WIREVIEW_EXPECT_BUILD_ID=git-0123456789ab-20260730122000 \
bash scripts/qualify-release.sh
```

## Sign-off

Before publishing, confirm:

- candidate package checksum and build ID match the release artifacts;
- the uninterrupted soak gate passed with its evidence retained;
- socket activation and service restart passed;
- history cancellation returned immediately without a partial file;
- temporary configuration was reloaded and original active settings restored;
- physical detach or VM attachment was observed as disconnected;
- return to the host created a new session for the same UID;
- final telemetry is fresh, display updates are active, and device behavior is
  normal;
- final configuration settings match the captured baseline;
- when the optional theme gate ran, exact theme readback matched its baseline;
- the journal contains no unexplained warnings or errors.

Calibration NVM and firmware/DFU operations are outside this checklist. They
remain unsupported until an authenticated firmware source and verified recovery
path are available.
