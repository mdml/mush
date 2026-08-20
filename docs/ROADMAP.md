# Roadmap

Mush advances through one active, usable vertical slice at a time. This file owns the current milestone, milestone sequence, gates, and deferred ideas. It is not a backlog, status log, or design document; durable product and architecture changes belong in the relevant decision records.

## Current milestone

### M4 — Bounded cross-harness collaboration

**Outcome:** A human or conversational coordinator can declare and run a bounded cross-harness collaboration without managing each agent session or retaining the workflow in conversational memory.

**Status:** Active since 2026-08-15. The dynamic task graph and durable observation, quality baseline, agent-configuration ownership boundary, explicit runner, and executable work-task dependency chain were accepted through 2026-08-20. A checkpoint-readiness slice was implemented with the full gate passing and remains unreviewed. Product review then found that M4 had optimized for a durable, incrementally extended mid-flight graph before stating the hypothesis that the graph serves. The [episodic execution graph decision](decisions/2026-08-20-episodic-execution-graphs.md) now reorients M4 around episodic declared graphs, semantic checkpoints, budgeted attempts, and repository-owned product truth; the existing implementation remains preserved pending classification against that direction.

**Current slice:** Make the checkpoint contract concrete in the maintained abstractions and turn it into the next implementation brief. A graph-gating checkpoint receives immutable declared criteria and one named result, then records exactly one `met`, `not_met`, or `blocked` decision with bounded evidence. The checkpoint owns adjudication; the declared episode policy owns success advancement and a maximum semantic-attempt loop. Do not review the existing checkpoint-readiness implementation or change product code until this contract and its acceptance example are approved.

**Gate:** Given a real repository goal and a declared collaboration policy, a fresh coordinator creates an episodic graph using at least two harness families. A result passes through a semantic checkpoint with declared criteria; `not_met` creates another immutable attempt only while a declared semantic-attempt budget remains; `met` advances the episode; and `blocked` or budget exhaustion stops legibly. A human can inspect and step the episode without transcript reconstruction, already-determined work can advance without the declaring conversation, and the result and unresolved decisions remain understandable from the repository and bounded evidence after the episode graph is discarded.

### Likely path to the gate

This is a forecast, not a schedule. Revise it when review evidence changes what the milestone needs.

1. Activate the quality gate, enforce its deterministic checks in clean-checkout CI and a protected-main ruleset, and remediate its coverage, code-health, and dependency-policy findings without changing product behavior. Done 2026-08-20.
2. Return Cursor's autonomy choice to the operator so the configuration-ownership boundary holds for every adapter. Done 2026-08-20.
3. Accept the explicit runner and executable work-task dependency chain against that gate. Done 2026-08-20.
4. Adjudicate the product hypothesis, public decision-authority boundary, and episodic-graph direction before accepting more product behavior. Done 2026-08-20.
5. Make semantic checkpoint criteria, decisions, and the bounded attempt policy concrete in the maintained abstractions and approve one executable acceptance example.
6. Classify existing B-oriented implementation as required episodic mechanism, temporarily tolerable generality, or conflicting product behavior without treating the pending checkpoint-readiness diff as the specification.
7. Implement only the smallest missing behavior needed for semantic checkpoint criteria, a bounded attempt policy, manual legibility, and mechanical advancement of already-declared work.
8. Ship the first Mush skill with the CLI so a fresh coordinator can translate an ordinary goal and collaboration policy into an inspectable episode without private machine or database knowledge.
9. Run the gate against a real project goal selected at use time, then revise the path from its evidence.

Blocking observation may already satisfy M4's notification boundary. The gate run should decide whether any additional notification mechanism is necessary; do not allocate work to one in advance.

### Decisions

- [Episodic execution graphs](decisions/2026-08-20-episodic-execution-graphs.md) supersedes the durable mid-flight graph as the maintained product direction without rewriting its implementation history.
- [Dynamic task graph and durable observation](decisions/2026-08-18-m4-dynamic-task-graph.md) establishes the task graph, readiness, delivery, and observation model.
- [The explicit runner surface](decisions/2026-08-19-explicit-runner-surface.md) supersedes that record's launch-pump and same-boot liveness choices; its other consequences remain in force.
- [Quality verification policy](decisions/2026-08-20-quality-verification-policy.md) establishes measured coverage ratchets, stock-rule CodeScene enforcement, dependency policy, clean-checkout checks, and protected-main requirements for the active stabilization slice.
- [Agent configuration ownership](decisions/2026-08-20-agent-configuration-ownership.md) records the three-layer configuration boundary whose Cursor remediation the quality slice implemented.

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
