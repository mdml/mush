# Staged loop bodies and mechanical report passing

Status: accepted on 2026-08-21.

## Context

The [bounded checkpoint loop decision](2026-08-20-bounded-checkpoint-loop.md) settled the loop as a first-class declared record carrying one work assignment, one set of checkpoint criteria and one adjudicator, `max_attempts`, and a success continuation. Reviewing that shape before implementation exposed a coupling it silently assumed: the adjudicator was also the reviewer. One agent had to perform the deep engagement with the result and emit the structured decision in the same session.

The maintainer rejected that coupling. Deep review and adjudication are different responsibilities: review produces a rich report, while adjudication is a narrow semantic judgment over that report — did the reviewer request changes, did two drafts converge, does the review conform to the declared report format, did it identify blocking issues. Coupling them concentrates schema-conformance risk in the longest-running agent, and it does not fit a human adjudicator, whose attention the product exists to conserve: a human should judge an independent reviewer's bounded report, not personally re-derive the result. Judging a bounded report is also consistent with the rest of the system, which passes bounded evidence between agents rather than transcripts; a checkpoint already adjudicates "the result of one named work task", and nothing requires that named task to be the producer.

Once review is a work task, a loop body of one work task cannot express the motivating collaborations: the path that must repeat on `not_met` is implement-then-review, not implement alone.

## Decision

### A loop body is an ordered path of work stages

A declared loop carries an ordered path of one or more work stages, each with its own exact agent assignment and declared description, followed by exactly one checkpoint whose subject is the final stage. Materializing an attempt creates every stage as an immutable work task, chains consecutive stages with ordinary dependency edges, and creates the attempt's checkpoint. `max_attempts` counts path executions, not work rows; each attempt has exactly one checkpoint, so materialized checkpoints count the budget. A single-stage body remains valid and is the shape the bounded checkpoint loop decision described, so the accepted executable checkpoint example is unchanged as the one-stage case. This supersedes that decision's single work assignment; its other consequences — the decision vocabulary and row migration, the first-class loop record read as a single task, policy-owned materialization and queueing, the per-checkpoint adjudicator, and the deferred episode entity — remain in force.

### The adjudicator judges the subject's report

The checkpoint's subject is the final stage, and its criteria are stated over that stage's bounded report. A collaboration that wants deep review declares it as a work stage whose report the checkpoint adjudicates; the adjudicator is not required to re-derive the underlying work, and for many loops it can be a small model or a human reading one report. Direct adjudication of a producer's work remains expressible as a one-stage loop where the criteria address the work itself.

### Reports pass mechanically; nobody authors the next prompt

A task's report is its result — the bounded Markdown every completed work task already records. The loop policy passes reports at the earliest moment each exists:

- Within an attempt, a stage's result does not exist when its successor is materialized, so the pass happens at launch: the executor assembles the successor's prompt from its immutable declared description plus the report of each prerequisite stage, and persists the assembled prompt in the task's artifacts. The prompt is a deterministic function of graph state, so the stage's bounded inputs remain inspectable without a transcript.
- On restart, everything exists at materialization, so it goes into the new first stage's description: the declared stage description, the prior attempt's final-stage report, and the checkpoint's decision and evidence. The pre-start human edit (`prepare_revision`) still applies to a materialized, unstarted stage.

The adjudicator's authority ends at the decision and its evidence. Criteria cannot drift, no agent writes another agent's instructions, and each attempt is legible from the graph alone.

### Restart re-runs the whole path

`not_met` with budget remaining re-materializes every stage. There is no re-entry index: the loop body is exactly the part that repeats, and work that should not repeat is declared outside the loop and connected by ordinary edges — for example, planning that precedes an implement-and-review loop. Per-decision routing, where the adjudicator chooses a re-entry stage, is refused outright because the checkpoint owns adjudication only.

### No enforced report schema

Mush does not validate report content. The expected report shape is part of the declared stage description, and a malformed or useless report is a criteria failure caught by the checkpoint, not a parse failure. Enforcing a schema at every stage would reintroduce, at every node, the conformance fragility that motivated separating review from adjudication, and structured evidence records are already deferred until real collaborations demonstrate the need.

## What this decision refuses

- A re-entry stage index or any per-decision routing of restart feedback.
- Checkpoints on intermediate stages, or more than one checkpoint per attempt.
- Enforced report schemas or validated structured output from stages.
- Report passing on ordinary dependency edges outside loop bodies; recorded in the roadmap parking lot as an evidence-gated generalization.
- Command stages whose executor is a deterministic command; recorded in the roadmap parking lot.

## Consequences

- [ABSTRACTIONS.md](../ABSTRACTIONS.md) restates the semantic-attempt budget around staged paths, the report-judging checkpoint subject, and mechanical report passing.
- The active slice implements the staged loop record, the vocabulary migration, per-checkpoint immutable criteria and adjudicator, budget-gated path materialization and queueing with both splice points, the loop-satisfying continuation, and observation restating criteria, decision, attempt number, remaining budget, and next eligible action.
- The bounded checkpoint loop decision's review findings — schema-level `subject_task_id` immutability and the unshipped-version refusal message — carry into the slice unchanged.
