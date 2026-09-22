# Agent Sessions — Design

> **Status:** archived. This design shipped; it records the reasoning behind agent sessions, not current behavior. Current behavior is described in [`docs/systems-usage/background-tasks.md`](../systems-usage/background-tasks.md). Implementation phases: [`agent-sessions-phases.md`](./agent-sessions-phases.md).

> Systems level only. No file or line references. This document must stand on its own: it will be implemented in fresh sessions that have only this doc, the phases doc, and the codebase.

## Goal & context

Residuum's subagents are fire-and-forget background tasks. A subagent runs one turn, returns one summary, and disappears. The agent that spawned it gets no handle back, cannot talk to it while it runs, and never hears from it again after the summary. Nothing a subagent learns reaches memory: its transcript is written to a log directory that the observer never reads. Nothing streams to the web UI, so the owner cannot see what background work is happening. Meanwhile every inbound message from every interface — including group chats and other people's DMs — lands in the single main agent's conversation, so the owner's private context is shared with whoever talks to the bot in a group.

This overhaul replaces background tasks with **agent sessions**: temporary forks of the main agent. A session has an address, a lifecycle, its own transcript and working memory, and can exchange messages with any other session, including the main agent. Sessions stay alive until they go idle, then their memory is observed and merged into the main observation log — an eventual-consistency model where every fork eventually contributes what it learned back to the whole. Conversations that aren't the owner's own DMs get their own sessions instead of pulling in the main agent. The web UI gains a sidebar to watch, message, and stop sessions.

Issues addressed: #70 (background transcripts reach observational memory), #99 (live inventory of running subagents), #157 (unmentioned group messages handed to the agent as context on Discord and Telegram).

## Terms

- **Main agent** — the single long-lived agent that owns the owner's conversation (owner DMs and the web UI). Always addressable as `main`. Its own lifecycle, memory pipeline, and idle transition are unchanged by this design except where stated.
- **Session** — a temporary fork of the main agent. Identified by a stable **address**.
- **Run** — one incarnation of a session, from fork to completion. A session is a chain of one or more runs sharing an address. Each run has its own run id, transcript, and (possibly) episode.
- **Category** — how a session was started, shown in the UI and discovery:
  - `scheduled` — pulses and scheduled actions.
  - `external` — non-owner-DM conversations on chat interfaces, and webhooks.
  - `spawned` — started by an agent: the `subagent_spawn` tool (from main or from another session) and the subconscious learner.
- **Source label** — the specific origin within a category, shown as a sub-label: e.g. `pulse:inbox_check`, `action:<name>`, `webhook:<name>`, `discord:<conversation label>`, `agent:<skill or "subagent">`, `learner`.
- **Spawner** — the agent that started a `spawned` session (main or another session). `scheduled` and `external` sessions have no spawner.
- **Hop count** — a counter carried on every agent-to-agent message, used to bound message loops.

## Shape

### Components

1. **Session registry** — the single source of truth for every live session (running or idle). Owns addresses, lifecycle state, spawner links, depth, category, source label, and a one-line purpose for each session. Replaces the background task spawner's active-task map. Everything that needs to know "what agents exist" reads it: discovery tools, the web API, the bug-report client context (#99), and message routing.

2. **Session runtime** — executes a session's turns. Each session owns an agent instance (message history, skill state, file tracker, path policy) and an interrupt receiver. The runtime runs a turn when input arrives, then parks the session in `idle` until the next input or the idle timeout.

3. **Fork builder** — constructs a new run's starting state from the main agent (see "Fork contents").

4. **Session store** — durable on-disk record of every run: metadata and an append-only transcript written as the run progresses. The web UI's finished-session list and transcript viewer read from it. Crash recovery reads it.

5. **Session memory** — per-run working memory: a staging area for observations produced during the run, plus the completion pipeline that merges them into the global memory.

6. **Memory merge writer** — the single serialized writer for global memory: the observation log, the episode store, the search index and vectors, and the reflector trigger. Both the main agent's observations and session merges go through it, so episode numbering and log appends never race.

