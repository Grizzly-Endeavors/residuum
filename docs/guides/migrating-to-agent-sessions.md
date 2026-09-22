# Migrating to Agent Sessions

Residuum's background-task system was replaced with **agent sessions**: every pulse, scheduled action, webhook, and spawned sub-agent now runs as a temporary fork of the main agent with its own address, memory, and lifecycle, instead of a one-shot, fire-and-forget task. Group chats and other people's DMs on Discord, Telegram, and Teams also moved off the main conversation onto their own sessions. This was a single cutover, not a gradual rollout, so if you're coming from an older version there are a few things to fix by hand and a few behavior changes worth knowing about. Nothing below requires reinstalling or resetting your workspace.

## `agent: "main"` no longer works in `HEARTBEAT.yml`

If a pulse in your `HEARTBEAT.yml` has `agent: "main"`, it will stop firing: the pulse is skipped and an error naming it is written to the log (`residuum logs --level error`) rather than being silently reinterpreted. Every other pulse in the file keeps running as normal — only the offending one is affected.

**Why it was removed:** every session now already carries the main agent's full identity (`SOUL.md`, `AGENTS.md`, `USER.md`) and a snapshot of memory when it forks, which is all `agent: "main"` used to add. There's no longer a distinct "run this on main" mode to ask for.

**What to do:** open `HEARTBEAT.yml` and either delete the `agent: "main"` line (the pulse then runs as a plain session at the small model tier, same as leaving `agent` unset) or point it at a skill name to give the session a role. No other change is needed — the pulse's schedule, prompt, and everything else carry over untouched.

## `include_identity` is gone from pulses

Same story, same fix: a pulse setting `include_identity` (`true` or `false`) is skipped and logged as an error, because every session fork already carries the identity files unconditionally now — there's nothing left for the flag to turn on or off. Delete the `include_identity` line from any pulse that has it.

## `agent_name: "main"` no longer works for scheduled actions

Calling `schedule_action` with `agent_name: "main"` (in any capitalization) is now rejected outright, with an error telling you to omit `agent_name` or name a skill instead. If you have an existing one-off action stored from before this change, it's dropped when Residuum starts up — logged as an error naming the action — rather than quietly running it as something it isn't. If you still need that action, recreate it with `schedule_action`, either without `agent_name` or with a skill name.

## Group chats and other people's DMs no longer reach your agent's own conversation

Previously, every inbound message on Discord, Telegram, and Teams — including messages from a group chat or someone else's DM — was handed to the single main agent, which meant your own private context could leak into a reply to someone else, and their conversation could bleed into yours.

