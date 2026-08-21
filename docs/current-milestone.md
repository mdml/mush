# Current milestone

This file owns the working state of the active milestone: its outcome, status, gate, the one active vertical slice, and that slice's brief. It is revised freely as work proceeds; git is its history. The milestone sequence, closed-gate evidence, operating rules, and deferred ideas stay in [the roadmap](ROADMAP.md), and durable rationale stays immutable in [decision records](AGENTS.md#decision-records). Exactly one milestone and one vertical slice are active at a time.

## M4 — Bounded cross-harness collaboration

**Outcome:** A human or conversational coordinator can declare and run a bounded cross-harness collaboration without managing each agent session or retaining the workflow in conversational memory.

**Status:** Active since 2026-08-15. The dynamic task graph and durable observation, quality baseline, agent-configuration ownership boundary, explicit runner, and executable work-task dependency chain were accepted through 2026-08-20. The [episodic execution graph decision](decisions/2026-08-20-episodic-execution-graphs.md) reoriented M4 around episodic declared graphs, semantic checkpoints, budgeted attempts, and repository-owned product truth. The implementation inventory that decision required was classified on 2026-08-20, its decision packet was adjudicated as the [bounded checkpoint loop decision](decisions/2026-08-20-bounded-checkpoint-loop.md), and the checkpoint-readiness implementation was reviewed and accepted as that slice's base. Pre-implementation review of the loop shape was adjudicated on 2026-08-21 as the [staged loop bodies decision](decisions/2026-08-21-staged-loop-bodies.md), which separates review stages from adjudication and generalizes the loop body to an ordered work path.

**Gate:** Given a real repository goal and a declared collaboration policy, a fresh coordinator creates an episodic graph using at least two harness families. A result passes through a semantic checkpoint with declared criteria; `not_met` creates another immutable attempt only while a declared semantic-attempt budget remains; `met` advances the episode; and `blocked` or budget exhaustion stops legibly. A human can inspect and step the episode without transcript reconstruction, already-determined work can advance without the declaring conversation, and the result and unresolved decisions remain understandable from the repository and bounded evidence after the episode graph is discarded.

## Current slice: staged bounded checkpoint loop

Implement the loop the [bounded checkpoint loop](decisions/2026-08-20-bounded-checkpoint-loop.md) and [staged loop bodies](decisions/2026-08-21-staged-loop-bodies.md) decisions settled: staged loop declaration, per-checkpoint immutable criteria and adjudicator assignment, the `met`/`not_met`/`blocked` vocabulary migration, budget-gated path materialization and queueing with mechanical report passing, the loop-satisfying success continuation, and observation that restates criteria, decision, evidence, attempt number, remaining budget, and next eligible action.

### Slice brief

