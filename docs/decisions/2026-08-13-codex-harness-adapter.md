# Codex harness adapter

Status: accepted on 2026-08-13, ahead of its milestone and by explicit maintainer direction.

## Context

M3 authorized exactly one additional harness adapter, Cursor Composer 2.5, and its gate is not yet met. AGENTS.md boundary 8 forbids behavior ahead of its milestone, and the roadmap places further harness configurations in M5.

A Claude Code upgrade broke the live M3 configurations, and the repair raised the question of whether the pinning rule generalizes beyond the two adapters that existed. The maintainer directed that Codex be added as a third adapter as part of that repair rather than deferred to M5. This record exists so the exception is visible as a decision rather than read later as drift.

## Decision

Mush supports a `codex` harness alongside `claude-code` and `cursor`. The adapter is exact in the same sense as the others and was validated against codex 0.146.1:

- Invocation is `codex exec` for a fresh run and `codex exec resume <thread-id>` to continue one, both with `--json` and a literal `-` prompt argument so the prompt arrives on stdin as it does for every other adapter.
- Reasoning effort, sandbox policy and approval policy travel as `-c` configuration overrides rather than flags, because `codex exec resume` accepts no `--sandbox` flag and both forms accept `-c`. They are named in agent settings as Codex names them.
- The working directory is the worktree Mush prepared, so no `--cd` is passed. Codex has no worktree concept, so Mush owns the worktree at `.mush/worktrees/` as it does for Cursor.
- Codex assigns its own thread id, reported as `thread_id` on the opening `thread.started` event, which Mush captures from the stream and persists for resume.
- Codex emits no single result event. The result is the text of the last completed `agent_message` item, and a `turn.failed` event is an execution failure that interrupts the task.
- The CLI's `multi_agent` feature is disabled unless the agent's settings explicitly opt in, mirroring the Claude Code rule that native subagents must not hide delegation from Mush.

## Consequences

M3's gate is unchanged and still unmet. The adapter is not evidence toward it: the gate asks for a coordinator that delegates a real subtask across harnesses and integrates the result, and this decision adds a capability rather than exercising one.

M5 no longer owns the Codex adapter. What remains there is machine expansion and the configurations that follow from it.

Each new adapter continues to cost a version-line format, a session-identity convention, and a result-extraction rule. Two of those three are now harness-neutral — version comparison matches any whole token, and the streamed session field is named per harness rather than assumed — so the recurring cost is mostly result extraction.
