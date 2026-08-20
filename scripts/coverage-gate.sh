#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

command -v cargo-llvm-cov >/dev/null 2>&1 || {
  echo "coverage: cargo-llvm-cov 0.8.7 is required; install it with 'cargo install cargo-llvm-cov --version 0.8.7 --locked'" >&2
  exit 1
}

expected_version="$(sed -n 's/^cargo_llvm_cov = "\(.*\)"$/\1/p' coverage-baseline.toml)"
actual_version="$(cargo llvm-cov --version | awk '{print $2}')"
if [[ "$actual_version" != "$expected_version" ]]; then
  echo "coverage: cargo-llvm-cov $expected_version is required (found $actual_version)" >&2
  exit 1
fi

toolchain="$(sed -n 's/^toolchain = "\(.*\)"$/\1/p' coverage-baseline.toml)"
if ! rustup toolchain list | grep -q "^${toolchain}-"; then
  echo "coverage: Rust toolchain $toolchain is required; install it with 'rustup toolchain install $toolchain --component llvm-tools-preview'" >&2
  exit 1
fi

mkdir -p target/llvm-cov
rustup run "$toolchain" cargo llvm-cov --all-targets --branch --json --output-path target/llvm-cov/coverage.json
python3 scripts/check-coverage.py target/llvm-cov/coverage.json coverage-baseline.toml
