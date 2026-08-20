# Quality verification policy

- Status: accepted
- Date: 2026-08-20

## Context

M4 grew Mush into a useful but substantially larger vertical slice before all of its intended quality controls were active. The existing behavioral suite measured 80.12% line coverage under `cargo llvm-cov --all-targets --summary-only`. A branch-instrumented run on the selected nightly measured 71.84% branch coverage. CodeScene and dependency policy were documented targets but had no executable configuration, and the deliberately failing merge gate could expose their absence without providing merge confidence.

The goal of this decision is to establish an honest baseline from the repository that exists, stop it from eroding, and make improvement visible. Dogtag provides transferable examples for fail-closed tools, credential handling, and repository protection, but its thresholds, kernel boundaries, platform matrix, and release machinery answer different product constraints and are not adopted here.

## Decision

### Coverage

Mush measures all Rust targets with cargo-llvm-cov 0.8.7 on `nightly-2026-07-30`. Nightly is required because Rust branch instrumentation remains unstable. The report includes every compiled production Rust file and the behavior exercised by unit, integration, binary, and other supported test targets. Test source is not counted as production coverage by LLVM's report, and the checker fails if any Rust file under the declared production paths is absent from the report. `src/store.rs` is declared non-executable because it contains module declarations, imports, data structures, and constants but no instrumentable lines; the checker fails if that file disappears or begins producing coverage while still declared.

The pre-remediation measurement was 80.12% line and 71.84% branch. Structural remediation and the first focused tests moved production-only coverage to 84.38192873323362% line and 74.27184466019418% branch after unit-test source was correctly externalized from the production denominator. Those measurements expose a real remaining gap rather than defining an acceptable bar.

The permanent floors are 90% line and 80% branch. They are fixed quality requirements approved after inspecting the measured repository: high enough to require meaningful tests of CLI outcomes, error semantics, runner decisions, and deterministic TUI logic, while leaving isolated platform and process-failure arms outside the contract when they cannot be induced honestly. Once both floors are met, the integrated measurements floored to hundredths become ratchets. A ratchet may rise with a clean measurement and may not fall merely to admit a change.

Repeated final runs of the completed focused suite measured production code between 90.9263357572% and 91.0211824218% line coverage (2,876–2,879 of 3,163 lines) and consistently measured 83.7378640777% branch coverage (345 of 412 branches). Three CLI lines vary with the process environment exercised by the all-target test run, so the enforced line ratchet uses the lower reproduced result. The ratchets floor that result to hundredths rather than rounding upward: 90.92% line and 83.73% branch.

A ratchet may rise whenever a clean measurement supports it. It may not fall merely to admit a change; a reduction requires an amended decision explaining why the denominator changed or why the previous measurement was not reproducible. The checker fails if branch instrumentation produces zero branches, so a stable-toolchain line-only run cannot masquerade as complete coverage.

### Code health

Every tracked or newly added Rust file under `src/` is supported production code and must receive CodeScene Code Health 10.0 under stock rules. There is no custom rules file, exclusion list, or reduced threshold. A file containing no scorable code may receive a null score, but a sweep in which no file is scored fails because it measured nothing.

The CodeScene CLI is pinned to 1.0.39. It requires `CS_ACCESS_TOKEN` and a network connection. A missing CLI, JSON parser, credential, failed review, or non-10.0 score fails the local gate with a diagnostic. CodeScene is not a required pull-request context: GitHub does not expose repository secrets to untrusted fork workflows, so requiring a token would either reject every fork contribution or create a path that passes without measurement. The maintainer's successful local `just gate` is therefore the authoritative CodeScene enforcement boundary.

The activation sweep measured 17 production files at 10.0 under stock rules. `src/store.rs` returned a null score because the facade contains declarations and no scorable functions; the gate reports that separately from scored files rather than counting it as a 10.0.

### Dependency policy

cargo-deny 0.20.2 enforces advisories, licenses, bans, and sources against the locked graph. Yanked dependencies are denied, and unmaintained and unsound advisories apply to the full graph. Wildcard requirements, unknown registries, and git sources are denied. crates.io remains the only source unless a later decision explicitly reviews another source.

The license allowlist reflects licenses cargo-deny encounters in the resolved graph rather than licenses present only in unused lockfile entries. Apache-2.0, MIT, and Zlib are allowed. Unicode-3.0 is scoped to `unicode-ident`, whose expression requires it in addition to MIT or Apache-2.0. Adding a license or widening that exception is a policy change, not a way to silence a scan.

Duplicate dependency versions are warnings rather than errors. The initial graph contains upstream-selected duplicate major lines of `hashbrown` and `syn`; forbidding them would require broad transitive exceptions or dependency choices driven by graph cosmetics rather than Mush's responsibilities. Warnings keep the convergence opportunity visible.

Any advisory ignore must have a matching entry in `security-exceptions.toml` with its identifier, tool, rationale, owner, expiration date, and a repository decision record. The repository check rejects unregistered, orphaned, expired, malformed, or unresolvable exceptions. cargo-deny has no native expiry mechanism, so this registry is the enforcement mechanism rather than supplementary prose.

### Gate, automation, and repository protection

`just gate` and `just gate-verbose` invoke the same ordered commands from one runner. Both run repository checks, coverage, dependency policy, and the complete CodeScene sweep, continue through independent failures to preserve evidence, and return nonzero if any requirement cannot run or fails. Verbose mode changes only whether successful tool output is rendered.

Pull requests and pushes to `main` run clean-checkout repository checks and coverage in `.github/workflows/ci.yml`. The full dependency policy runs at the same change boundary in `.github/workflows/security.yml`; that workflow refreshes only the advisory check daily because a new RustSec advisory or yanked release can change its answer without a repository change. Scheduled results complement rather than replace pull-request enforcement.

The protected-main payload requires the exact contexts `Repository checks`, `Coverage`, and `Dependency policy`, requires pull requests and an up-to-date branch, requires linear history, and prevents force pushes and deletion. It requires zero approving reviews and no last-push approval because a solo maintainer cannot approve their own pull request. Rebase and squash are both allowed because Mush does not currently make commit messages a verified product input. The payload has no bypass actors.

The checked-in payload is the reviewed provisioning input, not proof that GitHub is already enforcing it. Before applying it, the maintainer must observe the three context names on a green pull request, then post the payload through the repository administration API and read it back. A required context with the wrong display name can block every merge.

## Consequences

The local gate is meaningful but deliberately requires a maintainer CodeScene credential. External contributors and fork CI receive every deterministic and credential-free check without gaining access to that credential, and their work still needs a maintainer's local full gate before merge.

The repository-wide floors are a merge boundary, not a claim that 90% line or 80% branch coverage is sufficient for every component. Tests remain responsible for meaningful outcomes, and per-file measurements guide the M4 remediation without creating arbitrary per-file thresholds.

This decision creates no release workflow, release artifact, SBOM, provenance, immutable tag policy, or release ruleset. Those remain M5 work.

The responsibility remediation introduced named request and ownership types for Store operations that previously accepted long positional argument lists. Rust cannot preserve both inherent signatures through overloading, and wrapper or macro workarounds would retain the CodeScene finding or misrepresent source compatibility. Mush is pre-alpha, so this decision accepts the Rust library signature break. The headless CLI, TUI, persistence model, state transitions, and runtime outcomes remain behaviorally compatible.
