# Verification

Mush coordinates work and review, so its own state transitions and evidence must be trustworthy. This document defines the correctness and quality contract. The repository provides the local command ladder, hooks, coverage ratchet, code-health gate, dependency policy, and clean-checkout automation described below.

## Standards

The implementation requires:

- tests for observable behavior and every task-state transition;
- regression tests for fixed defects;
- coverage measurement with an enforced ratchet;
- Rust formatting, compilation, linting, and type checking;
- rustdoc checks with warnings denied;
- CodeScene code-health score 10 for supported production code;
- deterministic checks that fail when a required tool is missing instead of silently skipping it.

Rust compilation provides type checking. The gate should use `cargo check` for fast feedback and compile all test and supported feature targets before handoff.

Coverage is measured across all Rust targets with cargo-llvm-cov 0.8.7 on `nightly-2026-07-30`. The required floors are 90% line and 80% branch; the current ratchets are 91.14% line and 84.13% branch. A rising ratchet is not lowered merely to admit a change, and a zero-branch report fails rather than passing as line-only coverage. The measurement must also be reproducible: coverage that depends on which process wins a race is a defect in the test rather than a property of the code, and a region reached only through an undetermined outcome needs a deterministic test of its own. The scope, exact measurement, ratchet behavior, and threshold rationale are recorded in the [quality verification decision](decisions/2026-08-20-quality-verification-policy.md) and `coverage-baseline.toml`.

Every Rust file under `src/` must score CodeScene Code Health 10.0 under stock rules. The local gate uses CodeScene CLI 1.0.39 and fails when the CLI, `jq`, `CS_ACCESS_TOKEN`, network measurement, or required score is unavailable. CodeScene remains local-only because a credential-dependent required check cannot run safely for fork pull requests; absence is a gate failure, never a pass or skip.

cargo-deny 0.20.2 enforces the locked Rust graph's advisory, license, ban, and source policy. Suppressions are structured, justified, owned, linked to a decision, and expiring. The full policy runs on pull requests, while its time-varying advisory check also runs daily because advisory and yank results can change without a repository change.

## Command ladder

The following recipes are checked in:

| Command | Purpose | Contract |
| --- | --- | --- |
| `just fast` | Inner development loop | Formatting check, `cargo check`, Clippy with warnings denied, and the full test suite for supported targets. |
| `just check` | Local handoff | `fast`, rustdoc with warnings denied, and repository policy/document checks. |
| `just gate` | Merge confidence | `check`, coverage enforcement, CodeScene health, dependency security and license checks, and any slow integration tests required by the active architecture. |
| `just gate-verbose` | Diagnosis and evidence | The same requirements as `gate`, with full tool output and retained diagnostic artifacts where useful. |

`gate-verbose` must not be weaker than `gate`. Both must return a nonzero status when a required check fails or cannot run.

All four commands are active. `gate` and `gate-verbose` invoke one gate definition and differ only in rendering. They continue through independent failures so one run identifies every unmet requirement, but either returns nonzero when any required check fails or cannot run.

## Test strategy

Unit tests should cover pure domain rules and state-transition decisions. Store tests should exercise migrations, transactions, constraints, restart persistence, and idempotency against temporary SQLite databases. CLI tests should assert machine-readable behavior and exit status. TUI logic should be separated from terminal I/O so navigation and rendered state can be tested deterministically. Harness and project boundaries should use fakes for most tests, with focused integration tests for real process and filesystem behavior.

Tests should emphasize externally meaningful outcomes rather than mirroring implementation structure. Concurrency, crash recovery, and adapter failure tests should be added with the work that introduces those risks, not speculatively.

## Hooks and automation

The hook policy separates fast feedback from slow confidence:

- pre-commit runs `just fast`;
- pre-push runs `just gate`;
- CI independently runs repository checks, coverage, and dependency policy on a clean checkout for every pull request;
- commit-message validation follows the repository's eventual documented convention, if one is adopted.

Hooks are convenience and early feedback, not substitutes for CI. The checked-in Lefthook configuration makes local behavior inspectable and reproducible.

The checked-in protected-main payload requires the exact contexts `Repository checks`, `Coverage`, and `Dependency policy`, plus pull requests, linear history, and an up-to-date branch. It prevents force pushes and deletion and requires no approving review, preserving a viable solo-maintainer merge path. CodeScene is the credential-dependent local exception and must be run successfully by the maintainer before merge.

## Evidence

A completed change should report the strongest command actually run and any relevant focused tests. If the full gate was not run, say so and explain why. Never summarize a failed, skipped, or unavailable check as passing.

Verification output should remain concise by default and become diagnostic through `just gate-verbose`. Where an external artifact is needed for review, retain it in an inspectable location and reference it rather than pasting unbounded logs into task evidence.
