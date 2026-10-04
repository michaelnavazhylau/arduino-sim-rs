#!/usr/bin/env bash
# Fail-closed native parity gate. Node and vendor PDFs are not required.
set -euo pipefail
root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

# Catch accidental reintroduction of generated scenario gates. Include ignored
# tests in both runs as defense in depth, rather than trusting a green default.
test -d src/suites
if grep -Rn '^#\[ignore' src/suites; then
  echo 'Native parity requires every converted scenario to be enabled.' >&2
  exit 1
else
  status=$?
  if [ "$status" -ne 1 ]; then
    echo 'Could not inspect converted scenario gates.' >&2
    exit "$status"
  fi
fi
cargo fmt --check
cargo clippy --all-targets --locked --offline -- -D warnings
cargo test --all-targets --locked --offline -- --include-ignored
cargo test --all-targets --release --locked --offline -- --include-ignored