7. **Agent messaging** — routes messages between agents by address, applies the hop-count rules, and handles targets in each lifecycle state (running, idle, completed).

8. **Conversation router** — decides, for each inbound interface message, whether it goes to the main agent or to a conversation's session, and owns the per-conversation context buffer for unmentioned group chatter.

9. **Web sessions surface** — protocol messages and HTTP endpoints for listing sessions, streaming their events, reading transcripts, sending them messages, and stopping them; plus the sidebar in the web UI.

### Lifecycle

A run moves through these states:

```
forking → running ⇄ idle → completing → completed
                 ↘ (stop) ↗
```

- **running** — a turn is executing. The run holds one concurrency permit (see "Concurrency").
- **idle** — the turn ended; the session is still live, keeps its message history, and releases its permit. Any input (an agent message, a new mention in its conversation, a message from the web sidebar) starts a new turn.
- **completing** — the idle timeout elapsed, or the session was stopped. The session stops accepting input; its memory is finalized and merged (see "Memory model").
- **completed** — merged and closed. The run is immutable from here on. Its address still works: a message to it starts a new run in the same session (see "Messaging").

Idle timeouts differ by category. Defaults: `scheduled` 2 minutes, `spawned` 10 minutes, `external` 30 minutes. All three are configurable in the background configuration section. Webhook sessions are `external` but use the `scheduled` timeout, since a webhook call is one-shot with no expected back-and-forth.

Stopping a session (from `stop_agent`, or the web sidebar) cancels any in-flight turn and moves the run straight to `completing`. A stopped run's transcript is kept and merged like any other; stopping is not discarding.

### Addresses

- `main` is the main agent's fixed address.
- A `spawned` or `scheduled` session gets a generated address at its first run. It is short, unique, human-readable, and prefixed by category and source (e.g. `spawned-research-3f9a`). The exact format is an implementation choice; it must be safe in URLs and file names.
- An `external` conversation session's address is **derived deterministically** from its interface and conversation id, so every new message in that conversation resolves to the same session address whether or not a run is live. Webhook sessions get generated addresses (each webhook call is its own session).
- Run ids are distinct from addresses and unique across all runs.

### Fork contents

A new run starts with exactly:

- The main agent's identity and system prompt content (SOUL, AGENTS, HARNESS, USER, wiki index, skills index), assembled **once** in the system message. Forks always carry the full identity; the per-spawn `include_identity` option is removed.
- A **snapshot** of the global observation log and the recent-context narrative, taken at fork time. A run never sees merges that happen after it forked; it sees them on its next run. This is the eventual-consistency boundary.
- Its source-specific input: the task brief (spawned), the pulse or action prompt (scheduled), the webhook payload (webhook), or the conversation's inbound message plus buffered context (external).
- For a resumed session (second or later run), a pointer to the previous run's episode, or to its run id if the previous run produced no episode, with a note that it can be retrieved with `memory_get`. `memory_get` gains a run-id mode that reads a run's transcript from the session store, alongside its existing episode-id mode.
- The requested skill activated, when one was given (as today).
- The requested model tier (as today).

A fork does **not** copy the main agent's unobserved live conversation. The spawning agent is responsible for writing a task brief with whatever context the fork needs.

Pulse and action prompts keep telling the model not to create or modify pulses, but no longer tell it not to start further background work: scheduled sessions may spawn within the depth cap like any other.

