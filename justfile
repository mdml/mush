set shell := ["bash", "-euo", "pipefail", "-c"]

# Fast, deterministic feedback for local development.
fast:
    cargo fmt --all -- --check
    cargo check --all-targets
    cargo clippy --all-targets -- -D warnings
    cargo test --all-targets

# The strongest currently activated local handoff check.
check: fast
    RUSTDOCFLAGS="-D warnings" cargo doc --no-deps
    scripts/check-repository-policy.sh

# Merge-confidence gate. This intentionally fails while required M1 gate integrations remain unconfigured.
gate: check
    scripts/check-merge-requirements.sh

# The same requirements as `gate`, with commands and complete test output visible.
gate-verbose:
    just --verbose check
    MUSH_GATE_VERBOSE=1 scripts/check-merge-requirements.sh

# Install the checked-in Git hooks after the pinned tools are available.
hooks-install:
    lefthook install
