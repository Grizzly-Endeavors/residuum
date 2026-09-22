# Background Tasks

Background tasks let the agent run work without blocking the main conversation. The execution model is **agent sessions** — temporary forks of the main agent that run independently and deliver results through notification channels.

## Sessions

A session is a fork of the main agent with its own identity, memory snapshot, and tool registry, running off the main thread.

**What's included in a session's fork:**
- The main agent's full identity and system prompt content — `SOUL.md`, `AGENTS.md`, `HARNESS`, `USER.md`, the wiki root index, the skills index — assembled once in the system message, exactly as it is for the main agent.
- A snapshot of the global observation log and the recent-context narrative, taken at fork time. A session never sees observations merged after it forked.
- Its source-specific input as the user message: the task prompt (spawned), the pulse or action prompt (scheduled), the webhook payload (webhook), or the inbound message plus any buffered chatter since the conversation's last mention (conversation — see [Conversation Routing](#conversation-routing)). A session resumed by a message to a completed address (see [Messaging](#messaging)) gets that message instead, plus a pointer back to its previous run's episode.
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

## Lifecycle

A session run moves through: `forking` → `running` → `idle` → `completing` → `completed`.

- **forking** — the session is registered and its fork resources (identity, memory snapshot, tools) are being built; no turn has started.
- **running** — a turn is executing. The run holds one concurrency permit.
- **idle** — the turn ended; the session is still discoverable via `list_agents` and lingers for its category's idle timeout. A message delivered to it (see [Messaging](#messaging)) starts another turn in the same run instead of waiting out the timeout, so a run can span several turns.
- **completing** — the idle timeout elapsed, or the session was stopped via `stop_agent`. A completing run no longer accepts messages into itself; one addressed to it is queued for the resume that follows once it clears (see [Messaging](#messaging)).
- **completed** — the run's final transcript and metadata are recorded in the session store, and the result is delivered. The session is no longer listed by `list_agents`, though its address stays meaningful: a message to it starts a new run at the same address (see [Messaging](#messaging)).

Stopping a session (`stop_agent`) cancels its stop token: a running turn ends at its next checkpoint (a model-call or tool-loop boundary) with its transcript up to that point intact, rather than being dropped; an idle session skips straight to completing.

### Idle Timeouts

Configurable in the `[background]` config section:

| Category | Config key | Default |
|----------|-----------|---------|
| `scheduled` (also used by `external` webhook sessions — a webhook call is one-shot) | `idle_timeout_scheduled_minutes` | 2 minutes |
| `spawned` | `idle_timeout_spawned_minutes` | 10 minutes |
| `external` (non-webhook) | `idle_timeout_external_minutes` | 30 minutes |

## Addresses

Every `scheduled`, `spawned`, or webhook `external` session has a stable, human-readable address, e.g. `spawned-researcher-3f9a`: the category, a slugified qualifier (skill, pulse, action, or webhook name), and a short random suffix. `subagent_spawn` generates the address synchronously and returns it immediately, before the session has actually started running.

A conversation's `external` session instead gets a **deterministic** address, derived from its interface endpoint and its stable conversation id (e.g. `external-discord-3f9a2c1b0d4e5f6a`), so every message in that conversation resolves to the same session whether or not a run is currently live there. The conversation id is hashed rather than embedded — some interfaces' ids (Teams, in particular) carry characters that aren't safe in a URL path segment or a filename.

A run id, distinct from the address, identifies the specific run within the session's lifecycle.

## Messaging

Agents message each other by address with the `message_agent` tool, available to the main agent and every session. Delivery depends on the target's current lifecycle state:

- **`main`** — delivered as an interrupt at the next tool-call boundary if a main turn is running, otherwise it starts a main turn.
- **running session** — delivered as an interrupt at the session's next tool-call boundary, through the same interrupt channel `stop_agent` uses to end a turn. If that channel is saturated (vanishingly unlikely — 32 deep, drained continuously by a live run), the tool returns an error telling the sender to retry shortly, rather than silently falling back to a resume that would double-register the address.
- **idle session** — starts another turn in the same run, with the message as that turn's input. The run's transcript and per-turn memory staging (see [Memory](#memory)) span every turn this way, not just the first.
- **completing session** — the run is tearing down (its completion pipeline — memory merge, transcript write — may still be running) and no longer accepts input into itself. Delivery does not block on that pipeline: the tool call returns immediately reporting the message is queued, while a background task waits for the run to fully leave the registry (recording its resume point on the way out) and then re-checks the address before acting: if another queued message already resumed it in the meantime, this one is delivered straight into that live run instead; otherwise it resumes the session as a new run, the same as a completed session below. This is what lets two messages queued to the same completing session both reach it, as exactly one new run rather than a second one silently losing the race. A message still queued in a run's own interrupt channel at the moment its teardown drains it (e.g. one delivered just as a stop lands) is handled the same way, combined into the resumed run's opening prompt if more than one arrived.
- **completed session** — the session is resumed as a new run at the same address, forked the same way any other session is, carrying the previous run's model tier, spawner, and depth. The new run's context carries a pointer back to the previous run's episode id, or its run id if that run produced no episode, retrievable with `memory_get`. The sender's tool result says the session had completed and was resumed.
- **unknown address** — an address that has never run reports an error naming `list_agents` as the way to find live sessions.

Every delivered message names the sender's address and category, so the recipient knows who to reply to. If delivery requires publishing an event (a resume, or handoff to main) and that publish fails, the tool returns an error rather than reporting success — the sender should not assume the message arrived.

### Hop Counts

Every agent message carries a hop count, used to bound message loops. Input that originates outside the agent system — a user message, a pulse or action firing, a webhook, a web sidebar message — is hop `0`. A message an agent sends during a turn carries one more than the highest hop count among the inputs that drove that turn: the turn's kickoff input, plus any agent messages drained as interrupts during it. A `subagent_spawn` task brief carries the same rule — one more than the spawning turn's highest input hop count — so the new session's first turn starts at that hop count; a resumed session's new run instead starts at the hop count of the message that triggered the resume (that message *is* its first turn's input). Result relays (see [Result Routing](#result-routing)) count as agent messages for this purpose. The main agent tracks its own current-turn hop count the same way a session does, including across a turn boundary: if a message arrives mid-turn but isn't consumed before the turn ends, its hop count carries forward into whichever turn picks it up next rather than being reset — otherwise a looping message that happened to arrive at the wrong moment could reset the loop guard to zero.

Two limits, both configurable in `[background]`:

| Limit | Config key | Default | Effect |
|-------|-----------|---------|--------|
| Soft | `hop_soft_limit` | 8 | The delivered message carries a note asking the receiver to reply only if a reply is actually needed. |
| Hard | `hop_hard_limit` | 32 | Delivery is refused outright. The sender's tool call returns an error explaining the loop limit; the refusal is logged at `warn` with both addresses and the hop count; a best-effort note is recorded in the transcript of whichever side (sender, receiver) is a live, addressable session. |

A hard-limit refusal never reaches the target — the tool result is the only thing the sender sees.

## Conversation Routing

The main agent handles only the owner's own direct messages and the web UI. Every other conversation that Discord, Telegram, or Teams admits — a group chat, a channel, or a non-owner's DM — is routed to that conversation's own `external` session instead, so the owner's private context is never shared with whoever else talks to the bot. This holds even when the owner is the one speaking in a shared conversation: a group chat or channel always gets a session, never main. Admission (owner claim, `respond_to_others`, standing) is unchanged and happens first, per interface; routing only decides who handles a message the interface has already admitted.

Delivery into a conversation's session follows the same lifecycle rules as [Messaging](#messaging) — interrupt if running, new turn if idle, deferred resume if completing — with two differences: a conversation message is always hop `0` (it's external input, like a user message, not an agent-to-agent message), and an address with **no prior run at all** starts a brand-new session instead of reporting an unknown address, since a conversation's first-ever message is exactly when that happens.

The session sees the inbound message with the same sender attribution the main agent shows (`[From: name via interface (location)]`), plus any chatter buffered since the conversation's last mention, exactly as described in each interface's own systems-usage page. Its source label follows `<endpoint>:<location>`, e.g. `discord:#builds (Eng Team)`.

**Output never falls back to the owner's DM.** A conversation session's turn output is delivered straight to its own conversation. If the interface can't resolve or deliver to that conversation (the bot was removed, the channel was deleted), the output is dropped, an `error` is logged naming the session and conversation, and a failure notice reaches `main` as an agent message — main decides whether the owner needs to hear about it. This is deliberately different from the main agent's own proactive-output fallback (see each interface's "Where replies go"), which still falls back to the owner's DM: only main talks to the owner, so a session's output has nowhere else meaningful to fall back to.

## Nesting

Sessions can spawn sessions with their own `subagent_spawn` tool. Depth counts from the main agent: main is depth 0, every `scheduled`/`external` session is depth 1, and a `spawned` session is its spawner's depth plus 1 — whatever the spawner's own category. The spawned session's spawner is recorded as the calling agent's address (`main`, or the calling session's own address). A session resumed via `message_agent` keeps its original spawner and depth rather than resetting to a fresh depth-1 session.

Depth is capped by `subagent_depth_cap` in `[background]` (default 2). Spawning a session that would exceed the cap is refused with an error explaining the limit; the calling agent should either handle the task directly or ask a shallower agent to spawn it.

## Tools

### `message_agent`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `to` | string | yes | `"main"`, or a session address from `list_agents`. |
| `message` | string | yes | The message body. Must not be empty. |

Sends `message` to `to`, delivered per the rules in [Messaging](#messaging). Messaging yourself is rejected.

### `subagent_spawn`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `task` | string | yes | The prompt/instructions for the session. Must not be empty. |
| `skill` | string | no | Name of a skill to activate as the session's role. Omit to run on the task prompt alone. `"main"` is rejected. |
| `model` | string enum | no | `"small"`, `"medium"`, `"large"`. Default: `"medium"`. |

Available to the main agent and to every session, subject to the depth cap above. Returns the session's address immediately. A session's final result is a **self-report** — it describes what the session believes it did, not a verified outcome. When the task involves something checkable (a file written, a command run, a deployment, an external change), the spawning agent should ask for concrete handles in the task prompt (file paths, commit SHAs, URLs, ticket IDs) and treat the result as unverified until those handles check out.

### `list_agents`

No parameters. Lists the main agent plus every live (running or idle) session: address, category, source, state, depth, spawner, elapsed time, and purpose.

### `stop_agent`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `address` | string | yes | Stops the session at this address. |

The main agent cannot be stopped this way. To stop the main-agent turn itself — the conversation the user is having — see [Turn Control](turn-control.md) instead; that's a user-facing interface control, not a tool.

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

The session runtime uses a semaphore bounded by `max_concurrent` in the `[background]` config section. The permit is held only while a turn is actually running — an idle session holds nothing, so lingering sessions cost memory, not throughput. Runs that can't get a permit wait for one.

## Result Routing

A `spawned` session's turn result is relayed to its **direct spawner** — main, or whichever session spawned it — through the same agent-messaging path as `message_agent`, hop counts included. This happens after every turn in the run, not just once at completion, and a nested session relays to its own spawner rather than to main. Every outcome is relayed, not just a completed turn with output: a completed turn with no text response, a failed turn, a cancelled/stopped turn, and a turn whose task panicked (reported as failed) all relay a clear status line naming the session and what happened, so the spawner is never left simply not knowing. A relay failure (the spawner is busy, or unreachable — e.g. it restarted and lost its resume point) is never silent: it's logged and recorded as a note in the session's own transcript.

`scheduled` and `external` results still flow through the pub/sub bus to the notification router, which delivers them per the disposition the producing agent declared (inbox, or inbox plus urgent fanout). `spawned` results no longer pass through that router at all — the per-turn relay above replaces it.

See [notifications.md](notifications.md) for the full routing model.

## Memory

A session has its own working memory and merges what it learned into global memory when it completes — see [memory.md](memory.md#agent-sessions-and-memory) for the full model. In short: after every turn in the run, its accumulated messages are checked against the same observer thresholds the main agent uses, and crossing the force threshold stages observations locally — a multi-turn run (see [Messaging](#messaging)) stages after each of its turns, not just once. On completion the run produces an episode (tagged with its session address, run id, and category) unless it staged nothing and either its final turn ended with `HEARTBEAT_OK` or its transcript is under the configurable `episode_skip_token_floor`. Its transcript is kept in the session store either way. The run's metadata records the episode id once merged.

## Session Store

Every run's metadata is recorded under `memory/sessions/YYYY-MM/DD/<run-id>.json`, created on demand. While the run is live, its transcript is durably appended to a sibling `<run-id>.transcript.jsonl` file after every model response and tool result, so a crash mid-turn loses at most the message in flight; on completion the full transcript is folded into the metadata file too, so a finished run's record is a single, self-contained file. Stopped runs keep their transcript up to the point they were stopped and merge into memory like any other run.

At startup, any run left incomplete by a prior process exit goes through the full completion pipeline — skip check, final observation, merge — from its persisted transcript before normal operation resumes, then is marked completed.

The record carries the session's address, run id, category, source label, spawner, depth, purpose, lifecycle timestamps, the episode id once merged, and the full message transcript once the run completes.
