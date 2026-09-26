# Design Philosophy

## Start from what works

OpenClaw's architecture is good. The gateway pattern, channel normalization, Lane Queue, model-agnostic runtime, file-first workspace, self-evolving behavior — these are sound decisions for a personal assistant. The temptation when building something new is to rethink everything. The discipline is recognizing what doesn't need rethinking.

Every change in these designs targets a specific failure mode observed in real usage, not a theoretical weakness.

## Simplicity that stays practical

It's easy to design something elegant on a whiteboard that falls apart in practice. A centralized registry is cleaner than directory scanning until it desyncs. A knowledge graph is more powerful than flat files until it's impossible to debug. These designs favor boring, inspectable mechanisms over clever ones.

If a user can understand the system by looking at the filesystem, the system is working. If they need to query a database or read a state machine diagram, it's too complex.

## Put the right work in the right place

LLMs are expensive and inconsistent at deterministic tasks. The gateway should handle scheduling, file watching, and schema validation. The LLM should handle judgment — what's worth alerting about, which context is relevant, what to write in a project's notes.

The HEARTBEAT.yml change captures this principle precisely: move the scheduling logic (when to check) into the gateway, keep the evaluation logic (what to do about it) in the LLM. Most heartbeat cycles were burning tokens to say "nothing to do." That's work a YAML parser and a timestamp comparison can handle for free.

## Independent systems that compose through shared data

Memory and proactivity are designed independently. The Projects system is designed independently. They share a data layer — the workspace filesystem and the observation log — which means improvements to one naturally benefit the others. But they don't depend on each other. You can run OM without Projects. You can run pulses without OM. Each system should be valuable on its own.

Tight coupling between systems creates fragility. Shared data creates opportunity without obligation.

## The agent should know what it knows

OpenClaw's memory cliff — where context from two days ago becomes invisible unless the agent guesses it should search — is a failure of accessibility, not storage. The information exists in files. The agent just can't see it.

OM fixes this by keeping compressed history in the context window at all times. The Projects system fixes this by giving the agent a scannable index of active and archived work — what projects exist, what's in each, and what capabilities they carry — without bulk-loading contents. The principle is the same: don't make the agent guess that it should look for something. Make relevant context visible by default and let the agent manage scope.

## Make failure safe, not impossible

Good engineers try to anticipate and prevent every failure. Great engineers assume it will fail and make failure safe.

The agent should act on its own: activating contexts, archiving completed projects, adjusting alert behavior, creating new PARA entries, and working through long-running tasks without checking in. Requiring user permission for routine decisions defeats the purpose of having an agent, and agents are trusted with more every year. Residuum's default is open.

An agent that does long-running work on its own will hit problems mid-run. The goal isn't a system that can't fail; it's one where failure is visible, contained, and recoverable. Every autonomous action should be visible: files the user can read and edit, a mention when something gets archived, a running view of how long the agent has been working and what it has spent. The user should be able to stop the agent mid-run and undo what it did. The agent has broad autonomy; the user has full visibility and the final say.

Hard limits are reserved for failures that have actually happened. A limit built around an imagined risk blocks legitimate work, and it fails in ways that are harder to see than the problem it was meant to prevent. Users who want more oversight can opt into the guards they care about; Residuum doesn't impose them.

## File-first, always

If the system state lives in files the user can inspect, edit, and version control, the system is trustworthy. If it lives in a database or opaque embeddings, trust requires faith. These designs never introduce storage that can't be opened in a text editor.

context.yml over a database table. notes/ over a knowledge graph. CHANNELS.yml over a notification rules engine. The filesystem is the source of truth.
