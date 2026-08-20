# Abstractions

Mush begins with a small model for assigning work and deciding whether its result is acceptable. This document describes the abstractions the implementation exercises today. Later concepts remain provisional until implementation begins.

## Agent

An agent is an exact executor configuration: a harness, model identity, and relevant invocation settings. A task is assigned to one such configuration rather than to a capability alias or scheduler role.

Harness version is the one part of that configuration an agent may leave open. An agent may pin an exact harness version, which execution enforces before launch, or pin none and accept whatever version is installed. Pinning none is what keeps a configuration working across harness upgrades performed by whoever owns installation, and it does not weaken the record: every execution records the version that actually ran, so exactness that is not guaranteed in advance remains legible afterwards.

Configuration defects surface when an agent is registered or its settings are updated, not when a task first executes. An executable checkpoint agent must carry a non-empty review prompt; a reviewer Mush cannot execute, such as a manual human reviewer, needs none. An executable is named as a path or as a command resolved on the executing environment's `PATH`, so a configuration need not encode how the harness was installed. Whether that name keeps resolving across upgrades belongs to whoever owns installation; Mush refuses to launch an executable it cannot run and report a version for, and additionally refuses one whose version contradicts a pin.

## Project

A project is a registered checkout with a name and path. Project configuration identifies how checkpoints are assigned. Repository-specific execution is exercised only through the harness adapters (Claude Code, Codex, and Cursor); verification and overlay behavior remain outside Mush's project model.

## Task

A task is one semantic attempt in one project. Its description, assignment, budget, and relationships do not change after it starts. Execution advances its status and records its result.

The initial task kinds are:

- `work` for implementation, investigation, maintenance, or planning;
- `checkpoint` for reviewing the result of a work task.

