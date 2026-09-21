# Background Module: Agent Sessions

Owns the session registry, runtime, and store that execute work off the main agent's turn loop: pulse checks, scheduled actions, webhooks, and on-demand sub-agent delegation. See `docs/design/agent-sessions.md` for the systems-level design and `docs/systems-usage/background-tasks.md` for the user-facing behavior; this file covers how the module's own pieces fit together.

## Overview

A **session** is a temporary fork of the main agent: its own identity/memory snapshot, its own tool registry, its own message history. A session has an **address** (stable, human-readable, e.g. `spawned-researcher-3f9a`) and moves through a lifecycle — `forking` → `running` → `idle` → `completing` → `completed` — tracked by the registry for as long as it's live.

The module owns:
- **The session registry** (`registry.rs`): the single source of truth for every live session's address, run id, category, source label, lifecycle state, spawner, depth, and purpose. Discovery tools (`list_agents`, `stop_agent`) and the bug-report client context read it directly.
- **The session runtime** (`runtime.rs`): executes a session's turn, holding the concurrency permit only while a turn is actually running, then lingering the session `idle` until its category's timeout elapses or it's stopped, before finalizing the run and merging its memory.
- **The session store** (`store.rs`): durable on-disk record of every run — a metadata JSON file under `memory/sessions/YYYY-MM/DD/<run-id>.json`, plus a sibling `<run-id>.transcript.jsonl` appended to as the run progresses (see "Incremental Transcript Persistence" below).
- **Per-run memory** (`session_memory.rs`): threshold-based mid-run staging and the completion pipeline that merges a finished run's observations into global memory through the `MemoryMergeWriter` (`crate::memory::merge_writer`).
- **Fork construction** (`spawn_context.rs`, `subagent.rs`): resolves the model tier, activates the requested skill, snapshots the global observation log and recent-context narrative, and builds the isolated `SubAgentResources` a session's turn runs with — including its own `Observer` and a shared handle to the `MemoryMergeWriter`.

The module does **not** handle:
- **Channel delivery.** Each completed run publishes its own `AgentResultEvent` to the bus; the `notify` module's router decides the concrete destinations (inbox, external notification channels, or a relay to the main agent) and delivers to them.
- **Skill discovery.** `crate::skills` owns the index; this module activates a named skill on the session's own `SkillState` and lets that failure fail the fork.
- **Persisting a merge.** `session_memory.rs` decides what to merge and when; `crate::memory::merge_writer::MemoryMergeWriter` is the single serialized writer that actually allocates episode ids, appends the observation log, indexes, embeds, and checks the reflector — shared with the main agent's own observation flow so numbering never races.
- **Cross-session messaging and nesting.** Sessions cannot yet message each other or spawn further sessions — `subagent_spawn` is main-only, and `build_subagent_registry()` omits it.

## How It Works

### Core Abstractions

**`SessionRegistry`** (`registry.rs`): an in-memory map from address to `SessionInfo` plus the `CancellationToken` that stops the run. `register`/`set_state`/`stop`/`remove` mutate it; `list_live`/`get`/`subagent_snapshot` read it. `generate_address(trigger, qualifier)` builds a new address from the category implied by the trigger (`EventTrigger::Pulse`/`Action` → `scheduled`, `EventTrigger::Agent` → `spawned`, `EventTrigger::Webhook` → `external`) and a slugified qualifier (skill, pulse, action, or webhook name).

