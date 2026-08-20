# The explicit runner surface

Status: proposed on 2026-08-19 as a revision of the M4 dependency-chain slice. Supersedes the launch-pump and same-boot liveness portions of [the M4 dynamic task graph decision](2026-08-18-m4-dynamic-task-graph.md); every other part of that decision stands.

Amended 2026-08-20, before implementation was accepted, in two connected ways. The operation that declares work for execution is named `queue` rather than `submit`, throughout the product and this document. And the requirement that commands warn on standard error when no process holds the serve lock is withdrawn, along with its verification bullet. The warning existed to compensate for the word: "submit" implies work was handed to something that will act on it, so a silent queue was surprising and needed an apology at runtime. "Queue" states the same fact in the name, and a queue that waits for a worker is the word behaving correctly. Renaming the operation and deleting the warning are one change, not two, and the schema had not shipped, so the stored vocabulary was renamed at no migration cost. Because "queued" was already in use for a checkpoint waiting on its subject task, that older sense was renamed to "blocked", the word this product already uses for an unsatisfied prerequisite; whether the two waiting mechanisms should eventually become one is left open.

Amended again 2026-08-20, before implementation was accepted, to remove a contradiction this record carried against itself. A verification bullet left standing from an earlier draft still required `runner start` to refuse an unqueued task and `task run` to refuse a queued one, which the later accepted rule — both front doors execute a task in whatever state it is in, and select delivery and readiness bookkeeping from durable state — had already replaced. The bullet now states that rule. The behavior was never changed back to refusal, and mutual refusal remains rejected below.

## Context

The first M4 dependency-chain implementation introduced dependency edges, durable queueing and readiness, per-generation launch delivery, detached task-specific runners, bounded observation, and explicit recovery. The domain half of that work is sound and is not in question here.

The operational half is. That implementation has no component whose job is to run work, so it distributed that job across every process that happens to open the database. Any mutating command pumps pending launches after its mutation, every command reconciles at startup, and `task wait` reconciles and pumps on a 250 ms poll. A user who is told "Mush is a task database" and then discovers that the command declaring work spawns background processes, that an unrelated command may spawn them later, and that a blocking observation command is also a scheduler, has been handed a model that does not match anything they already know.

The same distribution produced the implementation cost. Because no process is the parent of a running execution, the implementation had to reconstruct process liveness from evidence: `/proc` probing by PID, a persisted first-missing-observation timestamp, a grace period before an absent owner becomes an intervention, boot-id comparison as the only trustworthy self-healing proof, and reservation debouncing so two clients do not both spawn the same runner. That cluster is the direct consequence of orphan-by-design execution with no ownership primitive.

Mush is a headless CLI and MCP surface for agents. That framing describes the interface, not the runtime, and it has been read as a prohibition on any process that runs work. The available answers are not "smear the runner across the CLI" or "ship a resident daemon". Task runners conventionally ship the worker and not the supervisor: the loop is a public command, and the operator chooses whether a terminal, a service manager, a container, a periodic scheduler, or an agent holds it. That answer preserves the simple mental model, gives callers a real interface to build their own loops on, and keeps Mush free of autostart, supervision, and a resident service to version-skew against.

## Decision

### Running work is a public command surface

Add `mush runner` as a first-class command group. It is the only surface that executes queued work.

`mush runner start [task-id]` executes one task in the calling process and exits when that task reaches a terminal state. Without an id it claims the next eligible ready task; with an id it executes that task in whatever readiness state it is in. It takes the task's execution lock, claims the current launch delivery only when the task is queued, runs the harness, and records the terminal transition. This promotes unit 2's hidden `mush runner <id>` subcommand to a public, documented operation, and it is the whole worker: an operator, a service manager, or a container that restarts `mush runner start` in a loop has a worker, and several of them have a worker pool.

`mush runner tick` performs one bounded pass: reconcile stale rows, then start every eligible ready task up to the concurrency bound, and exit. It does not wait for the work it starts. This is the primitive for callers who want work moving without being occupied, including periodic schedulers and an agent nudging its graph between turns.

`mush runner serve` repeats that pass until terminated, supervising the executions it starts as its own children so their exit status is direct evidence rather than inference.

