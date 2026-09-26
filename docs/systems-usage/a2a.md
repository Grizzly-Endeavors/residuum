# Agent2Agent (A2A)

A2A ([Agent2Agent protocol](https://a2a-protocol.org/), spec v1.0.1) is how other agents — including a user's own other Residuum instances — reach this agent. Residuum runs a dedicated listener implementing the protocol's server side: an Agent Card, JSON-RPC and REST bindings, an auth layer that gates every request behind a caller key or a sibling attestation, and a session executor that runs each task as an ordinary conversation session.

Every A2A task maps to a conversation session, addressed by `{caller}/{context_id}` under the `a2a` endpoint — the same session lifecycle (live, idle, completed, resumed) described in [background-tasks.md](background-tasks.md) applies, with the task's A2A status kept in sync with it. Only the owner ever reaches main; every A2A caller lands in its own session.

## Configuration

```toml
[a2a]
enabled = true
port = 7702
public_url = ""          # this instance's own tunnel/reverse proxy origin
visibility = "public"    # "public" or "private"
```

- **`enabled`** (default `true`): whether the listener runs at all. Every request is still authenticated, so leaving it on costs nothing until a caller key or sibling instance exists to use it.
- **`port`** (default `7702`): the dedicated listener's port, bound on `[gateway] bind`. Kept separate from the gateway port for the same reason Teams is: a public tunnel pointed at it exposes only the A2A endpoints, never the unauthenticated config API.
- **`public_url`**: the base URL other agents should use to reach this instance, when it runs its own tunnel or reverse proxy. See [Public URL](#public-url) for how it's resolved when left empty.
- **`visibility`**: `"public"` (default) or `"private"`. See [Visibility](#visibility).

Changing any `[a2a]` value, or the gateway `bind` it shares, restarts the A2A listener on reload — the config API's reload path, `residuum a2a` commands, and manual edits to `config.toml` (picked up by the running gateway) all take effect the same way. An `[a2a]` change also restarts the relay tunnel, since its capabilities (whether `a2a`/`a2a-private` are advertised) are only sent on the tunnel's upgrade.

## Public URL

The Agent Card's interface URLs, and the value the web UI shows for "how to reach this agent," come from one precedence, evaluated live:

1. `[a2a] public_url`, when set — an explicit setting always wins.
2. While the relay tunnel is connected and has announced both an origin and this instance's slug, `{origin}/a2a/{instance}`.
3. Otherwise, a local fallback: `http://{bind}:{port}` — good for same-host and same-network callers, not for callers over the public internet.

The card is rebuilt whenever the tunnel's connection status changes, so it picks up the relay's origin as soon as the tunnel connects (or falls back again if it drops), without needing a restart or a workspace-file edit.

## Caller keys

Other agents authenticate with a bearer token minted for them:

```bash
residuum a2a keys create laptop -d "my other instance, before siblings exist"
# A2A caller key 'laptop' created.
#
#   rsdm_a2a_Ax7f...   (32 base62 characters)
#
# Store this now — it is shown only once and is not recoverable. ...
residuum a2a keys list
residuum a2a keys revoke laptop
```

The caller gives the token as `Authorization: Bearer <token>`.

**Storage.** `~/.residuum/a2a-keys.toml`, mode 0600 on Unix, holds each key's name, description, a `sha256:<hex>` hash of the token, and its creation time — never the token itself. The store is unencrypted because there is nothing in it worth encrypting at rest: the hash's only job is to reject a stolen credential's replay, which it already does. `~/.residuum/a2a-keys.lock` serializes writes from the CLI, the web UI, and the running listener so none of them lose a concurrent change. Both files are write-blocked for `write_file` and `edit_file`, like the other credential stores.

**Web UI:** `GET /api/a2a/keys` (metadata only), `POST /api/a2a/keys` with `{ "name", "description" }` (returns the token once, in the response body — never again), `DELETE /api/a2a/keys/{name}` returning `{ "revoked": true, "checkpoint_id" }`. `checkpoint_id` is the config checkpoint taken just before the revoke, or null when that checkpoint could not be recorded. Like the rest of the config API, this is unauthenticated and meant to stay on loopback. Settings → A2A in the web UI wraps these three (see [Web UI](#web-ui) below).

A key name is lowercase letters, digits, and underscores, starting with a letter, at most 64 characters.

## The Agent Card

`{workspace}/config/agent-card.json` is what this agent advertises to other agents that reach it over A2A:

```json
{
  "name": "Residuum agent",
  "description": "A personal AI agent, reachable over the Agent2Agent (A2A) protocol.",
  "skills": []
}
```

- **`name`**, **`description`**: required, non-empty.
- **`skills`**: an array of `{ "id", "name", "description", "tags": [], "examples"?: [] }`. Every `id` must be unique. An empty list is valid. A skill `id` that also names a workspace skill (`skills/<name>/SKILL.md`) is a skill a caller can ask for — see [Skill mapping](#skill-mapping).
- **`default_input_modes`**, **`default_output_modes`**: optional; default to `["text/plain"]` when omitted.

Everything else in the wire Agent Card — the JSON-RPC and REST interface URLs, capabilities, version, and security scheme — is filled in by the server from the file plus runtime facts (the configured base URL and visibility), never edited in the file directly.

The file is bootstrapped on first run with a generic placeholder and an empty skill list (`write_if_missing`, so a user edit is never overwritten). Residuum watches it alongside `mcp.json` and `channels.toml`; a change is picked up within a few seconds without restarting the listener. If the file becomes invalid JSON or fails validation, the listener keeps serving the last good card, logs a warning, and posts a system notice — it never starts serving a broken or stale-to-empty card.

## Endpoints

The listener serves, on `[gateway] bind`:`[a2a] port`:

| Path | What |
|------|------|
| `GET /.well-known/agent-card.json` | The Agent Card |
| `GET /_a2a/auth-check` | `204` if the request's credentials are currently valid, `404` otherwise — used by directory probes and health checks, nothing else |
| `POST /` | JSON-RPC binding (`message/send`, `tasks/get`, …) |
| `POST /rest/...` | HTTP+JSON (REST) binding, mirroring the same operations |

The Agent Card's `supportedInterfaces` names the JSON-RPC interface at the base URL itself and the REST interface at `{base}/rest`.

## Auth layer

Every request passes through an axum middleware before it reaches anything else:

1. Any client-supplied `x-residuum-a2a-caller`, `x-residuum-tunnel`, or `x-residuum-sibling` header is stripped before it is ever inspected.
2. If `x-residuum-tunnel` matches this process's own tunnel nonce **and** `x-residuum-sibling` names a slug, the caller is `sibling:<slug>` — an attestation only this instance's own tunnel forwarder can produce, never something a client can present directly.
3. Otherwise, an `Authorization: Bearer <token>` that matches a live caller key authenticates as `key:<name>`.
4. A caller resolved either way gets `x-residuum-a2a-caller: <key:name|sibling:slug>` injected for the handler to read.

An unauthenticated request is refused according to [visibility](#visibility). `GET /_a2a/auth-check` is the one path that never refuses outright: it reports `204` (authenticated) or `404` (not) in both visibility modes, so a directory probe or health check can ask "is this credential currently valid" without needing a real operation to fail against.

## Visibility

- **`public`** (default): the Agent Card is open to everyone; every other route still requires a valid caller key or sibling attestation, and an unauthenticated request there gets `401` with `WWW-Authenticate: Bearer`.
- **`private`**: every route, including the Agent Card, answers a plain `404` with no `WWW-Authenticate` header to a caller without a valid key or attestation — a private agent is indistinguishable from one that doesn't exist. Present a valid key and everything (card included) answers normally.

## Tasks

Every A2A task is backed by a session. The conversation id is `{caller}/{context_id}` (`caller` is `key:<name>` or `sibling:<slug>`, `context_id` is the A2A spec's grouping id), which maps to the address `conversation_session_address("a2a", id)` — the same deterministic-address scheme every other conversation interface uses (see [Addresses](background-tasks.md#addresses)). Two different callers, or two different contexts from the same caller, never share a session.

**Lifecycle mapping:**

- The task starts `SUBMITTED`, then moves to `WORKING` once the session's turn starts running.
- Each turn's final text becomes a `WORKING` status update carrying that text as the agent's message — a caller watching the task sees progress as the session works, not just silence until it's done.
- A session tool, `a2a_task_update`, sets the task's outcome explicitly: `COMPLETED`, `INPUT_REQUIRED`, or `FAILED`, each with a message and optional artifacts. See [The `a2a_task_update` tool](#the-a2a_task_update-tool).
- If the session's run ends with no explicit signal: a cancelled run is `CANCELED`, a failed run is `FAILED`, and a completed run is `COMPLETED` with its last final text — **unless** the session has a live session it spawned (via `subagent_spawn`), in which case the task stays `WORKING`. That spawned session's own result relay resumes the parent session when it finishes, and the resumed run reappears at the same task — the task only reaches a terminal state once nothing is still working on it.
- A follow-up message to an `INPUT_REQUIRED` task continues the same session. If the session has since completed (gone idle past its timeout and wound down), the existing session-resume path starts a new run there, carrying the pointer to the previous run's episode.
- Canceling a task (`tasks/cancel`) stops the session run the same way `stop_agent` does, and reports `CANCELED`.

**One task per context:** a message that doesn't carry a `task_id` and whose context already has a non-terminal task is rejected with a plain-language error naming the open task — start a follow-up on that task instead, or use a different context.

**Ownership:** every task records which caller created it. `tasks/get`, `tasks/cancel`, `tasks/list`, and the push-config operations all check this — a caller can only ever see or act on its own tasks. A task belonging to someone else answers exactly like a task that doesn't exist, so a caller can't tell one apart from the other.

**Storage:** each task is a JSON file at `{workspace}/a2a/tasks/{task_id}.json`, written atomically, loaded into memory at startup. A task whose status is terminal (`COMPLETED`, `FAILED`, `CANCELED`, `REJECTED`) and over 30 days old is pruned on load.

### The `a2a_task_update` tool

Registered only in a session started from the `a2a` endpoint — a session started any other way doesn't have it.

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `state` | string enum | yes | `"completed"`, `"input_required"`, or `"failed"`. |
| `message` | string | yes | The caller's final answer (`completed`), the question to ask them (`input_required`), or an explanation (`failed`). This is the only thing the caller sees — not the rest of the session's turn output. |
| `artifacts` | array of strings | no | Workspace-relative paths of files to attach. |

Each artifact is read into an A2A part: valid UTF-8 text becomes a text part carrying the file's name; anything else becomes a raw part with a detected media type. A path that escapes the workspace, doesn't exist, or is over 20 MB is refused with a tool error naming the problem — the update itself is not published until every artifact resolves. The tool's result is a short confirmation, e.g. "Task marked completed; the caller has been notified."

### Inbound messages

An inbound A2A message's parts become the session's kickoff content:

- **Text parts** become the message content, concatenated in order.
- **Image raw parts** (`image/jpeg`, `image/png`, `image/gif`, `image/webp`, within the existing inline size limit) become inline images the model sees directly, the same as a pasted image on any other interface.
- **Other raw parts** are saved to the agent inbox and referenced in the content by path, the same as an interface attachment.
- **URL parts** naming an `http(s)` resource are downloaded and handled the same way as a raw part; any other scheme is reported as a failed attachment.
- **Data parts** (arbitrary JSON) are pretty-printed inline in the content.

A part that fails to save or download doesn't drop the message — it adds the same failed-attachment line every other interface uses, so the session still sees the rest of the message and knows something didn't come through.

### Skill mapping

`message.metadata.skill`, a string, starts a brand-new session with that skill activated as its role — the same mechanism `subagent_spawn`'s `skill` parameter uses (see [Session Roles](background-tasks.md#session-roles)) — when it names both a skill `id` in the Agent Card and a workspace skill (`skills/<name>/SKILL.md`) of the same name. If it's present but doesn't map to anything runnable (unknown id, or a card skill with no matching workspace skill), the session still starts, with a one-line note (`[Requested skill: <name>]`) prepended to its content instead of silently ignoring the request. Metadata on a follow-up message to an already-running or already-completed session has no effect — a skill only applies at the moment a session starts.

### Restarts

At startup, every task left `SUBMITTED` or `WORKING` from a previous run of the process is resumed automatically: a synthetic continuation message is sent through the handler as that task's caller, in the background, once the session spawner is ready. It says the process restarted, asks the session to continue and report with `a2a_task_update`, and repeats the caller's own messages on the task (each up to 2,000 characters), so the resumed session knows which task it is finishing even when the interrupted run left no episode. It is marked `residuum.synthetic` in its metadata and is never repeated in a later continuation. Each task's continuation runs independently and is logged (info on completion, warn/error on failure) — a continuation that can't be delivered leaves the task record intact for the next real message to reach it.

## Client: reaching other agents

Residuum can also delegate to other agents over A2A through the same native tools it uses to talk to its own sessions: `list_agents`, `message_agent`, and `stop_agent`. A remote agent's address is `a2a:<name>`.

### Configuring remote agents

`config/a2a.json` lists the agents this instance's client can reach:

```json
{
  "agents": {
    "laptop": {
      "url": "https://laptop.example.com/a2a/laptop",
      "headers": {
        "Authorization": "Bearer ${agent-key:laptop_token}"
      }
    }
  }
}
```

- **Name**: lowercase letters, digits, and underscores, starting with a letter, at most 64 characters — the same shape as an agent-key name.
- **`url`**: the agent's base URL (where its `.well-known/agent-card.json` lives).
- **`headers`**: optional; sent on every request to that agent, including the card fetch itself — this is how a caller key for a private remote agent gets attached. Values expand `${agent-key:<name>}` (via the same agent-key store `exec` and MCP servers use) and `${ENV}`/`${ENV:-default}`.

A bad entry (an invalid name, an empty url, or a header referencing an unknown agent key) is skipped with a warning; the rest of the file still loads. The agent may edit this file directly — it isn't write-blocked. It is watched alongside `mcp.json`, `channels.toml`, and `agent-card.json`: a change is picked up within a few seconds. **Web UI:** `GET`/`PUT /api/a2a/agents/raw` reads and saves the raw file; `GET /api/a2a/agents` returns each agent's live status.

Editing `config/a2a.json` through the agent's `write_file`/`edit_file` tools, the workspace editor, `POST /api/workspace/validate`, or the Settings page's `PUT /api/a2a/agents/raw` reports the same problems the loader would skip — a JSON syntax error (with `serde_json`'s line/column), an invalid agent name, or an empty url — without blocking the write; a diagnostic names which agent won't load instead.

### Card resolution and status

Each configured agent's `.well-known/agent-card.json` is resolved on load and reload, and cached. A card fetch failure doesn't block startup or the rest of the file — that agent is simply marked unreachable, and a background loop retries with backoff (5s, doubling to a 60s ceiling) until it succeeds. `list_agents` reports each remote agent's current status: `online` with its description and skills, `pending` while the first fetch is still in flight, or the specific reachability error.

### Sending and receiving

`message_agent` to `a2a:<name>`:
- If the sender has an open task with that agent waiting on a reply (`INPUT_REQUIRED` or `AUTH_REQUIRED`), the message is sent as a follow-up on that same task.
- Otherwise it starts a new task, in a conversation context that persists per (sender, agent) pair — so a later message to the same agent continues the same A2A context rather than starting fresh every time.
- An optional `skill` parameter names one of the remote agent's advertised skills, sent as `message.metadata.skill`.
- The call returns immediately once the remote agent has accepted the task (`configuration.return_immediately = true`) — it does not wait for the task to finish. The tool result names the task id and says the reply will arrive later as an agent message.

`stop_agent` on `a2a:<name>` cancels the sender's open task with that agent (`CancelTask`), if one exists.

**In the web UI.** Every open outbound task shows in the sessions sidebar's External group, from any sender: the remote agent, its latest status message, where the task stands (working, waiting on the sender's reply, or how long the agent has been unreachable), and how long ago it was sent. Rows update live as the tracker records changes (a `session_outbound_a2a_task` WebSocket frame); `GET /api/a2a/outbound` lists the open tasks. The row's Stop button makes the same `CancelTask` call as `stop_agent` (`POST /api/a2a/outbound/{task_id}/stop`), and the sender is told the task was canceled and that the user stopped it. When the agent can't be reached to cancel it, that endpoint answers `502` with code `unreachable` and the row offers **Stop watching** instead (`POST /api/a2a/outbound/{task_id}/stop-watching`): the task is closed locally, retries and their notices end, and the sender is told the task may still be running on the remote side. A late update from the agent never reopens a task the user closed this way.

**Delivery.** A background tracker watches every outbound task: if the remote agent's card declares `capabilities.streaming`, it subscribes to the task's event stream (reconnecting with backoff on a drop); otherwise it polls `GetTask` on a 5s-to-60s backoff. When a task reaches `INPUT_REQUIRED`, `AUTH_REQUIRED`, or a terminal state, the outcome is delivered to the original sender via the normal agent-messaging path, from `a2a:<name>`:

```
[Remote agent a2a:laptop — task <id>: input_required]
Which output format do you want?
```

A short text artifact (≤4 KB) is inlined in the same message; a longer one, or any file artifact, is saved to the agent inbox and referenced by path. If the sender no longer exists (e.g. a spawned session that has since completed with nothing waiting on it), the result goes to `main` instead, with a note that the original sender is gone. If an agent stays unreachable for 10 minutes of continuous failed checks, the sender gets one notice as a transcript note (`Still unreachable after 10 minutes (<reason>). Still retrying in the background.`) and the user gets a matching system notice (`a2a:<name> has been unreachable for 10 minutes (task <id>): <reason>. Still retrying in the background.`). Retries continue with the same backoff either way, and once the agent answers again both get a second notice (`Reachable again.` / `a2a:<name> is reachable again (task <id>).`). Polling never gives up on its own; stopping the task, or stopping watching it from the web UI, is what ends it. The log records a warning once when a failure streak starts and an info line when it recovers, not a line per failed retry.

**Persistence.** Every tracked task — its sender, agent, task and context ids, state, and last status text — is persisted at `{workspace}/a2a/outbound.json`, so open tasks resume being watched across a restart.

## Siblings

One user's own other Residuum instances find and trust each other automatically through the relay, with no `config/a2a.json` entry or caller key needed.

**Discovery.** While the relay tunnel is connected, a background task fetches `GET {origin}/a2a/agents` — the relay's per-user A2A directory — using the sibling bearer token the tunnel minted for this connection, and registers every instance except this one in the client hub as a `Sibling`-sourced remote agent: name is the instance's slug, url is `{origin}/a2a/{slug}`, and `Authorization: Bearer <token>` is sent on every request to it, including the card fetch. It refetches immediately on every (re)connect — the token is minted fresh per connection, so a reconnect needs the header updated too — and every 10 minutes while the tunnel stays connected. A directory fetch failure (network error, non-2xx, malformed JSON) logs one warning and retries with backoff (5s, doubling to a 5-minute ceiling); it never logs the token. Disconnecting the tunnel leaves already-registered siblings in place — they simply fail the next time something tries to use them, the same as any other agent whose card fetch is stale.

A sibling name is the relay's slug shape (lowercase letters, digits, and hyphens, up to 24 characters), which is looser than a `config/a2a.json` name — both work as `a2a:<name>` addresses without loosening the config file's own validation. If a `config/a2a.json` entry and a sibling ever share a name, the config entry always wins; the collision is logged once at debug.

**Trust.** A request whose bearer token matches a connected sibling's token authenticates as `sibling:<slug>` per the [auth layer](#auth-layer) above — the relay strips the caller's own `Authorization` and attests it as `x-residuum-sibling: <slug>` instead, which this instance's tunnel forwarder marks with its per-process nonce so nothing else can forge it. A sibling's session (and any resulting message) reads as coming from `laptop`, described as "your own other Residuum instance," rather than an anonymous caller — an external caller-key holder gets the bare key name instead, described as "an external agent with a caller key." The session's source label is `a2a:<slug>` for a sibling and `a2a:<keyname>` for a caller key, either way readable rather than the internal `key:`/`sibling:`-prefixed form used for the session address and task ownership.

`list_agents` marks a sibling with `(your instance)` next to its address; `GET /api/a2a/agents` reports it with `"source": "sibling"` (a `config/a2a.json` entry reports `"source": "config"`).

## Web UI

Settings → A2A is the web UI's view onto everything above, plus a preview of the Agent Card:

- **Status** — whether A2A is on, its visibility, the address other agents use to reach it, and any current problem with the listener or the workspace agent card. Backed by `GET /api/a2a/status`, which returns `{ enabled, port, visibility, public_url, listener_running, card_error }`. `listener_running` is a live probe of the A2A port's `/_a2a/auth-check` path rather than in-process state, so it reflects what an outside caller would actually see. `public_url` follows the same [Public URL](#public-url) precedence the Agent Card itself uses — `[a2a] public_url` when set, else the relay URL while the tunnel is connected, else `null` (the local listener address is not reachable from outside, so the page explains how to get a public address instead). `GET /api/a2a/card` builds its interface URLs with the same precedence, falling back to the local address, so it matches what the listener serves. The `enabled`/`visibility`/`port`/`public_url` fields themselves are edited the same way as the rest of `config.toml` — this page's toggle, select, and text fields are the `[a2a]` section's Simple/Advanced form controls, present in `config.toml`'s raw and Advanced editors too.
- **Caller keys** — the same list/create/revoke as the CLI, with a create form that shows the minted token once, and a revoke confirmation.
- **Remote agents** — every registered agent, `config/a2a.json` entries and discovered siblings alike, each with a reachability status, a summary of its card's skills, and its source (`config` or `sibling`); also a raw editor for `config/a2a.json` itself (siblings aren't part of that file and can't be edited there). Served by `GET /api/a2a/agents` and `GET`/`PUT /api/a2a/agents/raw` (see [Client](#client-reaching-other-agents) and [Siblings](#siblings)).
- **Tasks sent to other agents** live in the sessions sidebar rather than on this page — see [Sending and receiving](#sending-and-receiving).
- **Agent card** — a preview of the served card's name, description, and skills (via `GET /api/a2a/card`, which mirrors what the listener serves — same [Public URL](#public-url) resolution, so its interface URLs match — or a `503` with a plain-language reason if the workspace file is invalid), with a link to open the workspace panel to edit `config/agent-card.json` directly.

## Code

`src/a2a/`: `keys.rs` and `keys_runtime.rs` (the caller-key store and its shared runtime handle), `card.rs` (the workspace agent-card file, validation, and the live `CardState`), `auth.rs` (the middleware, `Caller`, and the `TunnelNonceSource` trait that supplies the tunnel nonce sibling attestation checks against), `listener.rs` (the axum listener), `executor.rs` (`SessionExecutor`, the `a2a_server::AgentExecutor` that delivers into a conversation session and maps its activity back onto A2A task states), `task_store.rs` (`FileTaskStore`, the persistent `a2a_server::TaskStore`), `handler.rs` (`ResiduumA2aHandler`, wrapping `a2a_server::DefaultRequestHandler` with ownership checks, caller-scoped listing, and the one-task-per-context rule, plus the restart continuation sweep), `public_url.rs` (resolving the card's — and the web UI's — public URL from config and live tunnel status). `src/tools/a2a_task_update.rs` is the session tool.

`src/a2a/client/`: `config.rs` (`config/a2a.json` loading and validation), `hub.rs` (`A2aClientHub`: the registered agents, their resolved cards, and building A2A protocol clients from the `a2a-client-lf` SDK — config entries always winning a name collision with a sibling lives here), `tracker.rs` (`RemoteTaskTracker`: persistence, the watch/poll loop, unreachable/recovery notices, and the user's stop and stop-watching), `siblings.rs` (the background task that watches `TunnelStatus` and calls `A2aClientHub::set_siblings` from the relay directory). The tools themselves live in `src/tools/message_agent.rs` and `src/tools/background.rs`.

`src/commands/a2a.rs` is the CLI. `src/gateway/web/a2a.rs` is the web API: caller keys, the remote-agents endpoints, the outbound-task list and stop endpoints, and the settings page's status and card endpoints. `web/src/components/OutboundTaskRow.svelte` is the sidebar row.
