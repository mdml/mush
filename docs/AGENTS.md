# Documentation conventions

These instructions apply to every document under `docs/`. They supplement the repository-wide instructions in [`../AGENTS.md`](../AGENTS.md).

## What the documentation is for

The documentation should let a new user understand Mush, let a contributor change it safely, and let a maintainer recover the reason for a consequential decision. Each document has one job:

- [`../README.md`](../README.md) introduces the product and its smallest useful path.
- [`PRODUCT.md`](PRODUCT.md) owns the maintained product hypothesis, refusals, and decision authority.
- [`ABSTRACTIONS.md`](ABSTRACTIONS.md) defines durable product concepts, relationships, and invariants.
- [`ARCHITECTURE.md`](ARCHITECTURE.md) explains components, dependency flow, file organization, and state management.
- [`ROADMAP.md`](ROADMAP.md) states the milestone sequence, gates, operating rules, and deferred ideas.
- [`current-milestone.md`](current-milestone.md) owns the active milestone, its active slice, and the slice brief.
- [`CONTRIBUTING.md`](CONTRIBUTING.md) explains setup and the development workflow.
- [`VERIFICATION.md`](VERIFICATION.md) defines correctness and quality standards and the command ladder.
- `guides/` explains how to operate an adopted integration or workflow.
- `decisions/` preserves the context, choice, and consequences of a decision whose rationale must outlive the change.

## Why the structure is strict

Mush coordinates agents that may join with no conversational context. A fact repeated in several places will eventually disagree, and a status document mixed with design rationale becomes difficult to scan or maintain. The documentation therefore optimizes for one canonical owner per fact, a clear distinction between current truth and historical rationale, and paths and examples that work in a public repository.

## Invariants

### Canonical ownership

- State each maintained fact in one canonical document and link to it elsewhere.
- Keep the full product hypothesis and maintainer decision boundary in `PRODUCT.md`; contributing and agent instructions may summarize their operational consequences and otherwise link to the canonical statement.
- Do not copy architecture or abstraction deltas into the roadmap. Link the decision record whose `## Consequences` section owns them.
- Do not put the command ladder anywhere except `VERIFICATION.md`.
- Keep integration-specific operational detail in `guides/`, not in architecture or the roadmap.
- Use a decision record for consequential rationale. Maintained documents describe the resulting system rather than replaying the discussion that produced it.

### Roadmap and current milestone

- There is exactly one active milestone. If work requires two active milestones, redefine the milestone boundary before proceeding.
- There is exactly one active vertical slice within that milestone.
- The active milestone, its slice, and the slice brief live in `current-milestone.md`, which is revised freely as work proceeds; git is its history. The milestone sequence, closed history, and parking lot live in the roadmap.
- A slice brief contains only sentences whose change would require maintainer adjudication — goal, worked example, settled decisions, discretion boundary, refusals, acceptance evidence, and stop conditions. Progress tracking and task lists belong nowhere in the documentation.
- A milestone has one outcome and one gate.
- Describe expected work as a revisable path to the gate rather than a numbered schedule.
- Future milestones state outcomes and gates, not speculative implementation designs.
- Closed milestone history records only enough evidence to show why the gate counts as met. Detailed rationale belongs in decision records.
- New ideas go in the parking lot unless the active gate requires them.

### Public and portable content

- Use relative links for files in this repository.
- Do not publish absolute paths containing a particular user's home directory or checkout location.
- Portable user-relative paths such as `~/.mush` are allowed when they describe a public installation or state convention. Use placeholders such as `<project-root>` or environment variables when an example needs an arbitrary location.
- Use paths, identifiers, and commands from private dogfood state only when they are durable public evidence and reveal no sensitive information. Omit ephemeral local paths, session details, and machine configuration.
- Never include credentials, tokens, private prompts, or transcript content.

### Prose and links

- Write Markdown and prose soft-wrapped: one line per paragraph or bullet, without artificial column wrapping.
- Prefer direct, descriptive prose over chronology or conversation summaries.
- Use descriptive relative links instead of duplicating the destination's contents.
- Describe current behavior in the present tense. Use dates and past tense for closed gates and historical evidence.
- Name unavailable or deferred behavior plainly; never describe a missing check or feature as passing.

### Decision records

- Name a decision record `YYYY-MM-DD-short-title.md` using the date the decision was accepted.
- A decision packet awaiting maintainer adjudication may carry an explicit `proposed` status; do not describe it as accepted or update maintained abstractions as though it were accepted.
- Record the context, decision, and consequences. Include alternatives only when they help a future maintainer understand why the boundary exists.
- A decision record is immutable once accepted. Do not edit it for any reason; correct, evolve, or supersede it with a new decision record that names what it supersedes and why. Records amended before this convention keep their dated amendments as history.
- Update the maintained abstraction or architecture document when a decision changes the current system. A decision record is rationale, not the sole description of current behavior.

## How to change documentation

1. Identify the canonical owner of the fact before editing.
2. Read the linked abstraction, architecture, roadmap, and decision context needed to avoid contradiction.
3. Update the canonical document, then replace any necessary repetition elsewhere with a link.
4. If a durable boundary changes, add a decision record — superseding an existing one if needed — and update its maintained owner.
5. Check that current-milestone status, roadmap sequence, examples, paths, and links remain public and current.
6. Run the repository policy check and the strongest applicable command from [`VERIFICATION.md`](VERIFICATION.md).

Do not preserve obsolete text merely as history. Git already records edits; decision records preserve only the rationale that the maintained documentation still needs.
