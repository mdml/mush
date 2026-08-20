# Working in this repository

Canonical instructions for humans and agents contributing to Mush.

## What this repository is

Mush is a maintainer-led pre-alpha experiment in bounded collaboration among coding agents across harness and model-family boundaries. A human or conversational coordinator declares an episodic execution graph of agent tasks and semantic checkpoints, which can be stepped manually or advanced mechanically without keeping the workflow in conversational memory. [docs/PRODUCT.md](docs/PRODUCT.md) owns the hypothesis and product refusals, while [docs/ROADMAP.md](docs/ROADMAP.md) owns current status.

## Reading order

Read [README.md](README.md) for the user-facing overview, [docs/PRODUCT.md](docs/PRODUCT.md) for the hypothesis and decision boundary, [docs/ABSTRACTIONS.md](docs/ABSTRACTIONS.md) for the implemented product model, [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) for components and state, and [docs/current-milestone.md](docs/current-milestone.md) for the active milestone, its slice, and its gate. Read [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) before changing the repository and [docs/VERIFICATION.md](docs/VERIFICATION.md) before handing off work. Integration-specific guidance lives under `docs/guides/`.

Documentation-specific conventions live in [docs/AGENTS.md](docs/AGENTS.md) and apply to every file under `docs/`.

## Load-bearing boundaries

Do not trade these boundaries for implementation convenience. If experience shows that one must change, update the canonical documentation and record the rationale rather than working around it.

1. **A task is one semantic attempt.** A semantic retry creates a new task linked through `previous_task_id`; do not introduce a parallel run model without demonstrated need.
2. **A checkpoint is a semantic task.** It adjudicates one named result against immutable declared criteria and records exactly one `met`, `not_met`, or `blocked` decision with evidence. The checkpoint does not own the workflow consequence; do not introduce parallel checkpoint or review entities without demonstrated need.
3. **Task relationships remain independent.** `parent_task_id` represents decomposition, `previous_task_id` represents a later semantic attempt, `subject_task_id` identifies the result under checkpoint adjudication, and a dependency represents execution readiness.
4. **The CLI and TUI share domain operations.** The CLI remains headless for agents and automation. `mush tui` is the human interface, and every TUI mutation must have a non-interactive equivalent.
5. **Harness adapters translate execution, not product semantics.** Harnesses invoke agents; Mush owns task state, checkpoint flow, evidence, and policy. Mush never writes harness or machine configuration; autonomy choices travel in registered agent settings in the vendor's own vocabulary.
6. **Project context preserves provenance.** Repository-owned documents and private overlays may be used together, but Mush must not present overlay content as upstream truth.
7. **Each project has one home machine initially.** Installation and configuration may span machines; live task state for one project does not.
8. **No behavior ahead of its milestone.** Implement only the active vertical slice in [docs/ROADMAP.md](docs/ROADMAP.md). Later concepts may be named when necessary to preserve a boundary, but their detailed behavior waits for the milestone that exercises them.

## Milestone discipline

Keep one active milestone and make each milestone a usable vertical slice. Prefer dogfooding Mush on work that already matters after the manual control loop exists. New ideas belong in the roadmap parking lot unless they are required to pass the active gate. Add abstractions only after real use demonstrates the need, and stop at each milestone boundary to review whether Mush is creating enough value to justify the next investment.

## Decision authority

The maintainer owns the product hypothesis, user-visible and domain semantics, milestone outcomes, public interfaces, data-model and difficult-to-reverse architecture, acceptance of consequential decisions, and acceptance of milestone evidence. Agents may attack assumptions, present alternatives, and recommend a choice. Within an accepted boundary, agents independently own local, reversible implementation details.

Do not infer intended product semantics from the current implementation. Implement only an accepted slice. When work exposes a product, domain, data-shape, public-interface, or difficult-to-reverse architectural decision that canonical documentation does not settle, stop and present a decision packet with the frame, one worked example, alternatives, a recommendation, and explicit refusals before changing code. Passing verification is implementation evidence, not product acceptance.

## Commands

The user surface includes the headless `mush` CLI and the explicitly launched `mush tui`. The active development command ladder is defined only in [docs/VERIFICATION.md](docs/VERIFICATION.md). Run the strongest available rung and report unavailable checks honestly.

## Verification

Correctness and quality standards live in [docs/VERIFICATION.md](docs/VERIFICATION.md). Formatting, compilation, linting, tests, rustdoc, repository policy, coverage, CodeScene quality, dependency policy, and the comprehensive merge gate are executable. Required tools and credentials fail closed; never describe a failed, missing, unavailable, or unmeasured check as passing.

## Documentation ownership

- [docs/AGENTS.md](docs/AGENTS.md) defines documentation structure, invariants, and authoring workflow.
- [README.md](README.md) is the brief user-facing introduction.
- [docs/PRODUCT.md](docs/PRODUCT.md) owns the product hypothesis, refusals, and maintainer decision authority.
- [docs/ABSTRACTIONS.md](docs/ABSTRACTIONS.md) owns the durable product concepts and relationships.
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) owns file organization, components, dependency flow, and state management.
- [docs/CONTRIBUTING.md](docs/CONTRIBUTING.md) owns setup and development process.
- [docs/VERIFICATION.md](docs/VERIFICATION.md) owns correctness and quality standards and their command ladder.
- [docs/ROADMAP.md](docs/ROADMAP.md) owns the milestone sequence, gates, and deferred ideas.
- [docs/current-milestone.md](docs/current-milestone.md) owns the active milestone, its active slice, and the slice brief.
- `docs/guides/` owns integration-specific operational guidance.
- `docs/decisions/` records consequential choices whose rationale must outlive the implementation that introduced them.

Keep each fact in one canonical home and link to it elsewhere. Documents should describe the maintained system, not preserve the conversation that produced it. Write Markdown and prose soft-wrapped: one line per paragraph or bullet, without artificial line breaks in running text.

## Change discipline

Preserve unrelated work in a dirty worktree. Keep changes within the active milestone, update the owning document when a boundary changes, and do not create commits or push unless the user asks. Run the verification rung appropriate to the handoff and report the evidence honestly.
