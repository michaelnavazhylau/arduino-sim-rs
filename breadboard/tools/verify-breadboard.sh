#!/usr/bin/env bash
# Headless gate for the breadboard host, scheduler, analog coupling and
# external components. Separate from the AVR engine gate: this crate is allowed
# to depend on the AVR core and on the ngspice-rs electrical backend, but the
# core must not depend on it.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# ngspice-rs and its numerical dependencies (faer, diffsol, petgraph, winnow)
# come from the package index, so this gate needs the registry populated. Fetch
# once -- a no-op when already cached -- and keep everything after it offline so
# the locked set is exactly the set under test.
cargo fetch --locked
cargo fmt --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --locked --offline
cargo test --release --locked --offline