**`SessionRuntime`** (`runtime.rs`): holds the concurrency `Semaphore`, an `Arc<SessionRegistry>`, an `Arc<SessionStore>`, and the per-category `IdleTimeouts` resolved once from `BackgroundConfig` at construction (like `max_concurrent`, a config reload does not resize this — the semaphore and timeouts are fixed for the runtime's lifetime). `spawn()` registers the session as `forking` synchronously, then drives the rest of the lifecycle on a detached task.

**`SessionStore`** (`store.rs`): writes a `RunRecord` (metadata: address, run id, category, lifecycle timestamps, episode id once merged) at fork time and again at completion, both to the same date-partitioned path. The transcript itself lives in a sibling `.transcript.jsonl` file, appended to via `append_transcript()`/`RunTranscriptSink` as the run progresses; on completion the full transcript is also folded into the metadata file so a finished run's read path is one file. `recover_incomplete_runs()` runs once at startup, sweeping the store for any run left in a non-terminal state by a prior process exit, running the full completion memory pipeline against its persisted transcript, and marking it `completed` with `interrupted: true`.

**`SessionMemory`/`SessionMemoryEnv`/`complete_session_memory()`** (`session_memory.rs`): a run's working memory — observations extracted mid-run when the observer's force threshold is crossed (`maybe_stage()`), held locally until completion. `complete_session_memory()` runs the skip check (a `HEARTBEAT_OK` ending, or a transcript below `episode_skip_token_floor` with nothing staged, produces no episode), a final extraction over whatever wasn't staged, and the merge — via a `SourceTag` naming the session's address/run id/category, so `recover_incomplete_runs()` can reuse the identical pipeline from a persisted `RunRecord` instead of a live `SessionInfo`.

**`SubAgentResources`** (`subagent.rs`): the isolated bundle a session's turn runs with — provider, tool registry, MCP registry (shared), skill state (cloned, isolated), identity, completion options, the fork-time memory snapshot (`observations`, `recent_context`), workspace layout, this session's own `Observer`, the shared `MemoryMergeWriter`, and the episode skip token floor. Built by `build_subagent_resources()`.

### Primary Data Flow

#### Fork Request

```
SpawnRequestEvent { address, skill, source_label, prompt, context, source, model_tier }
    ↓ published on the bus Background topic by the pulse executor,
      gateway action spawning, a webhook handler, the subconscious learner,
      or the subagent_spawn tool
spawn listener (listener.rs)
    ├─ build_spawn_resources() → resolve tier → provider, activate skill,
    │  snapshot observations + recent-context narrative
    └─ SessionRuntime::spawn(SessionSpawnRequest, SubAgentResources)
         ├─ Register in the registry as `forking`, with a fresh CancellationToken
         ├─ SessionStore::begin_run() — write the initial record
         └─ tokio::spawn(async move { run_session(...) })
```

Every producer generates the session's address itself, up front — `subagent_spawn` needs to hand it back to the model synchronously, and the rest do it for consistency. The listener never generates an address; it only builds resources and hands the request to the runtime.

#### Session Execution

`run_session()` (in `runtime.rs`) drives one run:

1. **Acquire a permit or get cancelled first.** `tokio::select!` races the session's `CancellationToken` against `Semaphore::acquire()`. If stopped before a permit is available, the run produces a `Cancelled` result immediately — no turn ever starts.
2. **Run the turn.** Once a permit is held, the registry moves to `Running` and `execute_subagent()` (`subagent.rs`) runs the turn through the shared `execute_turn()` executor, with the session's own `CancellationToken` wired in as `TurnResources.stop_token` and a `RunTranscriptSink` wired in as `TurnResources.transcript_sink`. This is what makes `stop_agent` cooperative: cancelling ends the turn at its next checkpoint (a model call or tool-loop boundary) with the transcript so far intact, rather than dropping the whole future.
3. **Stage mid-run, if warranted.** After the turn, `SessionMemory::maybe_stage()` checks the run's transcript against the session's own `Observer` thresholds; crossing the force threshold extracts the unstaged tail immediately, mirroring the main agent's own rotation. Staged observations are held locally, invisible to any other agent until the run completes.
4. **Go idle.** The permit is released (it's a stack-scoped guard); the registry moves to `Idle`. The run then races the `CancellationToken` against `tokio::time::sleep(idle_timeout)` — whichever fires first ends the idle wait.
5. **Complete.** The registry moves to `Completing`. `complete_session_memory()` runs the skip check, a final extraction over whatever wasn't staged, and the merge into global memory (see below). `SessionStore::complete_run()` then overwrites the run's metadata record with its final state, full transcript, and episode id (if merged), and an `AgentResultEvent` — tagged with the session's address and run id — is published on the bus. The registry entry is then removed; the session no longer appears in `list_agents`, though its store record remains.

#### Incremental Transcript Persistence

A session's transcript is not just written once at completion. `agent::turn::TranscriptSink` is a trait with one method, `append()`, called by `execute_turn()`/`execute_tool()` after every model response and every tool result is pushed onto the turn's message buffer. `RunTranscriptSink` (`store.rs`) implements it by binding a `SessionStore` to one run's address and start time, appending each message to the run's `.transcript.jsonl` file as it happens. The main agent's own turns pass `None` for this field (its persistence path is `recent_messages.json`, written after the whole turn) — sessions are the only caller that needs crash-mid-turn durability, since a session's transcript is otherwise only written at fork time (empty) and at completion.

#### Fork Contents

`execute_subagent()` builds the turn's starting user message from **only** the source-specific input (task prompt, pulse/action/webhook payload, plus any explicit context) — no identity, wiki, or skills content goes into it. Everything else — `SOUL.md`, `AGENTS.md`, `HARNESS`, `USER.md`, the wiki index, the skills index, and the fork-time observation/recent-context snapshot — flows through `MemoryContext`/`PromptContext` into the *system* message, assembled once per iteration by `execute_turn()`'s own `assemble_system_prompt()`, exactly as it is for the main agent. This is deliberate: putting identity content in both the user message and the system message (as the old sub-agent path did) would show up twice in every model call.

### Memory Merge

`complete_session_memory()` (`session_memory.rs`) decides whether a run produces an episode and, if so, submits it to the `MemoryMergeWriter`:

1. **Skip check.** No episode if the run's final turn summary contains `HEARTBEAT_OK`, or if the transcript is below `episode_skip_token_floor` (`BackgroundConfig`, default ~2000 tokens) with nothing staged. The transcript is still kept in the session store either way.
2. **Final extraction.** The observer extracts over whatever transcript tail wasn't already staged mid-run.
3. **Merge.** Staged and final observations, plus the run's full transcript, go to `MemoryMergeWriter::merge()` tagged with a `SourceTag` naming the session's address, run id, and category. The writer allocates the episode id, writes the transcript/observation archives, indexes, embeds, and checks the reflector — serialized against every other merge (main agent included) through its own lock, so episode numbering never races between concurrent sessions.

The main agent's own observation flow (`crate::gateway::memory`) goes through the identical `Observer::extract()` → `MemoryMergeWriter::merge()` split; only the main agent's flow additionally saves the merge's narrative to `recent_context.json` — session merges never touch it (a session's narrative is captured on its episode's own meta line instead).

### Result Routing

`notify::router` subscribes to the bus `Background` topic and routes each `AgentResultEvent` by the disposition its session declared (via `HEARTBEAT_OK`/`HEARTBEAT_URGENT` sentinels in its summary — computed in `runtime.rs::build_result_event`, using the same rule the old bridge module used): `Silent` results are discarded; `spawned`-category results (`EventTrigger::Agent`) relay to the main agent; everything else files to the inbox, plus every configured notification channel when `Urgent`.

---

## Design Decisions

**Decision: The concurrency permit is held only while a turn runs, not for a session's whole lifetime.**

**Why:** An idle session — lingering to receive a reply before it completes — costs memory, not a slot in the pool. Acquiring the semaphore permit inside `run_session()`, after registering the session as `forking`, means a session that's still being built (or waiting for a permit) is discoverable via `list_agents` before it's actually running, and a burst of idle sessions never starves new work.

**Why not one permit for the whole run:** it would mean `max_concurrent` bounds "how many sessions can exist," not "how many turns can execute concurrently" — the two aren't the same shape of resource pressure, and the latter is what actually protects the model provider from a fan-out.

---

**Decision: `stop_agent` cancels cooperatively through `execute_turn`'s own stop token, not by dropping the run's future.**

**Why:** The old background-task spawner raced a `tokio::select!` around the whole `execute_subagent()` future, so cancelling dropped everything, including whatever transcript had accumulated. Wiring the session's own `CancellationToken` into `TurnResources.stop_token` — the same mechanism the main agent's turn loop already uses for user-initiated stops — means a stopped session's transcript up to the cancellation point survives in the session store. Stopping is not discarding.

---

**Decision: Producers generate their own session address, not the listener.**

**Why:** `subagent_spawn` needs to return the address to the model in the same tool call that starts the spawn — the fork happens asynchronously, so there's no later point to hand it back. Rather than special-casing that one caller, every `SpawnRequestEvent` producer (pulse executor, gateway action spawner, webhook handler, subconscious learner) generates the address the same way, via `registry::generate_address()`. This also means the address never depends on a step the listener could fail before reaching.

---

**Decision: No sub-to-session nesting yet.**

**Why:** Depth-capped nesting (a session spawning another session) is a real part of the design but depends on cross-session messaging existing first — a nested session's result needs somewhere to relay to besides main. `build_subagent_registry()` still registers `stop_agent` and `list_agents` for a session's own tool set (any session can inspect or stop any other) but omits `subagent_spawn`, keeping the spawn tree exactly two levels deep (main → spawned) until messaging lands.

---

## Dependencies

### Depends On

- **`crate::config`** — `BackgroundConfig` (max_concurrent, idle timeouts per category, model tiers), `BackgroundModelTier`, `ProviderSpec`, `ModelSpec`.
- **`crate::inference`** — `InferenceProvider`, `CompletionOptions`, `SharedHttpClient`, `Message`. Used to build and call LLM providers for a session's turn.
- **`crate::inference::retry`** — `RetryConfig`, passed to provider construction.
- **`crate::agent::context`** — `assemble_system_prompt`/`build_system_content` (via `execute_turn`), `PromptContext`, `MemoryContext`, `SkillsContext`, and `crate::agent::context::loading::{load_observations, load_recent_context_narrative}` for the fork-time memory snapshot.
- **`crate::agent::turn`** — `execute_turn()`. The session executor calls this to run the turn loop, passing the session's `CancellationToken` as the stop token.
- **`crate::agent::recent_messages`** — `RecentMessages`, the message buffer for a session's turn.
- **`crate::agent::interrupt`** — `dead_interrupt_rx()`. A session's interrupt channel is a dead end until cross-session messaging exists.
- **`crate::memory::observer`** — `Observer`, `Extraction`. Each session gets its own `Observer` instance (built from the same `[observer]` config as the main agent's) for per-run threshold checks and extraction.
- **`crate::memory::merge_writer`** — `MemoryMergeWriter`, `SourceTag`. Shared with the main agent so episode numbering, the observation log, indexing, embedding, and the reflector trigger are all serialized through one writer.
- **`crate::memory::tokens`** — `estimate_message_tokens()`, for the episode skip token floor check.
- **`crate::mcp`** — `SharedMcpRegistry`, shared across the main agent and every session.
- **`crate::skills`** — `SkillState`, `SharedSkillState`. Each session gets an isolated clone.
- **`crate::tools`** — `ToolRegistry`, `PathPolicy`, `FileTracker`. Each session gets fresh isolated instances.
- **`crate::workspace`** — `IdentityFiles`, `WorkspaceLayout` (including `sessions_dir()`).
- **`crate::bus`** — `EventTrigger`, `SessionAddress`, `SkillName`, `AgentResultStatus`, `ResultDisposition`, `HEARTBEAT_OK`/`HEARTBEAT_URGENT`, `AgentResultEvent`, `SpawnRequestEvent`.
- **`tokio`** — `tokio::sync::{Semaphore, Mutex, Notify}`, `tokio_util::sync::CancellationToken`.
- **`chrono`, `chrono_tz`** — timestamps throughout; timezone conversion for `AgentResultEvent`.
- **`anyhow`**, **`serde_json`/`serde`** — error handling and (de)serialization of `RunRecord`.

### Used By

- **`src/gateway/startup/mod.rs`** — constructs the `SessionRegistry`, `SessionStore` (running its startup sweep), and `SessionRuntime`; builds the `SpawnContext`.
- **`src/gateway/event_loop/run_loop.rs`** / **`reload.rs`** — wire `SessionRuntime`/`SessionRegistry` into `GatewayRuntime`, and rebuild `SpawnContext` on config reload.
- **`listener.rs`** — the sole caller of `SessionRuntime::spawn()`.
- **`src/gateway/actions.rs`** — forks a `scheduled` session for each due scheduled action.
- **`src/pulse/executor.rs`** — builds a `SpawnRequestEvent` from a due pulse.
- **`src/interfaces/webhook.rs`** — forks an `external` session for a webhook routed to an agent.
- **`src/subconscious/learning.rs`** — forks a `spawned` session running the `learner` skill.
- **`src/tools/background.rs`** — `stop_agent`/`list_agents` (read the `SessionRegistry` directly), `subagent_spawn` (publishes a `SpawnRequestEvent` with a pre-generated address).
- **`src/tracing_service/client_context.rs`** / **`src/tools/file_bug_report.rs`** / **`src/gateway/web/tracing_api.rs`** — read `SessionRegistry::subagent_snapshot()` to populate a bug report's `active_subagents`.
- **`src/agent/turn.rs`** — `execute_turn()` is what a session's executor calls to run its turn loop.

---

## Module Organization

| File | Purpose |
|------|---------|
| `mod.rs` | Module exports: `SessionRegistry`, `SessionRuntime`, `SessionStore`, `SubAgentResources`/`build_subagent_resources`, `SubAgentBuildConfig`/`SubAgentConfig`. |
| `registry.rs` | `SessionRegistry`, `SessionInfo`, `SessionCategory`, `SessionState`, address/run-id generation. |
| `store.rs` | `SessionStore`, `RunRecord`, `RunTranscriptSink`: per-run metadata persistence, the incrementally-appended transcript file, and the startup incomplete-run recovery sweep. |
| `session_memory.rs` | `SessionMemory`, `SessionMemoryEnv`, `complete_session_memory()`: per-run mid-run staging and the completion pipeline that merges into global memory. |
| `runtime.rs` | `SessionRuntime`: concurrency-bounded execution, lifecycle driving (`run_session`), memory merge on completion, and `AgentResultEvent` construction. |
| `types.rs` | `SubAgentConfig` (a run's turn configuration), `SubAgentBuildConfig` (fork construction inputs), `truncate_prompt_preview()`. |
| `subagent.rs` | `SubAgentResources` (isolated state bundle), `build_subagent_resources()`, `execute_subagent()` (runs one turn through `execute_turn()`). |
| `spawn_context.rs` | `SpawnContext` (gathered at gateway startup/reload): config, provider specs, identity, workspace layout, session-tool dependencies, this session's `Observer` and the shared `MemoryMergeWriter`. `build_spawn_resources()` resolves the tier, activates the skill, snapshots memory, and builds `SubAgentResources`. |
| `listener.rs` | Bus listener: turns each `SpawnRequestEvent` into a `SessionRuntime::spawn()` call. |