Every fork gets the full tool suite, including `subagent_spawn` and the agent-messaging tools, with two exceptions: `switch_endpoint` stays main-only (it redirects main's background-turn output and is meaningless for a session), and `subagent_spawn` is refused once the depth cap is reached (see "Nesting"). The owner decides which environments the agent is added to; sessions are not sandboxed beyond what the main agent is.

### Memory model

Each run has working memory that mirrors the main agent's:

- The run's messages are checked against the same observer thresholds the main agent uses. When a run's context crosses them, the observer extracts observations from the run's older messages into the run's **staging area**, and the run's message history is rotated the same way the main agent's is. The run's prompt then carries the fork-time snapshot plus its own staged observations.
- Staged observations are not visible to any other agent until the run completes.

On completion:

1. **Skip check.** A run produces no episode only if it staged nothing over the course of its run and either its final turn ended with the `HEARTBEAT_OK` sentinel or its total transcript is below a small token floor (default around 2,000 tokens, configurable). Anything staged mid-run is real, already-extracted work, so it merges regardless of how the run ended. Its transcript is still kept in the session store and visible in the UI.
2. **Final observation.** Otherwise, the observer runs over the run's remaining unobserved messages.
3. **Merge.** The run's staged and final observations, and its episode transcript, are submitted to the memory merge writer. The merge writer assigns episode ids, appends observations to the global log, writes the episode to the episode store, indexes and embeds it, and checks whether the reflector should run. Each merged observation and episode is tagged with the session address, run id, and category so the source is traceable.

The observer is split into two capabilities: **extraction** (messages in, observations and narrative out, no side effects) and **persistence** (performed only by the merge writer). The main agent's existing observation flow uses the same split, so all persistence is serialized.

The recent-context narrative in global memory is only replaced by the main agent's observations, not by session merges. Session narratives are stored with their episodes.

### Messaging

Agents message each other by address with an agent-messaging tool, separate from the interface-facing `send_message` tool (which keeps its current meaning: posting to chat endpoints and conversations).

Delivery depends on the target's state:

- **running** — delivered as an interrupt at the target's next tool-call boundary, the same way user interjections reach the main agent today. The subagent interrupt channel, currently a dead end, is wired to the registry.
- **idle** — starts a new turn with the message as input.
- **completed** — starts a new run in the same session (see "Fork contents" for the pointer). The sender's tool result says the session had completed and was resumed as a new run.
- **main** — follows the same rules for the main agent: interrupt if a turn is running, otherwise it starts a main turn, as relayed background results do today.

Every delivered message names the sender's address and category, so the receiver can reply.

**Only the main agent talks to the owner.** A session never messages the owner directly. `send_message` called from a session refuses the owner's DM on any interface and the web UI, with an error telling the session to message `main` instead; it can still post to any other conversation or endpoint. This keeps the owner's replies coherent: whatever the owner answers lands in the main agent's conversation, and the main agent is the one that said it. When a session has something for the owner, it messages `main` (a `spawned` session's results already reach main through the result relay), and main decides what to tell the owner. Owner-facing notifications produced by the notification system rather than by a session — `scheduled` results in the agent inbox and the urgent fan-out to notify endpoints — are unchanged; they are not conversational and can't be replied to into a session.

**Result relay.** For a `spawned` session, each turn's final response is automatically delivered to its spawner as an agent message, tagged with the session address. This generalizes today's "background task result" relay: it happens after every turn, not once. `scheduled` session results keep going to the agent inbox under the existing disposition rules (silent on `HEARTBEAT_OK`, urgent on `HEARTBEAT_URGENT` fanning out to notify endpoints). `external` session output goes to its conversation and is not relayed to anyone.

**Hop count.** Every agent message carries a hop count. Input that originates outside the agent system (a user message, an external conversation message, a pulse or action firing, a webhook, a message sent from the web sidebar) has hop count 0. A message an agent sends during a turn carries one more than the highest hop count among the inputs that drove that turn. Result relays count as agent messages, and so does the task brief that `subagent_spawn` gives a new session: it carries one more than the spawning turn's highest input hop count, so chains of spawns are bounded the same way as chains of messages. A session's current hop count (the highest among the inputs driving its current turn) is tracked by the session runtime.

- At or above the **soft limit** (default 8), the delivered message carries a note asking the receiver to reply only if a reply is actually needed.
- At or above the **hard limit** (default 32), delivery is refused. The sender's tool call returns an error explaining the loop limit, the refusal is logged at `warn` with both addresses and the hop count, and it appears in both sessions' event streams so it's visible in the sidebar.

