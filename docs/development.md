# Development and releases

## Build and test

Rust 1.97.1 is pinned in `rust-toolchain.toml`. Install `pkg-config`, libudev
development headers, Fontconfig development headers, `desktop-file-utils`, and
Xvfb. The desktop backend loads Wayland or X11 at runtime. Then run:

```bash
cargo build --release --workspace --bins --locked
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --locked
cargo deny check
bash scripts/smoke-varlink.sh
bash scripts/smoke-gui.sh
bash scripts/validate-packaging.sh
```

`smoke-gui.sh` renders every page at the default and minimum supported window
sizes in deterministic demo mode under Xvfb, then connects the desktop client
to the real daemon process running its mock backend. The test proves startup,
responsive layout, and event-loop stability without mutating hardware. Use the
attended release checklist for physical-device qualification.

Check a Slint source change before compiling Rust with:

```bash
slint-viewer --check crates/wireview-gui/ui/app.slint
```

Regenerate the README screenshots from the same deterministic demo data used by
the application:

```bash
bash scripts/capture-screenshots.sh
```

The screenshot command uses Slint's offscreen software renderer and ImageMagick.
It does not require Wayland, X11, or a connected device.

## Packages

Install the native builders you need (`dpkg-deb`, `rpmbuild`, `makepkg`, and
`pacman`), then build every format or one selected format:

```bash
bash scripts/build-packages.sh
bash scripts/build-packages.sh deb
bash scripts/build-packages.sh rpm
bash scripts/build-packages.sh arch
```

Artifacts, SHA-256 checksums, and an SPDX 2.3 SBOM are written to `dist/`.
Package CI installs a synthetic prior package, upgrades it to the candidate,
launches every installed GUI page under Xvfb, and tests removal for Debian,
RPM, and Arch. Ubuntu additionally tests service restart and socket activation
with the daemon's mock backend.

Release binaries use `git-SHORT_SHA-yyyyMMddHHmmss` from a clean Git checkout.
Source-only or intentionally dirty builds use
`source-SHORT_SHA-yyyyMMddHHmmss`. Dates are UTC and derive from
`SOURCE_DATE_EPOCH`. Set `WIREVIEW_ALLOW_DIRTY=1` for an intentional dirty
build, or provide a portable 1-64 character `WIREVIEW_BUILD_ID`.

Changing `workspace.package.version` in `Cargo.toml` on `main` automatically
creates the matching `v<version>` tag and runs the package workflow from that
tag. Manually created tags remain supported and must equal `v` plus the
workspace version.

Every release includes the Debian, RPM, and Arch packages, SHA-256 checksums,
an SPDX SBOM, a Sigstore bundle, provenance and SBOM attestations, and a
visible commit-by-commit changelog since the previous release tag. GitHub's
generated pull-request and contributor notes are appended to that changelog.
CI also cross-checks the build identity across all package formats before
publishing.

## Verify a release

```bash
cosign verify-blob \
  --bundle SHA256SUMS.sigstore.json \
  --certificate-identity-regexp \
  '^https://github.com/Gustav0ar/wireview-pro-ii/.github/workflows/packages.yml@refs/tags/v' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  SHA256SUMS
sha256sum --check SHA256SUMS
gh attestation verify ./wireviewd_PACKAGE -R Gustav0ar/wireview-pro-ii
gh attestation verify ./wireviewd_PACKAGE \
  -R Gustav0ar/wireview-pro-ii \
  --predicate-type https://spdx.dev/Document/v2.3
```

## Fuzzing

CI pins the nightly toolchain and `cargo-fuzz` version. Local equivalents are:

```bash
cargo install cargo-fuzz --version 0.13.2 --locked
cargo +nightly-2026-07-30 fuzz run configuration
cargo +nightly-2026-07-30 fuzz run history
cargo +nightly-2026-07-30 fuzz run protocol
```

## Hardware qualification

```bash
WIREVIEW_HIL=1 bash scripts/smoke-hardware.sh
WIREVIEW_RELEASE_HIL=1 bash scripts/qualify-release.sh
bash scripts/soak-test.sh
```

The soak defaults to 24 hours and one sample per minute. It records telemetry,
daemon CPU/RSS, reconnects, failures, and publisher lag under `target/`.
Ctrl+C preserves evidence but does not constitute a passing release soak.
`docs/release-qualification.md` documents the attended systemd,
configuration, physical-removal, and VM-transfer gates.

Configuration mutation requires the explicit
`WIREVIEW_HIL_CONFIG_MUTATION=1` opt-in. Persistent store and factory reset are
tested by the mock integration suite and are not part of the ordinary hardware
smoke test.

`WIREVIEW_HIL_THEME_MUTATION=1` plus an explicit named
`WIREVIEW_HIL_THEME_SLOT` performs an attended read/write/read comparison using
the exact existing bytes. It intentionally remains opt-in because it erases
and rewrites flash. Firmware update is not part of this gate.
