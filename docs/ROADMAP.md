# Roadmap

Mush advances through one active, usable vertical slice at a time. This file owns the milestone sequence, gates, operating rules, and deferred ideas. [The current milestone](current-milestone.md) owns the active milestone, its active slice, and the slice brief. Neither is a backlog, status log, or design document; durable product and architecture changes belong in the relevant decision records.

## Milestone sequence

| Milestone | Outcome | Gate status |
| --- | --- | --- |
| M0 — Documentation system | Mush is explainable in five minutes and each document has one canonical home | Met 2026-08-06 |
| M1 — Manual control loop | A task can be created, completed, checkpointed, and revised by hand, and survives restart | Met 2026-08-07 |
| M2 — One real executor | A real harness runs a real task, and its evidence remains reviewable after reboot | Met 2026-08-07 |
| M3 — Cross-harness delegation and review | A coordinator delegates to another vendor's harness, and a checkpoint reviews work its own harness did not produce | Met 2026-08-15 |
| M4 — Bounded cross-harness collaboration | A human or coordinator can run a bounded, semantic collaboration without managing each agent session | Active |
| M5 — Public alpha | A new user can discover, install, understand, use, verify, upgrade, and remove a versioned Mush release | Not started |

### Next gate

- **M5:** From the public repository, a new user on a clean supported machine can understand Mush's status and support boundaries, install a versioned release and its matching skill, configure a supported harness, pass diagnostics, complete a reviewed task, upgrade, and remove the installation. Apache-2.0 licensing, generated release notes, checksummed platform archives, per-target CycloneDX SBOMs, and build-provenance and SBOM attestations are produced by pinned automation that creates a draft prerelease for human publication. Published releases and their tags are immutable. The full merge gate passes rather than skipping, and a simulated harness-output change fails an adapter compatibility test with an actionable diagnosis.

M4 activates the local merge-confidence gate and enforces its deterministic subset on pull requests before adding more product behavior. M5 adds release-specific enforcement: supported-platform artifact builds, immutable release tags, generated release notes, checksums, SBOMs, provenance attestations, and human-controlled publication.

## Operating rules

- Keep one active milestone and one active vertical slice.
- M2 and later must advance work already intended in another project.
- Add abstractions only after a real task demonstrates the need.
- Put new ideas in the parking lot rather than expanding the active milestone.
- Describe detailed behavior when its milestone becomes active, not in anticipation of it.
- Demonstrate each executable milestone before beginning the next.
- At each gate, ask whether existing tools plus a shared skill solve the problem well enough.

## Risks under test

Milestone gates test these risks through real use. Passing implementation tests does not show that Mush is helping.

- Evidence-backed review may cost as much attention as direct implementation.
- Faster candidate production may create a review backlog and comprehension debt rather than leverage.
- Dynamic workflows may hide an unbounded or illegible task graph behind the appearance of automation.
- Notifications may become unreliable side effects unless delivery state is durable and inspectable.
- Private overlays may drift into an inaccurate second source of truth.
- Project flexibility may become bespoke adapter work disguised as configuration.
- Mush may displace the work it exists to accelerate.

## Parking lot

Alpha evidence determines whether any of these becomes M6. Their presence here is not authorization to implement them.

- Repository or project-context portability, if alpha use exposes real incompatibilities.
- Additional harnesses or home-machine configurations, when demand identifies which ones matter.
- A small website, if repository documentation proves insufficient for discovery or onboarding.
- Stable capability aliases after observed model churn.
- Recurring project-review tasks.
- Additional Flue-backed executors.
- Deeper delegation, but only when a concrete case cannot be expressed with one level.
- Cross-machine execution of one project, as a separate architectural decision.
- Command stages: a loop stage or task kind whose executor is a deterministic command rather than an agent, whose exit status is the decision and whose output is the report, so verifiable quality bars need no adjudicator. Until then, deterministic bars are re-run by a reviewer stage and named in checkpoint criteria.
- Passing prerequisite reports into every dependent task's prompt, generalizing the loop's report-passing rule to ordinary dependency edges, if real episodes show manual descriptions failing to carry bounded context.
- Structured report or evidence records with an enforced schema, only after real collaborations demonstrate free-form bounded Markdown reports failing; ABSTRACTIONS already defers richer evidence types.

## Completed evidence

This section records why each closed gate counts as met. Detailed rationale belongs in decision records, not here.

### M3 — Cross-harness delegation and review

The gate closed on 2026-08-15 after 35 real project tasks across Claude Code, Cursor, and Codex: 27 work tasks, eight checkpoints, five acceptances, three requested revisions, and real interruption and recovery. The gate chain delegated a bounded SDK slice from Claude task `1` to Cursor task `2`; Claude checkpoint `3` accepted that work. A later checkpoint requested a substantive revision, linked tasks integrated and corrected the result, and checkpoint `8` accepted it. This established the value unique to Mush over a vendor's native subagents: work and independent review can cross harness boundaries.

Decisions: [parent completion after delegation](decisions/2026-08-12-m3-parent-completion-after-delegation.md), [Codex harness adapter](decisions/2026-08-13-codex-harness-adapter.md), and [harness executable pinning](decisions/2026-08-13-harness-executable-pinning.md).

### M2 — One real executor

The gate closed on 2026-08-07 after a real home-machine reboot. Mush preserved a four-task chain, its accepted decision, evidence, and resolvable artifact paths without transcript reconstruction or Mush-specific repair. The resulting documentation conformance change was independently accepted.

### M1 — Manual control loop

The gate closed on 2026-08-07. An automated acceptance scenario exercises project and agent registration, work creation, manual completion, automatic checkpoint creation, evidence review, acceptance or requested revision, and a linked follow-up through the CLI and TUI, with persistence across restart.

### M0 — Documentation system

The gate closed on 2026-08-06 with a small canonical documentation set, distinct homes for architecture and abstractions, documented contribution and verification standards, and a product model explainable in five minutes.