If completed work needs another semantic attempt, Mush creates a new linked task instead of editing the completed task or introducing a separate run record. Infrastructure retries resume the same task; see [Execution](#execution).

## Task relationships

Three independent relationships describe why tasks are connected:

| Relationship | Meaning |
| --- | --- |
| `parent_task_id` | This task contributes to a larger task. |
| `previous_task_id` | This is a new semantic attempt following an earlier task. |
| `subject_task_id` | This checkpoint reviews another task. |

They are not interchangeable. For example, a later revision of a subtask can keep the same parent while its `previous_task_id` points to the prior attempt.

A work task may be created under a parent to delegate a bounded piece of a larger task. Delegation is one level deep and work-only: the proposed parent must be a work task that itself has no parent, so a subtask cannot delegate further and a checkpoint can be neither parent nor child. The bound is expressed on the proposed parent rather than the caller because Mush cannot reliably know which process invoked its CLI; a consequence is that a subtask naming its own parent creates a sibling, which is permitted — the fence bounds comprehension depth, not caller identity, and a sibling remains legible at one level. The domain rejects violations with a specific error, and the store guarantees the same invariant in the schema.

A revision inherits its subject's parent, so a revision of a subtask is bounded exactly as the subtask was, while a revision of a parentless task remains parentless.

Delegation ends in integration. A parent work task cannot complete while any direct child is not completed or is still recorded as running, whether the completion is supplied manually or by a successful execution, and a completed parent cannot gain new children. The domain rejects both directions with an error naming the blocking children, and the store guarantees the same invariant in the schema. Mush refuses the completion rather than waiting for, detaching, or cancelling the child: a refused executor completion leaves the parent interrupted, and the child remains available for explicit recovery.

## Checkpoint

A checkpoint is a task whose subject is a completed work task. It records one decision and the evidence supporting it:

- `accepted` settles the subject successfully;
- `revision_requested` creates a new work task linked through `previous_task_id`, whose description carries the subject's description plus the checkpoint's decision and evidence so the follow-up does not depend on transcript access;
- `blocked` records that progress needs missing information, authority, or external state.

Completed work creates at most one checkpoint. Checkpoints do not create checkpoints. There is no separate checkpoint, review, or run entity.

A checkpoint is adjudication rather than integration. Its reviewer is not the task's delegator, does not need the result to continue other work, and may run on a different harness than the work it reviews. Delegation is the complementary relation: a coordinator that needs a subtask's artifact to proceed reviews it as part of doing its own work. Because a coordinator reviewing its own delegate is self-review, checkpoint creation is explicit rather than automatic on completion, so adjudication lands at the packet boundary instead of on every delegated subtask.

```text
work task 41
└── checkpoint task 42, subject_task_id: 41
    ├── accepted
    ├── blocked
    └── revision requested
        └── work task 48, previous_task_id: 41
```

A checkpoint may be assigned to a configured agent or left for human review through the TUI. Completing a work task records its result and evidence without creating a checkpoint; the one checkpoint is created explicitly, creating it again returns the existing checkpoint, and the TUI shows which completed work still has none.

A checkpoint may be created before its subject completes, recording declared review intent. Such a checkpoint is *awaiting its subject*: it holds placeholder evidence, cannot execute, and cannot receive a decision until its subject completes; when it first runs against the completed subject, the placeholder is replaced with the subject's recorded evidence. A checkpoint reviews its named subject only: it is never re-targeted to a newer attempt, and the attempt chain makes newer work visible where the decision is read.

## Evidence

Evidence begins as Markdown stored with a checkpoint task. It can cite commands, tests, manually supplied references, observations, and unresolved concerns. Large harness execution output lives in task-owned artifact files whose paths stay in the task evidence; richer evidence types remain deferred.

Mush will learn from real reviews before introducing structured evidence types or separate evidence records.

## Execution

Execution state lives directly on a task rather than in a parallel run entity. An executing task records the exact executable version observed at launch, the model, harness-specific invocation settings, session, worktree name, infrastructure attempt count, process identity, and artifact directory. Session identity is harness-specific: Mush pre-assigns Claude Code session ids before launch, while Cursor assigns its own chat id and Codex its own thread id, which Mush captures from the output stream after launch and persists so the same conversation resumes. An interrupted process may resume the same session and task; it does not create a semantic retry. Only checkpoint revision creates a new task through `previous_task_id`.

Large harness output remains in durable task-owned files. SQLite stores concise Markdown evidence and the paths needed to inspect those artifacts. A successful work execution completes the work with its result and evidence; its checkpoint is a separate explicit step. A successful checkpoint execution records review evidence but leaves the decision to the existing accept, block, or revision operation.

An executing agent process receives its own task id in the `MUSH_TASK_ID` environment variable, so a coordinator can delegate by creating subtasks under itself and running them through the ordinary blocking CLI.

One narrow follow-up affordance exists: before a newly created revision begins, its description may be replaced with the checkpoint's bounded delta and it may reuse the subject's durable worktree. Once execution begins, both remain immutable.

## Runner

Mush is a task database plus a runner the caller runs. Creating and queueing a task records durable intent; nothing executes until some process is asked to run it. The runner role is a public command surface — `mush runner start`, `mush runner tick`, `mush runner serve`, and `mush runner status` — and it is how work runs without a human naming it.

- `runner start [task-id]` executes one task in the calling process and exits when that task reaches a terminal state. Without an id it claims the next eligible ready task, which is the one thing only this command does; with an id it executes that task in whatever readiness state it is in.
- `runner tick` performs one bounded pass — reconcile stale rows, then start every eligible ready task up to the concurrency bound — and exits without waiting for the work it started.
- `runner serve` repeats that pass until terminated, supervising the executions it starts as its own children so their exit status is direct evidence rather than inference.
- `runner status` reports what is pending, claimed, running, and parked, and whether anything currently holds the serve lock.

`tick` and `serve` start work only by spawning `runner start`, so every execution of queued work takes one path.

Execution has two front doors over one engine: `task run <id>` is the human one, for running a specific task now, with the worktree, session, and prompt options, and `runner start [id]` is the worker one, the only one that can pick its own task. Neither refuses the other's case. What bookkeeping an execution owes — whether it claims a launch delivery and records the queue's readiness transitions — follows from the task's durable readiness, which the command reads for itself rather than making the caller pick a command by it. Running a queued task in the foreground is therefore ordinary, and it advances the graph exactly as a worker execution would. The invariant is not that only the runner group executes queued work; it is that nothing starts work implicitly, and `task run` is as explicit as a command gets.

No other command starts work. Queueing, dependency edits, completion, and recovery record durable state and stop there, and observation is read-only with respect to execution and readiness. Mush never starts a runner implicitly. When nobody holds the role, queued work waits in the queue until a runner drains it: no work is lost and none is silently started, and `mush runner status` is how the queue depth is read.

Who holds the role is the caller's choice — an operator's terminal, a service manager, a container, a periodic scheduler, or an agent nudging its graph between turns — and none of those is privileged over the others. Mush ships no supervisor, no autostart, no installed unit file, and no background service; [the runner role guide](guides/runner-role.md) documents the supported ways to hold it.

Scheduling and recovery are the caller's judgment: when to tick, how often, whether to hold a long-lived `serve`, and whether to recover a parked task. Ownership, claim takeover, and relaunch eligibility are mechanism, stay in the domain, and stay deterministic. `task recover` remains the only relaunch path, and Mush still never invents a task, dependency, checkpoint, or revision. An agent may hold the runner role; it does so by calling the same public commands, and durable state records that the role is held, so a human, a service manager, or a later agent can take it over when that agent disappears.

### Execution liveness

Every execution of a task holds an exclusive advisory lock on that task's lock file for the whole of its life, whether it was started by `runner start` or by `task run`. The lock is taken before the execution claim commits and released by the operating system when the process ends for any reason, including a crash or a kill. Liveness is therefore a non-blocking lock attempt with an exact answer, available to any process: a task whose lock can be taken has no live execution, and a task whose lock is held is never taken over. There is no PID-reuse hazard, no grace period before an absent owner becomes an intervention, and no ambiguity between an owner and its child. A boot leaves no lock held, so a restarted machine needs no special proof; recorded boot ids and process identities remain diagnostics rather than decision inputs.

Concurrent `runner start` processes are expected and supported, because each holds only its own task's lock. `serve` additionally holds one exclusive serve lock for the database, so a second `serve` refuses rather than silently doubling the concurrency bound; `start` and `tick` take no serve lock, and other commands only test it to report whether the role is held. One serve lock per database matches the one-home-machine bound.

## Dependencies and observation

M4 introduces a fourth, independent task relation: a directed dependency records execution readiness between a prerequisite and a dependent task. It does not mean decomposition, semantic retry, or checkpoint adjudication and therefore does not replace or overload `parent_task_id`, `previous_task_id`, or `subject_task_id`. The coordinator constructs the graph incrementally by creating ordinary one-off work and checkpoint tasks and adding dependencies before dependent execution begins.

The coordinator is an interactive client rather than a resumable Mush task. Waiting is a CLI observation behavior: the coordinator may query current state, block until any or all selected tasks reach a terminal state, disconnect, or be replaced. Mush advances only queued nodes and dependencies that the coordinator already declared; when further judgment is required, the current or a later coordinator creates another task with bounded context from earlier results.

Recent task transitions remain inspectable through a bounded, monotonic audit journal. Queued ready tasks receive durable, idempotent launch delivery, so the work survives the shell that queued it and waits for [a runner](#runner) to claim it; stale work remains recoverable after process or machine restart.

The first executable slice supports dependencies between work tasks only, with successful work completion as the sole prerequisite condition. A work task may have at most eight direct prerequisites. Self, duplicate, cross-project, cyclic, and post-start edge changes are rejected; SQLite triggers independently protect the load-bearing graph rules.

Queueing gives a work task one of these durable readiness states: `blocked`, `ready`, `claimed`, `running`, `completed`, or `intervention_required` (`unqueued` precedes the queue). Queueing a task whose prerequisites are satisfied creates one launch delivery for its queue generation, and that delivery waits until a runner claims it. Delivery remains at-least-once: an execution takes its task's lock, atomically claims the delivery, and then invokes the executor, which records the running harness's process identity as a diagnostic. Reconciliation belongs to the runner rather than to every command's startup; it resets claims and executions whose task locks are free, and never touches a task whose lock is still held. A runner startup or execution failure retains a bounded intervention diagnostic until the operator uses the headless recovery operation. Recovery requires task or project scope, refuses to reset a task whose execution lock is held, clears stale running, interrupted, and intervention states, and explicitly requeues queued work with a new generation while preserving diagnostics from prior delivery generations. Recovery re-derives readiness from the dependency graph rather than assuming it, so a recovered task whose prerequisites are incomplete returns to `blocked` without a launch delivery.

Status and wait observations bound aggregate text and mark the observation when anything was elided. A required description that cannot fit is replaced with an explicit elision marker; optional result, evidence, identifier, and path fields that cannot fit whole are returned as null, so a truncated path is never mistaken for an artifact location. Consumers use the observation-level `elided` flag to distinguish that bounded response from complete data and inspect task artifacts for full output. The transition journal is an audit-only window of the most recent 10,000 state changes; current task rows, not replay of the journal, determine observation correctness.

Both front doors share that lifecycle without competing for ownership, because both hold the task's lock while they run: whichever takes the lock executes, and the other reports that the task already has a live execution and starts nothing. A live `task run` execution cannot be queued underneath. A foreground execution of a queued task claims that task's delivery and is reconciled like any other, while reconciling an abandoned execution of an unqueued task records the interruption and stops there, since an unqueued task owns no launch delivery. Completion, interruption, and streamed session capture are scoped to the execution attempt that owns them, so a stale executor cannot overwrite or interrupt a replacement attempt.

`task status` immediately returns current task rows and the latest audit cursor. `task wait` polls those durable rows until any or all of at most 32 named tasks are terminal or an optional seconds-based timeout expires; it is pure observation and neither reconciles nor starts work. Because nothing advances an unqueued task on its own, `task wait` names any unqueued task among those it was given on standard error before it begins waiting; the wait still proceeds, since a manual run or completion can finish an unqueued task. Observation limits each result-bearing field to 4 KiB and all returned task text to 32 KiB in aggregate; elided optional fields are null and full output remains in task artifacts. Waiting is client behavior and does not add a task state or resume a coordinator.

The audit journal covers queueing, readiness changes, delivery claims, execution-state changes, terminal completion and checkpoint decisions, interventions, and recovery. Ordinary task creation and edits to an unqueued graph do not affect execution readiness and are observed from current rows rather than treated as transition events.

## Planned concepts

Deeper delegation, project overlays, and cross-project review are not yet implemented. Their detailed semantics belong here only when implementation begins.