Both limits are configurable.

### Nesting

Sessions can spawn sessions. Depth counts from the main agent: the main agent has depth 0, `scheduled` and `external` sessions have depth 1, and every `spawned` session has its spawner's depth plus 1, whatever the spawner's category. The depth cap (default 2, configurable) is the maximum depth a spawned session can have; `subagent_spawn` in a session at the cap is refused with an error that explains why. Nested sessions relay results to their direct spawner.

### Discovery

`list_agents` returns the main agent plus every live (running or idle) session: address, category, source label, state, depth, spawner, elapsed time, and a one-line purpose (the task brief, prompt, or conversation label, truncated). Completed sessions are not listed. Their addresses remain valid and are recorded in merged observations and episodes, so an agent can find one through memory.

`subagent_spawn` returns the new session's address.

`stop_agent` stops a session by address. Any agent can stop any session. The main agent cannot be stopped this way.

### Concurrency

The existing `max_concurrent` limit bounds **running turns** across all sessions, not live sessions. A session acquires a permit when a turn starts and releases it when the turn ends. Idle sessions hold nothing. The main agent is not counted. Since no tool blocks a turn waiting on another session (results arrive as messages), a parent waiting for a child never holds a permit, so nested spawning cannot deadlock the pool. Turns that can't get a permit wait in arrival order.

### Conversation routing

The main agent handles only the owner's DMs and the web UI. Every other conversation that the interfaces admit — group chats, channels, and DMs from admitted non-owners — goes to that conversation's session:

- If the conversation's session is live, the message is delivered to it (interrupt if running, new turn if idle).
- Otherwise a new run starts at the conversation's derived address (a resume, if the conversation has had a run before).

Admission rules (owner claim, `respond_to_others`, standing) are unchanged; they decide whether a message is handled at all, and routing decides who handles it.

A conversation session replies to its own conversation by default, and it can still use `send_message` to post to other conversations. Its output never falls back to the owner's DM: where an interface would otherwise fall back to the owner's DM because the reply target can't be resolved, session output is dropped with an `error` log naming the session and conversation, and a failure notice is delivered to `main` as an agent message so main can decide whether to tell the owner.

**Unmentioned chatter.** In shared conversations, the bot only responds when addressed (mention, or reply to the bot, per interface rules as today). Unmentioned messages are held in a per-conversation context buffer and handed over with the next addressed message, whether that message goes to a live session or starts a new run. The buffer is shared infrastructure across Discord, Telegram, and Teams, with a per-interface `context_messages` setting for its size. Discord starts buffering the unmentioned server messages it already receives. Telegram buffers group messages when the bot receives them, which requires the owner to turn off privacy mode in BotFather or make the bot a group admin; this is documented.

### Scheduled triggers

Every pulse and scheduled action runs as a `scheduled` session. The `agent: "main"` option for pulses and actions is removed: a fork already carries the main agent's memory, and "main" never actually ran a turn anyway (it only queued a note for the next user turn). A `HEARTBEAT.yml` pulse or a stored action that still says `agent: "main"` is rejected at load with an error naming the pulse or action and telling the owner to remove the field. It is never silently reinterpreted. The `include_identity` pulse field is removed the same way.

### Web sessions surface

- **Listing.** An HTTP endpoint returns live sessions from the registry and completed runs from the session store (newest first, paginated, filterable by category).
- **Transcript.** An HTTP endpoint returns a run's transcript, live or completed, in the same message shape the chat history endpoint uses, so existing message components render it.
- **Live events.** Sessions publish the same turn events the main agent does (turn started and ended, tool call, tool result, response, errors), tagged with session address and run id, over the existing WebSocket, plus lifecycle events (session started, state changed, completed) so the sidebar updates without polling. Events for the main agent keep their current shape. Hop-limit refusals appear as error events.
- **Control.** Client messages to send a message to a session (delivered as hop-count-0 agent input from the owner) and to stop a session.
- **Sidebar.** A dedicated sidebar lists live sessions with their category badge (scheduled, external, spawned), source label, and state. A dropdown shows finished sessions from the store. Selecting a session shows its live stream or transcript in the main pane, with an input box to message it and a stop button while it is live. Results relayed to the main agent appear in the main chat feed as a compact item that links to the originating session. Today these are hidden because background-visibility messages are filtered out.

