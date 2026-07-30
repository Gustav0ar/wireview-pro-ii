# CLI usage

The `wireview` CLI talks to `wireviewd`; it never opens the USB device
directly. Run `wireview --help` or `wireview COMMAND --help` for the installed
command reference.

## Everyday commands

```bash
wireview status
wireview info
wireview telemetry
wireview telemetry --watch
wireview telemetry --json
wireview faults
wireview screen
wireview screen main
wireview history
wireview history --format csv --output wireview-history.csv
wireview version
```

`telemetry --watch` redraws one terminal view in place. It does not append a
new copy for each update. JSON output is intended for programs and dashboards.

## Device history

`wireview history` reads the device's SPI-flash data log without modifying it.
The table, CSV, and JSON formats contain decoded raw numeric values suitable
for spreadsheets, graphs, and dashboards. `--format raw` writes the exact
8 MiB logging region for forensic analysis.

```bash
wireview history
wireview history --format csv --output history.csv
wireview history --format json --output history.json
wireview history --format raw --output history.bin
```

Records are decoded and written incrementally. Named outputs use a temporary
file in the destination directory and replace the destination atomically only
after success. Ctrl+C and SIGTERM close the daemon-side dump session and resume
display updates. If a client disappears without cleanup, a 10-second daemon
lease performs it. The recorded timestamps are a wrapping device millisecond
counter, not calendar dates.

## Screens, faults, and debug commands

```bash
wireview screen help
wireview screen main
wireview screen simple
wireview screen current
wireview screen temperature
wireview screen status
wireview screen same

wireview faults
wireview faults --json
wireview faults clear logged over_current --yes

wireview debug monitor
wireview debug poll-interval
wireview debug poll-interval 500
wireview debug pause-display 30
wireview debug resume-display
wireview debug reboot-device --yes
wireview debug factory-reset --yes
```

`debug monitor` is a diagnostic event stream for observing connection,
disconnection, telemetry, and screen events. `poll-interval` changes only the
running daemon and accepts 100 through 5000 milliseconds.

`pause-display` accepts a 1 through 300 second lease. It freezes screen updates,
not telemetry, and automatically expires. History holds a separate pause lease.

`reboot-device` reboots the controller without erasing saved configuration.
`factory-reset` permanently replaces saved configuration with firmware
defaults. Fault clearing, reboot, and factory reset require `--yes`.

## Configuration workflow

Read or change one setting without editing JSON:

```bash
wireview config get fan.mode
wireview config get backlight_percent --json
wireview config set fan.mode fixed
wireview config set fault_actions.buzzer over_current,over_power
wireview config set fault_actions.soft_power none
wireview config set friendly_name "My WireView" --store --yes
```

Single-setting changes are temporary unless `--store --yes` is supplied. The
daemon performs the read-modify-validate-write transaction and prints a short
success message or the actual failure.

Use JSON for backups and bulk edits:

```bash
wireview config show --json > wireview-config.json
$EDITOR wireview-config.json

# Active until reload, controller reboot, or power loss
wireview config apply wireview-config.json

# Apply and save permanently
wireview config store wireview-config.json --yes

# Discard temporary changes and reload saved settings
wireview config reload

# Restore and permanently save firmware defaults
wireview config reset --yes
```

The exported document contains an opaque `revision` and a `settings` object.
Edit only `settings`. The daemon rejects stale revisions after a reconnect or
concurrent change, validates every field before writing, verifies readback, and
attempts to restore the previous active and saved settings after failure.
Persistent NVM operations are serialized and separated by at least one second.

## Configuration reference

The tables below use the dotted keys accepted by `config get` and `config set`.
Factory defaults are from configuration V3. Always export the connected
device's configuration before bulk editing because older layouts differ.

### General and fan

| Key | Accepted values | Factory default |
|---|---|---|
| `friendly_name` | Up to 32 printable ASCII characters | Empty |
| `backlight_percent` | Integer `0` to `100` | `100` |
| `fan.mode` | `curve`, `fixed` | `curve` |
| `fan.temperature_source` | `input`, `output`, `external1`, `external2`, `maximum` | `maximum` |
| `fan.duty_min_percent` | Integer `0` to `100`, not above maximum | `0` |
| `fan.duty_max_percent` | Integer `0` to `100`, not below minimum | `100` |
| `fan.temperature_min_c` | One-decimal value `0.0` to `50.0` °C | `50.0` |
| `fan.temperature_max_c` | One-decimal value `50.0` to `100.0` °C | `80.0` |
| `shutdown_wait_seconds` | Integer `0` to `255` | `10` |
| `logging_interval_seconds` | Integer `0` to `255` | `60` |
| `averaging_ms` | `22`, `44`, `89`, `177`, `354`, `709`, `1417` | `1417` |

