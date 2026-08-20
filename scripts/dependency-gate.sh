#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

command -v cargo-deny >/dev/null 2>&1 || {
  echo "dependencies: cargo-deny 0.20.2 is required; install it with 'cargo install cargo-deny --version 0.20.2 --locked'" >&2
  exit 1
}
actual_version="$(cargo deny --version | awk '{print $2}')"
if [[ "$actual_version" != "0.20.2" ]]; then
  echo "dependencies: cargo-deny 0.20.2 is required (found $actual_version)" >&2
  exit 1
fi

python3 scripts/check-security-exceptions.py
cargo deny --locked check
