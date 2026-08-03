# Varlink API

`wireviewd` serves the `io.github.Gustav0ar.WireView` interface through the
Unix socket `/run/wireviewd/io.github.Gustav0ar.WireView`. The packaged
`wireviewd.socket` unit owns the listening socket and starts the daemon on the
first connection.

The implementation uses zlink 0.7.0 with its Tokio Unix transport.

The complete standalone interface is checked in at
[`interfaces/io.github.Gustav0ar.WireView.varlink`](../interfaces/io.github.Gustav0ar.WireView.varlink)
and installed under `/usr/share/varlink/interfaces/`.

Parameterless calls must omit the `parameters` member. Some older clients,
including `varlinkctl` from systemd 255, always send `parameters: {}` and
cannot call those methods or `org.varlink.service` introspection against zlink
0.7.0. Parameterized calls work normally. Use the packaged CLI or a zlink
client on those systems.

## Stability policy

The current package exposes API version `2`. This field is the JSON integer
`2`, never the strings `"2"` or `"v2"`.

Within API 2, existing methods, fields, types, errors, and documented semantics
will not be removed, renamed, or incompatibly changed. New optional methods,
types, fields, errors, or capabilities may be added while retaining API 2;
independent clients should feature-detect them. Any incompatible contract or
semantic change increments the integer to `2`, then `3`, and so on.

`GetStatus` also reports a compatibility fingerprint, required-capability set,
daemon version, and build ID. The fingerprint is generated from zlink's actual
interface definition plus the semantic capability set. It identifies an exact
contract build and is stricter than the API-major compatibility promise. The
packaged CLI validates the API version, fingerprint, and required capabilities
before every daemon-backed operation because the two binaries are shipped and
upgraded together.

Compatibility fingerprints use `wireview-API-DIGEST`, for example
`wireview-2-0123456789abcdef`. The API component is the same plain decimal
integer reported by `api_version`; it is not prefixed with `v`. The textual
`org.varlink.service.GetInfo.version` value is the daemon package version and
is separate from the WireView API integer.

## Methods

| Method | Result |
|---|---|
| `GetStatus` | API compatibility/build identity, connection/recovery details, candidates, sequence, session, port, poll interval, and display-pause ownership |
| `GetDeviceInfo` | UID, EF05 identity, firmware/config versions, product/build strings, and capabilities |
| `GetTelemetry` | Current normalized measurements plus lossless raw, unknown, and decoded fault masks |
| `GetConfiguration` | Complete active editable settings plus a session-bound revision |
| `GetConfigurationItem` | One dotted-path setting as a typed JSON value |
| `SetConfigurationItem` | Atomically changes one dotted-path setting, temporarily or permanently |
| `ApplyConfiguration` | Temporarily applies a complete configuration |
| `StoreConfiguration` | Applies and permanently stores a complete configuration |
| `ReloadConfiguration` | Discards temporary changes and reloads stored settings |
| `ResetConfiguration` | Permanently replaces stored configuration with firmware defaults |
| `RebootDevice` | Reboots the device controller without erasing stored configuration |
| `SetScreen` | Selects one verified volatile display mode |
| `BeginHistoryDump` | Opens one session-bound display-pause lease for the 8 MiB device log |
| `ReadHistoryDumpChunk` | Reads one bounded chunk belonging to that dump/session |
| `EndHistoryDump` | Resumes display updates and closes the dump lease |
| `ReadHistoryChunk` | Development compatibility method; pauses/resumes around one chunk |
| `ReadThemeAsset` | Reads one exact fixed RGB565 slot with geometry, length, and SHA-256 |
| `WriteThemeAsset` | Guarded sector-preserving replacement with verified readback and SHA-256 |
| `ClearFaults` | Selectively clears validated active/logged fault masks |
| `GetPollInterval` / `SetPollInterval` | Reads or changes the runtime daemon poll interval (`100..=5000` ms) |
| `PauseDisplay` / `ResumeDisplay` | Starts or ends a bounded debug display-pause lease without overriding a history-dump lease |
| `Monitor` | Streams connection, disconnection, telemetry, and screen events |

