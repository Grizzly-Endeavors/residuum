# Hub HTTP Surface

The hub serves everything the backend offers from one router over its `AgentDirectory` (`src/hub/http/`, built by `hub_router`). Every API path lives under `/api/`. The only paths outside it are the relay callback (`/cloud/callback`), webhooks (`/webhook/{agent}/{name}`), and the embedded web app (see [Embedded web app](#embedded-web-app)), which answers every other path. The gateway's listener serves the whole router. The artifacts listener serves its `/api` paths from the same router (see [Request guards](#request-guards)) and nothing else of it.

## Route layout

| Prefix | Serves |
|--------|--------|
| `/api/hub/...` | Things that exist once per process: agent lifecycle and status, the hub WebSocket, every agent's user inbox in one list, Web Push, hub config, secrets, agent keys, A2A caller keys, cloud, update, shutdown, tracing, timezone, the MCP catalog, onboarding, and the `hub` and `team` checkpoint repositories. |
| `/api/team/...` | The team folder: its file API (`/api/team/workspace/...`) and the workbench (`/api/team/workbench/...`). |
| `/api/agents/{name}/...` | Everything one agent owns, resolved on every request. |
| `/webhook/{agent}/{name}` | The named webhook of one agent. |

### Hub routes

| Route | Does |
|-------|------|
| `GET /api/hub/agents` | `{ "agents": [AgentSummary], "activity": { "<name>": AgentActivity }, "stopping": ["<name>"] }`. `agents` is sorted by name, and an empty list means the hub is not set up yet. `activity` has an entry for every agent. `stopping` names the agents whose stop has begun and not finished; their `state` still reads `running` (see [hub.md](hub.md#operations)). |
| `POST /api/hub/agents` | Creates and starts an agent from `{ name, description?, models_from?, providers_toml?, a2a_visibility? }`; `201` with its summary. `400` for an invalid name or request, `409` when the name exists. |
| `DELETE /api/hub/agents/{name}` | `{ "deleted": true, "checkpoint_id": ... }`. |
| `GET /api/hub/agents/deleted` | `{ "agents": [{ name, deleted_at, checkpoint_id }] }`: deleted agents that can be restored, newest deletion first. `deleted_at` is an RFC 3339 time and `checkpoint_id` the workspace checkpoint a restore uses by default. An agent that exists, or was restored, is not listed. |
| `POST /api/hub/agents/restore` | Restores a deleted agent from `{ name, checkpoint_id? }` and starts it when its `autostart` is on; `201` with its summary. Without `checkpoint_id` the files come from the checkpoint the deletion took. `404` when the name has no checkpoint history, `409` when an agent by that name exists, `400` for an invalid name, an unreadable body, or a `checkpoint_id` the agent's history doesn't have. See [Agent Creation, Deletion and Restore](agent-lifecycle.md#restoring-a-deleted-agent). |
| `POST /api/hub/agents/{name}/start`, `/stop`, `/restart` | The agent's new summary. |
| `PATCH /api/hub/agents/{name}` | Sets `autostart` and/or `a2a_visibility` (at least one); the agent's new summary. |
| `POST /api/hub/stop-all` | Stops every running or starting agent and leaves the hub running. `200` with `{ stopped, failed }` when all stopped, `500` with the same body when some did not. Reachable over the tunnel, since the hub keeps running and agents can be started again. |
| `GET /api/hub/status` | `{ version, uptime_secs, tunnel, agents }`. `tunnel` has the shape of `GET /api/hub/cloud/status`; `agents` counts `starting`, `running`, `stopped`, and `failed` agents. |
| `GET /api/hub/ws` | The hub WebSocket, below. |
| `GET /api/hub/events?before=&after=&limit=` | The team event log, newest first: `{ boot_id, events, next_before }`. See [Team events](#team-events). |
| `GET /api/hub/overview` | What Home shows about every agent: `{ boot_id, agents }`. See [Team overview](#team-overview). |
| `GET /api/hub/inbox`, `GET /api/hub/inbox/unread`, `PUT /api/hub/inbox/{agent}/{id}/read`, `POST /api/hub/inbox/{agent}/{id}/archive`, `POST /api/hub/inbox/{agent}/{id}/restore` | Every agent's user inbox, read from their files, below. |
| `GET /api/hub/push/key`, `GET`/`PUT /api/hub/push/devices`, `PATCH`/`DELETE /api/hub/push/devices/{id}`, `POST /api/hub/push/devices/{id}/test` | Web Push: the signing key and the devices that receive notifications, below. |
| `GET`/`PUT /api/hub/config/raw`, `PATCH /api/hub/config/patch`, `POST /api/hub/config/validate` | The hub's `config.toml`. |
| `POST /api/hub/config/complete-setup` | Onboarding: writes the hub config, the team layer, and the first agent's directory, then the hub starts that agent (see [hub.md](hub.md#start-up-and-shutdown)). `409` when an agent already exists. |
| `POST /api/hub/providers/models` | Lists the models a provider offers from the settings in the request, with no agent. `secret:` keys resolve against the hub's secret store. |
| `GET`/`POST /api/hub/secrets`, `DELETE /api/hub/secrets/{name}` | Secret names and writes. |
| `GET`/`POST /api/hub/agent-keys`, `DELETE /api/hub/agent-keys/{name}` | Agent keys (see [agent-keys.md](agent-keys.md)). |
| `GET`/`POST /api/hub/a2a/keys`, `DELETE /api/hub/a2a/keys/{name}` | A2A caller keys (see [a2a.md](a2a.md)). |
| `GET /api/hub/cloud/status`, `POST /api/hub/cloud/disconnect` | The relay tunnel (see [cloud-tunnel.md](cloud-tunnel.md)). |
| `GET /api/hub/update/status`, `POST /api/hub/update/check`, `/apply`, `/restart` | Self-update (see [self-update.md](self-update.md)). |
| `POST /api/hub/shutdown` | Graceful shutdown of the whole process. Refused over the tunnel. |
| `GET /api/hub/tracing/status` and the rest of `/api/hub/tracing/...` | Tracing and diagnostics. |
| `GET /api/hub/system/timezone`, `GET /api/hub/mcp-catalog` | The detected timezone and the MCP catalog. |
| `/api/hub/checkpoints...` | Checkpoints of the `hub` and `team` repositories (see [checkpoints.md](checkpoints.md)). |

`AgentSummary` is `{ name, state, last_error, autostart, role, a2a_visibility }`. `state` is `starting`, `running`, `stopped`, or `failed`; `last_error` is an `AgentLastError` while the state is `failed` and `null` otherwise. A change in an agent's activity or stopping set never changes its summary.

`AgentLastError` is `{ message, kind, reason, at }`. `message` is the plain-language text for the user, which wraps the failure with what to do next. `reason` is the underlying error text alone. `kind` says what sort of failure it was, so a client can offer the matching next step: `config` (start-up rejected the agent's configuration), `port_conflict` (another agent holds its Teams port), `crash` (the agent panicked, or its event loop ended on its own), or `other`. `at` is an RFC 3339 time.

`AgentActivity` is `{ busy, busy_since, unread }`. `busy` is true while a main turn runs and `busy_since` is when that turn began, as an RFC 3339 time, or `null` while none runs. `unread` counts main-conversation replies published while no web client had the agent's WebSocket open; a socket a workbench page opens through the artifacts origin doesn't count as a web client.

A lifecycle request is always made on the user's behalf. Errors are `{ "error": message }` with `404` for an unknown agent, `400` for an invalid name or request body, `409` for a name that exists or an agent in the wrong state, `503` for `start`, `restart`, or `create` refused because the hub is shutting down (see [hub.md](hub.md#start-up-and-shutdown)), and `500` for a failure the user can read in the message.

### Cross-agent inbox

The hub's inbox routes read and change each agent's user inbox files directly (see [Inbox](inbox.md)), so every agent answers in any state. An item is identified by its agent and its `id`, which is unique only within one agent.

| Route | Answers |
|-------|---------|
| `GET /api/hub/inbox?status=active\|archived&agent=<name>&before=<cursor>&limit=<n>` | `{ items: [HubInboxItem], next_cursor }`. All four parameters are optional: `status` defaults to `active`, `agent` limits the list to one agent, and every agent is listed otherwise. |
| `GET /api/hub/inbox/unread` | `{ total, by_agent: { <name>: number } }`: the unread active items, with an entry for every agent, those with none included. An agent whose inbox can't be read counts as none and is logged. |
| `PUT /api/hub/inbox/{agent}/{id}/read` | `{ item }`. Marks the item read where it is: in the active inbox, else in the archive. |
| `POST /api/hub/inbox/{agent}/{id}/archive` | `{ item }`. Moves an active item, and its attachments, to the archive. |
| `POST /api/hub/inbox/{agent}/{id}/restore` | `{ item }`. Moves an archived item, and its attachments, back to the active inbox. |

`HubInboxItem` is `{ agent, id, title, body, source, at, read, attachments }`:
- `at` is RFC 3339 with an offset, such as `2026-03-08T03:30:00-04:00` (`Z` for UTC). Items store a naive local time to the minute; the hub reads it in its configured timezone on every request. A time that occurred twice (a DST fall-back) takes its first occurrence, and a time that never occurred (a spring-forward gap) moves forward by the length of the gap.
- `attachments` is `[{ filename, mime_type, size, url }]`. `url` is the owning agent's attachment route, `/api/agents/{agent}/inbox/{id}/attachments/{index}`, which serves a stopped or failed agent as well and finds the file whether the item is active or archived.

A listing is newest first by `at`, then `id`, then agent. A page holds `limit` items, 50 when it isn't given, and a limit above 200 is treated as 200. `next_cursor` is `null` on the last page. Otherwise it is an opaque string to pass as `before` to get the page that follows, which starts after the item the cursor names even if that item has since been archived or removed. A listing fails as a whole when any listed agent's inbox can't be read, naming the agent, instead of answering with its items missing.

Errors are `{ "error": message }`: `400` for a `status` other than `active` or `archived`, a `limit` that is not a whole number of at least 1, a `before` the hub didn't issue, or an `id` that isn't a bare item id (empty, `.` or `..`, or containing `/`, `\`, or a NUL); `404` for an unknown agent, or an item that isn't where the call looks for it (`archive` needs it active, `restore` needs it archived); `409` when the destination already holds a different item with the same `id`, in which case both items stay where they are; and `500` when the files can't be read or changed.

### Web Push devices

The push routes register browsers and installed apps for notifications and say how delivery to each is going (see [Notifications](notifications.md#web-push) for what is sent, the signing key, and how failures are handled). They are open over the relay tunnel, so a phone on Residuum Cloud manages its own notifications.

| Route | Answers |
|-------|---------|
| `GET /api/hub/push/key` | `{ public_key }`: the hub's VAPID public key as base64url, which a browser subscribes with (`applicationServerKey`). The key is created the first time it is asked for. |
| `GET /api/hub/push/devices` | `{ devices: [PushDevice] }`, oldest first. |
| `PUT /api/hub/push/devices` | Registers a device from `{ subscription, label?, preferences? }` and answers `{ device }`. `subscription` is the browser's `PushSubscription` JSON (`endpoint` and `keys.p256dh`/`keys.auth`). A subscription with the same `endpoint` updates the existing device, keeping its `id` and delivery history, so the call can be repeated. A new device without a `label` is called "Unnamed device"; one without `preferences` gets the defaults; an existing device keeps what the request doesn't name. |
| `PATCH /api/hub/push/devices/{id}` | Changes `{ label?, preferences? }` (at least one) and answers `{ device }`. |
| `DELETE /api/hub/push/devices/{id}` | `204`. The device stops receiving notifications; the browser's own subscription is the browser's to cancel. |
| `POST /api/hub/push/devices/{id}/test` | Sends the test notification and answers `{ delivered, error }` once the push service has accepted or refused it. A refusal is a `200` with `delivered: false` and the reason in `error`. |

`PushDevice` is `{ id, label, created_at, last_success_at, last_failure, preferences }`. `created_at` and `last_success_at` are RFC 3339 times (`last_success_at` is `null` until a notification has reached the push service), and `last_failure` is `{ at, status, message }` or `null` (see [Notifications](notifications.md#delivery)). `preferences` is `{ inbox_item, agent_failed, outbound_unreachable, reply_while_away }`, all booleans. A request's `preferences` may name only some of them. The subscription's address and keys are never returned.

Errors are `{ "error": message }`: `400` for a body that can't be read, a blank `label`, a `PATCH` that sets nothing, or a subscription whose `endpoint` isn't an `https:` address or whose `p256dh` (a 65-byte P-256 public key) or `auth` (16 bytes) key is malformed; `404` for an unknown device; and `500` when the key or devices file can't be read or written, with a message that names the file.

### Team events

`GET /api/hub/events?before=<id>&after=<id>&limit=<n>` serves the hub's in-memory team event log (see [Team event log](hub.md#team-event-log)) as `{ boot_id, events, next_before }`. `events` are entries, newest first, each `{ id, at, agent, kind, level, summary, target }`. `boot_id` is the hub process's id, the one `hub_boot` announces; a client that sees it change holds events from an earlier process and starts over.

All three parameters are optional. `before` returns only entries with a lower id, and `after` only entries with a higher id, so `after` with a client's newest id returns what it has not seen. A page holds `limit` entries, 50 when it isn't given, and a limit above 200 is treated as 200. When more entries match than fit, the page holds the newest ones and `next_before` is the id of its oldest entry: pass it as `before`, with the same `after`, for the page that follows. `next_before` is `null` when nothing older matches.

Errors are `{ "error": message }`: `400` for a `before` or `after` that is not a whole number, and for a `limit` that is not a whole number of at least 1.

### Team overview

`GET /api/hub/overview` serves what the hub keeps about each agent beyond its state, activity and summary (see [Team overview](hub.md#team-overview)) as `{ boot_id, agents }`. `agents` has one overview per agent, sorted by name, in every state, and `boot_id` is the hub process's id, the one `hub_boot` announces. An overview is:

```
{
  name,
  last_message: { role: "user" | "assistant", preview, at, at_precision: "minute" | "day" } | null,
  live_sessions: [{ address, run_id, category, source_label, purpose, state, started_at }],
  upcoming: [],
  inbox_unread: number,
  outbound_problems: []
}
```

`preview` is plain text on one line of at most 200 characters. `at` is RFC 3339 with the hub timezone's offset. `source_label` is what started the session (`pulse:email_check`, `artifact:notes`). The request always answers `200`: a part that can't be read is logged and shown as empty. It counts every agent's user inbox again and reads a stopped agent's parts from its files. A client fetches it when it connects to the hub WebSocket, and again after an `agents_snapshot` that was sent because the connection fell behind, and replaces its copy of an agent's overview with each `agent_overview` frame.

### Team routes

`/api/team/workspace/...` is the workspace file API (`files`, `file`, `raw`, `tree`, `read`, `validate`, `dir`, `move`) over the team folder alone, with every path relative to `team/`. Writes are attributed to the user and coordinated with agent writes (see [team-files.md](team-files.md)). A write that changes the team's `AGENTS.md` or `USER.md` makes every running agent reload its workspace. `/api/team/workbench/info`, `/api/team/workbench/artifacts`, and `DELETE /api/team/workbench/artifacts/{name}` serve the workbench (see [workbench.md](workbench.md)).

### Agent routes

`/api/agents/{name}{rest}` reaches the agent's own routes. The hub removes `/api/agents/{name}` and hands the request, with its query and body, to the agent's router at `/api{rest}`; the exception is `/api/agents/{name}/ws`, which is the agent's WebSocket (`/ws`) and carries the protocol unchanged. So `/api/agents/scout/status` is the agent's `/api/status`, `/api/agents/scout/files/{id}` its `/api/files/{id}`, and `/api/agents/scout/sessions` its `/api/sessions`. The file URLs the agent puts in messages and frames use this form, so a client uses them as given.

An agent's routes come in two groups:

- **File routes** work on a running, stopped, or failed agent, because they only read and write the agent's files on disk. They are the repair routes, which let the user fix a configuration (`config/...`, `providers/...` including `providers/models`, `mcp/...`, `workspace/...`, and `checkpoints...` for the agent's `workspace` and `agent_config` repositories), and the routes that show what the agent kept: `chat/history` (recent messages, and `?episode=` for an archived episode), `usage`, the user inbox (`inbox`, `inbox/archive`, `inbox/{id}/read`, `inbox/{id}/archive`, `inbox/{id}/restore`, `inbox/{id}/attachments/{index}`), and `a2a/agents/raw` (`GET` and `PUT`). Requests and responses have the same shape in every state. A write to a stopped or failed agent only touches disk, and the agent reads it when it next starts; a write to a running agent signals it to reload where that applies, as a config or A2A settings write does. A stopped or failed agent's `chat/history`, `usage`, inbox, and `a2a/agents/raw` routes never open its checkpoint repositories, so they answer even when those can't be opened; the raw A2A settings write opens them to take its checkpoint and, when it can't, saves without one and logs a warning. The repair routes do open them, and answer `500` naming the checkpoint history when they can't.
- **Live routes** need the agent to be running: `ws`, `status`, `sessions...`, `scheduled/...`, `agent-inbox`, `files/...`, `memory/search`, `model/complete`, and `a2a/{agents,status,card,outbound...}`. `status` reports the running process, so it answers `409` for an agent in any other state.

Resolution answers before the agent's router sees the request:

| Condition | Response |
|-----------|----------|
| Unknown agent | `404 { "error": "no agent named '<name>'" }` |
| Agent not `running` (on a live route) | `409 { "error": "<name> is <state>", "state": "<state>" }` |

`POST /webhook/{agent}/{name}` is handled by that agent's `/webhook/{name}` route under the same rules.

### Sessions started by artifacts

A session runs on one agent, so an artifact names it (`residuum.sessions.start({ agent, prompt })`, see [workbench.md](workbench.md)) and the start goes to `POST /api/agents/{name}/sessions`. An unknown agent answers `404` and one that isn't running answers `409`. `POST /api/sessions`, which names no agent, answers `400` with a message explaining that.

## Request guards

- The **cross-site guard** covers every route on both listeners: state-changing requests and WebSocket upgrades from another site are refused with `403` (see [workbench.md](workbench.md#security-model)).
- The **remote-control guard** covers `POST /api/hub/shutdown` and `POST /api/hub/cloud/disconnect` (see [cloud-tunnel.md](cloud-tunnel.md)): a request that arrived through the relay tunnel is refused with `403`.
- The **artifacts-origin block list** covers `/api/hub/shutdown`, `/api/hub/stop-all`, `/api/hub/update/check`, `/api/hub/update/apply`, `/api/hub/update/restart` and `/api/hub/config/complete-setup`. The artifacts listener serves `/api` by handing requests to this router in-process, marked by an internal request extension that a client can't send, and a marked request to one of these routes is refused with `403` and `{ "error" }`. A marked agent socket (`/api/agents/{name}/ws`) doesn't count as a client for the agent's unread count or connected state. The same routes work on the gateway. See [API forwarding](workbench.md#api-forwarding).

## Embedded web app

The web app is embedded in the binary (`web/dist/`, served by `src/gateway/web/assets.rs`) and answers every path the routes above don't claim. A path naming an embedded file serves that file. A path with no dot that doesn't start with `api` or `ws`, a client-side route such as `/agent/atlas/files`, serves `index.html` so the app's router takes over. Every other path answers `404`, including a missing file and an unknown `/api` or `/ws` path, so a client calling a missing endpoint sees the failure rather than HTML.

| Files | `Cache-Control` | Validator |
|-------|-----------------|-----------|
| Everything under `/assets/`, which the build names by content hash | `public, max-age=31536000, immutable` | None. |
| HTML documents: `index.html`, also when it answers a client route | `no-cache` | None. |
| Every other embedded file: `/manifest.webmanifest`, the icons, `favicon.svg`, `mcp-catalog.json` | `no-cache` | A strong `ETag` from a hash of the file's content. |

A `GET` or `HEAD` for a file with an `ETag` whose `If-None-Match` lists that `ETag` (or `*`; a `W/` prefix on the client's copy is ignored) gets `304` with no body, carrying the `ETag`, `Cache-Control`, and `Vary`. An HTML document has no `ETag` and never answers `304`, so a browser always fetches it whole. The relay rewrites top-level HTML on the way out (see below), and a `304` would leave the browser showing its old rewrite.

JavaScript, CSS, JSON, SVG, and the web manifest are compressed with brotli or gzip, whichever the request's `Accept-Encoding` prefers. They carry `Vary: Accept-Encoding` whether or not the request accepted compression, and a file has the same `ETag` in every encoding. HTML and images are never compressed. HTML stays plain and uncached because Residuum Cloud's relay inserts its instance switcher before the `</body>` of a top-level page: it finds that tag by searching the body as text, and the switcher shows which instances are connected at the moment of the request. API responses are not compressed.

Through Residuum Cloud the tunnel's loopback client passes the browser's `Accept-Encoding` and `If-None-Match` to this router unchanged and returns the answer's headers and body as received, without decompressing (see [Residuum Cloud Tunnel](cloud-tunnel.md)).

## Hub WebSocket

`/api/hub/ws` sends JSON frames tagged by `type`, and accepts one message, `watch_team`.

| Frame | Sent when |
|-------|-----------|
| `hub_boot` `{ boot_id }` | First on every connection. `boot_id` is a random id the hub generates at startup, and the team event log's id too: every connection to one process sees the same id, and a restarted hub has a new one. |
| `agents_snapshot` `{ agents, activity, stopping }` | After `hub_boot`, and again whenever the connection fell behind the hub's event stream, the team event log or the overview frames and lost frames. It has the three fields of `GET /api/hub/agents`. A client that gets one after its first reads the events it missed from `GET /api/hub/events` and the overviews from `GET /api/hub/overview`. |
| `agent_state` `{ agent }` | An agent's state, `autostart`, or visibility changed. |
| `agent_stopping` `{ name }` | A running agent's stop began. Its `state` stays `running` until the stop finishes, which `agent_state` then reports. From this frame on, the team router refuses teammate messages for it and the relay stops listing it. |
| `agent_created` `{ agent, by }`, `agent_restored` `{ agent, by }`, `agent_deleted` `{ name, by }` | An agent was created, restored from its checkpoint history, or deleted. `by` is `user` or `agent:<name>`. |
| `agent_activity` `{ name, busy, busy_since, unread }` | An agent's main-conversation activity changed. |
| `notice` `{ level, message, agent? }` | A hub notice, or a warning about a message this connection sent that could not be used. Created, restored, deleted, and failed events travel only in their own frames. |
| `hub_config_reloaded` `{ ok, changed, message }` | The hub finished an attempt to reload `hub/config.toml`, beside the notice that tells the user about it. `ok` is false when the file couldn't be loaded and the hub keeps the config it was running. `changed` is true when the loaded config differs from the running one. `message` is the text of that notice, or `null` when nothing changed. |
| `agent_overview` `{ overview }` | Something in an agent's overview changed. `overview` is the agent's whole overview, as `GET /api/hub/overview` serves it, and replaces the client's copy. Changes are gathered, so an agent gets at most one frame per second and its last state is always sent; a created or restored agent is sent at once. |
| `team_event` `{ boot_id, event }` | The team event log recorded an entry. `event` is the entry, as `GET /api/hub/events` serves it, and `boot_id` the log's id. A connection hears the entries recorded after it connected; what came before is read from the route. |
| `workspace_changed` `{ changes }`, `workspace_resync` `{ reason }`, `workspace_watch_unavailable` `{ message }` | Team change-feed frames, with the shapes of the agent WebSocket's, for the paths the connection watches. |

`{ "type": "watch_team", "prefixes": [...] }` replaces the set of team paths the connection watches; `[]` stops watching. A prefix names `team` or a path under `team/`, the spelling the change feed uses (`team/wiki`), and matches whole path segments. A prefix outside `team/` or an unreadable message is refused with a warning `notice`, and the current watch stays in force. A connection that starts watching while the team watcher is off gets `workspace_watch_unavailable`.