For that evidence to be readable, `runner start` distinguishes its outcomes by exit code, and the codes are part of the public surface. One code means an execution ran to a terminal state, one distinct code means no execution was started at all — because nothing was eligible, or because another process already holds the task — and any other nonzero code means the execution itself failed. A supervisor therefore reads the outcome instead of probing the lock afterwards to guess whether a nonzero child had merely lost a race, and a restart loop can tell "nothing to do, back off" from "something is broken".

`tick` and `serve` start work by spawning `mush runner start <task-id>` for each eligible task, so the command group is self-hosting and there is one execution path rather than two. The concurrency bound is a single documented constant, initially four simultaneous executions, sized for agent harnesses rather than for the reservation batch it replaces. It bounds what a `tick` or `serve` pass starts, not the database: an explicitly invoked `runner start` takes only its own task's lock and is deliberately not throttled by it, because the caller asked for that task by name.

`mush runner status` reports what is pending, claimed, running, and parked, and whether anything currently holds the serve lock.

`start` takes only its own task's execution lock, so concurrent `start` processes are expected and supported. `serve` additionally holds one exclusive serve lock for the database, so a second `serve` refuses rather than silently doubling the concurrency bound.

### No other command starts work

`task queue`, `task depend`, `task undepend`, `task complete`, and `task run` stop pumping launches. Startup reconciliation is removed from every command. `task wait` becomes pure observation: it polls current rows and returns, and it no longer reconciles or launches.

Nothing then advances queued work on its own, and the command that queues it says so by its name. Work is *queued*, not *submitted*: a queue is a place things wait until a worker drains it, so an idle queue with no runner is the word behaving as advertised rather than a surprise needing an apology. Mush never starts a runner implicitly, and never starts one on the user's behalf from a command whose name does not say so.

`task run` and `runner start` are one engine behind two front doors, and neither refuses the other's case. `task run <id>` is the human door: run this task now, in the foreground, with the worktree, session, and prompt options. `runner start [id]` is the worker door, and its no-id form — claim the next eligible task — is the thing only it does and what `tick` and `serve` spawn. Either will execute a task in whichever state it is in.

Whether a launch delivery is claimed and which readiness transitions are recorded follow from the task's durable state, not from which command was typed. An execution takes the task's lock, and if the task is queued it claims the current delivery and records the queued transitions, so a foreground run advances the graph exactly as a worker run does. Making the caller select the command to match a fact the system already knows would be asking the user to do the system's bookkeeping, and it would mean a queued task could not be run by hand when nothing holds the runner role — a real loss with no safety return, since the per-task lock, not the command name, is what prevents two executions of one task.

The resulting model is the one a reader can already predict: a task database, and a runner you run.

### Liveness is an operating-system lock, not a process identity

An execution holds an exclusive advisory lock on a per-task lock file for the whole of its life, taken before the execution claim commits and released by the kernel when the process ends for any reason, including a crash or a kill. Liveness is therefore a non-blocking lock attempt with an exact answer, available to any process, with no PID reuse hazard and no ambiguity between an owner and its child.

This removes `/proc` probing, `tasks.first_missing_at`, the unverifiable-execution grace period, boot-id-based automatic reset, and reservation debouncing. A boot leaves no lock held, so a reboot needs no special proof; the recorded boot id and PIDs remain as diagnostics and stop being decision inputs.

The at-least-once delivery contract, the durable claim, and the existing task execution guards are unchanged. The lock replaces how Mush decides whether an owner is alive, not whether an owner exists.

One property of advisory locks is load-bearing and easy to get wrong, so it is recorded here rather than left to the implementation. The lock belongs to the open file description, not to the file descriptor, and a descriptor duplicated into a child process shares it; closing one descriptor releases nothing until the last one closes. A process that spawns a harness while holding a lock therefore appears to hold that lock after releasing it, for as long as the child takes to reach `exec`. Releasing must be an explicit unlock of the description rather than a close, which takes effect at once however many descriptors refer to it.

That leaves one window an orderly release cannot close. A process killed outright runs no release at all, so its lock falls to the kernel closing its descriptors — and if it is killed during the moment between forking a harness and that child reaching `exec`, the child still shares the description and the lock outlives the kill until it does. The window is bounded by `exec`, needs no grace period, and closes on its own. It is recorded because it is the one case where "a killed execution is immediately not live" is not literally instantaneous, and because a reader who does not know it will mistake it for a liveness bug.

### Advancement stays mechanical; scheduling and recovery are the caller's judgment