Configuration mutation methods are protected by daemon-side validation.
Malformed JSON, unknown or missing fields, wrong JSON types, invalid enums,
out-of-range or non-representable numbers, duplicates, cross-field conflicts,
and settings unsupported by the connected configuration version are rejected
before any serial write. Clients must not rely on their own validation for
hardware safety. Complete apply/store also compares the opaque `revision`; a
stale revision or reconnect causes zero hardware writes. `raw_version` and the
device CRC remain internal.

If a configuration write, readback, or NVM store fails while still connected,
the daemon rewrites and verifies the previous active configuration and, when
needed, the previous saved configuration. Errors distinguish successful
rollback, rollback failure, and a disconnect with unknown outcome.
Persistent NVM mutations are serialized and spaced by at least one second.
Safety rollback is allowed to write immediately because restoring the previous
saved state takes priority over normal client pacing.

`GetConfigurationItem` accepts a dotted `key` and returns that key with its
value encoded in `value_json`. Decoding `value_json` preserves the setting's
actual JSON type: number, boolean, string, or array.

`SetConfigurationItem` accepts a dotted `key`, a string `value`, and a
`persist` and `confirm` booleans. The daemon reads the active configuration, parses the value
according to the existing leaf type, validates the complete resulting
configuration, and only then writes it. List values use comma-separated enum
names; `none` or an empty value clears a list. Its result contains only the
changed key and typed `value_json`, not the complete configuration.

`StoreConfiguration`, persistent `SetConfigurationItem`,
`ResetConfiguration`, `RebootDevice`, `ClearFaults`, and `WriteThemeAsset`
require `confirm=true`
at the Varlink boundary. This is an intent check, not user authentication.

Theme methods accept only one of the eight named slots; they never accept a
flash address. `WriteThemeAsset` also requires the exact slot byte length, a
matching lowercase SHA-256 digest, and exact byte data before any serial
mutation. It preserves complete affected sectors and verifies full readback.
Ordinary failures attempt verified rollback; connection loss returns
`OperationOutcomeUnknown` only after mutation may have started. A disconnect
during pause or sector backup returns an ordinary connection-loss error.
Display-resume cleanup is reflected in daemon connection/pause state but does
not replace a verified write or rollback result. Theme methods are supported
only by configuration V3 (raw version 2).

Display colors in `configuration_json` use either six hexadecimal `RRGGBB`
characters or eight `AARRGGBB` characters. Six-digit colors are converted to
opaque hardware colors by adding `FF`; eight-digit colors preserve their alpha
channel. Do not send `#`, `0x`, spaces, other lengths, or JSON numbers. Input
is case-insensitive. Output is uppercase and uses six digits when alpha is
`FF`, otherwise all eight ARGB digits.

`Monitor` uses Varlink's multi-reply protocol. Each event includes its daemon
sequence and hardware session so clients can detect gaps and reconnects.

`PauseDisplay` accepts a lease from 100 through 300000 milliseconds. The daemon
automatically resumes updates at expiry and tracks debug/history ownership
separately. `ResumeDisplay` ends only the debug lease; it never resumes an
active history dump.

## Authorization

The Unix socket accepts connections from members of `wireview-client`. Members
may invoke every exposed method, including configuration writes. The `wireview`
CLI maps `--yes` to the daemon's explicit confirmation field for persistent
store, configuration reset, fault clearing, device reboot, and theme
replacement. The daemon remains the only process that opens the USB device;
its separate `wireview` group is not granted to clients. There is no polkit
prompt.

Add an interactive client user with:

```bash
sudo usermod --append --groups wireview-client "$USER"
```

Log out and back in before connecting so the new group membership takes
effect.

The CLI exposes `ResetConfiguration` through both `wireview config reset --yes`
and the recovery-oriented alias
`wireview debug factory-reset --yes`.

## Errors

The interface defines `Unavailable`, `InvalidArgument`, `RevisionConflict`,
`Busy`, `OperationOutcomeUnknown`, `VerificationFailed`, `RollbackFailed`,
`FailedAndRolledBack`, `Unsupported`, and a fallback `DeviceError`. Transport failures remain
distinct zlink connection errors. The CLI prints the human message without
Rust debug formatting.
