# Abstractions

Mush provides a small model for declaring bounded collaborations among coding agents and deciding whether their results meet explicit criteria. This document owns the maintained domain concepts and invariants. [The product hypothesis](PRODUCT.md) explains why they exist, [the roadmap](ROADMAP.md) states which vertical slice is active, and the architecture records how the current implementation realizes them.

## Agent

An agent is an exact executor configuration: a harness, model identity, and relevant invocation settings. A task is assigned to one such configuration rather than to a capability alias or scheduler role.

Harness version is the one part of that configuration an agent may leave open. An agent may pin an exact harness version, which execution enforces before launch, or pin none and accept whatever version is installed. Pinning none is what keeps a configuration working across harness upgrades performed by whoever owns installation, and it does not weaken the record: every execution records the version that actually ran, so exactness that is not guaranteed in advance remains legible afterwards.

Configuration defects surface when an agent is registered or its settings are updated, not when a task first executes. A settings requirement introduced after an agent was registered is the one exception: stored settings are never migrated or defaulted, because inferring a value Mush does not own would make the choice on the operator's behalf, so the defect surfaces at that agent's next execution and is repaired with `mush agent update`. An executable checkpoint agent must carry non-empty general adjudication instructions; the task-specific criteria belong to each checkpoint rather than to the agent configuration. An adjudicator Mush cannot execute, such as a manual human adjudicator, needs no harness prompt. An executable is named as a path or as a command resolved on the executing environment's `PATH`, so a configuration need not encode how the harness was installed. Whether that name keeps resolving across upgrades belongs to whoever owns installation; Mush refuses to launch an executable it cannot run and report a version for, and additionally refuses one whose version contradicts a pin.

## Project

A project is a registered checkout with a name and path. Project configuration may provide a default checkpoint adjudicator, while an episode's declared collaboration policy determines which tasks need checkpoints and which exact agent or human adjudicates them. Repository-specific execution is exercised only through the harness adapters (Claude Code, Codex, and Cursor); verification and overlay behavior remain outside Mush's project model.

## Episode and execution graph

An episode is one bounded collaboration declared for a repository goal. Its execution graph `G_n` contains task nodes, readiness dependencies, semantic checkpoints, and finite control rules such as a maximum semantic-attempt count.

Declaring an episode does not require the user to author a static graph file. A human may declare it through the public operations, or a conversational coordinator may translate an ordinary goal and collaboration policy into the same inspectable graph. The declaration must nevertheless be bounded: graph growth after execution begins comes only from a control rule already present in the episode, not from Mush inventing fresh work or silently changing the collaboration.

The graph is durable execution memory while the episode is active. It can survive the declaring client and can be inspected or stepped by a human, a later coordinator, or a runner. It is not a permanent project plan. The repository remains the source of maintained product intent and results, and an episode is disposable once deleting it would not erase context required to understand the project or declare later work.

The coordinator, graph, and runner are separate roles. A coordinator exercises judgment to declare the collaboration, the graph externalizes that declaration and its current position, and a runner mechanically invokes eligible tasks. One process may hold more than one role without merging their responsibilities.

## Task

A task is one semantic attempt in one project and episode. Its description, assignment, resource budget, and relationships do not change after it starts. Execution advances its status and records its result.

The initial task kinds are:

- `work` for implementation, investigation, maintenance, or planning;
- `checkpoint` for adjudicating a named work result against declared criteria.

If completed work receives `not_met` and the episode's declared attempt budget permits another semantic attempt, the collaboration policy creates a new linked work task instead of editing the completed task or introducing a separate run record. Infrastructure retries resume the same task and do not consume semantic-attempt budget; see [Execution](#execution).

## Task relationships

Four independent relationships describe why tasks are connected:

| Relationship | Meaning |
| --- | --- |
| `parent_task_id` | This task contributes to a larger task. |
| `previous_task_id` | This is a new semantic attempt following an earlier task. |
| `subject_task_id` | This checkpoint adjudicates the named task's result. |
| dependency | The prerequisite's required outcome controls whether the dependent may execute. |

They are not interchangeable. For example, a later attempt can keep the same parent while its `previous_task_id` points to the prior attempt, its checkpoint names only that new attempt as its subject, and a checkpoint that records `met` may satisfy a separate readiness dependency.

