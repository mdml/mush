# Product hypothesis

Mush is a maintainer-led pre-alpha experiment in coordinating coding agents across harness and model-family boundaries.

## Problem

People who want several coding agents to collaborate must usually coordinate the work themselves. They open and monitor separate sessions, move context and artifacts between them, remember which result another agent should inspect, and decide which session runs next. Native subagents reduce that burden within one harness or model family, while cross-harness workflows generally require manual coordination or a bespoke script.

The scarce resource is the person's attention and comprehension. Producing more agent output is not useful if supervising it costs as much attention as doing the work directly.

## Hypothesis

A human or one conversational coordinator should be able to declare and run a bounded collaboration among independently configured coding-agent harnesses without managing every agent session or retaining the workflow in conversational memory.

Mush supplies the common execution protocol. A coordinator translates an ordinary repository goal and a small collaboration policy into an inspectable episodic execution graph. The graph externalizes the intended collaboration, its current position, and the bounded context each node needs. A human may step through it manually, or a runner may advance everything the declared policy already determines. The episode stops when fresh judgment, missing authority, or an exhausted semantic-attempt budget prevents further declared progress.

The graph is useful execution memory, not permanent project truth. The repository remains the durable source of product intent and maintained results. Once an episode's useful outcome and unresolved decisions are reflected there, deleting the episode must not erase knowledge required to understand the project or formulate later work.

## Core collaboration model

An execution-graph node is a bounded agent task assigned to an exact harness configuration. An edge records declared execution order or a semantic gate; it does not turn the graph into a project plan.

A checkpoint is a semantic task that asks whether a result satisfies declared criteria. The human or agent executing the checkpoint is its adjudicator, not the definition of the checkpoint itself. The checkpoint records `met`, `not_met`, or `blocked` with bounded evidence. It does not invent the workflow consequence of that decision.

A collaboration policy may declare a budgeted revision loop. The budget counts semantic work attempts, including the initial attempt; infrastructure recovery does not consume it. A `not_met` checkpoint may produce another immutable attempt only when the declared budget permits one. `met` advances the declared success path, while blocked adjudication or budget exhaustion stops the episode for fresh judgment.

The coordinator, graph, and runner are separate roles. The coordinator exercises judgment to declare the collaboration, the graph externalizes that declaration and its progress, and the runner mechanically invokes eligible tasks. One process may hold more than one role, but their responsibilities remain distinct.

## Worked examples

- An implementation task is assigned to Codex and a checkpoint with explicit acceptance criteria is assigned to Claude. Downstream integration becomes eligible only when the checkpoint records `met`.
- Codex produces a plan, Claude checks it against declared planning criteria, and Codex receives bounded checkpoint evidence for another attempt while the semantic-attempt budget remains. Acceptance continues to implementation; exhaustion returns the episode for human judgment.
- A coordinator creates a collaboration and disappears after one node completes. A human or fresh coordinator can inspect the episode, understand the next declared task and its bounded inputs without replaying the original transcript, and run it manually or ask a runner to advance it.

## What Mush refuses to be

- A general-purpose workflow engine or static workflow language.
- An autonomous project manager that invents the next meaningful task indefinitely.
- A permanent project graph or a second source of repository truth.
- A system for maximizing agent count, concurrency, or output volume.
- Merely a cross-vendor process launcher with no semantic collaboration model.
- A requirement that users manually author graphs for ordinary goals.
- A promise that semantic judgment can always be automated.
- A store for complete coordinator or harness transcripts as workflow context.
- An unbounded "iterate until satisfied" loop whose criteria or stopping rule can move during execution.

## Decision authority

The maintainer owns the product hypothesis, user-visible and domain semantics, milestone outcomes, public interfaces, data-model and difficult-to-reverse architectural boundaries, acceptance of consequential decisions, and acceptance of milestone evidence. Human and agent contributors may attack assumptions, propose alternatives, and recommend technical choices. Within an accepted boundary they independently own local, reversible implementation decisions.

When implementation exposes a product, domain, data-shape, public-interface, or difficult-to-reverse architectural decision that the maintained documents do not settle, implementation stops and returns a decision packet to the maintainer. Passing verification shows implementation quality; it does not by itself accept a product or architectural decision.
