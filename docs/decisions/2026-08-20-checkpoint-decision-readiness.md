# Checkpoint subject prerequisites and decision-sensitive readiness

Status: accepted on 2026-08-20. Implements the graph unit that [the M4 dynamic task graph decision](2026-08-18-m4-dynamic-task-graph.md) deferred: checkpoint prerequisites and decision-sensitive readiness.

Amendment, 2026-08-20: the maintainer reviewed and accepted this implementation as the base for [the bounded checkpoint loop decision](2026-08-20-bounded-checkpoint-loop.md). That decision supersedes two behaviors recorded here without re-implementing them yet: the `accepted`-only satisfaction of a checkpoint prerequisite, and dependents of a `revision_requested` checkpoint staying blocked until a human re-points their edges. It also renames the decision vocabulary this record uses to `met`, `not_met`, and `blocked`. The subject gating, queueable checkpoints, readiness rest state, recovery behavior, and version 12 migration mechanics remain in force.

## Context

The accepted dependency chain advances work tasks only: an edge's sole prerequisite condition is successful work completion, checkpoints cannot be queued, and a checkpoint's wait on its incomplete subject is enforced in domain code alone. A coordinator can therefore declare work and its review up front, but the review does not run unattended, and nothing downstream can be gated on the review's outcome. The M4 gate needs a declared graph in which work runs, its checkpoint reviews it without the coordinator present, and acceptance versus requested revision visibly send the graph down different paths.

The vocabulary groundwork already exists. [The explicit runner decision](2026-08-19-explicit-runner-surface.md) renamed a checkpoint's wait on its subject to "blocked", the same word a work task uses for an unsatisfied prerequisite, and left open whether the two waiting mechanisms should become one. This slice makes them one mechanism.

## Decision

### The subject is a prerequisite, not an edge

A checkpoint's subject relationship itself gates execution readiness: a checkpoint cannot become ready, claim a launch, execute, or receive a decision while its subject work task is not completed. No dependency edge is created to express this, so `parent_task_id`, `previous_task_id`, `subject_task_id`, and dependency edges keep their four independent meanings. The prerequisite predicate for any task is now: every dependency-edge prerequisite is satisfied, and, for a checkpoint, its subject is completed.

The store enforces subject gating in schema triggers as well as in the domain: a direct database write cannot mark a checkpoint ready, running, or decided while its subject is incomplete. This matches how the existing graph rules are protected.

### Checkpoints are queueable

`task queue` accepts a pending checkpoint whose assigned reviewer Mush can execute. A queued checkpoint whose subject is incomplete is `blocked`; subject completion advances it to `ready` and creates its launch delivery in the same transaction, exactly as edge dependents advance. The runner then claims and executes the review through the ordinary front doors, and the existing subject sync replaces placeholder evidence before launch.

Queueing a checkpoint assigned to a reviewer Mush cannot execute — a manual human reviewer — is refused with a specific error, because a ready delivery that no execution can ever claim would park as an intervention that recovery cannot fix. A manual checkpoint is decided through the TUI or `checkpoint decide` as before, without queueing.

A successful review execution records evidence and leaves the decision to the existing accept, block, or revise operation, unchanged. The queue's job for that checkpoint is then done: its readiness becomes `completed` with its delivery `delivered`, the runner does not restart it, and reconciliation leaves it alone. For a checkpoint, readiness `completed` therefore means the queue delivered and executed the review; semantic completion remains the decision, and `task status` and `task wait` treat an undecided checkpoint as non-terminal whatever its readiness. An executing review may be decided from within — the reviewing agent holds `MUSH_TASK_ID` and may call `checkpoint decide` — and the execution then finishes by recording its success without disturbing the recorded decision or its evidence. Re-running an executed, undecided review remains available through `task run`, which is where follow-up context already lands.

### Readiness is decision-sensitive

A dependency edge may now name a checkpoint as its prerequisite; the dependent remains a work task. A checkpoint prerequisite is satisfied only when the checkpoint is completed with decision `accepted`. The two non-accepting decisions advance the graph differently, and both are terminal for the checkpoint itself:

- `accepted` settles the subject: the checkpoint completes, and blocked dependents whose remaining prerequisites are satisfied become ready in the same transaction.
- `revision_requested` completes the checkpoint and creates the linked revision work task, as before. Dependents gated on the checkpoint stay `blocked`: the accepted review they wait for will never come from this checkpoint, and Mush does not re-point their edges. The revision task and the attempt chain are the legible continuation; the current or a later coordinator queues the revision, creates its checkpoint, and re-points unstarted dependents' edges if the plan still holds.
- `blocked` completes the checkpoint and advances nothing, recording that adjudication needs missing information or authority.

