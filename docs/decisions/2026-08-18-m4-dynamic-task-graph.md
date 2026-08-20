# M4 dynamic task graph and durable observation

Status: accepted on 2026-08-18. The launch-pump and same-boot liveness portions were superseded by [the explicit runner surface decision](2026-08-19-explicit-runner-surface.md), and its incrementally extended live graph was superseded as the maintained product direction by [the episodic execution graph decision](2026-08-20-episodic-execution-graphs.md).

## Context

The M3 gate exposed a lifecycle failure rather than a need to preserve a coordinator process. An interactive coordinator delegated useful cross-harness work, the launching context disappeared, the child later completed, and no durable mechanism advanced already-declared work or let the coordinator observe progress without becoming a conversational polling loop.

The coordinator does not need to be a resumable Mush task. It is an interactive client that incrementally constructs a graph of one-off tasks. Each work task runs once with the purpose supplied by its description and exact agent assignment; checkpoints retain their existing review semantics. If another agent turn is useful after observing results, the coordinator creates another task with the relevant bounded context. Mush does not silently turn the interactive coordinator into a background agent or resume its harness session.

Mush currently stores tasks and three independent relationships: `parent_task_id` for decomposition, `previous_task_id` for a later semantic attempt, and `subject_task_id` for checkpoint adjudication. These relationships do not express execution readiness. M4 needs the smallest separate dependency relation, durable task submission, transition observation, and recovery necessary for a graph that the coordinator builds as it learns.

This decision does not define a static workflow document, planning language, resident general-purpose daemon, arbitrary scheduler policy, or automatic planning. The graph is the durable set of tasks and dependency edges created through ordinary domain operations.

## Decision

### The coordinator is an interactive graph client

An interactive coordinator creates tasks, adds dependencies, inspects bounded results, and adds further tasks as new information arrives. It may remain attached, block in a shell command, poll explicitly, disconnect, or be replaced by another coordinator. None of those client states changes task semantics.

Mush persists no coordinator waiting or resumption state. `execution_status` therefore does not gain a `waiting` value. A coordinator that wants another agent turn creates a new task node and supplies the relevant bounded prior results. Same-session execution recovery remains available for an individual interrupted node, but it is not the workflow's advancement mechanism.

If the coordinator disappears, Mush advances only work whose nodes and dependencies have already been declared. A decision that was never represented as a task cannot happen automatically. A later coordinator reconstructs the current graph from durable state and adds the next nodes.

### Tasks are nodes and dependencies express readiness

Tasks remain the executable and reviewable units. Add a separate directed dependency relation from a prerequisite task to a dependent task. An edge means only that the dependent task cannot execute until the prerequisite reaches its required terminal condition. It does not mean decomposition, semantic retry, or checkpoint review and therefore does not overload the three existing task relationships.

The first slice supports one readiness condition between work tasks: successful work completion, meaning `status=completed` through successful execution or an explicitly supplied result. Checkpoint prerequisites and decision-sensitive readiness are deferred to the next graph unit, after the smallest executable work-task dependency chain has been used. More elaborate predicates, failure branches, joins with custom expressions, and data transformations remain out of scope.

The coordinator may add nodes and edges incrementally while the graph is running. A dependency may be added only while the dependent task has not started. The domain rejects self-dependencies, duplicate edges, cross-project edges, edges targeting started tasks, and cycles. Cycle prevention is guaranteed transactionally rather than left to the coordinator prompt.

Existing M3 delegation bounds remain unchanged. A task's parent records decomposition and remains limited to one level. Dependencies may connect otherwise valid tasks without changing parentage, but the first slice must impose a small documented per-task prerequisite limit and reject graphs whose size exceeds the active workflow's configured bound. M4 should learn from one real graph before accepting unbounded fan-out or depth.

### Submission makes execution independent of the launching shell

Creating a task does not execute it. A public submission operation marks a pending task as declared for execution. A submitted task is either blocked on prerequisites, ready, claimed by a runner, running, completed, or in a visible intervention state. These are operational readiness and delivery facts; semantic task status and harness execution status keep their existing meanings.

Submission starts a detached, task-specific runner when the node is ready. The runner owns exactly one task execution, records its claim and process identity durably, and may outlive the shell or conversation that submitted it. Mush does not depend on a harness background flag or the coordinator's process tree to keep work alive.

When a node reaches a terminal state, the transition-producing process evaluates its declared dependents. Newly ready nodes receive idempotent launch deliveries and short-lived task runners. This is automatic advancement of an already-declared graph, not automatic planning: Mush never invents a task, dependency, checkpoint, or revision.

If a process or machine stops, submitted blocked and ready nodes remain durable. Runner reservations, claims, and executions record a boot id and PID. Reconciliation automatically resets them only when the boot id changed, which proves the recorded process did not survive and resets the delivery's attempt budget. On the same boot Mush never takes over a row marked running: only the observed absence of every recorded runner and execution PID for the full grace period ages into visible intervention, and observing either PID alive clears the missing observation. The operator uses `task recover` to clear stale running, interrupted, or intervention state and explicitly relaunch submitted work; recovery skips demonstrably live current-boot executions. Missing executables, invalid harness configuration, exhausted launch attempts, or unrecoverable worktree/session state likewise become a specific intervention with a bounded diagnostic visible through CLI and TUI.

