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
    python3 scripts/check-security-exceptions.py
    python3 scripts/check-ruleset-payload.py
    python3 scripts/test-coverage.py
    python3 scripts/test-gate.py

# Complete local merge-confidence gate. CodeScene requires CS_ACCESS_TOKEN.
gate:
    python3 scripts/gate.py

# The same commands and requirements as `gate`, with complete output visible.
gate-verbose:
    python3 scripts/gate.py --verbose

# Install the checked-in Git hooks after the pinned tools are available.
hooks-install:
    lefthook install
