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

Package version 1.x exposes API version `1`. This field is the JSON integer
`1`, never the strings `"1"` or `"v1"`.

Within API 1, existing methods, fields, types, errors, and documented semantics
will not be removed, renamed, or incompatibly changed. New optional methods,
types, fields, errors, or capabilities may be added while retaining API 1;
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
`wireview-1-711c3eafcc2df520`. The API component is the same plain decimal
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
`ResetConfiguration`, `RebootDevice`, and `ClearFaults` require `confirm=true`
at the Varlink boundary. This is an intent check, not user authentication.

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

The Unix socket accepts local connections for all methods. All local callers
that can connect to the socket may invoke every exposed method, including
configuration writes. The `wireview` CLI maps `--yes` to the daemon's explicit
confirmation field for persistent store, configuration reset, fault clearing,
and device reboot. The daemon remains the only process that opens the USB
device. There is no `wireview` group membership requirement for clients and no
polkit prompt.

The CLI exposes `ResetConfiguration` through both
`wireview config reset --yes` and the recovery-oriented alias
`wireview debug factory-reset --yes`.

## Errors

The interface defines `Unavailable`, `InvalidArgument`, `RevisionConflict`,
`Busy`, `OperationOutcomeUnknown`, `VerificationFailed`, `RollbackFailed`,
`FailedAndRolledBack`, `Unsupported`, and a fallback `DeviceError`. Transport failures remain
distinct zlink connection errors. The CLI prints the human message without
Rust debug formatting.
