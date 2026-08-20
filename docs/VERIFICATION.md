# Verification

Mush coordinates work and review, so its own state transitions and evidence must be trustworthy. This document defines the correctness and quality contract. The repository provides the local command ladder and hooks described below. Coverage enforcement, CodeScene health, dependency policy, and CI are not yet activated and must not be reported as passing.

## Standards

The implementation will require:

- tests for observable behavior and every task-state transition;
- regression tests for fixed defects;
- coverage measurement with an enforced ratchet;
- Rust formatting, compilation, linting, and type checking;
- rustdoc checks with warnings denied;
- CodeScene code-health score 10 for supported production code;
- deterministic checks that fail when a required tool is missing instead of silently skipping it.

Rust compilation provides type checking. The gate should use `cargo check` for fast feedback and compile all test and supported feature targets before handoff.

Coverage thresholds are intentionally not guessed before code exists. Before coverage enforcement is activated, the project must record a decision defining the measurement tool, included and excluded code, initial line and branch baselines, minimum floors, and ratchet behavior. New or changed semantic code should not reduce coverage, and a rising baseline should not be lowered merely to admit a change.

CodeScene score 10.0 under stock rules is a target gate for every CodeScene-supported file. When the integration is scaffolded, the project must document credential behavior and explicitly decide how genuinely unsupported file classes are handled; ordinary exclusions and custom rules are not escape hatches. A missing credential may make a remote advisory check unavailable, but it must not be presented as a successful required gate.

## Command ladder

The following recipes are checked in:

| Command | Purpose | Contract |
| --- | --- | --- |
| `just fast` | Inner development loop | Formatting check, `cargo check`, Clippy with warnings denied, and fast unit or targeted tests. |
| `just check` | Local handoff | `fast`, full tests for supported targets, rustdoc with warnings denied, and repository policy/document checks. |
| `just gate` | Merge confidence | `check`, coverage enforcement, CodeScene health, dependency security and license checks, and any slow integration tests required by the active architecture. |
| `just gate-verbose` | Diagnosis and evidence | The same requirements as `gate`, with full tool output and retained diagnostic artifacts where useful. |

`gate-verbose` must not be weaker than `gate`. Both must return a nonzero status when a required check fails or cannot run.

`just fast` and `just check` are active. `just gate` and `just gate-verbose` run the active checks and then fail explicitly while coverage enforcement, CodeScene health, and dependency security and license policy remain unavailable. This makes the missing merge requirements visible without weakening the gate.

## Test strategy

Unit tests should cover pure domain rules and state-transition decisions. Store tests should exercise migrations, transactions, constraints, restart persistence, and idempotency against temporary SQLite databases. CLI tests should assert machine-readable behavior and exit status. TUI logic should be separated from terminal I/O so navigation and rendered state can be tested deterministically. Harness and project boundaries should use fakes for most tests, with focused integration tests for real process and filesystem behavior.

Tests should emphasize externally meaningful outcomes rather than mirroring implementation structure. Concurrency, crash recovery, and adapter failure tests should be added with the work that introduces those risks, not speculatively.

## Hooks and automation

The hook policy separates fast feedback from slow confidence:

- pre-commit runs `just fast`;
- pre-push runs `just gate`;
- CI will independently run the deterministic merge gate on a clean checkout once CI and every required merge check are activated;
- commit-message validation follows the repository's eventual documented convention, if one is adopted.

Hooks are convenience and early feedback, not substitutes for CI. The checked-in Lefthook configuration makes local behavior inspectable and reproducible.

Credential-dependent checks require an explicit policy before CI activation. If CodeScene cannot run honestly in every CI context, the documentation must distinguish the authoritative maintainer gate from CI checks rather than marking a skipped check successful.

## Evidence

A completed change should report the strongest command actually run and any relevant focused tests. If the full gate was not run, say so and explain why. Never summarize a failed, skipped, or unavailable check as passing.

Verification output should remain concise by default and become diagnostic through `just gate-verbose`. Where an external artifact is needed for review, retain it in an inspectable location and reference it rather than pasting unbounded logs into task evidence.
