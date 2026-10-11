# Multi-Agent Hub — HTTP Contract

**Status:** Built and shipped; the current routes are in [Hub HTTP Surface](../../systems-usage/hub-http.md). This is the contract that went with [design.md](design.md).

The backend serves everything under `/api/`. The only exceptions are the relay callback and webhooks, which keep root-level paths because external systems call them.

## Route placement

**Hub-level (`/api/hub/...`)**. There is one of each per process.

| Today | Hub route |
|---|---|
| `/api/hub/config/{raw,patch,validate}` | unchanged |
| `/api/secrets`, `/api/secrets/{name}` | `/api/hub/secrets`, `/api/hub/secrets/{name}` |
| `/api/agent-keys` | `/api/hub/agent-keys` (the store is shared) |
| `/api/a2a/keys`, `/api/a2a/keys/{name}` | `/api/hub/a2a/keys`, `/api/hub/a2a/keys/{name}` (the store is shared) |
| `/api/cloud/status` | `/api/hub/cloud/status` |
| `/api/cloud/disconnect` | `/api/hub/cloud/disconnect` (remote-control guarded) |
| `/api/update/{check,status,apply,restart}` | `/api/hub/update/...` |
| `/api/tracing/*` | `/api/hub/tracing/*` |
| `/api/shutdown` | `/api/hub/shutdown` (remote-control guarded) |
| `/api/system/timezone` | `/api/hub/system/timezone` |
| `/api/mcp-catalog` | `/api/hub/mcp-catalog` |
| `/api/checkpoints...` for the hub config repo and the team repo | `/api/hub/checkpoints...?repo=hub\|team` |
| none | `/api/hub/status`, `/api/hub/agents...`, `/api/hub/ws` (below) |
| `/api/config/complete-setup` | `/api/hub/config/complete-setup`. Onboarding runs before any agent exists. The body carries the hub config, the first agent's name and config, and the user's name. |
| provider model listing used by onboarding | `/api/hub/providers/models`, which lists the models a provider offers, given its settings in the request, without needing an agent |

**Team-level (`/api/team/...`)**:

| Today | Team route |
|---|---|
| `/api/workspace/*` addressing `team/...` | `/api/team/workspace/*`, with paths relative to `team/`. `/api/agents/{name}/workspace/*` with a `team/` prefix reaches the same files. |
| `/api/workbench/{info,artifacts}`, `DELETE /api/workbench/artifacts/{name}` | `/api/team/workbench/...` |

**Agent-level (`/api/agents/{name}/...`)**. Everything else, with the same sub-path it has today:

- `/ws` becomes `/api/agents/{name}/ws`. The WebSocket protocol is unchanged.
- `/api/status` becomes `/api/agents/{name}/status`, which reports the agent's own status.
- Agent config and related routes: `/api/config/*`, `/api/providers/*`, `/api/mcp/*`, and channels.
- Chat and session data: `/api/chat/history`, `/api/usage`, `/api/sessions/*`, `/api/scheduled/*`.
- Inboxes: `/api/inbox*` and `/api/agent-inbox`.
- `/api/files/*`, `/api/memory/search`, `/api/model/complete`. File URLs the server puts in messages and frames are the agent-scoped form, `/api/agents/{name}/files/...`, so clients use them as given.
- `/api/workspace/*` (the agent's namespace, including `team/`).
- `/api/checkpoints...` for the agent's workspace and agent-config repos.
- `/api/a2a/{agents,agents/raw,status,card}`, which are the agent's own A2A client settings and card.

**Root-level, unchanged in shape:**

- `/cloud/callback` (the relay redirect).
- `/webhook/{agent}/{name}`. A webhook belongs to an agent, so its public URL names the agent.

## Resolving `{name}`

| Condition | Response |
|---|---|
| Unknown agent | `404 { "error": "no agent named '<name>'" }` |
| Agent not `running` | `409 { "error": "<name> is <state>", "state": "<state>" }` |

Config, providers, MCP, channels, workspace-file and checkpoint routes still work on a stopped or failed agent, so the user can repair it.

## Lifecycle API

`AgentSummary`:

```json
{
  "name": "scout",
  "state": "starting | running | stopped | failed",
  "last_error": { "message": "…", "at": "RFC3339" } ,
  "autostart": true,
  "role": "one-line role from its wiki role page, or null",
  "a2a_visibility": "public | private"
}
```

`last_error` is `null` unless the agent's state is `failed`.

| Method | Path | Body | Response |
|---|---|---|---|
| `GET` | `/api/hub/agents` | none | `{ "agents": [AgentSummary] }`, sorted by name. An empty list means the hub isn't set up yet, and clients show onboarding. |
| `POST` | `/api/hub/agents` | `{ "name", "description"?, "models_from"?, "a2a_visibility"? }` | `201 AgentSummary`. `400` for an invalid name, `409` if the name exists. `models_from` names an existing agent whose `providers.toml` is copied; without it the request must include `"providers_toml"` as a raw string. Visibility defaults to `private`. |
| `DELETE` | `/api/hub/agents/{name}` | none | `{ "deleted": true, "checkpoint_id": "…" \| null }` |
| `POST` | `/api/hub/agents/{name}/start` | none | `AgentSummary` |
| `POST` | `/api/hub/agents/{name}/stop` | none | `AgentSummary` |
| `POST` | `/api/hub/agents/{name}/restart` | none | `AgentSummary` |
| `PATCH` | `/api/hub/agents/{name}` | `{ "autostart"?: bool, "a2a_visibility"?: "public" \| "private" }` (at least one field) | `AgentSummary`. The server writes the agent's config file and reloads it, which re-announces its A2A entry to the relay. |
| `GET` | `/api/hub/status` | none | `{ "version", "uptime_secs", "tunnel": <cloud status shape>, "agents": { "starting", "running", "stopped", "failed" } }` |
| `POST` | `/api/hub/stop-all` | none | `{ "stopped", "failed" }`: `200` when every agent stopped, `500` with the same body otherwise. Not remote-control guarded. |

The CLI `residuum agent list|create|delete|start|stop|restart` maps one-to-one onto these routes.

## Hub WebSocket (`/api/hub/ws`)

Server to client only. Each message is a JSON object tagged by `type`.

| `type` | Fields |
|---|---|
| `agents_snapshot` | `agents: [AgentSummary]`, sent once on connect |
| `agent_state` | `agent: AgentSummary`, sent on any state, `autostart` or visibility change |
| `agent_created` | `agent: AgentSummary`, `by: "user" \| "agent:<name>"` |
| `agent_deleted` | `name`, `by` |
| `agent_activity` | `name`, `busy: bool` (a main turn is in progress), `unread: u32` (main-conversation messages the web UI hasn't shown). `unread` resets when a client opens that agent's `/ws`. |
| `notice` | `level: "info" \| "warn" \| "error"`, `message`, `agent?: name`. Hub notices as a toast. Created, deleted and failed events are conveyed only by their own frames (`agent_created`, `agent_deleted`, and `agent_state` with state `failed`) and never also as a `notice`, so a client raising a toast for each frame doesn't show duplicates. |
| `workspace_changed` | the same shape as the agent WebSocket's team change-feed frames, for `team/` paths the client watches via `{ "type": "watch_team", "prefixes": [...] }` (client to server, the only client message) |
