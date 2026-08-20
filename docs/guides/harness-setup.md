# Setting up a harness for Mush

Mush launches coding-agent CLIs that you install and authenticate yourself. This guide records what each supported adapter assumes about your machine and what belongs in a registered agent's settings instead. [ARCHITECTURE.md](../ARCHITECTURE.md) states the ownership rule this guide operationalizes, and [the agent-configuration-ownership decision](../decisions/2026-08-20-agent-configuration-ownership.md) records why the boundary sits where it does.

## What Mush does not do

Mush never writes harness or machine configuration. It does not install a CLI, log you in, refresh a token, edit a vendor's settings file, render a permission or deny list, enable or provision a sandbox, or register an MCP server. It reads the executable path you registered, launches it, and records what happened.

That leaves the durable machine layer entirely yours. How you maintain it — dotfiles, configuration management, or by hand — is outside Mush's concern, and Mush does not check or repair it. Its only interfaces to that layer are the per-adapter assumptions below and the evidence each run captures: every execution writes a launch manifest with the exact argument vector alongside the harness's own stream, so the posture a run actually got is observable after the fact rather than inferred.

A consequence worth stating plainly: if your machine's policy is permissive, Mush's runs are permissive. Mush narrows nothing on your behalf.

## What belongs in registered settings

Autonomy and capability are yours to choose, and you express them in the vendor's own words inside the agent's settings JSON. Mush validates the shape of those values at registration and relays them verbatim; it does not interpret them and offers no cross-vendor autonomy abstraction, because translating one vendor's permission semantics into another's would silently broaden or weaken them.

Mush adds only the invocation flags its own contract needs: print mode and stream output format so results and evidence can be parsed, session identity so a task resumes the same conversation, worktree placement so work lands where Mush recorded it, native-subagent visibility so delegation stays in the task graph, and workspace trust so an execution can start unattended. These are not preferences and are not configurable — changing them would break evidence capture, delegation visibility, or resume.

## Claude Code

Install and authenticate the `claude` CLI yourself. Registered settings carry `executable`, an optional pinned `version`, `model`, `effort`, `permission_mode` in Claude Code's own vocabulary, `tools`, `allowed_tools`, an optional `native_subagents` opt-in, and a `review_prompt` for checkpoint agents.

Mush passes the permission mode and tool lists straight through. Native subagents are off unless `native_subagents` is set, so delegation stays visible as Mush tasks rather than hidden inside one harness turn. Claude Code owns its own worktree feature, which Mush asks for by name under the registered project at `.claude/worktrees/`.

## Codex

Install and authenticate the `codex` CLI yourself. Registered settings carry `executable`, an optional pinned `version`, `model`, `effort`, `sandbox`, `approval_policy`, an optional `native_subagents` opt-in, and a `review_prompt` for checkpoint agents.

`sandbox` and `approval_policy` are Codex's own configuration keys and are relayed as such. Sandbox availability itself is a property of your machine, not something Mush provisions. Mush creates and owns the worktree at `.mush/worktrees/` inside the registered project and launches codex in the directory it prepared.

## Cursor

Install and authenticate the `cursor-agent` CLI yourself. Registered settings carry `executable`, an optional pinned `version`, `model`, a required `approval_mode`, and a `review_prompt` for checkpoint agents.

`approval_mode` takes cursor-agent's own run-mode vocabulary and is required, with no default, so the autonomy choice is always yours:

- `unrestricted` runs every tool call, and Mush invokes cursor-agent with `--force`. Your machine's deny rules remain the only backstop.
- `auto-review` sends unlisted calls to Cursor's server-side classifier, and Mush invokes cursor-agent with `--auto-review`. The classifier works headless: it denies with a written rationale that the agent can read and adapt to.
- `allowlist` is refused for an agent Mush executes. Run headless it denies every unlisted call while still reporting a successful, non-error result, so a task would complete having executed nothing — a result Mush would record as real work.

Registering or updating a Cursor agent with `allowlist`, an unrecognized mode, or no `approval_mode` at all fails with a message naming the accepted modes. Settings are strict about unknown fields, so a mistyped or borrowed key is reported rather than ignored.

Two Cursor behaviors stay on your machine. cursor-agent has no per-invocation tool-disable flag, so its subagent policy lives in machine-level Cursor permissions; and the classifier and sandbox rollout are server-side, so pinning the executable version does not pin autonomy behavior. Captured evidence, not the version pin, records what a run actually got. Mush creates and owns the worktree at `.mush/worktrees/` inside the registered project rather than using cursor-agent's own worktree feature.

## Repairing a registration

`mush agent update <id> --settings <json>` replaces an agent's settings and revalidates them exactly as registration does. Use it when a vendor's required keys change, when an executable moves, or when a pinned version is bumped. A rejected update leaves the stored settings untouched.

Cursor's `approval_mode` was added after the first Cursor agents were registered. A Cursor agent registered before it fails to run with a settings error naming the missing field; one `mush agent update` per agent repairs it. Mush does not migrate such settings silently, because inferring the mode would mean choosing an operator's autonomy on their behalf.
