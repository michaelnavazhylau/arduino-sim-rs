#!/usr/bin/env bash
# Headless gate for the breadboard host, scheduler and external components.
# Separate from the AVR parity gate and the analog-library gate: this crate is
# allowed to depend on the AVR core, but the core must not depend on it.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo fmt --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --locked --offline
cargo test --release --locked --offline
