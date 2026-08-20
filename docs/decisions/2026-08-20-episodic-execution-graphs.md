# Episodic execution graphs

Status: accepted on 2026-08-20.

## Context

Mush exists to let a human or one conversational coordinator manage a bounded collaboration across coding-agent harnesses without personally shepherding each agent session. The recurring examples are cross-family implementation review, plan and critique loops, and dynamically composed collaborations whose already-declared work can continue after the declaring conversation disappears.

M4 responded to a real M3 lifecycle failure: a coordinator delegated useful work, the launching context disappeared, and the completed result had no durable advancement or observation boundary. The [dynamic task graph decision](2026-08-18-m4-dynamic-task-graph.md) answered that failure with a durable graph that a coordinator constructs and extends in SQLite while work is in flight. Subsequent implementation added dependency readiness, durable delivery and observation, an explicit runner, and decision-sensitive checkpoint prerequisites.

That implementation is coherent, but it optimized the live graph as maintained coordination state before the repository had stated the product hypothesis that the graph serves. Product review showed that the intended reusable value is a bounded collaboration policy and semantic adjudication across harnesses, not an indefinitely extensible project workflow stored in Mush.

The original checkpoint idea was also narrower than generic review. A checkpoint receives declared criteria and a result, then adjudicates whether the result meets those criteria. A collaboration policy may use that decision to advance or to make a bounded semantic revision attempt, but the checkpoint does not invent that policy.

## Decision

Mush is oriented around episodic declared execution graphs. A human or conversational coordinator declares a bounded graph `G_n` for one collaboration episode. Its nodes are semantic tasks assigned to exact harness configurations, and its edges and checkpoint gates express the execution relationships already intended for that episode.

`G_n` may use durable storage while active. Disposable does not mean memory-only, tied to one shell, or unable to survive process or machine interruption. It means the graph is execution memory rather than permanent project truth: once the useful result and unresolved decisions are reflected in the repository, deleting `G_n` must not remove knowledge required to understand the project or declare `G_n+1`.

The repository owns maintained product intent and results. Mush may retain bounded task, checkpoint, decision, evidence, and artifact records for execution and audit, but a later coordinator must not need an old live graph to reconstruct the project's next meaningful decision.

The graph may expand only through control rules declared for the episode. In particular, a collaboration policy may declare a bounded semantic revision loop. Each work attempt and checkpoint decision remains an immutable node in the materialized execution history. Expansion under that rule is execution of prior intent, not fresh workflow planning. A new criterion, a changed stopping rule, or another undeclared semantic decision requires fresh coordination and may produce a later episode.

A checkpoint is a semantic task over declared criteria and a named result. It records `met`, `not_met` with bounded evidence, or `blocked` when adjudication requires missing information, authority, or external state. The checkpoint decides whether the criterion was met; the collaboration policy decides the consequence.

Revision budgets are stated as a maximum number of semantic work attempts so the initial attempt is counted without ambiguity. Infrastructure relaunch or same-session recovery does not consume another semantic attempt. `not_met` may create another attempt only while budget remains. `blocked` and budget exhaustion stop the affected path for fresh judgment.

The coordinator, graph, and runner remain separate roles. A coordinator uses judgment to declare an episode. The graph externalizes the declaration, bounded context, and current position. A runner mechanically invokes eligible nodes. A human or coordinator may instead step through eligible nodes manually; manual operation is a supported use of the same episode, not a degraded recovery path.

The maintained product hypothesis lives in [the product hypothesis](../PRODUCT.md). The active milestone must test it against real repository work rather than treating implementation completeness as product evidence.

## Executable checkpoint example

The next implementation slice must make this episode expressible through the public domain operations without relying on a coordinator transcript:

1. Declare plan work assigned to Codex with `max_attempts` three.
2. Before the first attempt starts, declare a checkpoint assigned to Claude whose criteria require the plan to cover the requested behavior, name the affected interfaces, preserve the repository's documented boundaries, and state how the result will be verified.
3. Declare implementation work as the attempt policy's success continuation, so `met` from any permitted plan attempt can make it eligible.
4. Run the first plan attempt and record its bounded result and evidence.
5. Run the checkpoint. It records `not_met` with evidence that one named criterion is missing.
6. Materialize the second plan attempt through the declared budget policy, link it to the first through `previous_task_id`, supply the bounded `not_met` evidence, and attach a new checkpoint with exactly the same criteria.
7. Run the second checkpoint. It records `met`, making implementation eligible.

The same surface must also demonstrate the two stop paths: `blocked` creates no attempt, and `not_met` on the final permitted attempt creates no further work and reports budget exhaustion. Infrastructure recovery of any one task does not change the semantic-attempt count.

The slice is complete only when a human can inspect the episode and restate the subject result, criteria, decision, evidence, current attempt number, remaining budget, and next eligible action without reading a harness transcript. Agent execution and manual adjudication must use the same checkpoint contract.

This example does not require structured criterion records, multiple checkpoints on one attempt, arbitrary condition expressions, mutable budgets, or a workflow-authoring language. Those behaviors remain outside the slice.

## What this decision refuses

- Treating the live graph as the durable project plan or the only source of future coordination intent.
- Arbitrary mid-flight graph growth that represents fresh planning rather than a declared control rule.
- Equating a checkpoint with a generic reviewer agent or code-review activity.
- Letting a checkpoint create its own revision policy, move its criteria, or continue after budget exhaustion.
- Requiring an unattended runner for the graph to provide value; manual execution and context externalization remain first-class.
- Erasing useful result, decision, or evidence context merely because an episode is disposable.

## Alternatives

Keeping durable mid-flight graphs as the product model would let a later coordinator resume and extend the exact workflow frontier stored in Mush. It is rejected because it splits project coordination truth between the repository and Mush, makes the graph a long-lived product object, and solves a broader problem than the motivating workflows require.

Generating a bespoke script for every collaboration remains a valid comparison. Mush must earn its additional machinery through shared checkpoint semantics, bounded attempts, cross-harness execution, manual legibility, recovery, or evidence. If it does not, scripts or native subagents are the smaller answer.

## Consequences

The current B-oriented implementation is preserved until it can be classified against the revised milestone gate. Existing work falls into one of three categories: mechanism the episodic model needs, unnecessary generality that can remain temporarily without distorting the product, or behavior that conflicts with the new truth boundary and must be superseded. That inventory follows this decision and is not a review of the pending checkpoint-readiness change.

The accepted dynamic-graph and runner records remain accurate history. This record supersedes their use of an incrementally extended live graph as the maintained product direction; it does not silently rewrite the behavior they introduced or prejudge which mechanisms remain useful.

`ABSTRACTIONS.md` must describe episodes, semantic checkpoint criteria, bounded attempts, manual and mechanical advancement, and the repository truth boundary. `ARCHITECTURE.md` changes only after the implementation inventory identifies which current components remain part of the chosen slice.

The M4 gate becomes a real bounded cross-harness episode rather than acceptance of the current graph machinery. Passing implementation verification remains necessary but cannot close the gate without understandable real-work evidence.
