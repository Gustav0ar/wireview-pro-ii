# Packaging

All package formats install the same payload:

| File | Purpose |
|---|---|
| `/usr/bin/wireviewd` | system daemon |
| `/usr/bin/wireview` | Varlink command-line client |
| `/usr/lib/systemd/system/wireviewd.service` | hardened systemd service |
| `/usr/lib/systemd/system/wireviewd.socket` | socket activation and all-local-user Varlink endpoint |
| `/usr/lib/sysusers.d/wireview.conf` | `wireviewd` user and `wireview` group |
| `/usr/lib/udev/rules.d/70-wireview-pro-ii.rules` | restricted USB tty access |
| `/usr/share/varlink/interfaces/io.github.Gustav0ar.WireView.varlink` | standalone Varlink API contract |
| `/usr/share/doc/wireviewd/` | user and protocol documentation |
| `/usr/share/licenses/wireviewd/LICENSE` | MIT license |

Run `bash scripts/build-packages.sh` to create Debian, RPM, and Arch packages
under `dist/`. Pass `deb`, `rpm`, or `arch` to build selected formats. The
directory also receives `SHA256SUMS` and a deterministic SPDX 2.3 dependency
SBOM when Cargo and Python 3 are available.

The build first compiles locked release binaries, creates a shared staged root,
and then invokes the native package builder:

| Format | Required tools |
|---|---|
| Debian/Ubuntu | `dpkg-deb` |
| Fedora/RPM | `rpmbuild`, `rpm` |
| Arch Linux | `makepkg`, `pacman` |

Package hooks create the system user, reload systemd and udev, and handle
service upgrades/removal. Debian and RPM installations start the activation
socket when systemd is running. Arch follows distribution policy and prints
the explicit `systemctl enable --now wireviewd.socket` command.

The socket permits all local users to invoke every supported Varlink method,
including validated writes. Client users do not join the `wireview` group. That
group controls only the dedicated daemon account's direct USB access. Package
removal intentionally leaves the system user and group in place, as recommended
for stable package-owned identities.

Every package installs the daemon and CLI together. The CLI requires Varlink
API 1 as an integer and checks it before every daemon-backed command. The
package workflow installs a synthetic prior version, upgrades it to the
candidate, executes it, and removes it for every native format. Ubuntu also
tests mock-backed systemd restart and socket activation. Tagged releases
additionally attest package provenance and the SPDX SBOM through GitHub's
OIDC/Sigstore-backed attestation service.
