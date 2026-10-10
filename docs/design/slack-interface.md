# Slack Interface (Socket Mode, DM-only MVP)

Status: **Draft — open for review** · Refs #490 · Approach discussion: #490 (comment)

A design for a Slack chat interface: residuum agents answering direct messages in Slack, over
Socket Mode, with no new dependencies. Written before any code exists; every decision below is
open to revision until the implementation plan is cut.

## Summary

Each agent that configures a `[slack]` section gains a Slack bot presence: the operator's direct
messages to that bot are delivered into the agent's **main conversation** — the same single
conversation the web UI and every other chat interface write into — and the agent's replies are
posted back to the originating DM. Slack becomes another door into the agent, not a separate
chat history.

The MVP is deliberately narrow: direct messages between the workspace member and the agent's
bot, text only. Channels, mentions, threads, media, reactions, and interactive surfaces are
out of scope for v1 and sketched under v2.

## Decisions

Each with its rationale and rejected alternatives, so future readers inherit the reasoning, not
just the choice.

**D1 — DM-only for v1.** One conversation surface, no mention parsing, no channel-join
bookkeeping. Channels and threads arrive in v2, where threads bring structural value (work in a
thread, one notification in the channel). *Rejected:* DM + channel mentions in v1 — doubles the
event-handling surface before the basic lane is proven.

**D2 — Owner-only by default (`respond_to_others = false`).** A new interface starts closed and
opens per deployment by config, matching the posture of the other chat adapters. The flag is
per-agent, so some agents can be workspace-open while others stay owner-only.

**D3 — Hand-rolled on in-tree primitives.** The Socket Mode client is built directly on
`tokio-tungstenite` + `reqwest` (both already in the dependency tree): `apps.connections.open`
→ websocket → JSON envelopes → ack via the envelope's `response_url` → `chat.postMessage` for
outbound. *Rejected:* a full Slack framework crate (`slack-morphism`-class) — it drags a second
HTTP/TLS stack alongside the pinned one, repeating exactly the transitive-TLS advisory class
`deny.toml` already documents and tolerates line-by-line, and the DM-only protocol subset is
small enough that pre-built event types buy little. *Rejected:* a lightweight middle crate — the
Rust ecosystem has no credible one for this subset. **Revisit trigger:** if v2 grows the event
surface to where pre-built types outweigh dependency weight, a crate can be adopted then
without rework of the transport layer — the transport is feature-neutral (every Slack feature
rides the same socket and envelope protocol).

**D4 — Unknown event types are logged and skipped, never an error** — and still acknowledged.
Slack-side additions must never break the adapter, and an unacked unknown event would be
redelivered forever.

**D5 — Delivery into the main conversation.** Inbound Slack messages enter the agent's main
conversation through the same machinery the other interfaces use. The adapter keeps no
conversation state of its own; the only Slack-specific bookkeeping is reply routing (which door
a response exits through).

**D6 — Conversation-only outbound contract from day one.** The subscriber attaches to the
post-`22c0773e` outbound contract shared by the other external chat interfaces: agent replies
and failed-turn report-backs reach Slack; system notices and errors stay in the web UI;
undeliverable addressed messages become a web-UI notice plus a note to main. Never a silent
drop.

## Architecture

New module `src/interfaces/slack/`, five files, one job each:

| File | Job |
|---|---|
| `socket.rs` | Socket Mode client: connect, read envelopes, ack, reconnect with backoff. Knows nothing about agents or conversations. |
| `events.rs` | Serde types for what the adapter consumes (envelopes, DM `message` events). The tolerant-unknowns rule (D4) lives here. |
| `api.rs` | Thin Web API wrapper: `chat.postMessage` behind a trait, plus the minimum-interval rate-limit sender. Knows nothing about sockets or conversations. |
| `handler.rs` | Inbound brain: DM check, owner check, hand the text to the agent-turn machinery. |
| `subscriber.rs` | Outbound: subscribes to conversation-only bus events, routes replies through shared chunking, sends via `api.rs`. |

Outside the module: the config surface (`SlackConfig`/`SlackConfigFile` following the
`DiscordConfig`/`TelegramConfig` precedent, including redacted `Debug`), the agent-config
registration points, the hot-reload comparison path, and a checked-in Slack app manifest
template plus a setup guide (below).

