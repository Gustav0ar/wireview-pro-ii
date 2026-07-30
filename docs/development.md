# Development and releases

## Build and test

Rust 1.97.1 is pinned in `rust-toolchain.toml`. Install `pkg-config` and the
libudev development headers, then run:

```bash
cargo build --release --locked
cargo fmt --all --check
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo deny check
bash scripts/smoke-varlink.sh
bash scripts/validate-packaging.sh
```

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
and tests removal for Debian, RPM, and Arch. Ubuntu additionally tests service
restart and socket activation with the daemon's mock backend.

Release binaries use `git-SHORT_SHA-yyyyMMddHHmmss` from a clean Git checkout.
Source-only or intentionally dirty builds use
`source-SHORT_SHA-yyyyMMddHHmmss`. Dates are UTC and derive from
`SOURCE_DATE_EPOCH`. Set `WIREVIEW_ALLOW_DIRTY=1` for an intentional dirty
build, or provide a portable 1–64 character `WIREVIEW_BUILD_ID`.

Tagged releases require the tag to equal `v` plus the Cargo version. CI
cross-checks the build identity across package formats, publishes checksums and
an SPDX SBOM, signs the checksum manifest with Sigstore, and publishes GitHub
provenance and SBOM attestations.

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