This is automatic advancement of declared structure, not planning: Mush still never invents a task, dependency, checkpoint, or revision, and a graph stopped by a non-accepting decision is ordinary legible state, not a failure.

### Deadlock through the subject relation is rejected as a cycle

A checkpoint implicitly waits on its subject, so cycle detection treats subject links as readiness edges: an edge making a checkpoint's subject depend on that checkpoint, directly or transitively, is rejected exactly as a dependency cycle is, in the domain and in the schema triggers.

### One migration, version 12

The trigger changes ship as schema version 12. Migration accepts exactly the schema versions earlier builds wrote to real databases — version 3, which M3 shipped, and version 11, which the explicit runner slice wrote to the dogfooding database — and continues to refuse the never-shipped versions 4 through 10. No column, table, or stored vocabulary changes; the migration is trigger recreation plus the version stamp, preceded by the standard pre-migration backup under the serve lock.

## Required invariants

- A checkpoint cannot become ready, be claimed, execute, or receive a decision while its subject is not completed, and direct database writes cannot bypass this.
- `parent_task_id`, `previous_task_id`, `subject_task_id`, and dependency edges retain independent meanings; no edge row expresses the subject prerequisite.
- A checkpoint prerequisite in a dependency edge is satisfied only by decision `accepted`; `revision_requested` and `blocked` leave dependents blocked while the checkpoint itself completes.
- Subject completion, edge-prerequisite completion, and checkpoint decisions advance eligible blocked tasks atomically with the transition that triggered them.
- An executed, undecided checkpoint is not restarted by `tick`, `serve`, reconciliation, or recovery, and is not terminal for observation.
- Dependency edges cannot create a readiness cycle through subject links.
- Queueing, claiming, delivery, recovery, and the launch invariant behave for checkpoints exactly as for work tasks, except where this record says otherwise.

## Verification required

- A declared work-plus-checkpoint chain advances unattended: work completes, its queued checkpoint becomes ready and executes, and an accepted decision readies the gated dependent, all without the queueing shell.
- Acceptance and requested revision produce their distinct outcomes, including the revision task and the still-blocked dependents.
- Direct SQL writes that would ready, run, or decide a checkpoint with an incomplete subject fail on triggers.
- Subject-link cycles, direct and transitive, are rejected in domain and schema.
- Queueing a manual-reviewer checkpoint is refused with a specific error.
- An interrupted review is reconciled and recoverable; an executed undecided review is left alone by reconciliation and recovery.
- Concurrent runners never double-claim a ready checkpoint; deciding a checkpoint mid-execution leaves one coherent outcome.
- CLI JSON and the deterministic TUI snapshot expose blocked, ready, executing, awaiting-decision, and decided checkpoint states without transcript reconstruction.
- Migration from versions 3 and 11 succeeds with a backup; versions 4 through 10 remain refused.

## Alternatives rejected

Expressing the subject prerequisite as a hidden dependency edge is rejected: it would overload the edge relation with checkpoint semantics, create a row the coordinator never declared, and let edge edits detach a checkpoint from its own subject.

A new persistent readiness value for an executed, undecided review is rejected for this slice. It would force a `tasks` table rebuild on migrated databases for a state that current rows already express as pending status, succeeded execution, and no decision; observation derives the state instead. If real use shows operators misread readiness `completed` on an undecided checkpoint, a later slice can rename it with the rebuild it costs.

Automatically queueing the revision task a requested revision creates is rejected. Queueing is the coordinator's declaration of intent; a decision that demands another semantic attempt should not silently commit the operator to running it, and the revision may deserve a changed description or worktree first.

Re-targeting dependents of a revision-requested checkpoint to the revision's future checkpoint is rejected: a checkpoint reviews its named subject only, the future checkpoint does not exist, and Mush does not invent one.

Letting a successful review execution record the decision itself is rejected, unchanged from M1: adjudication is an explicit operation with its own authority, and an agent reviewer that holds that authority exercises it through `checkpoint decide`.

## Consequences

A coordinator can now declare work, its review, and decision-gated continuations up front and disappear; the runner advances through the review, and only a non-accepting decision or missing declared structure stops the graph. The four-relationship model survives with dependency edges still meaning readiness only, and the blocked vocabulary now names one mechanism instead of two.

On implementation, [ABSTRACTIONS.md](../ABSTRACTIONS.md) records the queueable checkpoint, the subject prerequisite, and decision-sensitive readiness; [ARCHITECTURE.md](../ARCHITECTURE.md) records the version 12 schema and trigger enforcement; and [ROADMAP.md](../ROADMAP.md) tracks the slice.
