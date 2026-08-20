# Mush

Mush is a local runtime for bounded collaboration among coding agents across harness and model-family boundaries.

A human or one conversational coordinator declares an episodic execution graph of agent tasks and semantic checkpoints. The graph externalizes the intended collaboration, its current position, and bounded handoff context, so it can be stepped manually or advanced mechanically without managing every agent session or retaining the workflow in conversational memory. [The product hypothesis](docs/PRODUCT.md) states what this experiment is trying to learn and what it refuses to become.

Mush is pre-alpha and its public workflow is being reoriented around that hypothesis. The current implementation provides a local task and checkpoint control loop, exact adapters for Claude Code, Codex, and Cursor, queued dependency chains, durable observation, and an explicit runner. [The roadmap](docs/ROADMAP.md) identifies which behavior is accepted evidence, which work remains under review, and the next real-work gate.

Mush never writes harness or machine configuration: it reads and launches the harness CLIs you installed, autonomy choices travel in each agent's registered settings in the vendor's own vocabulary, and machine-level policy stays wherever you already keep it. [ARCHITECTURE.md](docs/ARCHITECTURE.md) records this ownership boundary, and [the harness setup guide](docs/guides/harness-setup.md) covers what each adapter assumes about your machine.

Mush is a task database plus a runner you run. Queueing a task records it; nothing starts work implicitly, so work runs only when something is asked to run it. There are two ways to ask, over one engine: `mush task run <id>` is the human front door for running a specific task now, with the worktree, session, and prompt options, and `mush runner start [id]` is the worker front door and the only one that can pick its own task. Neither refuses the other's case, and running queued work continuously is the runner role — `mush runner serve`, a periodic `mush runner tick`, or a loop over `mush runner start`. When nobody holds that role, queued work waits durably in the queue until someone does, and `mush runner status` reports how deep that queue is. Mush ships no supervisor, no autostart, and no background service; [the runner role guide](docs/guides/runner-role.md) covers the supported ways to hold it.

The interface is a headless `mush` CLI for agents and automation, plus `mush tui` for human operation.

See [CONTRIBUTING.md](docs/CONTRIBUTING.md) to work on Mush and [ARCHITECTURE.md](docs/ARCHITECTURE.md) for the system map.

## License

Mush is licensed under the [Apache License 2.0](LICENSE).
