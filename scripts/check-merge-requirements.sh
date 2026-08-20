#!/usr/bin/env bash
set -euo pipefail

missing=()

if ! cargo llvm-cov --version >/dev/null 2>&1; then
  missing+=("cargo-llvm-cov is not installed")
fi

if ! cargo deny --version >/dev/null 2>&1; then
  missing+=("cargo-deny is not installed")
fi

# docs/VERIFICATION.md requires decisions about both integrations before they can be gates.
missing+=("coverage enforcement is not activated: no measurement and ratchet decision is recorded")
missing+=("CodeScene health is not activated: no CLI, credential policy, or supported-file policy is configured")
missing+=("dependency security and license checks are not activated: no cargo-deny policy is configured")

if [[ ${MUSH_GATE_VERBOSE:-0} == 1 ]]; then
  printf 'merge gate requirement unavailable: %s\n' "${missing[@]}" >&2
else
  echo "merge gate is not yet available; run 'just gate-verbose' for the inactive requirements" >&2
fi

exit 1