A work task may be created under a parent to delegate a bounded piece of a larger task. Delegation is one level deep and work-only: the proposed parent must be a work task that itself has no parent, so a subtask cannot delegate further and a checkpoint can be neither parent nor child. The bound is expressed on the proposed parent rather than the caller because Mush cannot reliably know which process invoked its CLI; a consequence is that a subtask naming its own parent creates a sibling, which is permitted — the fence bounds comprehension depth, not caller identity, and a sibling remains legible at one level. The domain rejects violations with a specific error, and the store guarantees the same invariant in the schema.

A later attempt inherits its predecessor's parent, so another attempt at a subtask is bounded exactly as the subtask was, while another attempt at parentless work remains parentless.

Delegation ends in integration. A parent work task cannot complete while any direct child is not completed or is still recorded as running, whether the completion is supplied manually or by a successful execution, and a completed parent cannot gain new children. The domain rejects both directions with an error naming the blocking children, and the store guarantees the same invariant in the schema. Mush refuses the completion rather than waiting for, detaching, or cancelling the child: a refused executor completion leaves the parent interrupted, and the child remains available for explicit recovery.

## Checkpoint

A checkpoint is a task that receives a declared criterion packet and the result of one named work task, then answers one question: did this result meet these criteria?

The checkpoint packet contains:

- immutable criteria stated specifically enough for an adjudicator to apply;
- the subject task id and its bounded result, evidence, and artifact references;
- an exact agent assignment or a declaration that a human will adjudicate;
- one decision with bounded evidence explaining how the result relates to the criteria.

The decision is one of:

- `met`: the result satisfies every declared criterion;
- `not_met`: at least one criterion is not satisfied, with evidence identifying the concrete gap;
- `blocked`: the adjudicator cannot determine `met` or `not_met` without missing information, authority, or external state.

`blocked` is not a weaker form of `not_met`, and `not_met` does not itself mean "create a revision." The checkpoint owns semantic adjudication only. The episode's declared collaboration policy owns what each decision makes eligible.

A checkpoint is not generic critique. An agent may comment on a result without acting as a checkpoint, while a checkpoint must apply its declared criteria and produce exactly one decision. The adjudicator may use a different harness or model family than the subject's agent, but cross-family assignment is a collaboration choice rather than part of the checkpoint definition.

For the active slice, one work attempt has at most one checkpoint, and one checkpoint may contain several criteria that must all be met. Checkpoints do not have checkpoints. There is no parallel checkpoint, review, or run entity.

A graph-gating checkpoint and its criteria are declared before its subject starts, so the acceptance contract cannot move in response to the produced result. The checkpoint may be queued before the subject completes but remains awaiting its subject: it cannot execute or receive a decision until the named work task has completed successfully. It is never retargeted to a later attempt.

Executing a checkpoint presents the same bounded packet to a human or agent adjudicator. Successful checkpoint execution is not complete merely because critique text was produced; it must record one valid decision and its evidence. A manual decision uses the same domain operation and owes the same packet.

```text
work attempt 41
└── checkpoint 42, subject_task_id: 41, criteria: C
    ├── met → declared success dependents become eligible
    ├── blocked → episode stops for missing judgment or state
    └── not_met
        ├── budget remains → work attempt 48, previous_task_id: 41
        │                      └── checkpoint 49, same criteria C
        └── budget exhausted → episode stops
```

## Semantic-attempt budget

A budgeted loop is part of the episode's collaboration policy, not part of the checkpoint decision. It declares the work assignment, checkpoint criteria and adjudicator, the maximum number of semantic work attempts, and the success continuation. Downstream work waits on the loop's success continuation rather than on the first materialized checkpoint, because any permitted attempt may be the one whose checkpoint records `met`.

The maximum counts the initial work attempt. If `max_attempts` is three, the policy can materialize at most three immutable work tasks and three corresponding checkpoints. Infrastructure relaunches and same-session recovery resume one task and do not consume this budget.

When a checkpoint records `not_met` and budget remains, the policy creates a new work task linked through `previous_task_id`. The new attempt receives the original work purpose plus bounded checkpoint evidence, and its new checkpoint uses the same criteria. The previous work result and checkpoint decision remain unchanged.

`met` satisfies the loop's success continuation regardless of which permitted attempt produced it. `blocked` stops immediately. `not_met` with no remaining attempt stops as budget exhausted. Changing the criteria, adjudicator policy, or maximum after execution begins is fresh judgment rather than another iteration of the declared loop.

