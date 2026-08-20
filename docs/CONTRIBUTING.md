# Contributing

Mush is a pre-alpha Rust application. The checked-in command ladder covers the current implementation, while merge checks assigned to later setup decisions fail explicitly rather than being reported as passing.

## Getting started

Clone the repository and read [`../AGENTS.md`](../AGENTS.md), [`AGENTS.md`](AGENTS.md), [`ARCHITECTURE.md`](ARCHITECTURE.md), [`ABSTRACTIONS.md`](ABSTRACTIONS.md), and [`ROADMAP.md`](ROADMAP.md).

Development requires the pinned Rust toolchain and `just`. Lefthook is required to install the checked-in hooks with `just hooks-install`. Additional merge-gate tools and their policies remain deliberately inactive as documented in [`VERIFICATION.md`](VERIFICATION.md).

## Development workflow

Work against the active milestone in [`ROADMAP.md`](ROADMAP.md). Prefer a small vertical slice that leaves observable behavior working over a broad layer of unfinished infrastructure. Do not implement abstractions assigned to later milestones merely because the eventual need seems likely.

Before changing a durable boundary, update the relevant design document or add a focused decision record. Keep prose soft-wrapped: one line per paragraph or bullet, without artificial column wrapping.

The command ladder and its semantics live only in [`VERIFICATION.md`](VERIFICATION.md). Use `just fast` for local feedback and `just check` for handoff; `just gate` fails until every documented merge requirement is activated.

## Commits and worktrees

Keep commits cohesive and explain why the change exists. Follow the user-level commit conventions configured on the machine; do not invent a project-specific commit format until the project has a demonstrated need for one.

Use an isolated Git worktree for concurrent agent work. Never overwrite unrelated edits in a dirty worktree. A handoff should identify the changed files, verification performed, and any remaining uncertainty.

## Documentation ownership

- `docs/AGENTS.md` defines documentation structure, invariants, and authoring workflow.
- `README.md` is the brief user-facing introduction.
- `AGENTS.md` orients coding agents and names load-bearing boundaries.
- `docs/ARCHITECTURE.md` owns components, file organization, dependency flow, and state management.
- `docs/ABSTRACTIONS.md` owns durable domain concepts and invariants.
- `docs/ROADMAP.md` owns the active milestone, milestone sequence, gates, and deferred ideas.
- `docs/VERIFICATION.md` owns correctness and quality standards.
- `docs/guides/` owns operational guidance for adopted integrations.

Update the canonical document instead of repeating the same rule elsewhere. Documentation should describe the current system and the explicitly planned milestone, not preserve arguments from earlier design conversations.

## Dependencies

Add a dependency only when the active slice needs it. Pin toolchain and verification-tool versions so contributors and automation run the same checks. Record project-specific usage for a substantial vendor or integration in `docs/guides/` when the dependency is adopted; do not create empty guides in anticipation.

Dependency changes should include the relevant tests, verification, and documentation. Security, license, and update checks become part of the gate when the Rust scaffold activates the corresponding tools.
