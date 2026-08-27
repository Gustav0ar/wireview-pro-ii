# Packaging

Each package format installs one bundled payload:

| File | Purpose |
|---|---|
| `/usr/bin/wireviewd` | system daemon |
| `/usr/bin/wireview` | Varlink command-line client |
| `/usr/bin/wireview-gui` | native Slint desktop client |
| `/usr/lib/systemd/system/wireviewd.service` | hardened systemd service |
| `/usr/lib/systemd/system/wireviewd.socket` | socket activation and group-restricted Varlink endpoint |
| `/usr/lib/sysusers.d/wireview.conf` | daemon identity plus separate USB and client groups |
| `/usr/lib/udev/rules.d/70-wireview-pro-ii.rules` | restricted USB tty access |
| `/usr/share/varlink/interfaces/io.github.Gustav0ar.WireView.varlink` | standalone Varlink API contract |
| `/usr/share/applications/io.github.Gustav0ar.WireView.desktop` | desktop launcher |
| `/usr/share/icons/hicolor/scalable/apps/io.github.Gustav0ar.WireView.svg` | desktop icon |
| `/usr/share/doc/wireviewd/` | user and protocol documentation |
| `/usr/share/licenses/wireviewd/LICENSE` | MIT license |

Run `bash scripts/build-packages.sh` to create Debian, RPM, and Arch packages
under `dist/`. Pass `deb`, `rpm`, or `arch` to build selected formats. The
command requires Cargo and Python 3. The directory also receives `SHA256SUMS`
and a deterministic SPDX 2.3 dependency SBOM.

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

The socket permits members of `wireview-client` to invoke every supported
Varlink method, including validated writes. The separate `wireview` group
controls only the dedicated daemon account's direct USB access. Add client users
with `usermod --append --groups wireview-client USER`; their next login receives
the new membership. Package removal intentionally leaves package-owned users and
groups in place, as recommended for stable identities.

Every package installs the daemon, the CLI, and the desktop app together. Both
clients require Varlink API 2 and check compatibility before device access. The
package workflow installs a synthetic prior version, upgrades it to the
candidate, launches every installed GUI page under Xvfb, exercises the bundled
mock daemon and CLI, and removes it for every native format. Ubuntu also tests
mock-backed systemd restart and socket activation. Tagged releases additionally
attest package provenance and the SPDX SBOM through GitHub's OIDC/Sigstore-backed
attestation service.