## Reasoning & alternatives

- **Sessions as forks, not isolated workers.** A fork carries the main agent's identity and memory, so a pulse or a group-chat session behaves like the same agent rather than a generic tool runner. Isolation (reduced toolsets, sandboxing channel sessions) was considered and rejected: the owner controls which environments the agent is added to, and a crippled fork in a group chat is worse at its job with little real safety gain.
- **Memory, not live conversation, in the fork.** Copying the main agent's unobserved history (a "true fork") was rejected: it costs up to the force threshold (~60k tokens) per spawn, and it would leak the owner's private conversation into group-chat sessions and pulses. A per-trigger split (full fork for agent spawns, memory-only otherwise) was rejected as two context paths to maintain. The spawner writes a task brief instead.
- **Snapshot memory with merge on completion (eventual consistency).** Live-updating each session's view of the observation log would need a shared mutable log across concurrent agents and a prompt that changes mid-session. Snapshots keep each run's context stable and the concurrency model simple. The cost is that two concurrent sessions don't see each other's learnings until a later run, which messaging covers when it matters.
- **Linger until idle for every category.** Completing at turn end was simpler and merged faster, but then almost every reply to a message a session sent would land after it completed and force a resume-fork. One lifecycle for all categories means replies land in a session that still has its full context. Idle sessions release their permit, so lingering costs memory, not throughput.
- **Resume as a new run, not reloading a transcript.** Reopening a completed run would mean re-merging observations that were already merged (needing dedup) and would make "completed" not mean final. A new run with a pointer to the previous episode keeps runs immutable and still gives continuity through memory. Rejecting messages to completed sessions was considered and rejected: a conversation's address must keep working across idle gaps.
- **Skip no-op runs.** Observing every run would flood the log with "checked, nothing new" episodes from pulses and churn the reflector. Letting the observer decide on every run costs an LLM call per pulse. The sentinel plus token floor is cheap and predictable; transcripts are still kept.
- **Single merge writer.** Episode numbering, log appends, the index, and the reflector all assume one writer. Serializing merges (main's included) through one writer is simpler and safer than making each of those concurrency-safe.
- **Interrupt delivery for messages.** Queuing until a turn ends was simpler, but a long turn couldn't be redirected ("stop, main already found it"). The interrupt path already exists for the main agent. A sender-chosen urgency flag was rejected as a judgment call agents would get wrong.
- **Auto-relay each turn to the spawner.** Relying on explicit messages alone means a subagent that forgets to report fails silently. Relaying only on completion would delay results by the idle timeout.
- **Hop count with a soft note and a generous hard limit.** A per-session message budget can't tell a loop from legitimate long coordination. No guard at all burns tokens until someone notices. The soft note asks the model whether a reply is needed before it ever reaches the hard limit.
- **Depth-capped nesting.** No nesting would force a busy session to route work through main. Unlimited nesting relies on the concurrency limit alone to bound fan-out. Holding permits per turn, not per session, is what makes nesting safe from deadlock.
- **All non-owner-DM conversations get sessions.** Routing only group chats to sessions would still put other people's DMs into the owner's private thread. A per-interface setting was rejected as extra configuration surface with no clear use case.
- **Removing `agent: "main"`.** Making it actually run a main turn was the alternative. Since forks carry main's memory, the only thing "main" adds is the unobserved live conversation, which scheduled work shouldn't depend on. One execution path is simpler.
- **Three categories.** Per-source labels (six of them) were more to scan. The three categories match how the owner thinks about the work (things on a schedule, things other people or systems started, things the agent started), and the sub-label keeps the precise source visible.

## External touchpoints

- **LLM providers.** Sessions make the same provider calls the main agent makes, through the same provider and tier resolution. Observer extraction for sessions uses the same observer model configuration as the main agent. There's no new provider contract.
- **Chat interfaces (Discord, Telegram, Teams).** Inbound: interfaces keep publishing inbound messages with origin and sender. The conversation router needs the conversation id, the conversation kind (personal, group, channel), and whether the sender is the owner. Each interface handler determines all three internally when it admits a message, but none of them reach the inbound bus event today: the origin carries only a free-text location label. The inbound message schema therefore gains a stable conversation id, a conversation kind, and an owner flag, and the Discord, Telegram, and Teams handlers populate them before publishing. Web UI messages carry none of them and are treated as the owner's. Outbound: replies from a conversation session carry the conversation id so delivery targets the right conversation. The existing correlation-based reply targeting and owner-DM fallback remain. Telegram's privacy-mode requirement for receiving unaddressed group messages is a platform constraint, documented rather than worked around.
- **Webhooks.** Each webhook call routed to an agent starts an `external` session. The webhook's HTTP response contract is unchanged.
- **Web UI (WebSocket and HTTP).** New server messages (session lifecycle, session-tagged turn events), new client messages (message a session, stop a session), and new HTTP endpoints (list sessions, read a transcript). Protocol types are generated for the web client as today. Existing main-agent messages keep their shape, so a client that ignores session events keeps working.
- **Feedback ingest service.** The bug-report client context populates `active_subagents` (name, status) from the session registry. The wire contract is unchanged; the field just stops being empty.
- **Workspace files.** `HEARTBEAT.yml` pulses lose `agent: "main"` and `include_identity`, and stored actions lose `agent: "main"` (actions never had `include_identity`); entries containing them fail to load with an actionable error. The observer and reflector prompt files are unchanged in format. Global memory files keep their formats; observation and episode records gain source-tagging fields that must be optional when read, so existing records keep loading.
- **Session store on disk.** It lives under the memory directory and is organized by date, like episodes. Each run has metadata (address, run id, category, source label, spawner, depth, timestamps, final state, and episode id if merged) and an append-only transcript written as the run progresses. The existing background log directory is left in place and not migrated; the UI lists only runs recorded in the new store.

## Integration with existing system

- **Replaces:** the background task spawner, its active-task map, and the one-shot subagent execution path; the single-result bridge from subagents to the main agent; the fire-and-forget `subagent_spawn` return; the background log directory as the place new transcripts are written; `agent: "main"` pulse and action routing; `include_identity`; the `BackgroundResult` interrupt variant, which the turn loop handles but nothing currently sends (replaced by an agent-message interrupt that carries sender address, category, and hop count, and becomes the live delivery path for messages and result relays into running turns, main's included); the Teams-only context buffer (generalized).
- **Wraps / reuses:** `memory_get` (gains the run-id mode); the turn executor (sessions run turns through it with a real publisher, interrupt receiver, and output target instead of the no-op ones); the observer's extraction logic; the reflector; the spawn-request bus event (kept as the way triggers ask for a session, gaining spawner and depth); the pulse scheduler and action scheduler (unchanged scheduling, new execution target); the notification router's inbox and urgent-fanout rules for `scheduled` results; the admission rules on each interface; the WebSocket transport and generated protocol types.
- **Leaves untouched:** the main agent's conversation, idle transition, and subconscious (the subconscious keeps watching only main-agent turns); the user inbox; skills; MCP; the wiki; configuration outside the background section and interface `context_messages` settings.
- **Startup and shutdown.** Sessions do not survive a restart or reload. At shutdown, live sessions are stopped. At startup, any run in the store that never reached `completed` goes through the completion pipeline from its persisted transcript (skip check, observe, merge) before normal operation, so nothing a session learned is lost to a crash or restart.
- **Transition.** This is a single cutover, not a dual-run period. A migration guide covers the breaking changes: the `agent: "main"` and `include_identity` removals, other people's DMs and group chats no longer reaching the main conversation, and `subagent_spawn` now returning an address.

## Open questions

None.