Now, only your own direct message (and the web UI) reaches the main agent. Every other conversation the interface admits — a server channel, a group chat, a channel, or a DM from someone other than you (only reachable at all if you've turned on `respond_to_others`) — gets its own session instead: a separate fork with its own memory and its own idle timeout, addressed to that specific conversation. This holds even in a shared channel where you're the one talking — it's still a shared space, so it's still a session, not your main agent.

**What this means day to day:**
- Your own DM conversation is unaffected — nothing changes there.
- A reply in a group chat or channel now comes from that conversation's own session, not from your main agent. It replies only into that same conversation and never falls back to your DM.
- The session sees the same sender attribution and any buffered unaddressed chatter your interface already collects (see `context_messages` below) — it isn't starting blind.
- What a session learns in a group chat still reaches your agent's overall memory once the session finishes (see "Session memory" below), just not immediately and not as part of your live conversation.
- If you want to know what happened in a channel, you can check the web UI's session sidebar, or ask your main agent to look it up in memory once it's had a chance to merge.

No configuration is needed to get this — it's automatic. `respond_to_others` still controls whether non-owner DMs are admitted at all; it just no longer controls whether they land on your main agent.

## `subagent_spawn` returns an address, not a result

`subagent_spawn` used to run and hand back a result once the sub-agent finished. It now returns the new session's **address** immediately (e.g. `spawned-researcher-3f9a`) and the session keeps running in the background. Its result — after every turn, not just once at the end — is delivered back to you as an agent message, and you pass it along to whoever's waiting. If you (or a skill) were relying on `subagent_spawn`'s own output being the final answer, that assumption no longer holds — wait for the relayed message instead. `list_agents` shows what's still running, and `message_agent` lets you send a live session a follow-up or check in on one that's already finished.

## Sessions can't message you directly

Only the main agent can post to your DM or the web UI. If a session (a pulse, an action, a spawned sub-agent, or a conversation session) tries to use `send_message` to reach you directly, the call is refused with an error telling it to message `main` instead, which then decides what — if anything — you need to hear. This doesn't change what you see day to day (main still relays things that matter), but if you had a skill or a pulse prompt that instructed a session to `send_message` straight to your DM or the web endpoint, that instruction will now fail; have it report through the normal result relay (pulses/actions) or `message_agent` (spawned sessions) instead. Posting to any *other* conversation or notification endpoint is unaffected.

## New `[background]` configuration keys

A handful of new, optional settings landed in the `[background]` section of `config.toml`. None of them need to be set — every one has a default that matches what shipped — but they're worth knowing about if you want to tune session behavior:

| Key | Default | What it controls |
|-----|---------|-------------------|
| `idle_timeout_scheduled_minutes` | `2` | How long a pulse or action session lingers idle before it completes. Webhook sessions use this one too. |
| `idle_timeout_spawned_minutes` | `10` | How long a `subagent_spawn`/learner session lingers idle before it completes. |
| `idle_timeout_external_minutes` | `30` | How long a non-webhook conversation session (group chat, channel, non-owner DM) lingers idle before it completes. |
| `episode_skip_token_floor` | `2000` | Below this many tokens, a run that staged nothing produces no memory episode (its transcript is still kept). |
| `subagent_depth_cap` | `2` | How deep a chain of sessions spawning sessions can nest before `subagent_spawn` is refused. |
| `hop_soft_limit` | `8` | Agent-to-agent message hop count at which a delivered message starts carrying a "reply only if needed" note. |
| `hop_hard_limit` | `32` | Agent-to-agent message hop count at which delivery is refused outright, to bound message loops. |

See [`background-tasks.md`](../systems-usage/background-tasks.md) for the full model these settings tune.

## `context_messages` per chat interface

Discord, Telegram, and Teams each gained a `context_messages` setting (default `20`, `0` disables it) controlling how many unaddressed messages get buffered per conversation and handed over the next time the bot is addressed there. This is what lets a group-chat session answer with awareness of chatter it wasn't directly asked about. It's set per interface in `config.toml` (`[discord] context_messages`, `[telegram] context_messages`, `[teams] context_messages`) and defaults to on — no action needed unless you want to change the size or turn it off.

**Telegram-specific:** by default, Telegram's own privacy mode means the bot only ever receives messages that explicitly address it, so there's nothing for the buffer to collect in a group. To have it see and buffer the rest of a group's conversation, turn off privacy mode for your bot in [@BotFather](https://t.me/BotFather) (`/mybots` → your bot → *Bot Settings* → *Group Privacy* → *Turn off*), or make the bot a group admin. This is a Telegram platform setting, not something Residuum can turn on from its side.

## Old background-task logs are no longer written

Background task transcripts used to be written under `memory/background/`. That directory is no longer written to — new session transcripts and metadata live under `memory/sessions/` instead, created the first time a session actually runs. If you have an existing `memory/background/` directory from before this change, it's left exactly where it is; nothing reads it anymore, and nothing deletes it. It's safe to archive or remove by hand if you don't need the old transcripts, or leave it in place — Residuum won't touch it either way.

## Session memory merges into the observation log

Previously, what a background task learned stayed in its own transcript and never reached your agent's searchable memory — a gap tracked as issue #70. Now, every session (pulse, action, webhook, or spawned sub-agent) is checked against the same memory thresholds your main agent uses, and what it learns is merged into the same global observation log and episode store once the session finishes. That means `memory_search` and `memory_get` can now surface things a background session found, tagged with which session and run produced them. A session that reports nothing new (ends with `HEARTBEAT_OK`, or is too short to matter) still keeps its full transcript in the session store — it just doesn't add a memory episode for it.

## Where to go from here

- [`background-tasks.md`](../systems-usage/background-tasks.md) is the full reference for sessions, addresses, messaging, and the web sessions sidebar.
- [`heartbeats.md`](../systems-usage/heartbeats.md) and [`scheduled-actions.md`](../systems-usage/scheduled-actions.md) cover the updated pulse and action routing in detail.
- [`discord.md`](../systems-usage/discord.md), [`telegram.md`](../systems-usage/telegram.md), and [`teams.md`](../systems-usage/teams.md) cover conversation routing and unaddressed-message buffering per interface.
