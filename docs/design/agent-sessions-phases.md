# Agent Sessions — Implementation Phases

> **Status:** planned, not started. Design: [`agent-sessions.md`](./agent-sessions.md). Terms (session, run, address, category, spawner, hop count) are defined there.

> Module level only. No file or line references — those get mapped in each phase's own session. Each phase is self-contained, depends only on phases before it, and is verifiable on its own. Each phase ships as its own branch and PR, with passing pre-commit gates, and updates the `docs/systems-usage/` pages whose behavior it changes, along with their mirrors in the bundled `residuum-system` skill references.

## Phase 1 — Session core

Replace the background task spawner with the session registry, runtime, and store, without yet adding memory merging, messaging, or conversation routing.

- **Modules:** background execution (spawner, listener, subagent execution, result bridge, types); spawn-request bus event; agent context assembly; background management tools (`subagent_spawn`, `list_agents`, `stop_agent`); pulse and action executors and their config types; webhook and subconscious-learner spawn paths; workspace layout; background configuration; tracing client context for bug reports.
- **Preconditions:** none.
- **Shape when done:**
  - A session registry tracks every live session with its address, run id, category (`scheduled`, `external`, `spawned`), source label, state (`running`, `idle`, `completing`, `completed`), spawner, depth, and purpose. Every existing spawn source creates a session through it: `subagent_spawn` and the learner create `spawned` sessions, pulses and actions create `scheduled` ones, and webhooks create `external` ones.
  - Sessions run turns through the turn executor with their own agent instance. After a turn they go idle, then complete on the per-category idle timeout (configurable; defaults 2 / 10 / 30 minutes for scheduled / spawned / external). The concurrency permit is held per running turn, not per session.
  - Forks are built per the design's "Fork contents": identity assembled once in the system message (the double injection is gone), a snapshot of the observation log and recent-context narrative, the source input, skill, and tier. `include_identity` is removed from spawn requests and pulse definitions.
  - `agent: "main"` is removed from pulses and actions. A `HEARTBEAT.yml` pulse or a stored action that uses it, or a pulse with `include_identity`, fails to load with an error naming the entry and telling the owner what to remove.
  - The pulse and action prompts drop the instruction not to start further background work. They keep the instruction not to create or modify pulses.
  - The session store writes each run's metadata and an append-only transcript under the memory directory, organized by date, as the run progresses. Stopped runs keep their transcripts. At startup, runs left incomplete are marked completed. Memory merging for them arrives in Phase 2.
  - `subagent_spawn` returns the session address. `list_agents` returns main plus live sessions with the design's fields. `stop_agent` stops by address and moves the run to completing.
  - Result delivery keeps today's routing, now fed by sessions. A `spawned` session's final output from each turn is relayed to the main agent, tagged with the session address. `scheduled` results go to the inbox under the existing disposition rules.
  - The bug-report client context populates `active_subagents` from the registry (closes #99).
  - The existing background log directory is no longer written.
- **Verification:**
  - `cargo test --quiet` passes. Tests cover the lifecycle transitions (running → idle → completing → completed, and stop), idle timeouts per category, permit release on idle, fork contents (identity appears once; the observation snapshot is present; the live conversation is absent), load errors for `agent: "main"` and `include_identity`, and startup marking of incomplete runs.
  - Manually: spawn a subagent from the web chat and confirm the tool returns an address, `list_agents` shows it as running then idle, and after the timeout it disappears and its run exists in the store with a full transcript.
  - A bug report shows the live sessions.

## Phase 2 — Session memory and the merge writer

Give each run working memory and merge it into global memory on completion.

- **Modules:** memory observer (split into extraction and persistence); episode store; memory merge writer (new); reflector trigger; gateway memory flow for the main agent; session runtime (threshold checks and rotation); session store (episode link on metadata); startup recovery; observation and episode record types.
- **Preconditions:** Phase 1 (sessions, store, completion hook, startup handling of incomplete runs).
- **Shape when done:**
  - Observer extraction takes messages and returns observations plus a narrative, with no side effects. All persistence (observation log append, episode write with id allocation, indexing, embedding, reflector check) goes through a single serialized merge writer. The main agent's existing observation flow uses it too, and its behavior is otherwise unchanged, including the recent-context narrative, which only main's observations replace.
  - During a run, the session's messages are checked against the same observer thresholds as the main agent's. When a threshold is crossed, observations are extracted into the run's staging area and the run's history is rotated. The run's prompt carries its fork-time snapshot plus its staged observations.
  - On completion, a run whose final turn ended in `HEARTBEAT_OK`, or whose transcript is under the configurable token floor with nothing staged, produces no episode. Otherwise the final observation runs and the merge writer commits the staged and final observations and the episode. Merged records carry the session address, run id, and category, and are optional when read so existing records still load.
  - The run's metadata records its episode id when merged.
  - At startup, incomplete runs go through the full completion pipeline from their persisted transcripts before normal operation (replacing Phase 1's simple marking).
  - HARNESS text no longer says background runs aren't captured in memory (closes #70).
- **Verification:**
  - Tests: concurrent merges from several runs plus a main-agent observation produce unique, sequential episode ids and a well-formed observation log. Skip rules hold (a `HEARTBEAT_OK` run and a tiny run produce no episode but keep their transcripts). A run that crosses the threshold stages observations and rotates. A merged run's observations are tagged with their source. Crash recovery merges an incomplete run from its transcript. Existing observation and episode files without source tags still load.
  - Manually: a spawned research session finishes, idles out, and its findings appear in the global observation log and in `memory_search` results. A later fork's prompt contains those observations.

## Phase 3 — Agent messaging and nesting

Let agents address each other, relay results through messaging, and allow depth-capped nesting.

- **Modules:** agent messaging (new: routing by address, hop counts); agent-messaging tool (new); `memory_get` (run-id mode); interrupt types; session runtime (interrupt receiver wired, wake on input, resume on a completed address); main agent event loop (receives agent messages); result relay (moved onto messaging); subagent tool registry (adds `subagent_spawn` with the depth cap); background configuration (hop limits, depth cap); notification router (spawned results no longer routed there).
- **Preconditions:** Phase 1 (registry, addresses, lifecycle), Phase 2 (episode ids on completed runs, for resume pointers).
- **Shape when done:**
  - An agent-messaging tool sends text to an address. Delivery follows the design:
    - a running session gets an interrupt at the next tool-call boundary;
    - an idle session starts a turn;
    - a completed session starts a new run in the same session, with a pointer to the previous run's episode or transcript, and the sender is told it was resumed;
    - `main` gets an interrupt or a main turn.
  - An unknown address returns an error listing how to discover agents.
  - Every delivered message names the sender's address and category.
  - Each `spawned` session turn's final response is delivered to its spawner through the same mechanism. Nested sessions relay to their direct spawner, not main.
  - `memory_get` has a run-id mode that returns a run's transcript from the session store, so resume pointers to runs without an episode can be followed.
  - Hop counts are carried and computed per the design, including the task brief of a spawned session (spawning turn's highest input hop count + 1). At or above the soft limit (default 8) the message carries the "reply only if needed" note. At or above the hard limit (default 32) delivery is refused with a tool error, a `warn` log, and an error event in both sessions' event streams. Limits are configurable.
  - Sessions have `subagent_spawn`. Depth is tracked, and spawning past the cap (default 2, configurable) is refused with an explanatory error.
  - The `BackgroundResult` interrupt variant (handled by the turn loop, never sent) is replaced by an agent-message interrupt carrying sender address, category, and hop count. It is the live delivery path into running turns, main's included.
- **Verification:**
  - Tests: delivery in each target state. A spawn chain accumulates hop counts. `memory_get` returns a run transcript by run id. The resume creates a new run with the pointer and the same address. Hop counts propagate and the soft note and hard refusal trigger at the limits. The depth cap refuses at the cap. Relays go to the direct spawner. Two sessions messaging each other in a loop stop at the hard limit.
  - Manually: main spawns a session, messages it mid-turn and sees it change course, then messages it after it completes and sees a resumed run that can find its previous episode.

## Phase 4 — Conversation routing

Route non-owner-DM conversations to per-conversation sessions and generalize the unmentioned-message context buffer.

- **Modules:** inbound message schema (bus event and interface types); conversation router (new); inbound message handling in the gateway event loop; Discord, Telegram, and Teams handlers; context buffer (moved out of Teams into shared interface infrastructure); interface configuration (`context_messages` per interface); reply targeting for session output.
- **Preconditions:** Phase 1 (sessions with `external` category and deterministic addressing support), Phase 3 (delivery to running and idle sessions, resume on a completed address).
- **Shape when done:**
  - The main agent receives only the owner's DMs and web UI messages. Every other admitted conversation (group chats, channels, non-owner DMs) is delivered to that conversation's session at its deterministic address, following the design's delivery rules. Admission rules are unchanged.
  - A conversation session's responses go to its own conversation. The owner-DM fallback and failure notification behave as today.
  - Unmentioned messages in shared conversations on Discord, Telegram, and Teams go into a shared per-conversation buffer, sized by each interface's `context_messages` setting, and are handed over with the next addressed message, either to a live session or to seed a new run. Discord buffers the server messages it already receives. Telegram buffers group messages when it receives them (closes #157).
  - The inbound message schema carries a stable conversation id, the conversation kind, and an owner flag. The Discord, Telegram, and Teams handlers populate them, and web UI messages are treated as the owner's.
  - Webhook sessions are unaffected beyond Phase 1.
- **Verification:**
  - Tests: each interface populates conversation id, kind, and the owner flag. Routing by sender and conversation kind (owner DM → main, group → session, non-owner DM → session). The address is stable across runs of the same conversation. Buffered context is delivered with the next mention and cleared. The buffer is bounded by `context_messages`.
  - Manually: in a Discord server channel, chat without mentioning the bot, then mention it and get a reply that shows awareness of the prior chatter. The owner's web chat shows none of it. Mention it again after the external idle timeout and get a resumed run.
  - `discord.md`, `telegram.md` (including the privacy-mode requirement), and `teams.md` are updated.

## Phase 5 — Web sessions surface

Stream sessions to the web UI and add the sidebar.

- **Modules:** gateway protocol (session lifecycle and session-tagged turn events; client messages to message and stop a session; generated TypeScript types); WebSocket subscriber; session event publishing from the runtime; HTTP endpoints for session listing and transcripts; web client stores and components (sessions store, sidebar, session view, feed handling of relayed results).
- **Preconditions:** Phases 1–4. The listing endpoint and store exist from Phase 1, messaging from Phase 3, and external sessions from Phase 4. The sidebar is built last so it shows every category with real behavior.
- **Shape when done:**
  - Sessions publish turn events and lifecycle events tagged with address and run id over the WebSocket. Main-agent events keep their shape.
  - An HTTP endpoint lists live sessions and completed runs (newest first, paginated, category filter). Another returns a run's transcript in the chat-history message shape.
  - The sidebar lists live sessions with category badge, source label, and state, updated live. A dropdown shows finished runs. Selecting one renders its live stream or transcript with the existing message components. Live sessions have a message input (delivered as hop-count-0 owner input) and a stop button.
  - Results relayed to main appear in the main chat feed as compact items linking to the originating session, instead of being filtered out. Hop-limit refusals show as errors in the affected sessions.
- **Verification:**
  - `npm run check` and lint pass in `web/`, and the generated protocol types are current. Rust tests cover the new protocol messages and endpoints.
  - Manually, in the browser:
    - spawn a subagent and watch it stream live in the sidebar;
    - message it mid-turn and see it respond;
    - stop it;
    - find it under finished sessions with its full transcript;
    - confirm a pulse appears as `scheduled` and a group-chat session as `external`, each with its sub-label.

## Phase 6 — Wrap-up and end-to-end verification

- **Modules:** documentation (`docs/systems-usage/` and its bundled-skill mirrors, `docs/guides/`, HARNESS, README), design archive.
- **Preconditions:** Phases 1–5.
- **Shape when done:**
  - `background-tasks.md` is rewritten as the systems-usage page for agent sessions. `heartbeats.md`, `scheduled-actions.md`, `notifications.md`, `memory.md`, `tools.md`, and the interface pages describe current behavior.
  - A migration guide in `docs/guides/` covers:
    - removal of `agent: "main"` and `include_identity`;
    - non-owner conversations no longer reaching the main conversation;
    - `subagent_spawn` returning an address;
    - the new configuration keys;
    - the old background log directory no longer being written.
  - Both design documents move to `docs/archive/`.
- **Verification:** run the design's scenarios end to end on a live build.
  - A spawned session is watched, messaged, resumed, and merged.
  - A pulse ending in `HEARTBEAT_OK` produces no episode, while one with findings does.
  - A group-chat session gets context, replies, idles out, and merges.
  - Two sessions in a message loop are stopped at the hard limit.
  - A restart mid-session loses no memory.
  - Each design decision in the design doc's "Reasoning & alternatives" section is observable in the running system.
