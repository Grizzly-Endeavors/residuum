# Hub HTTP Surface

The hub serves everything the backend offers from one router over its `AgentDirectory` (`src/hub/http/`, built by `hub_router`). Every API path lives under `/api/`. The only paths outside it are the relay callback (`/cloud/callback`), webhooks (`/webhook/{agent}/{name}`), and the embedded web app (see [Embedded web app](#embedded-web-app)), which answers every other path.

## Route layout

| Prefix | Serves |
|--------|--------|
| `/api/hub/...` | Things that exist once per process: agent lifecycle and status, the hub WebSocket, hub config, secrets, agent keys, A2A caller keys, cloud, update, shutdown, tracing, timezone, the MCP catalog, onboarding, and the `hub` and `team` checkpoint repositories. |
| `/api/team/...` | The team folder: its file API (`/api/team/workspace/...`) and the workbench (`/api/team/workbench/...`). |
| `/api/agents/{name}/...` | Everything one agent owns, resolved on every request. |
| `/webhook/{agent}/{name}` | The named webhook of one agent. |

### Hub routes

| Route | Does |
|-------|------|
| `GET /api/hub/agents` | `{ "agents": [AgentSummary] }`, sorted by name. An empty list means the hub is not set up yet. |
| `POST /api/hub/agents` | Creates and starts an agent from `{ name, description?, models_from?, providers_toml?, a2a_visibility? }`; `201` with its summary. `400` for an invalid name or request, `409` when the name exists. |
| `DELETE /api/hub/agents/{name}` | `{ "deleted": true, "checkpoint_id": ... }`. |
| `GET /api/hub/agents/deleted` | `{ "agents": [{ name, deleted_at, checkpoint_id }] }`: deleted agents that can be restored, newest deletion first. `deleted_at` is an RFC 3339 time and `checkpoint_id` the workspace checkpoint a restore uses by default. An agent that exists, or was restored, is not listed. |
| `POST /api/hub/agents/restore` | Restores a deleted agent from `{ name, checkpoint_id? }` and starts it when its `autostart` is on; `201` with its summary. Without `checkpoint_id` the files come from the checkpoint the deletion took. `404` when the name has no checkpoint history, `409` when an agent by that name exists, `400` for an invalid name, an unreadable body, or a `checkpoint_id` the agent's history doesn't have. See [Agent Creation, Deletion and Restore](agent-lifecycle.md#restoring-a-deleted-agent). |
| `POST /api/hub/agents/{name}/start`, `/stop`, `/restart` | The agent's new summary. |
| `PATCH /api/hub/agents/{name}` | Sets `autostart` and/or `a2a_visibility` (at least one); the agent's new summary. |
| `POST /api/hub/stop-all` | Stops every running or starting agent and leaves the hub running. `200` with `{ stopped, failed }` when all stopped, `500` with the same body when some did not. Reachable over the tunnel, since the hub keeps running and agents can be started again. |
| `GET /api/hub/status` | `{ version, uptime_secs, tunnel, agents }`. `tunnel` has the shape of `GET /api/hub/cloud/status`; `agents` counts `starting`, `running`, `stopped`, and `failed` agents. |
| `GET /api/hub/ws` | The hub WebSocket, below. |
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

`AgentSummary` is `{ name, state, last_error, autostart, role, a2a_visibility }`. `state` is `starting`, `running`, `stopped`, or `failed`; `last_error` is `{ message, at }` while the state is `failed` and `null` otherwise.

A lifecycle request is always made on the user's behalf. Errors are `{ "error": message }` with `404` for an unknown agent, `400` for an invalid name or request body, `409` for a name that exists or an agent in the wrong state, `503` for `start`, `restart`, or `create` refused because the hub is shutting down (see [hub.md](hub.md#start-up-and-shutdown)), and `500` for a failure the user can read in the message.

### Team routes

`/api/team/workspace/...` is the workspace file API (`files`, `file`, `raw`, `tree`, `read`, `validate`, `dir`, `move`) over the team folder alone, with every path relative to `team/`. Writes are attributed to the user and coordinated with agent writes (see [team-files.md](team-files.md)). A write that changes the team's `AGENTS.md` or `USER.md` makes every running agent reload its workspace. `/api/team/workbench/info`, `/api/team/workbench/artifacts`, and `DELETE /api/team/workbench/artifacts/{name}` serve the workbench (see [workbench.md](workbench.md)).

### Agent routes

`/api/agents/{name}{rest}` reaches the agent's own routes. The hub removes `/api/agents/{name}` and hands the request, with its query and body, to the agent's router at `/api{rest}`; the exception is `/api/agents/{name}/ws`, which is the agent's WebSocket (`/ws`) and carries the protocol unchanged. So `/api/agents/scout/status` is the agent's `/api/status`, `/api/agents/scout/files/{id}` its `/api/files/{id}`, and `/api/agents/scout/sessions` its `/api/sessions`. The file URLs the agent puts in messages and frames use this form, so a client uses them as given.

An agent's routes come in two groups:

- **Repair routes** work on a stopped or failed agent, so the user can fix its configuration: `config/...`, `providers/...` (including `providers/models`), `mcp/...`, `workspace/...`, and `checkpoints...` (the agent's `workspace` and `agent_config` repositories).
- **Everything else** needs the agent to be running: `ws`, `status`, `chat/history`, `usage`, `sessions...`, `scheduled/...`, `inbox...`, `agent-inbox`, `files/...`, `memory/search`, `model/complete`, and `a2a/{agents,agents/raw,status,card,outbound...}`.

Resolution answers before the agent's router sees the request:

| Condition | Response |
|-----------|----------|
| Unknown agent | `404 { "error": "no agent named '<name>'" }` |
| Agent not `running` (on a non-repair route) | `409 { "error": "<name> is <state>", "state": "<state>" }` |

`POST /webhook/{agent}/{name}` is handled by that agent's `/webhook/{name}` route under the same rules.

### Sessions started by artifacts

A session runs on one agent, so an artifact names it (`residuum.sessions.start({ agent, prompt })`, see [workbench.md](workbench.md)) and the start goes to `POST /api/agents/{name}/sessions`. An unknown agent answers `404` and one that isn't running answers `409`. `POST /api/sessions`, which names no agent, answers `400` with a message explaining that.

## Request guards

- The **cross-site guard** covers every route: state-changing requests and WebSocket upgrades from another site are refused with `403` (see [workbench.md](workbench.md#security-model)).
- The **remote-control guard** covers `POST /api/hub/shutdown` and `POST /api/hub/cloud/disconnect` (see [cloud-tunnel.md](cloud-tunnel.md)): a request that arrived through the relay tunnel is refused with `403`.

## Embedded web app

The web app is embedded in the binary (`web/dist/`, served by `src/gateway/web/assets.rs`) and answers every path the routes above don't claim. A path naming an embedded file serves that file. A path with no dot that doesn't start with `api` or `ws`, a client-side route such as `/agent/atlas/files`, serves `index.html` so the app's router takes over. Every other path answers `404`, including a missing file and an unknown `/api` or `/ws` path, so a client calling a missing endpoint sees the failure rather than HTML.

| Files | `Cache-Control` | Validator |
|-------|-----------------|-----------|
| Everything under `/assets/`, which the build names by content hash | `public, max-age=31536000, immutable` | None. |
| Every other embedded file: `index.html` (also when it answers a client route), `/manifest.webmanifest`, the icons, `favicon.svg`, `mcp-catalog.json` | `no-cache` | A strong `ETag` from a hash of the file's content. |

A `GET` or `HEAD` whose `If-None-Match` lists the file's `ETag` (or `*`; a `W/` prefix on the client's copy is ignored) gets `304` with no body, carrying the `ETag`, `Cache-Control`, and `Vary`.

JavaScript, CSS, JSON, SVG, and the web manifest are compressed with brotli or gzip, whichever the request's `Accept-Encoding` prefers. They carry `Vary: Accept-Encoding` whether or not the request accepted compression, and a file has the same `ETag` in every encoding. HTML and images are never compressed. HTML stays plain because Residuum Cloud's relay inserts its instance switcher before the `</body>` of a top-level page, and it finds that tag by searching the body as text. API responses are not compressed.

Through Residuum Cloud the tunnel's loopback client passes the browser's `Accept-Encoding` and `If-None-Match` to this router unchanged and returns the answer's headers and body as received, without decompressing (see [Residuum Cloud Tunnel](cloud-tunnel.md)).

## Hub WebSocket

`/api/hub/ws` sends JSON frames tagged by `type`, and accepts one message, `watch_team`.

| Frame | Sent when |
|-------|-----------|
| `agents_snapshot` `{ agents }` | On connect, and again whenever the connection fell behind the hub's event stream and events were lost. |
| `agent_state` `{ agent }` | An agent's state, `autostart`, or visibility changed. |
| `agent_created` `{ agent, by }`, `agent_restored` `{ agent, by }`, `agent_deleted` `{ name, by }` | An agent was created, restored from its checkpoint history, or deleted. `by` is `user` or `agent:<name>`. |
| `agent_activity` `{ name, busy, unread }` | An agent's main-conversation activity changed. |
| `notice` `{ level, message, agent? }` | A hub notice, or a warning about a message this connection sent that could not be used. Created, restored, deleted, and failed events travel only in their own frames. |
| `workspace_changed` `{ changes }`, `workspace_resync` `{ reason }`, `workspace_watch_unavailable` `{ message }` | Team change-feed frames, with the shapes of the agent WebSocket's, for the paths the connection watches. |

`{ "type": "watch_team", "prefixes": [...] }` replaces the set of team paths the connection watches; `[]` stops watching. A prefix names `team` or a path under `team/`, the spelling the change feed uses (`team/wiki`), and matches whole path segments. A prefix outside `team/` or an unreadable message is refused with a warning `notice`, and the current watch stays in force. A connection that starts watching while the team watcher is off gets `workspace_watch_unavailable`.
