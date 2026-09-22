# Background Module: Agent Sessions

Owns the session registry, runtime, and store that execute work off the main agent's turn loop: pulse checks, scheduled actions, webhooks, and on-demand sub-agent delegation. See `docs/design/agent-sessions.md` for the systems-level design and `docs/systems-usage/background-tasks.md` for the user-facing behavior; this file covers how the module's own pieces fit together.

## Overview

A **session** is a temporary fork of the main agent: its own identity/memory snapshot, its own tool registry, its own message history. A session has an **address** (stable, human-readable, e.g. `spawned-researcher-3f9a`) and moves through a lifecycle — `forking` → `running` → `idle` → `completing` → `completed` — tracked by the registry for as long as it's live. A run is one or more turns: the first is always the fork's task prompt, and any later one is an agent message that arrived while the session was idle (see "Agent Messaging" below) — the run's history and per-turn memory staging span however many turns it takes.

The module owns:
- **The session registry** (`registry.rs`): the single source of truth for every live session's address, run id, category, source label, lifecycle state, spawner, depth, and purpose, plus the sender half of each session's interrupt channel and a standing `ResumePoint` per address (surviving past `completed`) for resuming a finished session. Discovery tools (`list_agents`, `stop_agent`) and the bug-report client context read it directly.
- **The session runtime** (`runtime.rs`): executes a session's turns, holding the concurrency permit only while a turn is actually running, then lingering the session `idle` until a message wakes it, its category's timeout elapses, or it's stopped, before finalizing the run and merging its memory.
- **The session store** (`store.rs`): durable on-disk record of every run — a metadata JSON file under `memory/sessions/YYYY-MM/DD/<run-id>.json`, plus a sibling `<run-id>.transcript.jsonl` appended to as the run progresses (see "Incremental Transcript Persistence" below).
- **Per-run memory** (`session_memory.rs`): threshold-based staging, run after every turn, and the completion pipeline that merges a finished run's observations into global memory through the `MemoryMergeWriter` (`crate::memory::merge_writer`).
- **Fork construction** (`spawn_context.rs`, `subagent.rs`): resolves the model tier, activates the requested skill, snapshots the global observation log and recent-context narrative, and builds the isolated `SubAgentResources` a session's turn runs with — including its own `Observer`, a shared handle to the `MemoryMergeWriter`, and its own `message_agent` tool identifying it as the sender.
- **Agent messaging** (`messaging.rs`): routes a `message_agent` call to its target by address — an interrupt into a running or idle session's channel, a fresh `SpawnRequestEvent` to resume a completed one, or a `MessageEvent` to main's own inbound path.

The module does **not** handle:
- **Channel delivery.** A `scheduled`/`external` run's completion still publishes its own `AgentResultEvent` to the bus; the `notify` module's router decides the concrete destinations (inbox, external notification channels) and delivers to them. A `spawned` run's result never reaches that router: it relays directly to its spawner via `AgentMessenger` after every turn (see "Result Routing" below), so `message_agent` and the disposition-based router are two distinct, non-overlapping delivery paths.
- **Skill discovery.** `crate::skills` owns the index; this module activates a named skill on the session's own `SkillState` and lets that failure fail the fork.
- **Persisting a merge.** `session_memory.rs` decides what to merge and when; `crate::memory::merge_writer::MemoryMergeWriter` is the single serialized writer that actually allocates episode ids, appends the observation log, indexes, embeds, and checks the reflector — shared with the main agent's own observation flow so numbering never races.

## How It Works

### Core Abstractions

**`SessionRegistry`** (`registry.rs`): an in-memory map from address to `SessionInfo`, the `CancellationToken` that stops the run, and the sender half of an `mpsc::Receiver<Interrupt>` created at `register()` time — the run's interrupt channel, alternately drained by an active turn (`execute_turn`'s checkpoint) and by the runtime's idle wait, for however long the run lives. `register()` is a compare-and-swap: it refuses (`Err(RegisterError)`) rather than overwriting when the address is already occupied, so two concurrent registration attempts for the same address can never silently clobber one another — the loser falls back to delivering its own input into whichever run won (see "Concurrent resumes" below). A separate map holds a `ResumePoint` per address (previous run id, episode id if any, category-defining trigger, source label, skill) that outlives the live entry, so a `message_agent` call after completion can still resume the address. `register`/`set_state`/`stop`/`remove`/`deliver`/`record_resume_point`/`resume_point` mutate or read it; `list_live`/`get`/`subagent_snapshot` read the live map. `generate_address(trigger, qualifier)` builds a new address from the category implied by the trigger (`EventTrigger::Pulse`/`Action` → `scheduled`, `EventTrigger::Agent` → `spawned`, `EventTrigger::Webhook` → `external`) and a slugified qualifier (skill, pulse, action, or webhook name).