The shape mirrors the existing adapters: **one interface instance per agent**, each with its own
tokens and its own socket connection — the same one-bot-per-agent pattern the Telegram interface
uses. Zero new dependencies.

## Inbound flow

1. **Connect.** Agent start (or config reload) with a valid `[slack]` section calls
   `apps.connections.open` with the app token, receives a websocket URL, connects, and waits
   for the `hello` envelope. Connection state changes are logged.
2. **Envelope arrives.** Every event rides an envelope carrying `envelope_id`, `type`, and a
   retry counter.
3. **Ack before processing.** Slack expects an ack (HTTP POST to the envelope's `response_url`)
   within seconds or it redelivers. The adapter acks immediately on receipt, before any
   processing. A small dedup cache of recent `envelope_id`s absorbs redeliveries from reconnects
   or ack failures — two guards against one failure mode (a message processed twice).
4. **Filter chain, cheapest first:** envelope type is a consumed kind → event is a plain
   `message` in a DM (`channel_type: "im"`) → no filtered subtype (edits, deletions, bot
   messages are skipped in v1) → not the bot's own message echoing back → **owner check**
   (`respond_to_others = false` passes only the owner's user ID). Everything else: logged and
   skipped, never an error.
5. **Deliver to the main conversation.** The message text is handed to the same inbound path
   the web UI and other interfaces use. The adapter records only the reply target (the DM
   channel) for the response. The owner-identification mechanism reuses the existing
   owner-identity pattern of the other adapters rather than inventing a Slack-specific one;
   the exact alignment is verified while writing the implementation plan.

Edge cases fenced: edited messages do not re-trigger the agent (v1); thread replies inside a DM
arrive as plain messages with no thread semantics (v1); unknown events log-and-skip (D4).

## Outbound flow

1. **Subscribe to conversation-only events.** The subscriber attaches exactly as the other
   external chat interfaces do post-`22c0773e`: agent replies and failed-turn report-backs,
   nothing else.
2. **Reply routing.** A reply event carries the door the triggering message came in through.
   A Slack-triggered turn's reply posts to the originating DM; a turn triggered elsewhere does
   not surface in Slack. A failed turn is reported back through the door that started it.
   Agent-initiated pushes ride the hub's existing relay policy; the adapter provides the door,
   the hub decides when to use it.
3. **Chunking.** Slack caps a message at roughly 4,000 characters; the shared chunking
   machinery the other interfaces use gets a Slack-sized setting, splitting at paragraph
   boundaries and never mid-code-block.
4. **Rate limiting.** A minimum-interval sender serializes posts with a small enforced gap. A
   429 response honors the stated `Retry-After` and backs off the send queue, with the event
   logged. Handling 429 is correctness, not polish.
5. **Formatting.** Agent output is markdown; Slack renders mrkdwn. A minimal deterministic
   converter maps the constructs agents actually emit — bold, italic, inline and fenced code,
   headers as bold lines, tables as code blocks — with everything else passed through as text.
   Unit-tested with fixture pairs. Explicitly rejected: a general markdown engine.
6. **Delivery failures.** A send that cannot complete (revoked token, unreachable API, gone
   channel) logs at error level, retries per the adapter's standard retry policy, and — per
   D6 — becomes a web-UI notice plus a note to main if ultimately undeliverable. Never a
   silent drop.

## Configuration and app setup

Per-agent section:

```toml
[slack]
bot_token = "xoxb-…"        # the agent's Slack bot identity; ${ENV_VAR} and secret: refs supported
app_token = "xapp-…"        # the app's Socket Mode connection
respond_to_others = false   # owner-only by default
```

Section absent or tokens missing → the interface is disabled, matching the other adapters'
pattern. Both tokens are redacted in debug output. No `context_messages` in v1 — that is
group-chatter machinery and v1 has no groups.

**App manifest template.** The repository ships a one-paste JSON manifest that creates a
residuum-ready Slack app: Socket Mode enabled, the DM message event subscription, and the
minimal scope set (`connections:write`, `chat:write`, `im:history` — the final list is verified
against Slack's current documentation at implementation time). Creating an app per agent
requires no familiarity with Slack's admin UI.

**Setup guide** (`docs/guides/`): manifest → install to workspace → harvest the two tokens →
config lines → restart, written for a first-time operator. Where tokens *live* follows each
deployment's existing secret-handling conventions; the adapter reads config, not policy.

## Lifecycle and failure handling

1. **Startup.** Unreachable Slack at boot does not block the agent: bounded backoff retries via
   the existing boot-retry pattern; every other lane comes up regardless.
2. **Hot reload.** Config reload drains and closes the old socket and boots the new one without
   an agent restart. Token rotation is a config edit, not an outage window.
3. **Reconnect.** Slack cycles socket connections routinely. Graceful `disconnect` envelopes
   are honored immediately; TCP drops are detected and reconnected through a fresh
   `apps.connections.open` with capped exponential backoff. State changes are logged without
   spamming during normal cycling.
4. **Ack failures.** A failed ack POST logs a warning; Slack's redelivery plus the dedup cache
   turns the transient failure into a no-op.
5. **Shutdown.** Clean socket close; in-flight outbound drains, and anything undeliverable
   follows the D6 contract. No zombie connections.
6. **Sustained rate limiting.** Sustained 429s surface as errors rather than silent throttling.

## Testing

1. **Envelope/event layer:** serde round-trips for consumed shapes; unknown types deserialize
   to a tolerant variant and never error. Fixtures are checked-in JSON written from Slack's
   published docs and marked *assumed-until-probed*; they are replaced with real captured
   payloads at first live deployment.
2. **Formatting converter:** fixture-pair tests for the mapping table, including pass-through
   oddities.
3. **Filter chain:** pure-function tests — DM gate, owner gate, subtype skips, self-message
   guard, dedup-cache behavior (one envelope delivered twice → one processed message).
4. **Conversation-only contract, executable:** the same test family as the existing
   post-`22c0773e` tests — system notices must never produce a Slack send; replies and
   failed-turn report-backs must.
5. **Rate limiter:** minimum-interval enforcement and 429 `Retry-After` honored, against a
   hand-rolled fake behind the API trait.
6. **Transport wiring:** a loopback fake-Slack websocket server (existing in-repo websocket
   server machinery) plays the protocol — connect, `hello`, envelope, expect ack, message
   event, expect outbound post. No network dependency.
7. **CI never touches real Slack.** Live verification is a documented manual acceptance step
   in the setup guide, performed at first deployment.

## Contracts appendix (assumed-until-probed)

Shapes below are written from Slack's published documentation and are **assumed until probed**
— replaced by captured payloads at first deployment. Full JSON fixtures live beside the tests.

- **Socket Mode envelope:** `{envelope_id, type: "events_api"|"hello"|"disconnect"|…, payload,
  retry_attempt, retry_context, accepts_unknown_payload, response_url}`; ack is a POST to
  `response_url`.
- **DM message event (Events API, subscribed):** `{type: "message", channel_type: "im",
  channel, user, text, ts, event_ts}` — subtypes (`message_changed`, `message_deleted`,
  `bot_message`, …) are present but skipped in v1.
- **`chat.postMessage` request/response:** `{channel, text, thread_ts?}` →
  `{ok, channel, ts}` or `{ok: false, error}` with `Retry-After` headers on 429.
- **`apps.connections.open`:** `{ok, url}` with the app token as bearer.

## Acceptance criteria (from #490)

- An agent with a `[slack]` section answers a workspace member's DM; the reply renders as the
  agent's bot user.
- Messages from non-owner members are filtered per `respond_to_others`.
- No system notices or errors reach Slack (web UI only) — same guarantee as the other
  external chat interfaces.
- Hub restart reconnects Socket Mode cleanly.
- `assets/config.example.toml` documents the `[slack]` section; the manifest template and
  setup guide ship with the feature.
- The conversation-only contract and filter behavior are covered by tests.

## Open items

- Verify the final scope list against Slack's current API documentation at implementation time.
- Confirm the exact owner-identification alignment with the existing adapters while writing the
  implementation plan.

## v2 boundary (explicitly deferred)

Channels, mentions, threads (with their notification-containment value), media and file
exchange, reactions, typing/presence, interactive surfaces (Block Kit), slash commands. The v2
revisit points recorded above: framework-crate adoption if the event surface grows, and
`context_messages` when groups exist.
