# wireviewd

`wireviewd` is a Linux daemon and command-line client for the Thermal Grizzly
WireView Pro II. It provides live telemetry, device history, fault inspection,
screen control, and validated configuration through a local Varlink API.

The daemon owns all USB access, handles physical removal, re-enumeration, and
VM passthrough, and exposes the device to local clients through systemd socket
activation. Calibration, arbitrary flash operations, and firmware updates are
deliberately not exposed.

This is an independent community project and is not affiliated with Thermal
Grizzly.

## Features

- Human-readable and JSON live telemetry, including an in-place watch mode
- Table, CSV, JSON, and exact-raw device-history export
- Exact RGB565 backup and guarded, verified replacement of eight named theme
  asset slots
- Complete validated configuration read, temporary apply, permanent store,
  reload, and factory reset
- Individual configuration get/set commands
- Screen selection, fault inspection, and selective fault clearing
- Device identity, firmware, build, and capability reporting
- Automatic cleanup and recovery across disconnects and abandoned clients
- Debian/Ubuntu, Fedora, and Arch packages with a hardened systemd service

## Install

Install the package for your distribution:

```bash
# Debian or Ubuntu
sudo apt install ./wireviewd_1.1.0-1_amd64.deb

# Fedora
sudo dnf install ./wireviewd-1.1.0-1*.x86_64.rpm

# Arch Linux
sudo pacman -U ./wireviewd-1.1.0-1-x86_64.pkg.tar.zst
```

Enable socket activation:

```bash
sudo systemctl enable --now wireviewd.socket
sudo usermod --append --groups wireview-client "$USER"
```

Log out and back in after joining `wireview-client`. Membership authorizes CLI
access, including validated device writes, without granting direct USB access.
The current package creates `wireview-client` during installation. If
`usermod` reports that the group does not exist, install or upgrade the package
before running it; an older or manually staged installation is still active.

Every package installs `/usr/bin/wireview`, `/usr/bin/wireviewd`, a
`wireview(1)` manual page, shell completions, the Varlink IDL, and service
files. Members of `wireview-client` may use the CLI; the separate `wireview`
group is reserved for the daemon's direct USB access.

## Quick start

```bash
wireview status
wireview info
wireview telemetry
wireview telemetry --watch
wireview history --format csv --output history.csv
wireview theme read fan-dark-1 --output fan-dark-1.raw
wireview theme write fan-dark-1 fan-dark-1.raw --yes
wireview faults
wireview config show
wireview config get fan.mode
wireview config set fan.mode fixed
wireview screen help
```

Use `wireview --help` and `wireview COMMAND --help` for the command reference.
Permanent configuration changes and destructive recovery actions require an
explicit `--yes`.

## Documentation

- [CLI usage and complete configuration reference](docs/usage.md)
- [Service operation, USB/VM handling, and troubleshooting](docs/operations.md)
- [Hardware and firmware compatibility](docs/compatibility.md)
- [Varlink API and stability policy](docs/varlink.md)
- [Recovered USB protocol](docs/protocol.md)
- [Building, testing, packaging, and releases](docs/development.md)
- [Attended release qualification](docs/release-qualification.md)
- [Package payload and hooks](packaging/README.md)

## Build

The project uses Rust 1.97.1, pinned in `rust-toolchain.toml`, plus `pkg-config`
and libudev development headers.

```bash
cargo build --release --locked
cargo test --all-targets --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
```

Build installable packages with:

```bash
bash scripts/build-packages.sh
```

See [development and releases](docs/development.md) for native package
dependencies, verification, fuzzing, build identities, and hardware tests.

## License

MIT