The first slice does not promise wall-clock progress while no Mush process can run. It promises that the launching client may disappear without killing a successfully detached runner and that any lost advancement remains durable, inspectable, and recoverable rather than silently forgotten.

### Recent transitions provide an audit cursor

Add a bounded task-transition journal in SQLite. Each committed domain transition that can affect readiness or observation receives a monotonically increasing event id and records the task id, transition kind, resulting semantic, execution, and readiness states, and commit time. The task transition, journal event, and any newly ready launch deliveries commit atomically. The journal retains the most recent 10,000 entries as an operational audit trail; current task and delivery rows remain authoritative.

The journal is audit-only observation state, not a replacement for current task rows, a durable event feed, or an agent transcript. Status and wait return the latest cursor as a snapshot marker but do not accept a resume cursor; wait polls and reconciles authoritative current rows. Building a resumable cursor consumer is deferred until a real external consumer requires retention and gap semantics.

### Waiting is a CLI observation behavior

Expose a headless blocking operation that waits for relevant task transitions without consuming coordinator model turns. The initial surface should support waiting for any or all named tasks to reach a terminal state, with an optional timeout and bounded JSON output. A separate status query provides an immediate poll when that is more convenient.

Illustrative commands are:

```sh
mush task status 41 42 --json
mush task wait 41 42 --until any --json
mush task wait 41 42 --until all --timeout 10m --json
```

The exact syntax may change during implementation, but status and blocking observation must remain distinct operations. The coordinator can issue one blocking subshell call, regain control after a meaningful transition or timeout, inspect bounded results, and extend the graph. Repeated conversational polling is unnecessary, but explicit quick polling remains valid.

An operating-system file notification may optimize the blocking implementation. Correctness remains in SQLite current rows: startup, every wake-up, and timeout reconciliation query current state. Lost, coalesced, or early filesystem notifications therefore cannot lose completion.

This observation channel is the M4 notification boundary. It notifies the waiting coordinator process of durable task progress; it does not implicitly notify the human or resume a coordinator harness. Additional human or external transports remain deferred until a real workflow requires them.

### Results passed between nodes are bounded

A completed prerequisite exposes a bounded result packet containing its task id, task kind, concise result, evidence summary, artifact paths, terminal states, and relationship ids. Commands that inspect a graph or render newly satisfied prerequisites return these packets deterministically and impose documented per-node and aggregate byte limits. Oversized content remains in task-owned artifacts. The bounded response carries an observation-level elision flag, required descriptions use an explicit marker, and optional fields become null rather than exposing misleading partial content.

Mush does not automatically transform or inject one node's result into another node's prompt unless the coordinator declared that input when creating or preparing the dependent task. The first slice may provide a public helper that renders selected prerequisite packets into a new task description, but the dependency edge itself carries readiness, not hidden prompt composition.

### Launch delivery is durable and idempotent

A node becoming ready creates one durable launch delivery keyed by task and submission generation. Delivery records `pending`, `claimed`, `delivered`, or `intervention_required`, together with attempt count, last attempt time, runner identity, and a bounded diagnostic.

Delivery is at least once, not exactly once. A runner marks launch delivered only after the task's durable execution claim commits. Existing task execution guards prevent two claims from becoming two concurrent semantic executions. Spawn failures retry up to three reservations before intervention. A previous-boot reservation or claim resets automatically; same-boot ownership requires explicit recovery only after every recorded owner PID has remained absent for the full grace period.

Every command that opens the database performs a bounded reconciliation pass, and an explicit headless recovery command handles larger or operator-requested recovery. Reconciliation must not turn an ordinary read into unbounded work.

## Public lifecycle

The intended first-slice sequence is:

1. An interactive coordinator creates one-off work or checkpoint tasks as needed, assigning each work task's purpose through its description and exact agent configuration.
2. It adds dependency edges before dependent tasks start and submits the nodes it wants Mush to execute.
3. Mush immediately launches ready roots while keeping dependent nodes durably blocked.
4. The coordinator may use an immediate status query, block on a concise wait command, or disconnect entirely.
5. A terminal node transition is journaled and atomically makes eligible declared dependents ready.
6. Short-lived runners claim and execute those nodes independently of the original shell.
7. When fresh judgment is required and no corresponding node was declared, the graph stops in a legible state until the current or a new interactive coordinator adds the next node.
8. Runner or readiness failures surface as specific interventions rather than absent progress.

Every TUI mutation or recovery action must have a non-interactive equivalent. A fresh coordinator must be able to understand current nodes, edges, readiness, results, and interventions without reconstructing prior agent transcripts.

## Required invariants