**Goal.** The [executable checkpoint example](decisions/2026-08-20-episodic-execution-graphs.md#executable-checkpoint-example) runs end to end through the public CLI as a one-stage loop, and a two-stage implement-and-review loop exercises report passing: a declared bounded loop with `max_attempts` three, immutable per-checkpoint criteria, a named adjudicator, budget-gated materialization of linked path attempts, a `met` decision from any permitted attempt satisfying the success continuation, and both stop paths reported legibly.

**Worked example.** The seven-step episode and its two stop paths in the episodic decision record are the acceptance example, unchanged as the one-stage case. Infrastructure recovery of any one task must not change the semantic-attempt count.

**Settled decisions.** The [bounded checkpoint loop decision](decisions/2026-08-20-bounded-checkpoint-loop.md) settles the vocabulary and its row migration, the first-class loop record with the single-task mental model, materialization and queueing by the policy, per-checkpoint adjudicator assignment, the deferred episode entity, and the reviewed checkpoint-readiness base. The [staged loop bodies decision](decisions/2026-08-21-staged-loop-bodies.md) settles the ordered work-stage path, the report-judging checkpoint subject, mechanical report passing at both splice points, whole-path restart without re-entry configuration, and the absence of enforced report schemas. Two review findings carry into this slice: add schema-level `subject_task_id` immutability alongside criteria immutability, and correct the unshipped-version refusal message that names a backup that may not exist.

**Contributor discretion.** Within those boundaries: exact CLI command and flag naming, the loop and stage records' storage layout, schema version numbering and migration mechanics, criteria and retargeting trigger shape, report-splice formatting, internal module organization, and test scaffolding. A choice that would change domain semantics, a public interface's meaning, or a difficult-to-reverse data shape beyond what the decisions settle stops implementation and returns a decision packet.

**Refusals.** Structured criterion or report records, multiple checkpoints on one attempt, re-entry indexes or per-decision routing, arbitrary condition expressions, mutable budgets, a workflow-authoring language, command stages, report passing on ordinary edges, episode deletion, and notification mechanisms stay out of the slice.

**Acceptance evidence.** Automated tests exercise the worked example, a staged loop with both splice points, both stop paths, the budget's non-consumption by infrastructure recovery, criteria and subject immutability at the schema layer, and the vocabulary migration of decided M3-era rows. Observation and the TUI restate criteria, decision, evidence, attempt number, remaining budget, and next eligible action without transcript access. The full gate passes.

**Stop conditions.** Budget, criteria, or continuation semantics turning out to need an episode boundary after all; the migration reinterpretation proving lossy for real M3 rows; or any unsettled consequential choice surfacing as above.

## Likely path to the gate

This is a forecast, not a schedule. Revise it when review evidence changes what the milestone needs.

1. Activate the quality gate, enforce its deterministic checks in clean-checkout CI and a protected-main ruleset, and remediate its coverage, code-health, and dependency-policy findings without changing product behavior. Done 2026-08-20.
2. Return Cursor's autonomy choice to the operator so the configuration-ownership boundary holds for every adapter. Done 2026-08-20.
3. Accept the explicit runner and executable work-task dependency chain against that gate. Done 2026-08-20.
4. Adjudicate the product hypothesis, public decision-authority boundary, and episodic-graph direction before accepting more product behavior. Done 2026-08-20.
5. Make semantic checkpoint criteria, decisions, and the bounded attempt policy concrete in the maintained abstractions and approve one executable acceptance example. Done 2026-08-20.
6. Classify existing B-oriented implementation as required episodic mechanism, temporarily tolerable generality, or conflicting product behavior without treating the pending checkpoint-readiness diff as the specification. Done 2026-08-20, adjudicated as the bounded checkpoint loop decision.
7. Implement only the smallest missing behavior needed for semantic checkpoint criteria, a bounded attempt policy, manual legibility, and mechanical advancement of already-declared work. Done 2026-08-21.
8. Ship the first Mush skill with the CLI so a fresh coordinator can translate an ordinary goal and collaboration policy into an inspectable episode without private machine or database knowledge.
9. Run the gate against a real project goal selected at use time, then revise the path from its evidence.

Blocking observation may already satisfy M4's notification boundary. The gate run should decide whether any additional notification mechanism is necessary; do not allocate work to one in advance.

## Decisions

- [Episodic execution graphs](decisions/2026-08-20-episodic-execution-graphs.md) supersedes the durable mid-flight graph as the maintained product direction without rewriting its implementation history.
- [Dynamic task graph and durable observation](decisions/2026-08-18-m4-dynamic-task-graph.md) establishes the task graph, readiness, delivery, and observation model.
- [The explicit runner surface](decisions/2026-08-19-explicit-runner-surface.md) supersedes that record's launch-pump and same-boot liveness choices; its other consequences remain in force.
- [Quality verification policy](decisions/2026-08-20-quality-verification-policy.md) establishes measured coverage ratchets, stock-rule CodeScene enforcement, dependency policy, clean-checkout checks, and protected-main requirements.
- [Agent configuration ownership](decisions/2026-08-20-agent-configuration-ownership.md) records the three-layer configuration boundary whose Cursor remediation the quality slice implemented.
- [Checkpoint decision readiness](decisions/2026-08-20-checkpoint-decision-readiness.md) records the subject prerequisite and decision-sensitive readiness behavior, reviewed and accepted 2026-08-20 with two behaviors superseded by the loop decision.
- [Bounded checkpoint loop](decisions/2026-08-20-bounded-checkpoint-loop.md) settles the loop record, decision-vocabulary migration, budget-gated materialization, per-checkpoint adjudicator, and deferred episode entity for the active slice.
- [Staged loop bodies](decisions/2026-08-21-staged-loop-bodies.md) supersedes that record's single work assignment with an ordered work-stage path, separates review stages from adjudication, and settles mechanical report passing.
