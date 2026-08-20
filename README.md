# Mush

Mush is a local meta-API and task runner for AI agent harnesses.

It provides a stable interface over changing models and coding harnesses, keeps delegated work and independent review durable and visible, and supports project context without requiring every repository to adopt Mush conventions.

Mush is pre-alpha. It implements a local task → checkpoint → follow-up control loop with three exact harness adapters — Claude Code, Codex, and Cursor — plus queued work-task dependency chains. Work and review can run on different vendors while task state, transition cursors, launch deliveries, and artifacts remain durable across client exit and restart.

Mush never writes harness or machine configuration: it reads and launches the harness CLIs you installed, autonomy choices travel in each agent's registered settings in the vendor's own vocabulary, and machine-level policy stays wherever you already keep it. [ARCHITECTURE.md](docs/ARCHITECTURE.md) records this ownership boundary, and [the harness setup guide](docs/guides/harness-setup.md) covers what each adapter assumes about your machine.

Mush is a task database plus a runner you run. Queueing a task records it; nothing starts work implicitly, so work runs only when something is asked to run it. There are two ways to ask, over one engine: `mush task run <id>` is the human front door for running a specific task now, with the worktree, session, and prompt options, and `mush runner start [id]` is the worker front door and the only one that can pick its own task. Neither refuses the other's case, and running queued work continuously is the runner role — `mush runner serve`, a periodic `mush runner tick`, or a loop over `mush runner start`. When nobody holds that role, queued work waits durably in the queue until someone does, and `mush runner status` reports how deep that queue is. Mush ships no supervisor, no autostart, and no background service; [the runner role guide](docs/guides/runner-role.md) covers the supported ways to hold it.

The interface is a headless `mush` CLI for agents and automation, plus `mush tui` for human operation.

See [CONTRIBUTING.md](docs/CONTRIBUTING.md) to work on Mush and [ARCHITECTURE.md](docs/ARCHITECTURE.md) for the system map.

## License

Mush is licensed under the [Apache License 2.0](LICENSE).
