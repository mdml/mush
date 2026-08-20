# Harness executable pinning

Status: accepted on 2026-08-13.

## Context

A registered agent pins both an executable path and an expected version, and execution refuses to launch when the two disagree. [The M3 remediation decision](2026-08-12-m3-parent-completion-after-delegation.md) found Cursor registered through a mutable auto-updating symlink and responded by repinning both harnesses to versioned executable paths.

That response was correct for Cursor and wrong for Claude Code, because the two installers keep different things stable:

- The Cursor installer retains every version it has fetched under `~/.local/share/cursor-agent/versions/<version>/`, so a versioned path stays valid indefinitely and remains available for recovery after an upgrade.
- mise installs Claude Code by replacing the install directory in place. Upgrading to 2.1.228 deleted `~/.local/share/mise/installs/claude/2.1.227/` entirely, so the pinned path no longer existed and every launch failed with a missing executable before any version comparison could run.
- mise installs Codex by version and keeps prior versions, but its `latest` symlink tracks the newest installed version rather than the version the toolchain selected, so `latest` can silently name a different binary than the one on `PATH`.

A dangling path is strictly worse than a version mismatch. A mismatch names both versions and leaves the working binary available; a dangling path removes the recovery route the earlier decision relied on.

## Decision

The version pin is optional. An agent that pins a version has it enforced exactly before launch, as before. An agent that pins none runs whatever version its executable reports and records that version in the launch manifest and in the task evidence. Exactness for an unpinned agent therefore lives in the execution record rather than in a pre-launch gate, which is the trade this decision accepts: two runs of one unpinned agent may use different harness versions, and both runs say which one they used.

This is what makes a configuration survive upgrades performed by any installer. A version pin has to be maintained by whoever performs upgrades; nothing about mise, an auto-updater, Homebrew or npm makes that maintenance unnecessary, so the pin is reserved for configurations that genuinely need reproducibility rather than imposed on all of them.

The executable may be a path or a bare command name resolved on `PATH`, which removes the installer's directory layout from the configuration entirely. A bare name depends on the environment Mush itself is launched from, so an absolute path remains the right choice when Mush runs from a context that does not load the toolchain's `PATH`. Mush refuses to launch an executable that cannot be run and report a version, pinned or not.

When a version is pinned, the pinned executable is the path the installer currently reports for the selected version — on this toolchain, the output of `mise which <tool>` or the equivalent for a vendor installer — and the pinned version is the version that path reports at the moment of pinning. Whether that path is a stable symlink or a versioned directory is the installer's answer, not Mush's convention: `mise which claude` reports a `latest` symlink that mise keeps current, while `mise which codex` and the Cursor installer report versioned paths.

Exactness is guaranteed by the version check, not by the shape of the path. Both must be repinned together after a harness upgrade, through `mush agent update`, which revalidates settings as at registration.

Version comparison is harness-neutral. Each CLI decorates its version line differently — `claude --version` prints `2.1.228 (Claude Code)`, `codex --version` prints `codex-cli 0.146.1`, and `cursor-agent --version` prints `2026.08.04-aaa8809` alone — so a pin matches when it equals a whole whitespace-separated token of the observed output. This stays exact for all three and does not require a new parser for the next adapter.

## Consequences

An unpinned agent needs no attention after a harness upgrade, on any installer. Its next run picks up the new version and says so in the launch manifest and the evidence a reviewer reads.

A pinned agent continues to stop execution deliberately after an upgrade, which remains the intended behavior where reproducibility is the point. It now stops with an actionable version mismatch naming both versions, rather than with a missing file, and repinning stays a two-value operation through `mush agent update`.

Mush still does not resolve a pin at launch or treat it as a floor. A version range or minimum would need a version-ordering rule per harness, and the three supported harnesses do not share one — Cursor's `2026.08.04-aaa8809` is not a semantic version. Open and exact are the two positions that need no such rule.
