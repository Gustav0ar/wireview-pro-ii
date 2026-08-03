# Contributing

Contributions should preserve the project's safety boundary: configuration
writes must remain constrained to the recovered versioned layouts and NVM
operations. Undocumented calibration, arbitrary flash operations, bootloader
commands, and firmware updates are not accepted without protocol evidence,
recovery documentation, and hardware-in-the-loop fault testing.

Theme changes must remain restricted to the enumerated V3 RGB565 slots. They
must preserve full shared sectors, verify complete readback, attempt verified
rollback after an ordinary failure, and never retry a mutation or continue
after connection loss. Public APIs must never accept a flash address or length.

Before submitting a change, run:

```bash
cargo fmt --all --check
cargo check --all-targets --locked
cargo clippy --all-targets --all-features --locked -- -D warnings
cargo test --all-targets --locked
cargo deny check
bash scripts/smoke-varlink.sh
bash scripts/validate-packaging.sh
cargo check --manifest-path fuzz/Cargo.toml
```

Parser changes should also run the relevant target with nightly Rust and
`cargo-fuzz`. The scheduled workflow runs bounded configuration, history, and
protocol-parser campaigns. Release candidates additionally run
`scripts/soak-test.sh`; its default 24-hour run captures telemetry freshness,
CPU, RSS, reconnects, failures, and event-publisher lag under `target/`.

Before publishing a hardware-qualified package, follow
`docs/release-qualification.md`. The runner is opt-in and preserves its
evidence under `target/`.

Protocol changes should include focused decoder fixtures or deterministic backend
tests. Lifecycle changes should cover disconnects, serial errors, duplicate
devices, and session transitions. Never include vendor firmware, serial
captures containing private identifiers, or decompiled vendor binaries in a
commit.
