#!/usr/bin/env bash
# Headless/offline electrical-library gate, separate from the AVR parity gate.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
for crate in analog-solver circuit-components; do
  (
    cd "$root/$crate"
    cargo fmt --check
    cargo clippy --all-targets --locked --offline -- -D warnings
    cargo test --locked --offline
    cargo test --release --locked --offline
  )
done
cargo run --manifest-path "$root/circuit-components/Cargo.toml" --example blink --locked --offline
cargo run --manifest-path "$root/circuit-components/Cargo.toml" --example blink --locked --offline -- --reverse
