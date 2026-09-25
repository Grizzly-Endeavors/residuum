# Background Tasks

Background tasks let the agent run work without blocking the main conversation. The execution model is **agent sessions** — temporary forks of the main agent that run independently and deliver results through notification channels.

## Sessions

A session is a fork of the main agent with its own identity, memory snapshot, and tool registry, running off the main thread. Its tools share the main agent's live tool `PATH`, write policy (the same config and credential files are blocked), and [agent key](agent-keys.md) store.

**What's included in a session's fork:**
- The main agent's full identity and system prompt content — `SOUL.md`, `AGENTS.md`, `HARNESS`, `USER.md`, the wiki root index, the skills index — assembled once in the system message, exactly as it is for the main agent.
- A snapshot of the global observation log and the recent-context narrative, taken at fork time. A session never sees observations merged after it forked.
- Its source-specific input as the user message: the task prompt (spawned), the pulse or action prompt (scheduled), the webhook payload (webhook), the inbound message — images included — plus any buffered chatter since the conversation's last mention (conversation — see [Conversation Routing](#conversation-routing)), or the artifact's prompt, preceded by a line naming the workbench artifact that started the session and saying its responses go to that artifact, plus any context the artifact sent (artifact — see [Artifact Sessions](#artifact-sessions)). A session resumed by a message to a completed address (see [Messaging](#messaging)) gets that message instead, plus a pointer back to its previous run's episode; any images on that resuming message carry over the same way.
- The requested skill activated, when one was given. A resumed session keeps the skill its previous run used.
- The requested model tier.

**What's excluded:**
- The main agent's live, unobserved conversation. A session never sees what the user and main agent are currently discussing — the spawning agent writes a task prompt with whatever context the session needs.

**Tools excluded from sessions:** `switch_endpoint` — it only makes sense for the main agent's own output routing. Everything else, including `subagent_spawn` (subject to the depth cap — see [Nesting](#nesting)), the action-scheduling tools, and `message_agent`, is available to a session too.

**Sessions can talk to other endpoints, but not the owner directly.** A session's `send_message` refuses the WebSocket endpoint and the owner's DM on every chat interface — whether named explicitly as `conversation` or reached through the no-conversation default — with an error telling it to message `main` instead. Posting to any other conversation or notification endpoint still works; see [notifications.md](notifications.md).

Sessions share the MCP registry with the main agent.

For shell commands and scripts, the agent uses its own `write_file` and `exec` tools directly — there is no separate "script task" type.

## Categories

Every session has a category, derived from what started it:

| Category | Started by | Address prefix |
|----------|-----------|-----------------|
| `scheduled` | Pulses and scheduled actions | `scheduled-` |
| `external` | Webhooks, and non-owner-DM conversations on Discord/Telegram/Teams (see [Conversation Routing](#conversation-routing)) | `external-` |
| `spawned` | `subagent_spawn`, the subconscious `learner` | `spawned-` |
| `artifact` | A workbench artifact, through `residuum.sessions.start` (see [Artifact Sessions](#artifact-sessions)) | `artifact-` |

## Lifecycle

A session run moves through: `forking` → `queued` → `running` → `idle` → (`queued` → `running` → `idle` again, for each later turn) → `completing` → `completed`.

- **forking** — the session is registered and its fork resources (identity, memory snapshot, tools) are being built; no turn has started.
- **queued** — the run is ready for its next turn (its first, right after forking, or a later one, right after idle) but is waiting on a `max_concurrent` concurrency permit held by other live sessions.
- **running** — a turn is executing. The run holds one concurrency permit.
- **idle** — the turn ended; the session is still discoverable via `list_agents` and lingers for its category's idle timeout. A message delivered to it (see [Messaging](#messaging)) starts another turn in the same run instead of waiting out the timeout, so a run can span several turns.
- **completing** — the idle timeout elapsed, or the session was stopped via `stop_agent`. A completing run no longer accepts messages into itself; one addressed to it is queued for the resume that follows once it clears (see [Messaging](#messaging)).
- **completed** — the run's final transcript and metadata are recorded in the session store, and the result is delivered. The session is no longer listed by `list_agents`, though its address stays meaningful: a message to it starts a new run at the same address (see [Messaging](#messaging)).

Stopping a session (`stop_agent`) cancels its stop token: a running turn stops immediately — the model call aborts, the tool call in progress is interrupted, and every other tool call in that batch is skipped — with its transcript up to that point intact, rather than being dropped; an idle session skips straight to completing. See [Stopping a Turn](turn-control.md) for exactly what a stop interrupts and how it's recorded. Either way the run is reported as cancelled; a run that simply idles out keeps its last turn's outcome.

A session's turn loop is unlimited by default, same as the main agent's — see [Tool-Call Limit](turn-control.md#tool-call-limit) for the optional `max_tool_iterations` config and what happens when a turn hits it.

### Idle Timeouts

Configurable in the `[background]` config section:

| Category | Config key | Default |
|----------|-----------|---------|
| `scheduled` (also used by `external` webhook sessions — a webhook call is one-shot) | `idle_timeout_scheduled_minutes` | 2 minutes |
| `spawned` | `idle_timeout_spawned_minutes` | 10 minutes |
| `external` (non-webhook) | `idle_timeout_external_minutes` | 30 minutes |
| `artifact` | `idle_timeout_artifact_minutes` | 10 minutes |

Each timeout is also editable in the web UI under Settings → Runtime → Pulse & Background.

## Addresses

Every `scheduled`, `spawned`, `artifact`, or webhook `external` session has a stable, human-readable address, e.g. `spawned-researcher-3f9a`: the category, a slugified qualifier (skill, pulse, action, webhook, or artifact name), and a short random suffix. Addresses never contain `:`. `subagent_spawn` generates the address synchronously and returns it immediately, before the session has actually started running.

A conversation's `external` session instead gets a **deterministic** address, derived from its interface endpoint and its stable conversation id (e.g. `external-discord-3f9a2c1b0d4e5f6a`), so every message in that conversation resolves to the same session whether or not a run is currently live there. The conversation id is hashed rather than embedded — some interfaces' ids (Teams, in particular) carry characters that aren't safe in a URL path segment or a filename.

A run id, distinct from the address, identifies the specific run within the session's lifecycle.

## Messaging

Agents message each other by address with the `message_agent` tool, available to the main agent and every session. Delivery depends on the target's current lifecycle state:

- **`main`** — delivered as an interrupt at the next tool-call boundary if a main turn is running, otherwise it starts a main turn.
- **running session** — delivered as an interrupt at the session's next tool-call boundary, through the same interrupt channel `stop_agent` uses to end a turn. If that channel is saturated (vanishingly unlikely — 32 deep, drained continuously by a live run), the tool returns an error telling the sender to retry shortly, rather than silently falling back to a resume that would double-register the address.
- **idle session** — starts another turn in the same run, with the message as that turn's input. The run's transcript and per-turn memory staging (see [Memory](#memory)) span every turn this way, not just the first.
- **completing session** — the run is tearing down (its completion pipeline — memory merge, transcript write — may still be running) and no longer accepts input into itself. Delivery does not block on that pipeline: the tool call returns immediately reporting the message is queued, while a background task waits for the run to fully leave the registry (recording its resume point on the way out) and then re-checks the address before acting: if another queued message already resumed it in the meantime, this one is delivered straight into that live run instead; otherwise it resumes the session as a new run, the same as a completed session below. This is what lets two messages queued to the same completing session both reach it, as exactly one new run rather than a second one silently losing the race. A message still queued in a run's own interrupt channel at the moment its teardown drains it (e.g. one delivered just as a stop lands) is handled the same way, combined into the resumed run's opening prompt if more than one arrived.
- **completed session** — the session is resumed as a new run at the same address, forked the same way any other session is, carrying the previous run's model tier, spawner, and depth. The new run's context carries a pointer back to the previous run's episode id, or its run id if that run produced no episode, retrievable with `memory_get`. The sender's tool result says the session had completed and was resumed. This works the same way across a process restart: resume points are persisted (see [Session Store](#session-store)), so an address whose session completed before the restart still resumes with its episode pointer intact rather than starting over.
- **unknown address** — an address that has never run reports an error naming `list_agents` as the way to find live sessions.

An `artifact` session cannot message `main`: its `message_agent` call to `main` fails with a tool error saying artifact sessions can't reach the main conversation and to file an inbox item (`user_inbox_add`) instead. It can message any other address as usual.

Every delivered message names the sender's address and category, so the recipient knows who to reply to. If delivery requires publishing an event (a resume, or handoff to main) and that publish fails, the tool returns an error rather than reporting success — the sender should not assume the message arrived.

### Hop Counts

Every agent message carries a hop count, used to bound message loops. Input that originates outside the agent system — a user message, a pulse or action firing, a webhook, a web sidebar message, a workbench artifact's start or message — is hop `0`. A message an agent sends during a turn carries one more than the highest hop count among the inputs that drove that turn: the turn's kickoff input, plus any agent messages drained as interrupts during it. A `subagent_spawn` task brief carries the same rule — one more than the spawning turn's highest input hop count — so the new session's first turn starts at that hop count; a resumed session's new run instead starts at the hop count of the message that triggered the resume (that message *is* its first turn's input). Result relays (see [Result Routing](#result-routing)) count as agent messages for this purpose. The main agent tracks its own current-turn hop count the same way a session does, including across a turn boundary: if a message arrives mid-turn but isn't consumed before the turn ends, its hop count carries forward into whichever turn picks it up next rather than being reset — otherwise a looping message that happened to arrive at the wrong moment could reset the loop guard to zero.

Two limits, both configurable in `[background]`:

| Limit | Config key | Default | Effect |
|-------|-----------|---------|--------|
| Soft | `hop_soft_limit` | 8 | The delivered message carries a note asking the receiver to reply only if a reply is actually needed. |
| Hard | `hop_hard_limit` | 32 | Delivery is refused outright. The sender's tool call returns an error explaining the loop limit and naming the `hop_hard_limit` setting; the refusal is logged at `warn` with both addresses and the hop count; a best-effort note is recorded in the transcript of whichever side (sender, receiver) is a live, addressable session, and shown as an error on that session in the web UI. |

A hard-limit refusal never reaches the target — the tool result is the only thing the sender sees.

## Conversation Routing

The main agent handles only the owner's own direct messages and the web UI. Every other conversation that Discord, Telegram, or Teams admits — a group chat, a channel, or a non-owner's DM — is routed to that conversation's own `external` session instead, so the owner's private context is never shared with whoever else talks to the bot. This holds even when the owner is the one speaking in a shared conversation: a group chat or channel always gets a session, never main. Admission (owner claim, `respond_to_others`, standing) is unchanged and happens first, per interface; routing only decides who handles a message the interface has already admitted.

Delivery into a conversation's session follows the same lifecycle rules as [Messaging](#messaging) — interrupt if running, new turn if idle, deferred resume if completing — with two differences: a conversation message is always hop `0` (it's external input, like a user message, not an agent-to-agent message), and an address with **no prior run at all** starts a brand-new session instead of reporting an unknown address, since a conversation's first-ever message is exactly when that happens.

The session sees the inbound message with the same sender attribution the main agent shows (`[From: name via interface (location)]`), plus any chatter buffered since the conversation's last mention, exactly as described in each interface's own systems-usage page. Its source label follows `<endpoint>:<location>`, e.g. `discord:#builds (Eng Team)`. Unlike a busy `message_agent` send — which errors immediately, telling the calling agent to retry, since the agent on the other end can act on that — a conversation message that arrives while its target session's interrupt channel is saturated (vanishingly unlikely — 32 deep, drained continuously by a live run) is never dropped: there's nobody on the other end of a chat message to hand a refusal to, so delivery retries on a detached task until the channel drains, however long that takes, logging a `warn` only if it's taking an unusually long time.

**Typing indicator.** Discord, Telegram, and Teams show a typing indicator in a conversation while its own session's turn is running, the same as they do for the main agent's own turns, but driven by a separate signal keyed by conversation id rather than the main turn's own lifecycle events — a conversation session's turn isn't a reply to any one message, so it has no correlation id to key off. See each interface's own systems-usage page.

**Output never falls back to the owner's DM.** A conversation session's turn output — both its final response and any intermediate (pre-tool-call) text along the way, the same as main posts to its own conversation mid-turn — is delivered straight to its own conversation. If the interface can't resolve or deliver to that conversation (the bot was removed, the channel was deleted), the output is dropped, an `error` is logged naming the session and conversation, and a failure notice reaches `main` as an agent message — main decides whether the owner needs to hear about it. This is deliberately different from the main agent's own proactive-output fallback (see each interface's "Where replies go"), which still falls back to the owner's DM: only main talks to the owner, so a session's output has nowhere else meaningful to fall back to.

**A2A conversations.** Each caller of the A2A listener (see [a2a.md](a2a.md)) gets its own conversation session, addressed by `{caller}/{context_id}` under the `a2a` endpoint, delivered through the same conversation-session machinery described above rather than through the endpoint registry's `send_message`/`list_endpoints` path — the `a2a` endpoint carries no `INTERACTIVE` capability and is never listed there. There's no admission gate the way Discord/Telegram/Teams have (owner claim, `respond_to_others`, standing): the A2A auth layer's caller-key or sibling check is the only gate, and every authenticated caller reaches its own session. A conversation session started from the `a2a` endpoint is the only kind that gets the `a2a_task_update` tool, which reports the A2A task's outcome back through the listener rather than through this page's own output-delivery path.

## Nesting

Sessions can spawn sessions with their own `subagent_spawn` tool. Depth counts from the main agent: main is depth 0, every `scheduled`/`external`/`artifact` session is depth 1, and a `spawned` session is its spawner's depth plus 1 — whatever the spawner's own category. The spawned session's spawner is recorded as the calling agent's address (`main`, or the calling session's own address). A session resumed via `message_agent` keeps its original spawner and depth rather than resetting to a fresh depth-1 session.

Depth is capped by `subagent_depth_cap` in `[background]` (default 3). Spawning a session that would exceed the cap is refused with an error explaining the limit and naming the `subagent_depth_cap` setting; the calling agent should either handle the task directly, ask a shallower agent to spawn it, or raise the setting.

## Tools

### `message_agent`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `to` | string | yes | `"main"`, a session address from `list_agents`, or `"a2a:<name>"` for a remote agent listed in `config/a2a.json`. |
| `message` | string | yes | The message body. Must not be empty. |
| `skill` | string | no | Only meaningful when `to` is `"a2a:<name>"`: the id of one of that agent's advertised skills, sent as `message.metadata.skill`. |

Sends `message` to `to`, delivered per the rules in [Messaging](#messaging). Messaging yourself is rejected, and so is an `artifact` session messaging `main`. See [a2a.md](a2a.md#client-reaching-other-agents) for `to: "a2a:<name>"`: delivery isn't synchronous — the call returns once the remote agent has accepted the task, and its reply arrives later as an agent message from `a2a:<name>`.

### `subagent_spawn`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `task` | string | yes | The prompt/instructions for the session. Must not be empty. |
| `skill` | string | no | Name of a skill to activate as the session's role. Omit to run on the task prompt alone. `"main"` is rejected. |
| `model` | string enum | no | `"small"`, `"medium"`, `"large"`. Default: `"medium"`. |

Available to the main agent and to every session, subject to the depth cap above. Returns the session's address immediately. A session's final result is a **self-report** — it describes what the session believes it did, not a verified outcome. When the task involves something checkable (a file written, a command run, a deployment, an external change), the spawning agent should ask for concrete handles in the task prompt (file paths, commit SHAs, URLs, ticket IDs) and treat the result as unverified until those handles check out.

### `list_agents`

No parameters. Lists the main agent plus every live (running or idle) session: address, category, source, state, depth, spawner, elapsed time, and purpose. Also lists every remote agent configured in `config/a2a.json`: its address (`a2a:<name>`), online/pending/error status, description and skills once its card resolves, and the caller's own open tasks with it.

### `stop_agent`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `address` | string | yes | Stops the session at this address, or `"a2a:<name>"` to cancel the caller's open task with that remote agent. |

The owner can also stop a session, or message it, from the web UI (see [Web UI](#web-ui)). The main agent cannot be stopped this way. To stop the main-agent turn itself — the conversation the user is having — see [Turn Control](turn-control.md) instead; that's a user-facing interface control, not a tool.

## Model Tiers

| Tier | Default Use | Fallback Chain |
|------|-------------|----------------|
| Small | Heartbeat pulses, lightweight checks | Medium → Large → Main |
| Medium | Default for `subagent_spawn` and scheduled actions | Large → Main |
| Large | Complex analysis, multi-step reasoning | Main |

Model tiers are configured in `[background]` config section (`models.small`, `models.medium`, `models.large`).

## Session Roles

The only thing that distinguishes one spawned session from another is what the caller passes at fork time: a prompt, a model tier, and optionally a **skill** whose body becomes the session's role instructions.

There is no separate preset format. A role is an ordinary skill in `skills/<name>/SKILL.md`, so the same file can be activated in-turn by the main agent or handed to a session as its brief.

```yaml
---
name: memory-analyst
description: Answers synthesized questions about the user and past history from episodic memory.
---

(Body — the instructions this session runs with.)
```

Spawn configuration lives at the call site, not in the file:

| Caller | Where the config lives |
|--------|------------------------|
| `subagent_spawn` | The tool's `skill` and `model` parameters. |
| Heartbeat pulses | `agent` and `model_tier` on the pulse in `HEARTBEAT.yml`. `agent: "main"` and `include_identity` are removed — a pulse using either fails to load with an error naming it. |
| Scheduled actions | `agent` and `model_tier` on the action. `agent_name: "main"` is rejected by `schedule_action` and dropped (loudly) from any stored action that predates this change. |
| Subconscious `learner` | Fixed in code: the `learner` skill, `large` tier. |

A spawn naming a skill that does not resolve fails loudly rather than running a session without the instructions that define its job.

### Bundled Role Skills

| Skill | Spawned by |
|-------|------------|
| `introspection` | The built-in `reflection` pulse, at `large`. |
| `wiki` | The built-in `memory_tending` and `wiki_lint` pulses, at `large`. `memory_tending` ingests episodes since the last `ingest` entry in `wiki/log.md` into wiki pages and `USER.md`; `wiki_lint` fixes index drift, missing frontmatter, stale pages, old drafts, duplicates, contradictions, and missing links. |
| `learner` | A subconscious `learn` signal (subject to `learning_cooldown_minutes`), or the `[learning] nudge_after_turns` fallback. Corroborates the signal against episodic memory and, for `preference` signals, files it as a wiki page — `status: draft` for a single episode, promoted to `stable` once a second independent episode supports it — and adds only corroborated core facts to `USER.md`. For `recovery` signals, it prefers queuing a durable fix via the user inbox over encoding the workaround into a skill — a skill is only warranted when the obstacle is an external constraint that can't be fixed. Reports via at most one user-inbox item. See [subconscious.md](subconscious.md#learning-trigger). |
| `memory-analyst` | The main agent, when it needs a synthesized answer about the user or past history rather than raw search results. Uses multiple search phrasings for enumeration questions, surfaces contradictions with dates instead of silently picking one, abstains rather than fabricating when the record is silent, and cites episode IDs. |

## Concurrency

The session runtime uses a semaphore bounded by `max_concurrent` in the `[background]` config section, shared by every category, `artifact` sessions included. The permit is held only while a turn is actually running — an idle session holds nothing, so lingering sessions cost memory, not throughput. Runs that can't get a permit wait for one.

## Result Routing

A `spawned` session's turn result is relayed to its **direct spawner** — main, or whichever session spawned it — through the same agent-messaging path as `message_agent`, hop counts included. This happens after every turn in the run, not just once at completion, and a nested session relays to its own spawner rather than to main. Every outcome is relayed, not just a completed turn with output: a completed turn with no text response, a failed turn, a cancelled/stopped turn, and a turn whose task panicked (reported as failed) all relay a clear status line naming the session and what happened, so the spawner is never left simply not knowing. A relay failure (the spawner is busy, or unreachable — e.g. it restarted and its live run is gone, even though its resume point persists across the restart for the next message to it) is never silent: it's logged, recorded as a note in the session's own transcript, and shown as an error on the session in the web UI (see [Web UI](#web-ui)).

`scheduled` results, and `external` results from a webhook trigger, flow through the pub/sub bus to the notification router, which delivers them per the disposition the producing agent declared (inbox, or inbox plus urgent fanout). `spawned` results do not pass through that router at all — the per-turn relay above replaces it. `artifact` results go nowhere on their own: they are never relayed to main, and the router discards them whatever their disposition, so they reach neither the inbox nor notification channels. The artifact that started the session reads its output from the session frames and the transcript endpoint (see [Artifact Sessions](#artifact-sessions)). An `external` result from a conversation trigger (A2A, or a non-owner Discord/Telegram/Teams chat) is discarded the same way: its output already reached the conversation it came from (see [Conversation Routing](#conversation-routing)), and its observations are merged into memory as an episode, so the router files it nowhere.

See [notifications.md](notifications.md) for the full routing model.

## Memory

A session has its own working memory and merges what it learned into global memory when it completes — see [memory.md](memory.md#agent-sessions-and-memory) for the full model. In short: after every turn in the run, its accumulated messages are checked against the same observer thresholds the main agent uses, and crossing the force threshold stages observations locally — a multi-turn run (see [Messaging](#messaging)) stages after each of its turns, not just once. On completion the run produces an episode (tagged with its session address, run id, and category) unless it staged nothing and either its final turn ended with `HEARTBEAT_OK` or its transcript is under the configurable `episode_skip_token_floor`. Its transcript is kept in the session store either way. The run's metadata records the episode id once merged.

## Session Store

Every run's metadata is recorded under `memory/sessions/YYYY-MM/DD/<run-id>.json`, created on demand. While the run is live, its transcript is durably appended to a sibling `<run-id>.transcript.jsonl` file after every model response and tool result, so a crash mid-turn loses at most the message in flight; on completion the full transcript is folded into the metadata file too, so a finished run's record is a single, self-contained file. Stopped runs keep their transcript up to the point they were stopped and merge into memory like any other run.

At startup, any run left incomplete by a prior process exit goes through the full completion pipeline — skip check, final observation, merge — from its persisted transcript before normal operation resumes, then is marked completed.

The record carries the session's address, run id, category, source label, spawner, depth, purpose, lifecycle timestamps, the episode id once merged, and the full message transcript once the run completes.

Each session's resume point — the previous run id, its episode pointer (if any), the trigger, source label, skill, model tier, spawner, and depth needed to fork the address again — is recorded when a run starts and again, with its episode, when it completes. Each recording is persisted write-through to `memory/sessions/resume_points.json`, and the file is loaded back when the session registry starts. Recording at start means a run cut short by a crash still leaves the next message a pointer back to it (by run id, since its episode is merged during startup recovery). This is what lets [Messaging](#messaging)'s completed-session resume, and the same rule in [Conversation Routing](#conversation-routing), keep working across a process restart rather than only within one run of the process. Entries older than 90 days are pruned on load. A load or parse failure is logged at `warn` and never blocks startup — the registry simply starts with no resume points, the same as if the file were empty.

## Artifact Sessions

A workbench artifact (see [workbench.md](workbench.md)) runs agent work as a session of its own, through `residuum.sessions.start`, which calls `POST /api/sessions`. The session is an ordinary fork, with the same fork contents and tool registry a `spawned` session gets, but:

- **Category and origin.** Its trigger is the artifact, its category `artifact`, its source label `artifact:<name>`, and its address `artifact-<name>-<suffix>`. It has no spawner and runs at depth 1. A message to its address after it completes resumes it as an `artifact` session again.
- **Results stay with the artifact.** Its turn output is never relayed to the main agent, and its completion is not routed to the inbox or notification channels. The artifact follows it through the `session_*` frames for its address and the transcript endpoint. It is listed in the sessions sidebar like any other session.
- **No line to main.** Its `message_agent` to `main` fails with a tool error telling it to file an inbox item instead. Its `user_inbox_add` works, for when its task calls for putting something in front of the user. Sessions it spawns relay their results to it, as spawned sessions always relay to their spawner, never to main.
- **Idle timeout** `idle_timeout_artifact_minutes` (default 10), and it shares the `max_concurrent` limit with every other session.

**`POST /api/sessions`** takes `{ prompt, context?, skill?, model? }`, where `model` is `small`, `medium` (default), or `large`. It requires the `X-Residuum-Artifact` header, which the workbench bridge stamps on every request it relays; without it (or with a value that isn't an artifact name) it answers `400`. A blank prompt, an unknown field, an unknown model tier, or a skill that doesn't exist is also a `400` with `{ error }`. It answers `202` with `{ address }` once the start is published; the run itself starts asynchronously, and its run id arrives in the `session_started` frame for that address. A start that can't be published (Residuum is shutting down) is a `503`. Every start is logged at `info` with the artifact, address, skill, and model tier.

## Web UI

The web UI follows sessions live over its existing WebSocket and reads their history over HTTP. Protocol types are generated for the web client into `web/src/lib/generated/` (`cargo test --test ts_export` regenerates them); `SessionSummary`, `SessionListResponse`, and the enums below are exported alongside `ServerMessage`/`ClientMessage`.

### Session summary

Both the listing and the `session_started` frame describe a run as a `SessionSummary`: `address`, `run_id`, `category` (`scheduled` | `external` | `spawned` | `artifact`), `source_label`, `state` (`forking` | `queued` | `running` | `idle` | `completing` | `completed`), `spawner` (address or `null`), `depth`, `purpose`, `started_at` and `completed_at` (RFC 3339 UTC; `completed_at` is `null` until the run completes), `episode_id` (`null` unless the run was merged into an episode), `interrupted` (`true` when startup recovery completed the run after a process exit), and `usage` (`SessionUsageTotals`: `input_tokens`, `output_tokens`, `context_tokens` — this run's cumulative token usage, for the session view's footer; see [Turn Control](turn-control.md#running-turn-indicator-and-usage-totals)). It also carries `outcome` (`completed` | `cancelled` | `failed`, `null` while still live) and `error` (the failure reason when `outcome` is `failed`, else `null`) — persisted with the run's record, so a completed run shows its real outcome even after a reload or once it's paged into the sidebar's older-runs list, not just "finished". `overlap` is set on a `scheduled` pulse run that started while its previous run was still live (see [heartbeats.md](heartbeats.md#scheduling-behavior)): `previous_run_id` and `previous_started_at`.

### Live events (server → client)

Every session publishes the same turn events the main agent does, as their own `session_*` frames tagged with `address` and `run_id`. The main agent's frames (`turn_started`, `response`, `error`, and so on) keep their shape and never carry session activity, so a client that ignores `session_*` frames is unaffected.

| Frame | Fields | When |
|-------|--------|------|
| `session_started` | `session: SessionSummary` | A run was registered (state `forking`) — a fresh fork or a resume. |
| `session_state_changed` | `address`, `run_id`, `state` | The run moved to `running`, `idle`, or `completing`. |
| `session_completed` | `address`, `run_id`, `status` (`completed` \| `cancelled` \| `failed`), `error` (the failure when `failed`, else `null`), `episode_id` | The run was recorded in the store and is no longer live. There is no separate `completed` state change. |
| `session_turn_started` / `session_turn_ended` | `address`, `run_id`, `turn_id` | Brackets each turn. `turn_ended` is sent whatever the outcome. `turn_id` is `<run_id>-t<n>`, numbering the run's turns from 1. |
| `session_turn_usage` | `address`, `run_id`, `output_tokens`, `has_usage`, `session_totals` (`SessionUsageTotals`, once known) | Published after every model call, for the session view's running-turn indicator and footer — never surfaced to the agent. See [Turn Control](turn-control.md#running-turn-indicator-and-usage-totals). |
| `session_tool_call` | `address`, `run_id`, `id`, `name`, `arguments` | Verbose only. |
| `session_tool_result` | `address`, `run_id`, `tool_call_id`, `name`, `output`, `is_error` | Verbose only. |
| `session_broadcast_response` | `address`, `run_id`, `content` | Intermediate text the session emitted alongside tool calls. For a conversation session, the same text is also delivered to its own conversation (see [Conversation Routing](#conversation-routing)). |
| `session_response` | `address`, `run_id`, `turn_id`, `content` | A turn's final text response (not sent for a turn with no text, or one that was stopped). |
| `session_error` | `address`, `run_id`, `message` | A failed turn, a message refused at the hop limit (on both the sender's and the receiver's stream, when each is a live session), a result relay that couldn't be delivered, a deferred delivery that failed, or a panicked session task. |
| `session_message_to_main` | `address`, `run_id`, `content` | The session's message reached the main agent: a turn-result relay to its spawner `main`, or a `message_agent` call to `main`. `content` is the body main received, without the sender header. |

Tool call and result frames follow the connection's verbose setting (`set_verbose`), the same as the main agent's. A turn's frames arrive in order: `session_state_changed` (`running`), `session_turn_started`, any intermediate/tool frames, `session_response` or `session_error`, `session_turn_ended`, then `session_state_changed` (`idle`) — and at the end of the run, `session_state_changed` (`completing`) followed by `session_completed`.

### Control (client → server)

| Message | Fields | Reply (to this connection only) |
|---------|--------|---------------------------------|
| `session_send_message` | `id`, `address`, `content` | `session_message_delivered` (`id`, `address`, `outcome`: `live` \| `resumed` \| `queued`) or `session_command_failed`. |
| `session_stop` | `id`, `address` | `session_stop_requested` (`id`, `address`) or `session_command_failed`. The run's `completing` state change and `session_completed` follow. |

`session_command_failed` carries `id`, `address`, `code`, and a plain-language `message`. Codes: `invalid_request` (empty message, or `main` as the target — main is messaged from the main chat), `unknown_address` (no session has ever run there in this process), `busy` (the session is live but its message queue is full — retry shortly), `not_live` (nothing to stop: the session is already finishing or has completed), `delivery_failed` (the message couldn't be handed over).

A sidebar message is delivered like any agent message, at hop count 0: an interrupt at the next tool-call boundary while the session is running, a new turn while it's idle, a new run at the same address once it has completed (`resumed`), or a new run once a finishing run has cleared (`queued`). The session sees it labelled as the owner's message rather than an agent's, sent from the reserved sender address `owner`; the owner reads the session's reply in the sidebar, so the session answers in its response rather than with `message_agent`.

### HTTP endpoints

**`GET /api/sessions`** returns a `SessionListResponse`:

- `live` — every live session (forking, queued, running, idle, or completing) from the registry, newest first. Always complete; not paginated.
- `completed` — one page of completed runs from the session store, newest first (by start time, then run id).
- `next_cursor` — an opaque string to pass back as `before` for the next page, or `null` on the last page.

Query parameters: `category` (`scheduled` | `external` | `spawned` | `artifact`; filters both lists), `address` (only runs of the session at that address; filters both lists), `artifact` (only sessions that workbench artifact started — not sessions those spawned in turn; filters both lists; an invalid artifact name is a `400`), `limit` (completed runs per page, 1–200, default 50), `before` (a `next_cursor` from a previous response). An unknown category, an out-of-range limit, or a malformed cursor is a `400`; pass back only a `next_cursor` the server returned. A run that has just been recorded but hasn't yet left the registry is listed only under `live`.

**`POST /api/sessions`** starts an `artifact` session; see [Artifact Sessions](#artifact-sessions).

**`POST /api/sessions/{address}/stop`** stops any live session, with the `session_stop` command's rules: `202` with `{ address }` when stopping, `404` with code `not_live` when there is nothing to stop, `400` with code `invalid_request` for `main`.

**`POST /api/sessions/{address}/messages`** with `{ content }` sends any session a message, with the `session_send_message` command's delivery rules. It answers `200` with `{ outcome: "live" | "queued" | "resumed" }`. A failure answers `{ error, code }` with the command's code and a status per code: `invalid_request` `400` (a malformed body, blank content, `main`, or a malformed artifact identity header), `unknown_address` `404`, `busy` `409`, `delivery_failed` `502`. With the `X-Residuum-Artifact` header, the session sees the message as that artifact's (`[Message from the workbench artifact "<name>" …]`, sender `artifact:<name>`); without it, as the owner's.

**`GET /api/sessions/runs/{run_id}/transcript`** returns `{ session: SessionSummary, messages: RecentMessage[] }`. `messages` has the same shape `GET /api/chat/history` returns, so the chat's message components render it. A live run's transcript is read from its incremental transcript file, current to the last message produced, and `session` reflects its live state; a completed run's comes from its final record. Runs don't record per-message times, so every message carries the run's start time (in the configured timezone, like chat history). A run id containing anything but ASCII letters, digits, `-`, and `_` is a `400`; an unknown run is a `404`.

### Sessions sidebar

The web UI shows sessions in a sidebar to the left of the chat, opened and closed from the sessions button in the header (which also shows how many sessions are live, in its badge and its accessible name). On wide screens the sidebar is a column whose open or closed state is remembered in the browser; below 900px wide it is a modal drawer over the page: keyboard focus stays inside it, the page behind is inert, and it closes on Escape, on the backdrop, or when a session is picked, returning focus to the sessions button.

- **Category groups.** The sidebar is split into four collapsible groups, External, Scheduled, Spawned, and Artifacts, always in that order. Each group starts expanded, its heading shows how many of its sessions are live, and hovering over it shows what the category means. A group with nothing running says so and names what shows up there.
- **Live sessions** are listed newest first within their group, each with its category badge (`scheduled`, `external`, `spawned`, `artifact`), source label, purpose, state (`starting`, `queued`, `working`, `idle`, `finishing`), and how long it has been running. A conversation session (an `external` session handling a chat conversation) shows its interface and conversation as the source label, e.g. `discord:#builds`. An `artifact` session shows the name of the artifact that started it in place of its source label, as a link that opens the artifact. A light down the row's left edge shows a session that is working, forking, or queued. A session that reported an error shows the start of that error in the row until it finishes. Every row in a stoppable state (forking, queued, running, idle) carries its own stop button (`session_stop`), in every category, so a session can be stopped straight from the list without opening it.
- **Finished** is a collapsed section inside each group listing that category's completed runs newest first, 25 at a time, with a "Show older" button that follows that category's `next_cursor`. Each group pages on its own: the listing is one `GET /api/sessions?category=…` request per category, and together their `live` lists cover every live session. A reload keeps runs already paged in beyond each group's first page, matched by run id in the server's order. Every finished run shows whether it was stopped or failed — from a live outcome frame for a run that finished while the page was open, or from the listing's own persisted `outcome`/`error` fields (see [Session summary](#session-summary)) for an older run paged in later; either way it reads "finished" only when it actually completed normally (or "interrupted" when startup recovery closed it).
- The listing loads when the WebSocket connects and reloads after every reconnect. `session_*` frames keep it current: `session_started` adds a run, `session_state_changed` updates it, and `session_completed` moves it to its group's Finished list. A frame for a run the page doesn't know about (one that started while disconnected) triggers a reload of the listing.

Selecting a session replaces the main chat with the **session view**; "Main chat" returns to the chat, which stays loaded underneath. The view shows the session's address, category (with a line saying what that category means), source label, state, duration, purpose, who started it (a spawner session's address links to it; an `artifact` session names its artifact as a link that opens it), nesting depth when deeper than 1, the episode it was merged into, and its run id. Below that is the run's transcript from the transcript endpoint, rendered with the chat's message components, with live frames appended as they arrive: intermediate text, responses, errors, and tool calls when verbose mode is on, plus the running-turn indicator while a turn is in progress. The view follows new output while scrolled to the bottom; scrolled up, it stays put and offers "Jump to latest". Messages from other agents appear as compact items naming the sender; the owner's own messages appear as the owner's chat bubbles, an artifact's messages appear as chat bubbles under the artifact's name, and people in a conversation session appear under their names. A message that merely starts with an agent header is shown as whoever typed it. Below the transcript, a quiet footer shows the run's cumulative token usage and context size (see [Turn Control](turn-control.md#running-turn-indicator-and-usage-totals)) — no model segment, since a session run has a model tier rather than one resolved model string.

A live session has a **Stop session** button (`session_stop`), and every session has a message box (`session_send_message`). The reply shows in the view as a one-line note: "Delivered." for a live session; for a finished one, a note that the message started a new run, after which the view continues into that new run under a "new run" divider (or, if the old transcript hadn't finished loading, loads the new run's transcript instead). A queued delivery says the new run starts once the finishing run clears. A `session_command_failed` reply shows its plain-language message.

### Session messages in the main chat

Messages a session sends the main agent (its relayed results, and any `message_agent` call to `main`) appear in the main chat as compact items with the session's category and address, a preview that expands with "Show all", and an **Open session** link. Live, they come from `session_message_to_main` frames. In history, they are recognized by the `agent_sender` field (`{ address, category }`) the backend records on every message one agent sends another; the `[Agent Message from <address> (<category>)]` header in the text is honoured only on history written without that field, and then only on background-visibility messages, so an owner typing the header gets their own message. An agent message that started a background turn is shown along with main's reply to it (which was shown live when it happened), including when the turn spans an episode boundary, while other background turns stay hidden. After the WebSocket reconnects, the main chat reloads its recent history and adds whatever arrived while it was disconnected, including day dividers. A turn still running at the reconnect keeps its live output (history records a turn only when it ends); one that ended meanwhile is replaced by its recorded form and the chat stops showing it as in progress. If the history can't be lined up with what's shown (the observer compressed it into a new episode meanwhile), the chat reloads, keeping a running turn's live output and, for a reader scrolled up, the message they were reading in place. An item from history knows only the address, so **Open session** opens that session's live run, or its most recent completed run (looked up with `?address=` when it isn't already loaded).