## Evidence

Evidence begins as bounded Markdown stored with a task. Work evidence can cite commands, tests, manually supplied references, observations, and unresolved concerns. Checkpoint evidence maps the decision back to the declared criteria instead of offering unbounded general commentary. Large harness execution output lives in task-owned artifact files whose paths stay in the task evidence; richer evidence types remain deferred.

Mush will exercise the checkpoint packet in real collaborations before introducing structured criteria or separate evidence records.

## Execution

Execution state lives directly on a task rather than in a parallel run entity. An executing task records the exact executable version observed at launch, the model, harness-specific invocation settings, session, worktree name, infrastructure attempt count, process identity, and artifact directory. Session identity is harness-specific: Mush pre-assigns Claude Code session ids before launch, while Cursor assigns its own chat id and Codex its own thread id, which Mush captures from the output stream after launch and persists so the same conversation resumes. An interrupted process may resume the same session and task; it does not create a semantic retry. Only a declared collaboration policy or fresh coordinator judgment creates a new task through `previous_task_id`.

Large harness output remains in durable task-owned files. SQLite stores concise Markdown evidence and the paths needed to inspect those artifacts. A successful work execution completes the work with its result and evidence; its checkpoint is a separate explicit task. A successful checkpoint execution must record `met`, `not_met`, or `blocked` with evidence against the criteria; critique without adjudication does not complete the checkpoint.

An executing agent process receives its own task id in the `MUSH_TASK_ID` environment variable, so a coordinator can delegate by creating subtasks under itself and running them through the ordinary blocking CLI.

Before a newly materialized semantic attempt begins, its description includes the bounded `not_met` evidence and it may reuse the predecessor's durable worktree when the declared policy permits that execution choice. Once execution begins, its purpose, relationships, and worktree choice remain immutable.

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

Scheduling and recovery are the caller's judgment: when to tick, how often, whether to hold a long-lived `serve`, and whether to recover a parked task. Ownership, claim takeover, and relaunch eligibility are mechanism, stay in the domain, and stay deterministic. `task recover` remains the only infrastructure relaunch path. Mush never invents a task, dependency, checkpoint, or semantic attempt; it may materialize one only when the episode declaration or later fresh judgment supplies the rule. An agent may hold the runner role; it does so by calling the same public commands, and durable state records that the role is held, so a human, a service manager, or a later agent can take it over when that agent disappears.

### Execution liveness

Every execution of a task holds an exclusive advisory lock on that task's lock file for the whole of its life, whether it was started by `runner start` or by `task run`. The lock is taken before the execution claim commits and released by the operating system when the process ends for any reason, including a crash or a kill. Liveness is therefore a non-blocking lock attempt with an exact answer, available to any process: a task whose lock can be taken has no live execution, and a task whose lock is held is never taken over. There is no PID-reuse hazard, no grace period before an absent owner becomes an intervention, and no ambiguity between an owner and its child. A boot leaves no lock held, so a restarted machine needs no special proof; recorded boot ids and process identities remain diagnostics rather than decision inputs.

Concurrent `runner start` processes are expected and supported, because each holds only its own task's lock. `serve` additionally holds one exclusive serve lock for the database, so a second `serve` refuses rather than silently doubling the concurrency bound; `start` and `tick` take no serve lock, and other commands only test it to report whether the role is held. One serve lock per database matches the one-home-machine bound.

## Dependencies and observation

The fourth, independent task relation is a directed dependency recording execution readiness between a prerequisite and a dependent task. It does not mean decomposition, semantic retry, or checkpoint adjudication and therefore does not replace or overload `parent_task_id`, `previous_task_id`, or `subject_task_id`. A human or coordinator declares dependencies before the dependent begins, and execution may add only nodes and edges determined by a finite control rule already present in the episode.

The coordinator is an interactive client rather than a resumable Mush task. Waiting is a CLI observation behavior: the coordinator may query current state, block until any or all selected tasks reach a terminal state, disconnect, or be replaced. Mush advances only queued nodes, dependencies, and bounded loop transitions the episode already declared. When further judgment is required, the current or a later coordinator declares later work from bounded results and repository truth rather than mutating the episode as though it were a permanent project plan.

