# Bounded checkpoint loop

Status: accepted on 2026-08-20.

## Context

The [episodic execution graph decision](2026-08-20-episodic-execution-graphs.md) requires its executable checkpoint example to be expressible through the public domain operations. The implementation inventory that decision called for found that the first inexpressible point is declaration: `max_attempts` has no representation, checkpoint criteria have no per-checkpoint home, and the loop's success continuation can only be mis-stated as a dependency edge on the first materialized checkpoint. Below that layer, the attempt chain, subject gating, shared decision operation, and infrastructure-versus-semantic attempt separation are reusable. Three behaviors conflict with the accepted model: the `accepted`/`revision_requested` decision vocabulary, `decide_checkpoint` fusing adjudication with an unbudgeted automatically created revision, and continuation edges a later attempt's `met` can never satisfy.

The inventory returned a decision packet; the maintainer adjudicated it as follows.

## Decision

### Decision vocabulary is `met`, `not_met`, `blocked` — including storage

The domain, CLI, and stored rows use the adjudication vocabulary. Existing decided checkpoints migrate `accepted` → `met` and `revision_requested` → `not_met`. This reinterprets M3-era history — a requested revision is recorded as the adjudication that produced it rather than the consequence — and the maintainer accepts that reinterpretation. No storage/surface split is kept.

### The loop is a first-class declared record, and its mental model is a single task

A bounded loop is declared as its own record carrying the work assignment, the checkpoint criteria and adjudicator, `max_attempts` counting the initial attempt, and its success continuation. From outside, the loop reads as one semantic task: downstream work depends on the loop itself, not on any materialized attempt or checkpoint, and the continuation is satisfied when any permitted attempt's checkpoint records `met`. Attempts and their checkpoints are the loop's internal, immutable, materialized structure. This rejects both spreading the budget across task columns and walking `previous_task_id` chains inside the readiness predicate.

### The policy materializes and queues the next attempt

When a checkpoint records `not_met` and budget remains, the loop policy materializes the next attempt and its checkpoint — identical criteria, identical adjudicator, linked through `previous_task_id`, with the bounded `not_met` evidence supplied — and queues them when the assignment is executable, so already-declared iteration advances without the declaring conversation. The pre-start revision edit (`prepare_revision`) remains the human override before a materialized attempt begins. `blocked` and `not_met` on the final permitted attempt materialize nothing and stop the path legibly. `decide_checkpoint` stops creating follow-up work on its own authority; the checkpoint owns adjudication only.

### The adjudicator is assigned per checkpoint

Checkpoint declaration names its adjudicator explicitly. The project-level `checkpoint` agent flag is demoted to a default used when no assignment is given. A human adjudicator remains an agent row whose harness Mush cannot execute; such a checkpoint is decided through the same decision operation and is refused from the queue, unchanged.

### The episode entity is deferred

No episode row or disposal operation is added in this slice. Tasks stay project-scoped, the loop record is the only new boundary, and the M4 gate demonstrates disposability by inspection rather than mechanizing deletion. Introducing an episode boundary before one real episode has run was judged speculative shape.

### The pending checkpoint-readiness change is reviewed first

The slice logically builds on subject gating, queueable checkpoints, and decision-sensitive readiness, which exist only in the unreviewed working-tree change recorded by [checkpoint decision readiness](2026-08-20-checkpoint-decision-readiness.md). That change is reviewed and adjudicated before the slice is built on whatever survives. Its `accepted`-only edge satisfaction and its permanently blocked dependents on a requested revision are superseded by the vocabulary and loop decisions above; its trigger and gating mechanics are expected to survive in shape. Its version-12 migration story is not treated as settling the schema this slice needs.

The review completed the same day and the maintainer accepted the change as the slice's base. `just fast` passed on the reviewed tree, the added tests track that record's verification list, and the review found no blocking defect. Two findings carry into this slice: `subject_task_id` has no schema-level immutability, so the slice adds the retargeting trigger alongside criteria immutability, and the unshipped-version refusal message names a backup that may not exist.

## Implementation brief

This section is the slice brief [CONTRIBUTING.md](../CONTRIBUTING.md) requires; no separate plan document exists.

**Goal.** The [executable checkpoint example](2026-08-20-episodic-execution-graphs.md#executable-checkpoint-example) runs end to end through the public CLI: a declared bounded loop with `max_attempts` three, immutable per-checkpoint criteria, a named adjudicator, budget-gated materialization of linked attempts, a `met` decision from any permitted attempt satisfying the success continuation, and both stop paths reported legibly.

**Worked example.** The seven-step episode and its two stop paths in the episodic decision record are the acceptance example, unchanged. Infrastructure recovery of any one task must not change the semantic-attempt count.

**Settled by this record.** The vocabulary and its row migration, the first-class loop record with the single-task mental model, materialization and queueing by the policy, per-checkpoint adjudicator assignment, the deferred episode entity, and the reviewed checkpoint-readiness base.

**Contributor discretion.** Within those boundaries: exact CLI command and flag naming, the loop record's storage layout, schema version numbering and migration mechanics, criteria and retargeting trigger shape, internal module organization, and test scaffolding. A choice that would change domain semantics, a public interface's meaning, or a difficult-to-reverse data shape beyond what this record settles stops implementation and returns a decision packet.

**Refusals.** Structured criterion records, multiple checkpoints on one attempt, arbitrary condition expressions, mutable budgets, a workflow-authoring language, episode deletion, and notification mechanisms stay out of the slice.

**Acceptance evidence.** Automated tests exercise the worked example and both stop paths, the budget's non-consumption by infrastructure recovery, criteria and subject immutability at the schema layer, and the vocabulary migration of decided M3-era rows. Observation and the TUI restate criteria, decision, evidence, attempt number, remaining budget, and next eligible action without transcript access. The full gate passes.

**Stop conditions.** Budget, criteria, or continuation semantics turning out to need an episode boundary after all; the migration reinterpretation proving lossy for real M3 rows; or any unsettled consequential choice surfacing as above.

## Consequences

- [ABSTRACTIONS.md](../ABSTRACTIONS.md) gains the single-task reading of the loop and the queueing of materialized attempts; its existing checkpoint packet, vocabulary, and budget sections already state the rest of the contract.
- The implementation slice is: adjudicate the pending checkpoint-readiness change, then implement loop declaration, per-checkpoint immutable criteria and adjudicator, the vocabulary migration, budget-gated materialization and queueing, the loop-satisfying continuation, and observation that restates criteria, decision, attempt number, remaining budget, and next eligible action.
- Structured criterion records, multiple checkpoints per attempt, condition expressions, mutable budgets, a workflow language, episode deletion, and notification mechanisms remain outside the slice.
- Exact schema shape and version numbering are implementation decisions taken after the checkpoint-readiness adjudication, within the boundaries above.
