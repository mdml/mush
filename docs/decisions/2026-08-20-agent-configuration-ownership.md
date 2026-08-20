# Agent configuration ownership

Status: accepted on 2026-08-20.

## Context

Mush launches three vendors' harnesses, and their autonomy controls live in different places. Claude Code and Codex accept per-invocation autonomy in their own vocabulary, which the adapters relay verbatim from registered agent settings — `permission_mode` for Claude Code, `sandbox` and `approval_policy` for Codex. Cursor's durable policy lives in machine-level configuration outside Mush, and its adapter instead hardcodes `--force --trust` into every invocation, with the machine's deny rules as the assumed backstop.

Live probes on 2026-08-20 sharpened what that assumption costs. cursor-agent's run modes are a three-value vendor enum: `unrestricted` runs everything, `auto-review` routes unlisted calls to a server-side classifier that works headless — it denies with a written rationale and the agent adapts rather than hanging — and `allowlist` headless denies every unlisted call while still reporting a successful, non-error result, so a run can "succeed" having executed nothing. On a machine with a rendered deny-list, hardcoded `--force` is a backstopped convenience. On a stock machine it is unbounded autonomy that Mush selected in source code rather than the operator selecting in configuration — the wrong owner for that decision in a public tool.

## Decision

Harness configuration divides into three ownership layers.

- **The machine owns everything durable:** harness installation, authentication, machine-wide permission policy, sandbox availability, MCP configuration. Mush never writes these — it reads and launches installed harness CLIs and nothing more. How that layer is maintained (dotfiles, configuration management, by hand) is the operator's business, and Mush's only interfaces to it are documented per-harness assumptions and observed evidence.
- **The operator owns autonomy and capability:** permission and approval modes, sandbox policy, models, and tool selections, expressed in each vendor's own vocabulary inside registered agent settings and relayed verbatim. Mush validates shape at registration but does not interpret these values, and it does not define a cross-harness autonomy abstraction — translating between vendors' permission semantics broadens or weakens them, so the vendor's words are the interface.
- **Mush owns only what its contract needs:** the invocation flags that make execution observable, attributable, and resumable — print mode and stream output format, session identity, worktree placement, native-subagent visibility, and workspace trust. These stay hardcoded because changing them breaks evidence capture, delegation visibility, or resume, not because they are preferences.

The boundary to ambient machine state is observation, not management. Launch manifests and captured streams record the posture in effect — Cursor's stream discloses per-call approval skipping and the applied sandbox policy — and Mush documents what each adapter assumes about the machine rather than checking or repairing machines.

One deviation stands and is scheduled: the Cursor adapter's hardcoded `--force --trust` places an operator-layer autonomy choice in Mush's layer. The remediation gives `CursorSettings` a required approval-mode field carrying Cursor's own enum, with `--trust` remaining Mush-owned as execution enablement. `allowlist` is rejected for executable agents because its headless deny-everything behavior reports success while doing nothing, which would corrupt result semantics. The field is required rather than defaulted so the autonomy choice is always the operator's, matching the registration surface's existing explicitness. It lands as its own follow-up change after the active quality slice passes its gate; it is a breaking settings change under `deny_unknown_fields`, repaired by one `mush agent update` per registered Cursor agent. The same change ships `docs/guides/harness-setup.md`, recording each adapter's machine-layer assumptions for operators — the guide waits for the field because it documents the registration surface the field changes.

## Consequences

[ARCHITECTURE.md](../ARCHITECTURE.md) records the layer rule as maintained truth and [AGENTS.md](../../AGENTS.md) extends the harness-adapter boundary with the never-writes-configuration guarantee, which the README states to users. Adding a harness now includes classifying each of its controls into a layer, and a control that fits none of them is a design problem to resolve before the adapter lands.

Until the remediation lands, registered Cursor agents continue to mean "run everything", and the machine-level deny-list remains the only backstop — unchanged behavior, now stated rather than implied. Cursor's classifier and sandbox rollout are server-side, so an executable version pin does not pin autonomy behavior there; evidence, not the pin, is what records the behavior a run actually got.

## Amendment 2026-08-20

The scheduled Cursor remediation landed as decided, so no Cursor agent's autonomy is chosen in Mush's source any longer. `CursorSettings` requires `approval_mode`; the adapter emits `--force` for `unrestricted` and `--auto-review` for `auto-review`, refuses `allowlist`, and keeps `--trust`. [ARCHITECTURE.md](../ARCHITECTURE.md) owns the resulting behavior and [the harness setup guide](../guides/harness-setup.md) records each adapter's machine-layer assumptions. The paragraph above describes the state before that change.