- The three existing task relationships retain their independent meanings; dependency edges express readiness only.
- A dependency connects tasks in one project, cannot target itself, and cannot create a cycle.
- A dependent task cannot gain or lose dependency edges after its first execution claim.
- A blocked submitted task cannot execute before every required prerequisite condition is satisfied.
- Readiness is calculated from durable task rows, never solely from an ephemeral wake-up.
- A task transition, journal event, and resulting launch delivery commit atomically.
- A submitted task has at most one launch delivery per submission generation.
- A delivery is not `delivered` until a durable execution claim exists.
- A previous-boot runner claim resets automatically, while same-boot ambiguity requires explicit recovery without starting concurrent executions of one task.
- Agent nodes run once unless existing infrastructure recovery resumes the same task or checkpoint revision creates a new semantic attempt.
- Observation correctness comes from current rows; the bounded audit cursor is not a resumable event-stream contract.
- Results and diagnostics returned to coordinators are bounded; full output stays in task-owned artifacts.
- Launch exhaustion and unrecoverable configuration, identity, session, or worktree failures are visible as interventions.

## Verification required

The implementation derived from this decision must include focused observable tests for:

- independent preservation of parent, previous-attempt, checkpoint-subject, and dependency meanings;
- rejection of self, duplicate, cross-project, late, cyclic, and over-limit dependency edges;
- readiness for work-task roots, chains, bounded fan-out, and bounded joins;
- submission without execution of blocked nodes and immediate execution of ready nodes;
- task runner survival after the submitting shell exits;
- atomic task transition, journal insertion, readiness update, and launch-delivery creation;
- idempotent delivery creation and claiming under concurrent transition handlers;
- spawn-failure retry exhaustion, automatic previous-boot recovery, same-boot refusal, and explicit recovery after a runner or execution disappears;
- blocking observation of transitions committed before, during, and after watcher setup, including lost or coalesced wake-up signals;
- `any`, `all`, and timeout behavior with deterministic bounded JSON;
- deterministic bounded prerequisite packets, truncation notices, and artifact references;
- a stopped graph when undeclared fresh judgment is required, followed by extension from a new coordinator;
- missing executable, invalid configuration, exhausted launch attempts, and unrecoverable worktree/session failures producing specific visible interventions;
- CLI JSON and deterministic TUI rendering for blocked, ready, claimed, running, terminal, and intervention states.

After focused tests pass, run `just check`. The first reviewed implementation packet must include the decision, schema and migration behavior, public dependency, submission, status, wait, and recovery commands, plus crash/restart evidence for one small dynamically constructed graph.

## Alternatives rejected

Persisting and automatically resuming the interactive coordinator is rejected. The coordinator is a client; if another agent turn is desired, it creates a new one-off task node.

Keeping a coordinator agent blocked on a child harness command is rejected as the execution mechanism because the M3 failure showed that the coordinator's process tree is not a durable ownership boundary. A coordinator may block on Mush's observation command because that command is optional client behavior and does not own child execution.

Representing coordinator waiting as a task execution state is rejected because waiting belongs to observation, not to a one-off node's semantic lifecycle.

Inferring readiness from `parent_task_id` is rejected because decomposition does not state execution order, and siblings or integration tasks may have readiness relationships independent of parentage.

Defining a static workflow file or planning language is rejected because the active gate requires the coordinator to add nodes as it learns. The live task and dependency records are the graph.

Relying only on filesystem notifications, process signals, or runner callbacks is rejected because each can be lost across the SQLite commit boundary. They may optimize wake-up; the journal and launch delivery remain authoritative.

Starting a resident general-purpose scheduler daemon is rejected for the first slice. Detached per-task runners, transition-triggered advancement, bounded startup reconciliation, and explicit recovery are sufficient to test the observed failure.

Exactly-once launch is rejected as an unsupported guarantee across process crashes. At-least-once delivery with transactional claims and task execution guards is the smaller honest contract.

Automatically notifying the human is rejected as a requirement of this slice. The durable blocking observation channel serves the coordinator; additional notification destinations need a demonstrated consumer and delivery contract.

## Consequences

Mush gains a fourth task relation with deliberately narrower semantics: dependency edges govern readiness, while the existing three relationships remain unchanged. It also gains a transition journal and durable launch-delivery state to cross process and crash boundaries.

The graph can advance through already-declared one-off agents after the launching conversation disappears. It intentionally stops when progress requires a planning decision that no task represents. That stop is ordinary legible graph state, not a failed coordinator resumption.

Interactive coordination remains context-efficient. A coordinator can block in one shell call, request a quick poll, disconnect, or be replaced; bounded durable state supplies the next view without replaying agent transcripts.

The roadmap's phrase "intentional wait and resume" is implemented as intentional client observation and durable graph advancement, not as a resumable coordinator process. The first real workflow will determine whether detached task runners and reconciliation are sufficient or whether an always-on component earns a later decision.

The smallest executable work-task dependency chain is the next slice defined by this boundary.