Recent task transitions remain inspectable through a bounded, monotonic audit journal. Queued ready tasks receive durable, idempotent launch delivery, so the work survives the shell that queued it and waits for [a runner](#runner) to claim it; stale work remains recoverable after process or machine restart.

An edge's dependent is a work task with at most eight direct prerequisites. A work prerequisite is satisfied by successful completion. A checkpoint-gated success continuation is decision-sensitive: `met` on any permitted attempt satisfies it, `not_met` leaves it blocked and invokes the declared attempt policy, and `blocked` advances nothing. The continuation is part of the loop declaration rather than an edge permanently attached to its first materialized checkpoint. A checkpoint is additionally gated by its subject without any explicit dependency: it cannot become ready, execute, or receive a decision until its named work result exists. Self, duplicate, cross-project, cyclic, and post-start edge changes are rejected, and subject links participate in cycle prevention as readiness gates.

A checkpoint whose assigned adjudicator Mush can execute may be queued while it awaits its subject; a checkpoint assigned to a human cannot be claimed by a runner and remains available through the TUI or headless decision operation. Agent and human paths both complete the checkpoint by recording one decision and its evidence. Producing observations without `met`, `not_met`, or `blocked` leaves the adjudication incomplete rather than turning critique into a terminal checkpoint.

Queueing gives a task one of these durable readiness states: `blocked`, `ready`, `claimed`, `running`, `completed`, or `intervention_required` (`unqueued` precedes the queue). Queueing a task whose prerequisites are satisfied creates one launch delivery for its queue generation, and that delivery waits until a runner claims it. Delivery remains at-least-once: an execution takes its task's lock, atomically claims the delivery, and then invokes the executor, which records the running harness's process identity as a diagnostic. Reconciliation belongs to the runner rather than to every command's startup; it resets claims and executions whose task locks are free, and never touches a task whose lock is still held. A runner startup or execution failure retains a bounded intervention diagnostic until the operator uses the headless recovery operation. Recovery requires task or project scope, refuses to reset a task whose execution lock is held, clears stale running, interrupted, and intervention states, and explicitly requeues queued work with a new generation while preserving diagnostics from prior delivery generations. Recovery re-derives readiness from the dependency graph rather than assuming it, so a recovered task whose prerequisites are incomplete returns to `blocked` without a launch delivery.

Status and wait observations bound aggregate text and mark the observation when anything was elided. A required description that cannot fit is replaced with an explicit elision marker; optional result, evidence, identifier, and path fields that cannot fit whole are returned as null, so a truncated path is never mistaken for an artifact location. Consumers use the observation-level `elided` flag to distinguish that bounded response from complete data and inspect task artifacts for full output. The transition journal is an audit-only window of the most recent 10,000 state changes; current task rows, not replay of the journal, determine observation correctness.

Both front doors share that lifecycle without competing for ownership, because both hold the task's lock while they run: whichever takes the lock executes, and the other reports that the task already has a live execution and starts nothing. A live `task run` execution cannot be queued underneath. A foreground execution of a queued task claims that task's delivery and is reconciled like any other, while reconciling an abandoned execution of an unqueued task records the interruption and stops there, since an unqueued task owns no launch delivery. Completion, interruption, and streamed session capture are scoped to the execution attempt that owns them, so a stale executor cannot overwrite or interrupt a replacement attempt.

`task status` immediately returns current task rows and the latest audit cursor. `task wait` polls those durable rows until any or all of at most 32 named tasks are terminal or an optional seconds-based timeout expires; it is pure observation and neither reconciles nor starts work. Because nothing advances an unqueued task on its own, `task wait` names any unqueued task among those it was given on standard error before it begins waiting; the wait still proceeds, since a manual run or completion can finish an unqueued task. Observation limits each result-bearing field to 4 KiB and all returned task text to 32 KiB in aggregate; elided optional fields are null and full output remains in task artifacts. Waiting is client behavior and does not add a task state or resume a coordinator.

The audit journal covers queueing, readiness changes, delivery claims, execution-state changes, terminal completion and checkpoint decisions, interventions, and recovery. Ordinary task creation and edits to an unqueued graph do not affect execution readiness and are observed from current rows rather than treated as transition events.

## Planned concepts

Deeper delegation, project overlays, and cross-project review are not yet implemented. Their detailed semantics belong here only when implementation begins.