**`SessionRuntime`** (`runtime.rs`): holds the concurrency `Semaphore`, an `Arc<SessionRegistry>`, an `Arc<SessionStore>`, and the per-category `IdleTimeouts` resolved once from `BackgroundConfig` at construction (like `max_concurrent`, a config reload does not resize this — the semaphore and timeouts are fixed for the runtime's lifetime). `spawn()` registers the session as `forking` synchronously, then drives the rest of the lifecycle — however many turns it takes — on a detached task. If `register()` refuses (another attempt already won the address — see "Concurrent resumes" below), `spawn()` delivers its own input into the run that won instead of starting anything.

**`AgentMessenger`** (`messaging.rs`): the `message_agent` tool's routing target. `send(to, from, from_category, content, hop_count)` refuses outright once `hop_count` reaches the configured hard limit (logged at `warn`, with a best-effort transcript note on whichever side is a live session), otherwise checks `to` against `MAIN_ADDRESS` first (publishes a `MessageEvent` on the `UserMessage` bus topic — the main event loop's own inbound path already implements interrupt-if-running/new-turn-if-idle), then tries `SessionRegistry::deliver()` (a live session — a `completing` target is handed to a detached task that waits for it to clear rather than blocking the caller), then falls back to `SessionRegistry::resume_point()` (publish a fresh `SpawnRequestEvent` at the same address) before reporting an unknown address. Needs an `Arc<SessionRegistry>`, a `Publisher`, an `Arc<SessionStore>` (for the hop-limit transcript note), and the configured `HopLimits` — resuming a completed session goes through the ordinary spawn-listener path rather than forking directly, so the messenger never needs a `SpawnContext`.

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

`run_session()` (in `runtime.rs`) drives a run's turn loop, accumulating one `RecentMessages` history across however many turns it takes:

