# Contributing

Mush is a pre-alpha Rust application. The checked-in command ladder covers the current implementation and fails explicitly when a required tool, credential, or measurement is unavailable.

## Getting started

Clone the repository and read [`../AGENTS.md`](../AGENTS.md), [`AGENTS.md`](AGENTS.md), [`ARCHITECTURE.md`](ARCHITECTURE.md), [`ABSTRACTIONS.md`](ABSTRACTIONS.md), and [`ROADMAP.md`](ROADMAP.md).

Development requires the pinned Rust toolchain, `just`, ripgrep 14.1.1, and Python 3.11 or newer. The full gate additionally requires cargo-llvm-cov 0.8.7, `nightly-2026-07-30` with `llvm-tools-preview`, cargo-deny 0.20.2, CodeScene CLI 1.0.39, `jq`, network access, and `CS_ACCESS_TOKEN`. Lefthook is required to install the checked-in hooks with `just hooks-install`.

## Development workflow

Work against the active milestone and slice in [`current-milestone.md`](current-milestone.md). Prefer a small vertical slice that leaves observable behavior working over a broad layer of unfinished infrastructure. Do not implement abstractions assigned to later milestones merely because the eventual need seems likely.

Before changing a durable boundary, update the relevant design document or add a focused decision record. Keep prose soft-wrapped: one line per paragraph or bullet, without artificial column wrapping.

## Decision authority

Mush is a maintainer-led pre-alpha experiment. Contributions and design proposals are welcome, while the maintainer retains decision authority for the product hypothesis, user-visible and domain semantics, milestone outcomes, public interfaces, data-model and difficult-to-reverse architecture, acceptance of consequential decisions, and acceptance of milestone evidence. [`PRODUCT.md`](PRODUCT.md) owns the maintained hypothesis and full boundary.

Human and agent contributors should attack assumptions, present alternatives, and make recommendations. Within an accepted boundary they may independently choose local, reversible implementation details such as naming, internal file organization, test scaffolding, and refactors that preserve established interfaces.

When implementation exposes an unsettled product, domain, data-shape, public-interface, or difficult-to-reverse architectural choice, stop implementation and return a decision packet with the frame, a worked example, alternatives, a recommendation, and explicit refusals. Passing verification establishes implementation evidence; it does not accept that decision or close a milestone gate.

Each implementation slice should state its goal, product frame, one worked example, settled decisions, contributor discretion, refusals, acceptance evidence, and stop conditions. The active slice's brief lives in [`current-milestone.md`](current-milestone.md); do not create a parallel repository backlog or plan document.

The command ladder and its semantics live only in [`VERIFICATION.md`](VERIFICATION.md). Use `just fast` for local feedback, `just check` for handoff, and `just gate` before opening or merging a pull request. Use `just gate-verbose` for the same requirements with complete successful output.

## Commits and worktrees

Keep commits cohesive and explain why the change exists. Follow the user-level commit conventions configured on the machine; do not invent a project-specific commit format until the project has a demonstrated need for one.

Use an isolated Git worktree for concurrent agent work. Never overwrite unrelated edits in a dirty worktree. A handoff should identify the changed files, verification performed, and any remaining uncertainty.

## Documentation ownership

- `docs/AGENTS.md` defines documentation structure, invariants, and authoring workflow.
- `README.md` is the brief user-facing introduction.
- `AGENTS.md` orients coding agents and names load-bearing boundaries.
- `docs/ARCHITECTURE.md` owns components, file organization, dependency flow, and state management.
- `docs/PRODUCT.md` owns the product hypothesis, refusals, and maintainer decision authority.
- `docs/ABSTRACTIONS.md` owns durable domain concepts and invariants.
- `docs/ROADMAP.md` owns the milestone sequence, gates, operating rules, and deferred ideas.
- `docs/current-milestone.md` owns the active milestone, its active slice, and the slice brief.
- `docs/VERIFICATION.md` owns correctness and quality standards.
- `docs/guides/` owns operational guidance for adopted integrations.

Update the canonical document instead of repeating the same rule elsewhere. Documentation should describe the current system and the explicitly planned milestone, not preserve arguments from earlier design conversations.

## Dependencies

Add a dependency only when the active slice needs it. Pin toolchain and verification-tool versions so contributors and automation run the same checks. Record project-specific usage for a substantial vendor or integration in `docs/guides/` when the dependency is adopted; do not create empty guides in anticipation.

Dependency changes should include the relevant tests, verification, and documentation. Security, license, ban, and source checks are enforced by the cargo-deny policy. Any advisory suppression must be registered with a rationale, owner, decision link, and expiration date in `security-exceptions.toml`.