A caller decides when to tick, how often, whether to hold a long-lived `serve`, and whether to recover a parked task. Those are policy and may come from an agent.

No caller decides who owns a task, whether a claim may be taken over, or whether a task looks safe to relaunch. Those are mechanism, remain in the domain, and remain deterministic. `task recover` stays the only relaunch path, and Mush still never invents a task, dependency, checkpoint, or revision.

An agent may hold the runner role. It does so by calling the public surface, and the durable state records that the role is held, so a human, a service manager, or a later agent can take it over when that agent disappears. This is not M3's failure repeated: the ownership boundary is declared in the database rather than implied by a process tree.

### Supervision is the operator's choice

Mush ships no supervisor, no autostart, no installed unit file, and no background service. `docs/guides/` documents a `systemd --user` unit, a container invocation, and a periodic `runner tick` as supported ways to hold the role, and none of them is privileged over an operator running `mush runner serve` in a terminal or an agent ticking between turns.

Each project keeps one home machine, so one serve lock per database is a sufficient bound for this slice.

### One process migrates, and one-way migrations back up first

Schema migration requires the database's serve lock, whoever performs it, so a starting runner and a concurrent CLI command cannot migrate the same database at once. A process that finds a schema older than its target and cannot take the lock refuses and names the process holding it, rather than migrating underneath it.

Before applying a migration that cannot be reversed, Mush copies the database file to `<database>.pre-v<version>.bak` and refuses to migrate if that copy fails. This replaces the manual "back up before first use" operator note with a guarantee.

The dependency-chain implementation has not shipped, so its schema version 10 must never ship. The dependency-chain schema, the removal of `first_missing_at`, and the lock-based liveness collapse into a single migration to version 11. From version 11 onward the database records the oldest binary version permitted to open it, and a binary that is older refuses rather than opening; the pre-M4 hazard, where an old binary silently re-stamps `user_version` and completes tasks without advancing dependents, is therefore the last one of its kind and remains real only for databases already migrated by an unreleased build.

A newer binary against an older database migrates under the lock. An older binary against a newer database refuses. Downgrade is unsupported, and the pre-migration backup is its only path.

## Public lifecycle

1. An interactive coordinator creates one-off tasks, adds dependency edges, and queues the nodes it wants executed. Nothing executes yet.
2. Someone holds the runner role: an operator running `mush runner serve`, a `systemd --user` unit, a restart loop over `mush runner start`, a periodic `mush runner tick`, or the coordinator agent itself.
3. Each execution takes its task's lock, claims the ready delivery, and runs the harness.
4. A terminal transition journals the node, advances eligible declared dependents to ready, and creates their deliveries atomically. The next pass starts them.
5. The coordinator observes with `task status` or blocks on `task wait`, disconnects, or is replaced. Observation never starts work.
6. If nothing holds the runner role, queued work waits durably in the queue. No work is lost and none is silently started.
7. Failures park the task with a bounded diagnostic. `task recover` is the explicit relaunch boundary.

## Required invariants

- Nothing starts work implicitly. `runner start` and `task run` start an execution because a caller said so by name; no other command spawns a process that runs work, and no mutation, observation, or startup path does it as a side effect.
- `task wait` and `task status` are read-only with respect to execution and readiness.
- An execution holds its task's lock for the whole of its life, and the lock is taken before the execution claim commits.
- A task whose lock can be taken has no live execution, and a task whose lock is held is never taken over.
- Concurrent `start` processes are safe and expected; at most one `serve` holds a database at a time.
- `tick` and `serve` start work only by spawning `runner start`, so every execution takes the same path.
- Readiness, delivery, claim, and advancement semantics from the M4 dynamic task graph decision are unchanged.
- Scheduling and recovery may be decided by a caller; ownership and relaunch eligibility may not.
- Schema migration happens only while holding the database's serve lock, and a one-way migration only after a successful backup.
- A binary older than the database's recorded minimum refuses to open it.

## Verification required

