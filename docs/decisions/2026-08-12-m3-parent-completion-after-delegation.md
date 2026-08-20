# M3 parent completion after delegation

Status: accepted for M3 remediation on 2026-08-12.

## Context

The first real M3 gate attempt used a Claude Code coordinator to delegate a bounded implementation task to Cursor Composer 2.5 through a nested `mush task run`. The coordinator created the child correctly, but Claude Code invoked the blocking command with its Bash tool's background mode enabled. It continued other work and returned a successful progress result while the child was still running.

Mush accepted that result and atomically marked the parent work task completed with successful execution. When the Claude process exited, its background process group was terminated, interrupting the nested Cursor process. The child remained recorded as running with a dead PID until it was resumed manually in the same Cursor session.

The recovered child produced a useful commit, passed its verification, and received an accepted checkpoint from a Claude reviewer. This is evidence that cross-harness execution, recovery, and review work, but it is not a successful coordination attempt: the coordinator neither waited for nor integrated the delegated result before completing.

The run also exposed three separate configuration and validation defects:

- Cursor was registered through a mutable auto-updating symlink, so the registered version no longer matched the executable when the child was resumed. The original versioned executable remained available and allowed recovery.
- The executable checkpoint agent was registered without a review prompt, so checkpoint execution rejected the otherwise valid task until an explicit prompt was supplied.
- Mush supplied `MUSH_TASK_ID`, but the coordinator's least-privilege Bash configuration allowed `mush` commands without allowing a narrow command to read that variable. Repeated attempts to inspect it were denied, and the coordinator recovered its literal task id through task listing.

## Decision

A work task must not become completed while any direct child is incomplete or still recorded as running. The completion check and state transition belong to the domain/store transaction, not to a harness adapter or prompt. The invariant must cover both executor-driven completion and manually supplied completion.

The invariant must remain true across both transition directions:

- completing a parent is rejected while a direct child has a status other than `completed` or has `execution_status=running`;
- creating or reparenting a child under an already completed parent is rejected.

Mush will enforce both directions in domain operations and SQLite constraints or triggers. Checking only before the parent update is insufficient because a new child could otherwise be created after completion. The completion error should identify the blocking child tasks so a coordinator or operator has an actionable recovery path.

Mush will refuse parent completion rather than detach, wait for, or automatically cancel descendant processes. The current executor owns its direct harness process, not arbitrary processes launched inside that harness. Detachment contradicts M3's blocking integration model, waiting would introduce implicit process supervision, and cancellation needs explicit lifecycle semantics that M3 has not established. A refused completion leaves the parent interrupted and the child available for ordinary stale-process detection and same-session recovery.

Harness prompts should continue to tell coordinators to run delegated work synchronously, but prompt compliance is not the mechanical defense. The Claude adapter currently controls executable arguments and tool availability, not the background flag of each permitted Bash invocation. A future harness capability may add an input-level restriction, but the domain invariant remains necessary.

Executable checkpoint-agent registration must reject a missing or empty review prompt instead of deferring the error until execution. An intentionally human/manual reviewer is not required to carry an executable review prompt.

Exact Cursor configurations should use a versioned executable path rather than a mutable vendor symlink. Existing pre-launch version verification remains the authoritative refusal when the observed executable does not match the registered version. Canonical-path or binary-digest recording and doctor warnings may improve diagnosis later, but are not required for the minimal M3 remediation.

Coordinator configuration should allow the narrow `printenv MUSH_TASK_ID` command in addition to the required `mush` commands. A future self-parent CLI affordance may remove the need for shell environment inspection, but it is not required for the remediation.

## Verification required

The remediation must add focused coverage at the boundaries that can violate the invariant:

- store tests for manual and executor completion with pending, running, interrupted, and completed direct children;
- schema-bypass tests for completing a parent with unfinished children and adding a child beneath a completed parent;
- a concurrency test showing child creation and parent completion cannot commit an invalid final state;
- an executor test in which a successful parent harness result is refused while a child is running;
- CLI tests showing both completion surfaces fail with an actionable machine-readable error;
- registration tests for executable checkpoint agents with missing, empty, and valid review prompts;
- retention of the existing version-mismatch test, plus same-session recovery through a versioned executable path.

After remediation, M3 needs a new semantic coordination attempt. The gate requires the coordinator to wait for and integrate the cross-harness child result before its own completion, followed by independent cross-harness checkpoint review. Manual recovery of the first child remains useful evidence, but does not retroactively satisfy that gate.

## Consequences

Coordinators cannot abandon a created direct child merely by returning success. Until Mush has an explicit cancellation or abandonment operation, an unfinished child must be completed or recovered before its parent can complete. This is intentionally narrower than adding a scheduler or descendant-process supervisor.

The failed first gate attempt remains useful evidence about cross-harness review quality and identifies the minimum work required before repeating the gate.