1. **Acquire a permit or get cancelled first.** `tokio::select!` races the session's `CancellationToken` against `Semaphore::acquire()`. If stopped before a permit is available, the run produces a `Cancelled` result immediately — no turn ever starts. A later turn (started by a wake, below) races the same way and acquires its own permit, just like the first.
2. **Run the turn.** Once a permit is held, the registry moves to `Running` and `execute_subagent()` (`subagent.rs`) runs the turn through the shared `execute_turn()` executor, with the session's own `CancellationToken` wired in as `TurnResources.stop_token`, a `RunTranscriptSink` wired in as `TurnResources.transcript_sink`, and the run's own live interrupt channel receiver passed through as `execute_turn`'s `interrupt_rx`. This is what makes `stop_agent` cooperative (cancelling ends the turn at its next checkpoint with the transcript so far intact) and what delivers an agent message to a *running* session: `execute_turn`'s existing checkpoint-based draining picks it up at the next tool-call boundary, exactly like a main-agent user message.
3. **Stage after the turn.** `SessionMemory::maybe_stage()` checks the run's accumulated transcript against the session's own `Observer` thresholds; crossing the force threshold extracts the unstaged tail immediately, mirroring the main agent's own rotation. This runs after every turn, not just once, so a multi-turn run stages incrementally. Staged observations are held locally, invisible to any other agent until the run completes.
4. **Go idle.** The permit is released (it's a stack-scoped guard); the registry moves to `Idle`. `wait_idle()` then races the `CancellationToken`, `tokio::time::sleep(idle_timeout)`, and the same interrupt channel's `recv()` — whichever fires first ends the idle wait. An `Interrupt::AgentMessage` received here becomes the next turn's `TurnKickoff::AgentMessage`, looping back to step 1 instead of ending the run.
5. **Complete.** Once idle ends in a stop or timeout (not a wake), the registry moves to `Completing`. `complete_session_memory()` runs the skip check, a final extraction over whatever wasn't staged, and the merge into global memory (see below). The registry's `record_resume_point()` captures this run's id, episode id (if any), trigger, source label, and skill before `SessionStore::complete_run()` overwrites the run's metadata record with its final state, full transcript, and episode id, and an `AgentResultEvent` — tagged with the session's address and run id — is published on the bus (the notification router treats a `spawned` result on this event as already handled — see "Result Routing" — and discards it there). The registry's live entry is then removed, notifying `SessionRegistry::wait_until_clear()`'s waiters; the session no longer appears in `list_agents`, though its store record and resume point remain.

Between steps 2 and 4, a `spawned` session with a spawner relays that turn's outcome via `AgentMessenger::relay_result_to_spawner()` (`runtime.rs`) — after every turn, not only at step 5 — so a multi-turn run reports progress incrementally rather than only once at the end. Every outcome relays, not just a completed turn with text: `maybe_relay_result()` builds a status line (`relay_content()`) for a completed turn with no output, a failed turn, or a cancelled one; `recover_from_panic()` (the task-panic recovery path) relays the same way, reporting `Failed`. `relay_result_to_spawner()` itself treats `DeliveryOutcome::Unknown` (the spawner is unreachable, e.g. it restarted and lost its resume point) as a delivery failure, not silent success.

#### Agent Messaging

`message_agent` (`crate::tools::message_agent::MessageAgentTool`) is registered for both the main agent and every session, each instance carrying its own address, category, and a `HopCounter` clone so it can identify itself as the sender and compute the outgoing hop count (`hop_counter.outgoing()`, one more than the current turn's). It calls `AgentMessenger::send()`, which:
- refuses outright if `hop_count` has reached the configured hard limit, before touching `to` at all;
- for `main`, publishes a `MessageEvent` on `UserMessage` — the same bus path a relayed session result already uses, which the main event loop treats as an interrupt when a turn is running and as a fresh turn's input when idle; the hop count is recorded under the published `MessageEvent`'s id (`AgentMessenger::take_main_hop()`) rather than carried on the shared, interface-facing `MessageEvent` type itself;
- for a live session (any state `SessionRegistry` still has an entry for), calls `SessionRegistry::deliver()`, which hands an `Interrupt::AgentMessage` to that session's interrupt channel — picked up by a running turn's next checkpoint, or by the idle wait if the session isn't currently in a turn; a `completing` target's delivery is handed to a detached task (`deferred_resume()`) that awaits `wait_until_clear()`, then re-runs the same live-vs-resume decision (`resume_or_deliver_after_clear()`) before acting — if another message already resumed the address in the meantime, this one delivers straight into that live run instead of also resuming it — so the sender's call returns immediately rather than blocking on that run's own completion pipeline;
- for a completed session (no live entry, but a `ResumePoint` recorded), publishes a fresh `SpawnRequestEvent` at the same address — carrying the delivered message as the new run's prompt, that message's own hop count as the new run's starting hop count, and a pointer to the previous run's episode (or run id) as its context — which the ordinary spawn listener picks up exactly like any other fork request;
- otherwise, reports the address unknown.

Two messages queued to one completing session must produce exactly one new run with both delivered, not a second one silently dropped — see "Concurrent resumes" below for how `deferred_resume()`, the spawn listener, and `SessionRegistry::register()`'s compare-and-swap close that race together.

Every delivered message is formatted (`AgentMessageEvent::format_for_agent()`) naming the sender's address and category, so the recipient can reply. At or above the soft hop limit, the content also carries a note asking the receiver to reply only if needed.

Every session (and the main agent) tracks a `HopCounter` (`crate::agent::hop`) for whichever turn is currently running: set to the turn's kickoff input's hop count at the start (`TurnKickoff::hop_count()` for a session, or the looked-up `take_main_hop()` value for main), and raised (never lowered) whenever `agent::turn::drain_interrupts()` processes an `Interrupt::AgentMessage` mid-turn. `message_agent`/`subagent_spawn` read the same `HopCounter` instance to compute what they send.

Main's own turn loop (`gateway/event_loop/turns.rs`) folds the kickoff hop into the counter (`bump`, not `set`) rather than overwriting it, because `process_leftover_interrupts()` — run after every turn on whatever interrupts arrived but weren't consumed — resets the counter to zero only when there were none, and otherwise leaves it holding whatever an unconsumed leftover is still carrying (a mid-turn agent message relayed to main arrives as `Interrupt::UserMessage`, so its hop is recovered from the counter's own current value rather than the interrupt itself; a leftover `Interrupt::AgentMessage` carries its hop directly). Without this, a hop count that happened to still be sitting undrained when a turn ended would reset to zero at the next turn's kickoff, defeating the hard-limit loop guard. A session's own leftover messages don't have this problem the same way: they're drained into a fresh resumed run (`resume_with_messages()`), which already takes the max hop count across every combined message.

#### Incremental Transcript Persistence

A session's transcript is not just written once at completion. `agent::turn::TranscriptSink` is a trait with one method, `append()`, called by `execute_turn()`/`execute_tool()` after every model response and every tool result is pushed onto the turn's message buffer. `RunTranscriptSink` (`store.rs`) implements it by binding a `SessionStore` to one run's address and start time, appending each message to the run's `.transcript.jsonl` file as it happens. The main agent's own turns pass `None` for this field (its persistence path is `recent_messages.json`, written after the whole turn) — sessions are the only caller that needs crash-mid-turn durability, since a session's transcript is otherwise only written at fork time (empty) and at completion.

#### Fork Contents

`execute_subagent()` builds each turn's opening user message from a `TurnKickoff` — `TurnKickoff::Initial` (the task prompt, pulse/action/webhook payload, or a resume pointer, plus any explicit context) for a run's first turn, `TurnKickoff::AgentMessage` (the delivered message, formatted with the sender's address and category) for any later one — and **only** that; no identity, wiki, or skills content goes into it. Everything else — `SOUL.md`, `AGENTS.md`, `HARNESS`, `USER.md`, the wiki index, the skills index, and the fork-time observation/recent-context snapshot — flows through `MemoryContext`/`PromptContext` into the *system* message, assembled once per iteration by `execute_turn()`'s own `assemble_system_prompt()`, exactly as it is for the main agent. This is deliberate: putting identity content in both the user message and the system message (as the old sub-agent path did) would show up twice in every model call.

### Memory Merge

`complete_session_memory()` (`session_memory.rs`) decides whether a run produces an episode and, if so, submits it to the `MemoryMergeWriter`:

1. **Skip check.** No episode if the run's final turn summary contains `HEARTBEAT_OK`, or if the transcript is below `episode_skip_token_floor` (`BackgroundConfig`, default ~2000 tokens) with nothing staged. The transcript is still kept in the session store either way.
2. **Final extraction.** The observer extracts over whatever transcript tail wasn't already staged mid-run.
3. **Merge.** Staged and final observations, plus the run's full transcript, go to `MemoryMergeWriter::merge()` tagged with a `SourceTag` naming the session's address, run id, and category. The writer allocates the episode id, writes the transcript/observation archives, indexes, embeds, and checks the reflector — serialized against every other merge (main agent included) through its own lock, so episode numbering never races between concurrent sessions.

The main agent's own observation flow (`crate::gateway::memory`) goes through the identical `Observer::extract()` → `MemoryMergeWriter::merge()` split; only the main agent's flow additionally saves the merge's narrative to `recent_context.json` — session merges never touch it (a session's narrative is captured on its episode's own meta line instead).

### Result Routing

A `spawned` session's turn result is relayed to its **direct spawner** — the address recorded in `SessionInfo.spawner`, main or another session — via `AgentMessenger::send()`, from `maybe_relay_result()`/`relay_result_to_spawner()` in `runtime.rs`, after every turn (not only at completion), carrying the normal hop-count rules. A relay failure is never silent: it's logged at `warn` and appended as a system note to the run's own transcript (both the in-progress sidecar and, since it's also pushed onto `recent_messages`, the run's final completed record).

`notify::router` subscribes to the bus `Background` topic and routes each `AgentResultEvent` by the disposition its session declared (via `HEARTBEAT_OK`/`HEARTBEAT_URGENT` sentinels in its summary — computed in `runtime.rs::build_result_event`, using the same rule the old bridge module used): `Silent` results are discarded; `spawned`-category results (`EventTrigger::Agent`) are discarded here too, since they were already relayed per-turn as above; everything else files to the inbox, plus every configured notification channel when `Urgent`.

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

**Decision: Nesting is depth-capped, not unbounded or disabled.**

**Why:** `build_subagent_registry()` registers `subagent_spawn` (alongside `stop_agent`, `list_agents`, and `message_agent`) for every session's own tool set, so a session can spawn further sessions the same way main does. Unbounded nesting would rely on the concurrency limit alone to bound fan-out; a session at `subagent_depth_cap` (`[background]`, default 2; main is depth 0) has its own `subagent_spawn` refuse with an explanatory error instead, so a runaway spawn chain fails fast and visibly rather than silently consuming the whole permit pool.

---

**Decision: Hop counts bound message loops independently of the depth cap.**

**Why:** The depth cap bounds how deep a *spawn tree* can grow, but two sessions (or a session and main) messaging each other back and forth never spawns anything new — depth alone can't catch that loop. A hop count carried on every agent message, incremented once per turn it passes through, catches it instead: a soft limit (`hop_soft_limit`, default 8) nudges the model to stop replying once a reply isn't adding anything, and a hard limit (`hop_hard_limit`, default 32) refuses delivery outright so a genuine loop can't run forever. Because a `subagent_spawn` task brief also carries a hop count (the spawning turn's plus one), a chain of spawns is bounded by the same mechanism as a chain of replies, not just by the depth cap.

---

**Decision: Delivery to a `completing` target hands the wait off to a detached task instead of blocking the sender.**

**Why:** A `completing` run's teardown includes its full memory-merge pipeline, which can make an LLM call — the old code awaited that inline inside `AgentMessenger::send()`, so a `message_agent` tool call could block for however long that pipeline took. The resume point it needs also isn't guaranteed to exist the instant a run enters `Completing` (`finish_run` records it partway through teardown, before removing the entry) — this rules out reading it immediately and firing a resume synchronously. Spawning the wait (`deferred_resume()`, driven by `SessionRegistry::wait_until_clear()`) and returning a `Queued` outcome right away keeps the sender's turn moving; the resume itself still only ever fires once the previous run has genuinely cleared, which is what makes it safe to read the resume point at that point instead of racing it.

---

**Decision: Concurrent resumes at one address are resolved by re-checking liveness at every stage, with `SessionRegistry::register()`'s compare-and-swap as the last line of defense.**

**Why:** Two messages queued to the same completing session each get their own `deferred_resume()` task; both wait on the same `wait_until_clear()`, so both can observe the address as free and both attempt to resume it. Fixing this only where the two tasks first diverge isn't enough — `wait_until_clear()` only proves the address *was* empty at some point, not that it still is by the time the caller acts on that — so every stage downstream re-checks instead of trusting a stale answer:
- `deferred_resume()` re-runs `AgentMessenger::send()`'s own live-vs-resume decision (`resume_or_deliver_after_clear()`) right after the wait, delivering into a run that already won rather than blindly publishing a second resume.
- The spawn listener's own `Completing` handling recurses through the same guard (`handle_spawn_request()`) after its own wait, instead of forking directly — it can race a *second* deferred task's wait the same way.
- If both spawn requests still make it to the listener, the second one finds the address already `forking`/`running`/`idle` — it delivers its content into that live run as a message instead of refusing and dropping it (this is the *normal* outcome of two queued messages, not an error case).
- `SessionRegistry::register()` itself refuses (rather than overwriting) if the address is already occupied — the backstop for the one race none of the above serializes: two *detached* tasks (the listener's own `Completing`-deferral, `deferred_resume()`) that both pass their liveness check and both reach `fork_and_spawn`/`SessionRuntime::spawn()` concurrently. The loser delivers its own input into the winner instead of clobbering its live entry.

Net effect: exactly one new run starts, and every queued message reaches it — either as that run's kickoff or as a message delivered into it once it's live.

---

**Decision: Resuming a completed session goes through a fresh `SpawnRequestEvent`, not a direct fork call.**

**Why:** Every other spawn path (pulses, actions, webhooks, `subagent_spawn`, the learner) already goes through `SpawnRequestEvent` on the bus, picked up by the one spawn listener that builds fork resources and hands them to the runtime. Reusing that path for a resume means `AgentMessenger` needs only a `SessionRegistry` and a `Publisher` — not a `SpawnContext` — which avoids a reference cycle (`SpawnContext` already carries an `Arc<AgentMessenger>` for every fork's `message_agent` tool; the messenger holding a `SpawnContext` back would be circular). The cost is that a resume is asynchronous like any other fork: `message_agent`'s tool result reports that the session was resumed, not the new run's id.

---

**Decision: Delivery to a running session reuses `execute_turn`'s existing interrupt draining; delivery to main reuses the existing inbound-message bus path.**

**Why:** `execute_turn`'s tool loop already drains its `interrupt_rx` at every checkpoint — wiring a session's real interrupt channel into that (replacing the dead-end receiver every fork used to get) is enough to deliver "an interrupt at the next tool-call boundary" with no new mid-turn plumbing. Main's own turn loop already treats an inbound `MessageEvent` as an interrupt when a turn is running and as fresh input when idle (this is how a relayed session result reaches main today) — reusing it for `message_agent` avoids a second, parallel delivery mechanism for the one target that isn't a session.

---

## Dependencies

### Depends On

- **`crate::config`** — `BackgroundConfig` (max_concurrent, idle timeouts per category, model tiers), `BackgroundModelTier`, `ProviderSpec`, `ModelSpec`.
- **`crate::inference`** — `InferenceProvider`, `CompletionOptions`, `SharedHttpClient`, `Message`. Used to build and call LLM providers for a session's turn.
- **`crate::inference::retry`** — `RetryConfig`, passed to provider construction.
- **`crate::agent::context`** — `assemble_system_prompt`/`build_system_content` (via `execute_turn`), `PromptContext`, `MemoryContext`, `SkillsContext`, and `crate::agent::context::loading::{load_observations, load_recent_context_narrative}` for the fork-time memory snapshot.
- **`crate::agent::turn`** — `execute_turn()`. The session executor calls this to run the turn loop, passing the session's `CancellationToken` as the stop token.
- **`crate::agent::recent_messages`** — `RecentMessages`, the message buffer accumulated across a run's turns.
- **`crate::agent::interrupt`** — `Interrupt` (specifically `Interrupt::AgentMessage`), the type a session's real interrupt channel carries.
- **`crate::memory::observer`** — `Observer`, `Extraction`. Each session gets its own `Observer` instance (built from the same `[observer]` config as the main agent's) for per-run threshold checks and extraction.
- **`crate::memory::merge_writer`** — `MemoryMergeWriter`, `SourceTag`. Shared with the main agent so episode numbering, the observation log, indexing, embedding, and the reflector trigger are all serialized through one writer.
- **`crate::memory::tokens`** — `estimate_message_tokens()`, for the episode skip token floor check.
- **`crate::mcp`** — `SharedMcpRegistry`, shared across the main agent and every session.
- **`crate::skills`** — `SkillState`, `SharedSkillState`. Each session gets an isolated clone.
- **`crate::tools`** — `ToolRegistry`, `PathPolicy`, `FileTracker`. Each session gets fresh isolated instances.
- **`crate::workspace`** — `IdentityFiles`, `WorkspaceLayout` (including `sessions_dir()`).
- **`crate::bus`** — `EventTrigger`, `SessionAddress`, `SkillName`, `AgentResultStatus`, `ResultDisposition`, `HEARTBEAT_OK`/`HEARTBEAT_URGENT`, `AgentResultEvent`, `SpawnRequestEvent`, `AgentMessageEvent`, `MessageEvent`, `Publisher`, `topics`.
- **`crate::interfaces::types`** — `MessageOrigin`, for the `MessageEvent` `AgentMessenger` publishes to main.
- **`tokio`** — `tokio::sync::{Semaphore, Mutex, Notify, mpsc}`, `tokio_util::sync::CancellationToken`.
- **`chrono`, `chrono_tz`** — timestamps throughout; timezone conversion for `AgentResultEvent`.
- **`anyhow`**, **`serde_json`/`serde`** — error handling and (de)serialization of `RunRecord`.

### Used By

- **`src/gateway/startup/mod.rs`** — constructs the `SessionRegistry`, `SessionStore` (running its startup sweep), `SessionRuntime`, and the shared `AgentMessenger`; builds the `SpawnContext`; registers `message_agent` for the main agent (address `"main"`).
- **`src/gateway/event_loop/run_loop.rs`** / **`reload.rs`** — wire `SessionRuntime`/`SessionRegistry`/`AgentMessenger` into `GatewayRuntime`, and rebuild `SpawnContext` (cloning the same `AgentMessenger`) on config reload.
- **`listener.rs`** — the sole caller of `SessionRuntime::spawn()`; handles both a fresh fork and a `message_agent`-triggered resume identically, since both arrive as a `SpawnRequestEvent`.
- **`src/gateway/actions.rs`** — forks a `scheduled` session for each due scheduled action.
- **`src/pulse/executor.rs`** — builds a `SpawnRequestEvent` from a due pulse.
- **`src/interfaces/webhook.rs`** — forks an `external` session for a webhook routed to an agent.
- **`src/subconscious/learning.rs`** — forks a `spawned` session running the `learner` skill.
- **`src/tools/background.rs`** — `stop_agent`/`list_agents` (read the `SessionRegistry` directly), `subagent_spawn` (publishes a `SpawnRequestEvent` with a pre-generated address).
- **`src/tools/message_agent.rs`** — `MessageAgentTool`, registered for main and (via `build_subagent_registry()`) every session; calls `AgentMessenger::send()`.
- **`src/tracing_service/client_context.rs`** / **`src/tools/file_bug_report.rs`** / **`src/gateway/web/tracing_api.rs`** — read `SessionRegistry::subagent_snapshot()` to populate a bug report's `active_subagents`.
- **`src/agent/turn.rs`** — `execute_turn()` is what a session's executor calls to run its turn loop; its `drain_interrupts()` is what actually delivers an `Interrupt::AgentMessage` mid-turn.

---

## Module Organization

| File | Purpose |
|------|---------|
| `mod.rs` | Module exports: `SessionRegistry`, `SessionRuntime`, `SessionStore`, `AgentMessenger`/`DeliveryOutcome`/`SendError`, `SubAgentResources`/`build_subagent_resources`, `SubAgentBuildConfig`/`SubAgentConfig`, and a re-export of `HopCounter`/`HopLimits` from `crate::agent::hop` (defined there, alongside the main agent, since both main and sessions track one). |
| `registry.rs` | `SessionRegistry`, `SessionInfo`, `SessionCategory`, `SessionState`, `ResumePoint`, address/run-id generation, the interrupt-channel plumbing (`register()`/`deliver()`). |
| `store.rs` | `SessionStore`, `RunRecord`, `RunTranscriptSink`: per-run metadata persistence, the incrementally-appended transcript file, and the startup incomplete-run recovery sweep. |
| `session_memory.rs` | `SessionMemory`, `SessionMemoryEnv`, `complete_session_memory()`: per-turn staging and the completion pipeline that merges into global memory. |
| `runtime.rs` | `SessionRuntime`: concurrency-bounded execution, the turn loop and idle wait driving a (possibly multi-turn) run (`run_session`, `wait_idle`), memory merge and resume-point recording on completion (`finish_run`), and `AgentResultEvent` construction. |
| `messaging.rs` | `AgentMessenger`, `DeliveryOutcome`: routes a `message_agent` call to main, a live session, or a resume for a completed one. |
| `types.rs` | `SubAgentConfig` (a run's turn configuration), `SubAgentBuildConfig` (fork construction inputs, including the session's own address/category/messenger), `truncate_prompt_preview()`. |
| `subagent.rs` | `SubAgentResources` (isolated state bundle), `build_subagent_resources()`, `TurnKickoff` (a turn's opening input), `execute_subagent()` (runs one turn through `execute_turn()`, given the run's accumulated history and interrupt channel). |
| `spawn_context.rs` | `SpawnContext` (gathered at gateway startup/reload): config, provider specs, identity, workspace layout, session-tool dependencies, this session's `Observer`, the shared `MemoryMergeWriter`, and the shared `AgentMessenger`. `build_spawn_resources()` resolves the tier, activates the skill, snapshots memory, and builds `SubAgentResources` for a given address and category. |
| `listener.rs` | Bus listener: turns each `SpawnRequestEvent` (a fresh fork or a `message_agent` resume) into a `SessionRuntime::spawn()` call. |
