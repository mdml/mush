# Architecture

Mush is a local Rust application for coordinating coding-agent work. It exposes a headless CLI for agents and automation and a TUI for human operation. Both interfaces use the same domain operations and persistent state, and tasks execute through per-harness adapters for Claude Code, Codex, and Cursor.

## Component boundaries

The diagram names the ownership boundaries. The executor dispatches on the agent's registered harness (`claude-code`, `codex`, or `cursor`) to a per-harness adapter that owns argument construction, version verification, session semantics, worktree ownership, and result parsing; the shared execution lifecycle owns task lookup, worktree and session bookkeeping, artifact directories, store transitions, and evidence assembly.

```text
CLI ─┐
    ├──> domain services ──> SQLite and task artifacts
TUI ─┘              │
                 ├──> harness adapters ──> coding-agent CLIs
                 └──> project adapters ──> repositories and overlays
```

- **CLI** provides non-interactive operations for humans, agents, scripts, and external tooling.
- **TUI** presents task state, review work, evidence, and operational health. Snapshot derivation, selection and key handling, checkpoint eligibility, task-detail rendering, and frame drawing are separated from terminal setup and event reads so deterministic behavior can be tested without an interactive terminal.
- **Domain services** own task transitions, relationships, checkpoint decisions, and policy.
- **Store** persists domain state, dependency readiness, transition cursors, and launch delivery in SQLite.
- **Task artifacts** hold large, inspectable execution outputs outside SQLite.
- **Harness adapters** translate domain requests into invocations of existing coding harnesses without deciding product semantics.
- **Runner** is the execution engine, reached through two front doors: `mush runner start`, the worker one, and `mush task run`, the human one. `runner start` is a short-lived process that claims and executes exactly one task independently of the shell that queued it, and it alone can pick its own task; `task run` names a task and adds the worktree, session, and prompt options. Neither refuses the other's readiness case, because the engine reads the task's durable readiness to decide whether the execution claims a launch delivery. `runner tick` and `runner serve` are bounded and continuous passes that start `runner start` processes. Capacity calculation and child-exit classification are pure seams; process spawning, reaping, and store mutations remain at the boundary. See [ABSTRACTIONS.md](ABSTRACTIONS.md#runner) for the role and [the runner role guide](guides/runner-role.md) for how an operator holds it.
- **Project adapters** do not exist yet; they will integrate registered repositories, worktree behavior, verification commands, native documentation, and private overlays.

Interface code depends on domain services rather than directly implementing task transitions. Harness and project adapters execute boundary-specific operations but do not decide whether work is accepted.

A harness's native subagents hide delegation from Mush, so adapters keep them off by default and subtask-enabled agents delegate through `mush`. The Claude Code adapter disallows the CLI's built-in subagent tool unless the agent's registered settings explicitly opt in, and the Codex adapter disables the CLI's `multi_agent` feature on the same terms. cursor-agent has no per-invocation tool-disable flag, so Cursor subagent policy lives in machine-level Cursor permissions managed outside Mush; that asymmetry is a recorded boundary, not an accident.

Harness configuration divides into three ownership layers, recorded in [the agent-configuration-ownership decision](decisions/2026-08-20-agent-configuration-ownership.md). The machine owns durable state — installation, authentication, machine-wide permission policy, sandbox availability — and Mush never writes it. The operator owns autonomy and capability through registered agent settings in the vendor's own vocabulary, relayed verbatim: Claude Code's `permission_mode`, Codex's `sandbox` and `approval_policy`, and Cursor's required `approval_mode`. Mush itself owns only the invocation flags its contract needs: output format, session identity, worktree placement, native-subagent visibility, and workspace trust. Ambient machine posture is observed in launch manifests and captured streams rather than managed. [The harness setup guide](guides/harness-setup.md) records what each adapter assumes about the machine and how an operator expresses the rest in settings.

Cursor's `approval_mode` is required and has no default. The adapter translates it into exactly one cursor-agent flag — `unrestricted` becomes `--force` and `auto-review` becomes `--auto-review` — refuses `allowlist` for an executable agent because it cannot run headless, and keeps `--trust` as Mush-owned execution enablement.

## Repository organization

The small implementation lives in modules matching these ownership boundaries:

```text
src/
├── main.rs          CLI entry point
├── tui.rs           interactive TUI and deterministic snapshot rendering
├── domain.rs        domain types and errors
├── executor.rs      shared execution lifecycle
├── executor/
│   └── harness.rs   per-harness argument, version, session, and result translation
├── runner.rs        the runner surface: start, tick, serve, and status
├── lock.rs          exclusive advisory locks for executions and for serve
├── store.rs         Store facade and shared persistence surface
├── store/
│   ├── schema.rs      database opening, backup, and versioned migration
│   ├── tasks.rs       project, agent, task, and checkpoint operations
│   ├── execution.rs   attempt ownership, sessions, and completion
│   ├── graph.rs       dependency mutation and validation
│   ├── delivery.rs    launch claims and runner counts
│   ├── recovery.rs    recovery, reconciliation, and intervention
│   ├── observation.rs bounded deterministic observation
│   ├── query.rs       shared row decoding
│   └── workflow.rs    transition, readiness, and launch-invariant helpers
└── lib.rs           library surface and state-path resolution
tests/
├── common/
├── unit/            focused unit targets for production-only coverage measurement
├── manual_control_loop.rs
├── m4_dependency_chain.rs
├── quality_coverage.rs
└── store_quality_invariants.rs
docs/
├── ARCHITECTURE.md
├── ABSTRACTIONS.md
├── CONTRIBUTING.md
├── VERIFICATION.md
├── ROADMAP.md
├── decisions/
└── guides/
```

The executor and store directories keep boundary-specific responsibilities visible while their root modules remain the stable facades. A project adapter directory remains absent until that work begins.

Store mutations that carry several related values use named request and ownership types such as `AgentRegistration`, `WorkTaskRequest`, `LaunchClaim`, `ExecutionStart`, `WorkExecutionResult`, and `ExecutionOwner`. This replaced the earlier positional Rust method signatures during pre-alpha stabilization; it is a source-level library API break, while the CLI and runtime behavior remain unchanged.

## State management

Mush keeps inspectable user state under `~/.mush`:

```text
~/.mush/
├── mush.sqlite
├── locks/
│   ├── serve.lock
│   └── task-<id>.lock
└── artifacts/
    └── task-<id>/
```

The lock directory is derived from the database path rather than from the state root, so a database opened from elsewhere keeps its locks beside itself and two databases never share a serve lock.

SQLite stores registered projects and exact agent configurations as well as task state, relationships, and concise execution metadata. Prompts, launch manifests, stdout, and stderr live under task-owned artifact directories. Claude Code owns its native worktree under the registered project at `.claude/worktrees/`; for Codex and Cursor work tasks, Mush creates and owns the worktree itself at `.mush/worktrees/` inside the registered project, never using cursor-agent's own worktree feature and simply launching codex in the directory Mush prepared. In both cases Mush records the deterministic name and resumes the same session after interruption. A machine-rendered configuration file is deferred until non-registered runtime configuration is needed. Overlays are not yet implemented; real use will define their requirements.

State transitions that create related records are transactional, and a retried transition must not leave half of it persisted or create duplicate records. The database opens in WAL mode with a busy timeout, so a delegating agent's nested `mush` invocations share the one database without spurious lock failures. Delegation depth and checkpoint subject gating — no checkpoint becomes ready, executes, or receives a decision while its subject is incomplete — are guaranteed by schema triggers as well as checked in the domain, so direct database writes cannot bypass them.

The M4 work-chain slice stores independent edges in `task_dependencies`, a bounded 10,000-entry audit window in `task_transitions`, and per-generation launch work in `launch_deliveries`. Read-then-write store operations acquire an immediate SQLite transaction only after a read-only candidate check, so ordinary multi-process completion, polling, and launch concurrency avoid needless write-lock contention. Reconciliation decides every candidate's action from reads alone and opens its immediate transaction only when at least one row needs a write, re-reading candidates under the lock because another process may have advanced them meanwhile; observing a healthy running graph therefore costs no write lock. The completion transaction journals the root, advances eligible queued dependents, journals their readiness, and inserts their delivery atomically. No mutating command pumps launch work afterwards and no command reconciles at startup: pending deliveries sit in the database until `mush runner tick` or `mush runner serve` picks them up, and both start work only by spawning `mush runner start <task-id>`, so there is one execution path. The concurrency bound on a `tick` or `serve` pass is a single constant, four simultaneous executions. An execution of queued work atomically claims one delivery before invoking the existing executor, whichever front door started it; `serve` supervises the executions it starts as its own children and reads their exit status directly. Reconciliation requeues an abandoned delivery until its attempt count reaches the three-attempt bound and parks it after that; a failed spawn of `runner start` parks immediately rather than retrying.

Mush decides whether an execution is live from operating-system locks rather than from process identity, as [ABSTRACTIONS.md](ABSTRACTIONS.md#execution-liveness) describes. Two lock scopes exist. Every execution holds an exclusive advisory lock on `locks/task-<id>.lock` for the whole of its life, taken before the execution claim commits; the runner module acquires it for both front doors and passes it into the locked run, so holding it is structural rather than incidental, and concurrent executions never contend beyond the task they claim. It is also what settles a `task run` racing a `runner start` for one task: whichever takes the lock executes, and the other exits with the did-not-start code without touching the task's rows. One exclusive `locks/serve.lock` covers the whole database: `runner serve` holds it for its lifetime, a second `serve` refuses, and other commands test it without taking it to report whether anything currently holds the runner role. `runner tick` and `runner start` take no serve lock; only `serve` and schema migration do. `task recover <id>…` or `task recover --project <id>` remains the explicit reset and relaunch boundary, and it skips a task whose execution lock is held.

Blocking CLI observation polls authoritative current rows, reports whether text was elided, returns null rather than partial optional fields, and fails fast on permanent validation errors. It performs no reconciliation and starts no work, so correctness does not depend on an ephemeral wake-up and an observation cannot alter the graph it reports.

Execution-side writes carry the owning attempt number so an older process cannot finish, interrupt, or replace runtime identity.

Schema migration happens only while the migrating process holds the database's serve lock, whoever that process is, so a starting runner and a concurrent CLI command cannot migrate one database at once; a process that finds a schema older than its target and cannot take the lock refuses and names the holder rather than migrating underneath it. Before applying a migration that cannot be reversed, Mush checkpoints the write-ahead log with `wal_checkpoint(TRUNCATE)` so the copy is a complete database rather than a torn prefix, copies the database file to `<database>.pre-v<version>.bak`, and refuses to migrate if that copy fails. The current schema is version 12, which adds the checkpoint subject-gate triggers and the decision-sensitive readiness rules to the version 11 work-chain schema; version 11 dropped `tasks.first_missing_at` and added a single-row `schema_meta` table recording the oldest binary version permitted to open the database. Migration accepts exactly the versions earlier builds wrote to real databases — 3 and 11 — and refuses the never-shipped versions 4 through 10. An older binary refuses to open a newer database rather than re-stamping the schema; a newer binary against an older database migrates under the lock. Downgrade is unsupported, and the pre-migration backup is its only path.

Each registered project and its queue belong to one home machine initially. Installing Mush on multiple machines does not imply synchronization of one project's state between them.

## Planned project context

Project context from repository-owned documents, a private overlay, or both is not yet implemented. Repository documents will remain authoritative about the repository's stated behavior and intent. Overlays will preserve private context without modifying upstream repositories, and their provenance will remain visible when the two sources disagree.

Detailed domain semantics belong in [ABSTRACTIONS.md](ABSTRACTIONS.md). Implementation sequencing belongs in [ROADMAP.md](ROADMAP.md).
