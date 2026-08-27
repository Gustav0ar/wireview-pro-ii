# Desktop app

`wireview-gui` is the native Linux desktop client for WireView Pro II. It uses
the local `wireviewd` Varlink API and never opens the USB device directly.

## Start the app

Install a project package, enable the daemon socket, and authorize your user:

```bash
sudo systemctl enable --now wireviewd.socket
sudo usermod --append --groups wireview-client "$USER"
```

Log out and back in after changing group membership. Then open **WireView Pro
II** from the desktop application menu or run:

```bash
wireview-gui
```

The window reconnects automatically when the daemon or device becomes
available. The status in the top-right corner distinguishes a missing daemon,
a missing device, recovery, stale telemetry, and a ready device session.

## Use the pages

- **Overview** shows total power, recent samples, and all six conductors.
- **Pins** compares conductor current and deviation against the configured
  per-conductor limit, and shows controller supply, external probes, and cable
  capability.
- **Graphs** plots power, all six conductor currents, all six conductor
  voltages, or temperature probes across 60-second, 5-minute, and 10-minute
  windows. Pause, clear, and per-series controls do not stop live telemetry in
  the rest of the app.
- **Faults** separates active, recorded, and unknown register bits. Unknown
  bits remain visible and cannot be cleared by the app.
- **History** reads the complete 8 MiB logging region. It shows the latest 80
  decoded rows and exports the complete result as CSV, JSON, or exact raw data.
- **Configure** edits every supported device setting and the daemon polling
  interval. The page keeps edits local until you apply, store, or discard them.
- **Themes** reads, previews, backs up, and replaces the eight recovered named
  RGB565 slots. It does not accept arbitrary flash addresses.
- **Device** shows hardware and daemon identity, build information, recovery
  actions, and Slint license attribution.

The left rail keeps primary telemetry visible while changing pages. The graph
uses all remaining width. Other pages keep the protection, device screen,
connection, and operation rail visible on the right.

The graph retains at most 1,200 unique samples. It draws only the selected unit
and visible series, caps path geometry independently from history length, and
does not run an animation or repaint timer.

## Apply configuration safely

Change fields on **Configure**, then choose one action:

- **Apply until reload** validates and activates the complete edited document.
  A reload, device reboot, or power loss restores the stored configuration.
- **Store permanent** validates, applies, and writes the edited device settings
  to nonvolatile memory. The app requires confirmation. The daemon polling
  interval remains runtime-only.
- **Reload stored** discards local edits and active temporary changes, then
  loads the stored configuration.

The daemon rejects stale revisions, invalid combinations, unsupported firmware
layouts, and failed readback. Changing one fan bound does not validate an
invalid intermediate document; the complete edited form is validated as one
candidate.

Factory reset, device reboot, fault clearing, permanent configuration writes,
and theme replacement require confirmation. A confirmed fault clear targets
only the selected known active and recorded bits. A persistent condition can
assert the bit again immediately.

## Export history

Select **Load history** before exporting. The device display is paused only for
the leased read and resumes on completion, cancellation, disconnect, or lease
expiry. Named output files are replaced atomically after a complete export.

CSV and JSON contain all valid decoded measurement records. Exact raw preserves
all 8 MiB, including unparsed bytes and end markers. Device timestamps are a
wrapping millisecond counter, not wall-clock time.

## Back up or replace a theme slot

On **Themes**:

1. Select one named slot.
2. Choose **Read preview**.
3. Enter a destination and choose **Export backup**.
4. To replace the slot, enter an exact-size RGB565 file and choose **Replace
   slot**.
5. Review the warning and confirm the write.

Background slots are 320 x 170 and 108800 bytes. Fan slots are 73 x 73 and
10658 bytes. The daemon preserves surrounding sectors, verifies the SHA-256
digest and readback, and attempts rollback after an ordinary failure. A device
disconnect after flash mutation starts has an unknown outcome and is never
retried automatically.

## Diagnostic startup options

Use a custom daemon socket for development:

```bash
wireview-gui --socket /tmp/wireviewd.sock
```

Use a deterministic read-only screen without a daemon:

```bash
wireview-gui --demo ready --page overview --no-tray
wireview-gui --demo ready --page graphs --no-tray
wireview-gui --demo fault --page faults --no-tray
wireview-gui --demo stale --page pins --no-tray
wireview-gui --demo offline --page device --no-tray
```

Demo mode disables device mutations. Supported startup pages are `overview`,
`pins`, `graphs`, `faults`, `history`, `configure`, `themes`, and `device`.

## Troubleshoot connection problems

Check the daemon and socket first:

```bash
systemctl status wireviewd.socket wireviewd.service
wireview status
journalctl --unit wireviewd.service --boot
```

If the CLI works but the desktop app reports permission denied, start a new
login session after joining `wireview-client`. If both clients report no
device, follow the USB, VM passthrough, and recovery checks in
[`operations.md`](operations.md).