- `runner start`, `runner tick`, and `runner serve` each execute queued ready work, and no other command does under any ordering.
- `runner start` without an id claims the next eligible task and with an id claims that task, and neither front door refuses the other's case: `runner start` executes an unqueued task and claims no delivery, while `task run` executes a queued task by claiming its delivery and recording its readiness transitions, so the graph advances the same way whichever was typed.
- Several concurrent `runner start` processes drain a ready queue without two of them claiming one task.
- A chain advances across `tick` boundaries and across a `serve` restart, with the queueing shell gone.
- `serve` reaps a child execution and records its failure from the exit status without inference, and a child that merely lost a race to another starter is distinguished by its exit code rather than by probing the lock afterwards.
- A second `serve` against one database refuses; a `tick` concurrent with a `serve` does not double-start a task.
- A `tick` or `serve` pass starts at most four executions, counting each execution it supervises once: when one of a `serve`'s children finishes it starts the next eligible task while its remaining children are still live, rather than waiting for the batch to drain. The bound is per pass rather than a database-wide reservation, so concurrent explicit `runner start` processes remain supported and untouched by it.
- A killed execution releases its lock and is immediately recognised as not live, with no grace period and no boot-id involvement.
- A killed `serve` leaves its orphaned executions locked and live, and a later `tick` neither takes them over nor parks them.
- The wait command performs no reconciliation and no launch, and `runner status` reports queue depth so a caller can see waiting work without being warned about it.
- Migration refuses without the lock, backs up before a one-way step, and an older binary refuses a newer database.
- The existing dependency, readiness, delivery, observation, and recovery tests continue to pass unchanged where the decision does not change their behavior.

## Alternatives rejected

Keeping the distributed launch pump is rejected. It makes every command a scheduler, contradicts the model the product name and documentation promise, and directly causes the orphan-liveness ambiguity this revision removes.

Shipping a resident daemon that Mush installs and supervises is rejected for this slice. It would deliver the same mechanical benefit as `runner serve` while adding autostart, service supervision, an upgrade path that can leave an old service running against a new schema, and a background component users must reason about even when they never touch it. `runner serve` is that daemon, minus the claim that Mush owns its lifecycle.

An agent blocked on a child harness command as the execution mechanism remains rejected, exactly as in the M4 dynamic task graph decision: a process tree is not a durable ownership boundary. An agent holding the public runner role is a different thing and is supported, because the role is recorded in durable state and can be taken over.

Process identity as the liveness primitive is rejected. PID and boot-id comparison cannot distinguish a reused PID from a survivor, cannot distinguish an owner from its child, and forces a timing heuristic in place of an answer.

Scheduling policy inside `serve` is rejected for this slice. It runs eligible work up to one documented concurrency bound, in task order. Priorities, fairness, per-project limits, and configurable concurrency wait for a workload that demonstrates the need, because a runner with policy is the resident scheduler this decision declines to build.

Automatically starting a runner from a mutating command is rejected. It would restore the smeared model in a less visible form.

Making `task run` and `runner start` refuse each other's case is rejected. It was tried, and it made the caller responsible for matching the command to a readiness state the system already knew, while making a queued task unrunnable by hand precisely when nothing held the runner role. The two commands remain separate because a human door with worktree, session, and prompt options and a worker door that can pick its own task are genuinely different affordances — not because the execution underneath differs.

Folding them into a single command is still rejected, for that reason and no longer for the readiness one.

## Consequences

Mush gains a public execution surface and loses implicit background behavior. The mental model becomes a task database plus a runner the caller runs, which matches what a reader already expects from a task runner and is teachable in one sentence.

Callers gain a real interface rather than a side effect. A periodic scheduler, a service manager, a container, a bespoke loop, and an agent are all first-class ways to hold the runner role, and Mush needs to know about none of them.

The implementation loses the orphan-liveness cluster: `/proc` probing, the missing-observation timestamp, the grace period, boot-id reset, reservation debouncing, and startup reconciliation in every command. The dependency, readiness, delivery, journal, observation, and recovery work survives unchanged.

Queued work no longer advances when nobody holds the runner role. That is a real behavior change, and it is the price of the explicit model. It needs no runtime warning because the vocabulary carries it: work put in a queue is understood to wait for a worker.

This decision revises the M4 dependency-chain slice in place: the runner surface is how that chain executes, so the branch is corrected before it ships rather than merged and repaired later.

On implementation, [docs/ABSTRACTIONS.md](../ABSTRACTIONS.md) records the runner role and the lock-based liveness, [docs/ARCHITECTURE.md](../ARCHITECTURE.md) replaces the launch-pump and reconciliation description, [README.md](../../README.md) states that running work requires a runner, `docs/guides/` documents the supported ways to hold the role, and [docs/ROADMAP.md](../ROADMAP.md) describes the dependency-chain slice as delivered by the runner surface.
