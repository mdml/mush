# Holding the runner role

Queued work in Mush advances only while some process is running it, and a process running queued work without being told which task holds the runner role. This guide covers the supported ways to hold it; [ABSTRACTIONS.md](../ABSTRACTIONS.md#runner) defines what the role is and what each `runner` command does.

Mush ships no supervisor, no autostart, no installed unit file, and no background service. It does not start a runner on your behalf, and it does not know or care which of the arrangements below you use. Nothing here is privileged: an operator with a terminal window is as supported as a service manager.

## Choosing an arrangement

All five arrangements below are first-class. The practical differences are how long the role is held, who restarts it, and whether work starts within seconds or at the next scheduled pass.

- Hold the role continuously with `mush runner serve` under a service manager or in a terminal when you want queued work to start as soon as it becomes ready.
- Hold it periodically with `mush runner tick` when a delay of one interval is acceptable and you would rather not keep a process resident.
- Hold it per task with `mush runner start` when an external system already decides what to run and when.

At most one `mush runner serve` may hold a database at a time; a second refuses. Any number of `mush runner start` processes may run concurrently, each claiming its own task. A pass starts at most four executions at once.

## Running one task yourself

Holding the runner role is not the only way to run work. `mush task run <id>` executes one named task in your terminal, whatever state it is in, and it is the front door to reach for when you want to watch a specific task run — it takes `--worktree`, `--restart-session`, and `--prompt-file`, which the worker front door does not. `mush runner start [id]` is the worker: it is what a loop, a unit, or a `serve` pass runs, and without an id it picks the next eligible task, which is the one thing only it does.

The two run the same engine and neither refuses the other's case. Running a queued task with `task run` claims its launch delivery and records its readiness transitions exactly as a worker execution would, so the graph advances when it finishes; running an unqueued task with `runner start` simply has no delivery to claim. If a `task run` and a `runner start` reach for one task at the same time, the task's execution lock settles it: one executes, and the other exits `3` saying the task already has a live execution.

`mush runner status` reports what is pending, claimed, running, and parked, and whether anything currently holds the serve lock — use it to check whether the role is held before assuming work is stalled. `mush runner status --json` returns those as `pending`, `claimed`, `running`, `parked`, `serve_lock_holder`, and `concurrency_bound` for scripts and agents.

`mush runner serve` runs until it is terminated; there is no bounded-pass mode. For a single supervised pass, run `mush runner tick`, which starts the same executions and exits without waiting for them.

## The exit codes of `mush runner start`

`mush runner start` exits with a code that says what happened, and those codes are a public contract you can build a restart loop or a service unit on. `mush task run` uses the same codes, since it runs the same engine.

| Code | Meaning |
| --- | --- |
| `0` | An execution ran to a terminal state. The task succeeded, or it failed and was parked with a diagnostic. |
| `3` | No execution started. Nothing was eligible, or the task was already claimed by another runner. Nothing is wrong. |
| other nonzero | The command was refused, or an execution started and could not be carried through. Worth looking at. |

Code `3` is the one a loop has to understand. It means "the queue had nothing for me", not "something is broken", so a loop should back off and retry rather than escalate. It is deliberately not `2`, which clap uses for a usage error, and not `1`, which a real failure uses.

A `serve` process reads the same code from the children it spawns: a child that exits `3` merely lost a race to a concurrent starter, and `serve` leaves the task alone rather than parking it. Nothing is inferred after the fact.

### Restart and backoff

A loop over `mush runner start` should restart on every exit, including `3` — the queue is expected to be empty most of the time — but must not restart instantly, or an idle database turns into a spin loop. For a `systemd` unit, `Restart=always` with a `RestartSec=` of a few seconds is the shape to use:

```ini
[Service]
ExecStart=%h/.local/bin/mush runner start
Restart=always
RestartSec=5
```

`systemd` treats a nonzero exit as a failure and will trip `StartLimitBurst` on a database that is idle for long enough, so declare `3` a success for the unit rather than raising the burst limit:

```ini
SuccessExitStatus=3
```

A shell loop wants the same shape — retry on `3`, stop or escalate on anything else:

```sh
while :; do
  mush runner start && continue
  [ $? -eq 3 ] && sleep 5 && continue
  break
done
```

Latency against an idle database is bounded by the backoff, so pick the interval the way you would pick a `runner tick` schedule. If you want work to start within milliseconds instead, hold the role with `mush runner serve`, which polls internally and needs no restart loop at all.

## An operator in a terminal

Run `mush runner serve` in a terminal and leave it running. Stopping it stops new work from starting; executions already running are unaffected, and the next process to hold the role picks up where this one left off. This is the arrangement to reach for while dogfooding or debugging, because the runner's output goes straight to your terminal.

## A `systemd --user` unit

Write your own unit; Mush installs none. A minimal one:

```ini
[Unit]
Description=Mush runner

[Service]
ExecStart=%h/.local/bin/mush runner serve
Restart=always

[Install]
WantedBy=default.target
```

Install it as `~/.config/systemd/user/mush-runner.service`, then `systemctl --user enable --now mush-runner.service`. Use `loginctl enable-linger` if the role should be held while you are not logged in. Because a runner executes coding-agent harnesses, the unit's environment must contain whatever those harnesses need — `PATH` entries for the harness executables, and any credentials they read. A unit that starts cleanly but cannot resolve `claude` or `codex` parks tasks with a diagnostic rather than failing loudly at startup.

A restart loop over `mush runner start` is an equally valid unit: it exits after each task, and `Restart=always` makes the next iteration claim the next eligible task. That gives a worker; several such units give a worker pool. Such a unit also needs `RestartSec=` and `SuccessExitStatus=3` — see [the exit codes](#the-exit-codes-of-mush-runner-start) for why.

## A container

Run the same command as the container's entrypoint:

```sh
mush runner serve
```

The container needs the Mush state directory mounted, the registered project checkouts mounted at the paths recorded in the database, and the harness executables and their credentials available inside the image. Mush decides liveness with advisory file locks kept beside the database, so the mount holding the database must support them. A project and its queue belong to one home machine, so run the container on that machine against that machine's state rather than pointing several hosts at one database.

## A periodic `mush runner tick`

A timer, cron entry, or scheduler that runs `mush runner tick` holds the role for the duration of one bounded pass: it reconciles stale rows, starts every eligible ready task up to the concurrency bound, and exits without waiting for what it started. Work therefore starts at the granularity of the schedule. Overlapping ticks are safe — a task already claimed is not started twice — so the interval is a latency choice, not a correctness one.

## An agent between turns

An agent that coordinates a graph may hold the role itself by calling `mush runner tick` between its turns, or by holding a `mush runner serve` for as long as it is working. It uses the same public commands as anyone else, and the durable state records that the role is held, so a human, a service manager, or a later agent can take over when that agent disappears.

Nothing is lost if the agent stops mid-graph. Queued work that has not started waits; executions already running finish or park. The agent does not need to hand anything off beyond ceasing to tick.

## When nobody holds the role

Queued work waits durably until a runner drains it, and `mush runner status` reports how deep the queue is and whether anything holds the serve lock. Observation commands do not start work: `task wait` polls durable rows and keeps waiting, since a runner may appear at any time.

Parked tasks are never relaunched by a runner. `task recover` is the only relaunch path, and it is deliberately an operator decision rather than something a resident process does on its own.

## Installation ownership

The operator owns installation and machine configuration, including any unit file, timer, container, or environment used to hold the runner role. Mush owns the public runner commands, state format, diagnostics, and migrations rather than prescribing one machine-management system.