### Fault actions

Each fault-action key accepts a comma-separated list containing any of
`chip_over_temperature`, `sensor_over_temperature`, `over_current`,
`wire_over_current`, `over_power`, and `current_imbalance`. Use `none` for an
empty list. Duplicates are rejected.

| Key | Factory default |
|---|---|
| `fault_actions.display` | `sensor_over_temperature,over_current,wire_over_current,over_power,current_imbalance` |
| `fault_actions.buzzer` | `sensor_over_temperature,over_current,wire_over_current,over_power` |
| `fault_actions.soft_power` | `none` |
| `fault_actions.hard_power` | `sensor_over_temperature,over_current,wire_over_current,over_power` |

Enabling `soft_power` or `hard_power` allows the matching fault to initiate the
corresponding power action.

### Fault thresholds

| Key | Accepted values | Factory default |
|---|---|---|
| `fault_thresholds.temperature_c` | One-decimal value `0.0` to `120.0` °C | `80.0` |
| `fault_thresholds.total_current_a` | Integer `0` to `150` A | `55` |
| `fault_thresholds.wire_current_a` | One-decimal value `0.0` to `25.5` A | `10.5` |
| `fault_thresholds.total_power_w` | Integer `0` to `2000` W | `660` |
| `fault_thresholds.current_imbalance_percent` | Integer `0` to `100` | `40` |
| `fault_thresholds.current_imbalance_min_load_a` | Integer `0` to `10` A | `6` |

Choose thresholds for the cable, power supply, load, and cooling setup. A low
threshold can immediately trigger the display, buzzer, fan, or configured
power actions.

### Display

| Key | Accepted values | Factory default |
|---|---|---|
| `display.default_screen` | `main`, `simple`, `current`, `temperature`, `status` | `main` |
| `display.current_scale_a` | `5`, `10`, `15`, `20` | `10` |
| `display.power_scale` | `auto`, `watts300`, `watts600` | `watts600` |
| `display.rotation_degrees` | `0`, `180` | `0` |
| `display.timeout_mode` | `static`, `cycle`, `sleep` | `static` |
| `display.cycle_screens` | Unique comma-separated screen names, or `none` | `main,current,temperature` |
| `display.cycle_time_seconds` | Integer `0` to `255` | `10` |
| `display.timeout_seconds` | Integer `0` to `255` | `30` |
| `display.primary_color` | Six-digit RGB or eight-digit ARGB hex | `FFFFFF` |
| `display.secondary_color` | Six-digit RGB or eight-digit ARGB hex | `646464` |
| `display.highlight_color` | Six-digit RGB or eight-digit ARGB hex | `E64121` |
| `display.background_color` | Six-digit RGB or eight-digit ARGB hex | `000000` |
| `display.background` | `thermal_grizzly_orange`, `thermal_grizzly_dark`, `disabled` | `thermal_grizzly_orange` |
| `display.fan_theme` | `thermal_grizzly_orange`, `thermal_grizzly_dark`, `black_and_white` | `thermal_grizzly_orange` |
| `display.inverted` | `true`, `false` | `false` |

Six hexadecimal characters mean `RRGGBB` with full opacity. Eight mean
`AARRGGBB`; for example, `80FFFFFF` is white at about 50% opacity. Do not add
`#`, `0x`, spaces, or non-hexadecimal characters. Input is case-insensitive;
output is uppercase and omits the leading `FF` for opaque colors.

### Configuration version differences

- V1 has no configurable averaging, default screen, independent colors,
  independent fan theme, or inversion. Averaging is fixed at `1417`.
- V2 adds averaging but retains the legacy display-theme limitations.
- V3 supports every setting listed above independently.

For V1 and V2, retain exported compatibility fields unless the layout supports
them. The daemon applies the legacy preset-theme conversion and rejects
combinations that cannot be represented safely.

## Machine-readable output

Commands that support `--json` emit stable typed values for automation.
History CSV and JSON use numeric measurements without localized units or
formatted gauge strings. Human-readable commands include labels and units.
