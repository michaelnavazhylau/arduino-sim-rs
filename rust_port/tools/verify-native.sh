#!/usr/bin/env bash
# Fail-closed gate for the avr-sim engine and its own regressions. Node is not
# required and neither is the upstream submodule: the converted AVR8js contract
# is generated and verified in the avr8js-parity repository, whose gate rejects
# any converted scenario that is `#[ignore]`d.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

cargo fmt --check
cargo clippy --all-targets --locked --offline -- -D warnings
# Include ignored tests in both runs as defense in depth, rather than trusting a
# green default if one is ever added here.
cargo test --all-targets --locked --offline -- --include-ignored
cargo test --all-targets --release --locked --offline -- --include-ignored
