# Design: Hub Architecture

**Status:** Proposed. Not yet implemented.
**Date:** 2026-04-18.

This document proposes a refactor of Residuum's multi-agent model from "one gateway process per agent" to "one hub process supervising many agent worker processes." The hub owns the user-facing HTTP surface (web UI, API, tunnel) and the lifecycle of its children; each agent is a subprocess bound to loopback that the hub reverse-proxies.

## Context

Today, `residuum agent create <name>` provisions a named agent at `~/.residuum/agent_registry/<name>/` with its own port, its own tunnel, its own web UI, and its own relay connection. Users launch each agent with `residuum serve --agent <name>`. Every agent is a fully independent process.

Multi-agent UX is painful because of this symmetry:

- Users must remember a port per agent to open the web UI.
- Remote access via the relay exposes one slug per agent; switching between agents on the same machine is awkward.
- Each agent runs a duplicate HTTP server, setup wizard, web UI, update checker, and tunnel.

Users *do* want separate Discord/Telegram identities per agent (that's the point of named agents), so adapter consolidation is not a goal. The goal is a single entry point for management and for the web UI; adapters stay per-agent.

The relay's existing slug-based routing (`username:instance_slug → WebSocket`) is opaque about what's on the other end, so this refactor requires **no relay-side changes**. One hub registers one slug with the relay; agent selection happens entirely inside the hub.

## Goals

- One local URL for the web UI, regardless of how many agents are configured.
- One relay connection per machine (node), replacing per-agent tunnels.
- Unified lifecycle management: create/start/stop/restart/delete agents through the hub.
- Keep per-agent Discord/Telegram adapters untouched.
- No half-measures: the old standalone-agent model is removed at cutover.

## Non-goals

- Consolidating adapters. Discord and Telegram stay per-agent.
- Cross-agent memory, bus, or state sharing. Agents remain isolated.
- Orphan agent adoption. If the hub dies, its children die.
- Resource pooling (shared HTTP clients, shared embedding models, etc.). Deferred.
- Windows support for named-pipe transports. Loopback HTTP is cross-platform.

## High-level architecture

```
                      ┌──────────────────────────────────────┐
                      │            Hub process               │
                      │                                      │
  Web UI ─────────────┤  HTTP + WS server (one port)         │
                      │  ├── /app/...         (static UI)    │
  Relay (1 tunnel)  ──┤  ├── /api/hub/...     (hub API)      │
                      │  └── /agents/<n>/...  (proxy)        │
                      │                                      │
                      │  Supervisor ──────┬──────┬──────┐    │
                      │                   │      │      │    │
                      └───────────────────┼──────┼──────┼────┘
                                          │      │      │
                                          ▼      ▼      ▼
                                     ┌─────┐┌─────┐┌─────┐
                                     │ Agt ││ Agt ││ Agt │   child processes
                                     │  A  ││  B  ││  C  │   bound to loopback
                                     └──┬──┘└──┬──┘└──┬──┘   ephemeral ports
                                        │      │      │
                                   Discord  Telegram  Discord  (per-agent
                                                                adapters,
                                                                unchanged)
```

The hub is the only process a user interacts with directly. Agents are implementation details.

## Process model

### Single binary, mode-switched

One binary, `residuum`. Mode is determined by subcommand:

| Command | Mode |
|---|---|
| `residuum serve` | Starts the hub. |
| `residuum __agent-worker --name <n> ...` | Internal: agent worker process. Hidden from `--help`. |
| `residuum agent <subcmd>` | CLI client — talks to the running hub. |
| `residuum connect [--agent <n>]` | CLI client — opens a session via the hub. |
| `residuum logs [--agent <n>]` | CLI client — reads logs through the hub. |
| `residuum stop [--agent <n>]` | CLI client — stops the hub or a single agent. |

There is no public way to run an agent worker directly. The `__agent-worker` subcommand is intended exclusively for the supervisor; running it manually is unsupported.

### Hub lifecycle

1. On `residuum serve`, the hub:
   1. Loads `~/.residuum/config.toml` (now hub-only config).
   2. Reads the agent registry from `~/.residuum/agent_registry/registry.toml`.
   3. Binds the HTTP+WS server to the configured address/port.
   4. Starts the tunnel (if `[cloud]` is configured).
   5. Spawns each agent with `autostart = true`, **serially** (see *Startup fan-out* below).
   6. Begins supervising children.
2. On SIGTERM or shutdown request, the hub:
   1. Sends SIGTERM to each child.
   2. Waits up to `shutdown_grace_secs` (default 10) for each child to exit.
   3. Sends SIGKILL to stragglers.
   4. Closes the tunnel, shuts down the HTTP server, exits.
3. On SIGKILL, the process group takes children with it (see *Process grouping* below).

### Agent worker lifecycle

Spawned by the hub with an argv like:

```
residuum __agent-worker --name researcher --workspace /home/u/.residuum/agent_registry/researcher/workspace --config /home/u/.residuum/agent_registry/researcher/config.toml
```

The child:

1. Binds `127.0.0.1:0` (ephemeral port).
2. Starts the full agent runtime (same code as today's `GatewayRuntime`, unchanged).
3. Writes a single JSON line to stdout:
   ```json
   {"kind": "ready", "name": "researcher", "port": 43829, "pid": 12345, "started_at": "2026-04-18T12:34:56Z"}
   ```
4. Continues to serve HTTP + WS on loopback until shut down.

The child **never** binds a non-loopback address. The child **never** opens a tunnel. Auth is dropped on the loopback listener since the hub is the trust boundary (a local attacker can read the filesystem anyway).

### Process grouping

The hub spawns children in its own process group (Linux/macOS) or job object (Windows). When the hub dies for any reason including SIGKILL, children are killed too. No orphan adoption, no PID file reclamation. Hub stability is the only availability story.

### Supervisor: startup

Agents with `autostart = true` are spawned serially at hub startup. This is simpler, keeps the log stream linear, and avoids bursts of provider initialization. If serial startup becomes painful (10+ agents, slow MCP init), parallelism can be added later with a small concurrency cap. Not in scope for v1.

Readiness is the JSON line on stdout. The hub waits up to `startup_timeout_secs` (default 30) for each agent to report ready. If an agent times out, it's marked `failed` and the hub proceeds with the rest.

### Supervisor: health monitoring

Once an agent is ready, the hub periodically (every 10s) sends `GET /api/health` to its loopback port. Three consecutive failures → restart per the crash policy below.

### Supervisor: crash policy

- Restart immediately on first unexpected exit.
- Retry with backoff: `30s`, `2m`, `10m`.
- Three retries total. After the third, mark the agent `crash-looping` and stop trying.
- Reset the retry counter if the agent has been healthy for 5 minutes.
- `crash-looping` state persists until the user explicitly restarts via CLI or UI.

The captured stderr from the most recent exit is kept in memory (last 16 KiB) and surfaced through `/api/hub/agents/<name>/stderr` for crash diagnostics.

## Hub ↔ agent protocol

The hub is a reverse proxy. All HTTP and WS traffic that the web UI and CLI clients send to `/agents/<name>/...` is proxied transparently to the child's loopback port at the corresponding path. The protocol spoken between hub and agent is exactly the protocol the agent already speaks today.

**Paths:**

- `/agents/<name>/ws` → `http://127.0.0.1:<child-port>/ws` (WS upgrade)
- `/agents/<name>/api/...` → `http://127.0.0.1:<child-port>/api/...`
- `/agents/<name>/files/<id>` → `http://127.0.0.1:<child-port>/files/<id>` (attachment serving)

**Out-of-band:**

- Agent → hub: the readiness JSON line on stdout at startup.
- Hub → agent: signals (SIGTERM for graceful shutdown).

No custom protocol, no shared memory, no IPC socket. If we later want lower-latency IPC, we can add it without changing the agent's API surface.

## Config split

Two config files, distinct roles.

### `~/.residuum/config.toml` — hub-level

```toml
[gateway]
bind = "127.0.0.1"
port = 7700

[hub]
timezone = "America/Chicago"          # node-level TZ, passed to children via TZ env var
shutdown_grace_secs = 10
startup_timeout_secs = 30

[cloud]                                # node-level tunnel; replaces all per-agent tunnels
slug = "bearf:residuum-home"
relay_url = "wss://relay.residuum.app"
token = "secret:relay_token"

[tracing]
log_level = "info"

[update]
check_interval_hours = 24
```

Hub owns: bind/port, timezone, tunnel, update checks, hub-level tracing, supervisor knobs.

### `~/.residuum/agent_registry/<name>/config.toml` — agent-level

```toml
workspace_dir = "/home/u/.residuum/agent_registry/researcher/workspace"
autostart = true
timezone = "America/Chicago"           # mirrored from hub at creation time; env var overrides at runtime

[pulse]
# ...

[memory]
# ...

[observer]
# ...

[reflector]
# ...

[skills]
# ...

[projects]
# ...

[integrations.discord]
# ...

[integrations.telegram]
# ...

[tracing]
log_level = "debug"                    # optional per-agent override
```

Agent owns: workspace path, autostart flag, pulse/memory/observer/reflector/skills/projects config, integrations (Discord/Telegram), optional per-agent tracing override.

**Removed from agent config at cutover:**

- `[gateway]` section (agents always bind `127.0.0.1:0`).
- `[cloud]` section (hub owns the tunnel).
- `[update]` section (hub checks).

### Timezone propagation

Hub config has the canonical `timezone`. When spawning an agent, the hub sets the `TZ` environment variable for the child. The agent reads `TZ` first; if unset (e.g., hand-run worker), it falls back to `timezone` in its own config. On agent creation, the hub populates the agent's `timezone` from hub config so the two stay consistent and the agent can still function if `TZ` is ever missing from the spawn.

## CLI surface

### New

- `residuum serve` — starts the hub. Flags: `--foreground`, `--config <path>`.

### Changed

- `residuum agent create <name>` — talks to the running hub's `POST /api/hub/agents`. **Requires hub to be running.**
- `residuum agent list` — `GET /api/hub/agents`. **Requires hub to be running.**
- `residuum agent info <name>` — `GET /api/hub/agents/<name>`. **Requires hub to be running.**
- `residuum agent delete <name>` — `DELETE /api/hub/agents/<name>`. Agent must be stopped. **Requires hub to be running.**
- `residuum agent start <name>` — `POST /api/hub/agents/<name>/start`.
- `residuum agent stop <name>` — `POST /api/hub/agents/<name>/stop`.
- `residuum agent restart <name>` — `POST /api/hub/agents/<name>/restart`.
- `residuum connect` — interactive agent picker listing running agents from the hub; `--agent <name>` skips.
- `residuum logs [--agent <name>]` — proxies through the hub.
- `residuum stop [--agent <name>]` — with no flag, stops the hub (and therefore all agents). With `--agent`, stops a single child via the hub API.

### Removed

- `residuum serve --agent <name>` — no longer valid.
- `residuum connect --port <port>` — ports are not user-facing.

### Hidden

- `residuum __agent-worker --name <n> --workspace <path> --config <path>` — supervisor-only.

## Hub HTTP API

All hub-level endpoints live under `/api/hub/` to avoid collision with per-agent `/agents/<name>/api/`.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/api/hub/agents` | List agents with runtime status. |
| `POST` | `/api/hub/agents` | Create agent. Body: name, timezone (optional override), initial config. |
| `GET` | `/api/hub/agents/<name>` | Agent details (config path, workspace path, status, pid, port, uptime, last error). |
| `DELETE` | `/api/hub/agents/<name>` | Delete agent. Errors if running. |
| `POST` | `/api/hub/agents/<name>/start` | Spawn agent. |
| `POST` | `/api/hub/agents/<name>/stop` | SIGTERM + grace period. |
| `POST` | `/api/hub/agents/<name>/restart` | Stop then start. |
| `GET` | `/api/hub/agents/<name>/stderr` | Last 16 KiB of captured stderr from most recent exit. |
| `GET` | `/api/hub/status` | Hub uptime, version, tunnel status, child summary. |
| `POST` | `/api/hub/migrate` | Run legacy-layout migration. Idempotent after success. |
| `POST` | `/api/hub/shutdown` | Graceful hub shutdown. |

Per-agent endpoints under `/agents/<name>/...` are the existing agent API, proxied.

## Web UI

Current Svelte 5 SPA lives at `residuum/web/`. Changes:

- **Agent switcher.** Persistent UI element (top-of-app dropdown) listing running agents plus a status dot. Selecting one sets the active agent and all per-agent API/WS calls route through `/agents/<name>/...`. Selection is stored in local storage.
- **Path rewrites.** Every current call that hits `/api/...` and `/ws` becomes `/agents/<selected-agent>/api/...` and `/agents/<selected-agent>/ws`. Implemented via a client-side base-URL helper.
- **Node view.** A new top-level page showing hub status, list of agents with status, create/delete/start/stop/restart controls, autostart toggles, crash diagnostics (stderr tail for failed agents), tunnel status. This is the "node management" surface.
- **Setup wizard.** Moves to the hub. First-run flow creates the hub config and the first agent in one pass.
- **Deep links.** URLs become `/app/agents/<name>/chat`, `/app/agents/<name>/memory`, etc., for stable agent-scoped linking. `/app/node` is the node view.

A dedicated logs UI is out of scope for this refactor (per decision). Logs are accessed via CLI (`residuum logs`) which proxies through the hub.

## Tunnel model

The hub owns the one relay connection for the node. Per-agent `[cloud]` config is removed.

- Hub registers with the relay using its node slug.
- Remote traffic arriving through the relay hits the hub's HTTP surface, which routes `/agents/<name>/...` the same way as local traffic.
- Migration removes any per-agent `[cloud]` sections; a warning is logged listing what was dropped.

The relay sees one instance per node. Node switching on the relay side is a pure rename/reskin of the existing agent-switching UI — relay protocol is unchanged.

## File registry and attachments

File registry stays per-agent. The hub's reverse proxy forwards multipart uploads and download requests transparently. When an agent is deleted, its files are deleted with it. No cross-agent attachment sharing.

## Logs and tracing

- Each agent continues writing its own log files to its own `logs/` directory (unchanged behavior).
- The hub captures each child's stdout/stderr and writes them to `~/.residuum/logs/agent-<name>.stderr.log` for crash diagnostics only. Last 16 KiB is also kept in memory for the `/stderr` API.
- The hub writes its own log to `~/.residuum/logs/hub.<date>.log`.
- OTEL export stays per-agent. If aggregation is ever wanted, agents' exports can be pointed at the same collector endpoint; they'll be tagged by resource attributes.

## Migration

### One-shot legacy migration

On the first `residuum serve` after update, the hub detects legacy layout:

- `~/.residuum/config.toml` contains `[gateway]` with a port AND there is a workspace at `~/.residuum/workspace/` AND there is no `~/.residuum/config.toml`'s `[hub]` section.

If detected, the hub enters an interactive migration flow (or surfaces it in the setup wizard if running headless):

1. Prompt: "What name should we give your existing agent?" (default suggestion: `main`).
2. Create `~/.residuum/agent_registry/<name>/` and move `~/.residuum/workspace/` → `~/.residuum/agent_registry/<name>/workspace/`.
3. Split `~/.residuum/config.toml`:
   - Hub-level keys (`[gateway]`, `[cloud]`, `[update]`, `[tracing]`) stay in `~/.residuum/config.toml`.
   - Agent-level keys are moved to `~/.residuum/agent_registry/<name>/config.toml`.
   - A new `[hub]` section is added with `timezone` copied from the old top-level.
   - `autostart = true` is set on the new agent.
4. Per-agent `[cloud]` (if any) is dropped; a warning lists what was removed.
5. Write `~/.residuum/.migrated` sentinel to prevent re-prompts.

Migration is bulletproof (atomic rename per directory, config written to temp + rename). A failure rolls back by moving files back to their original locations. Pre-flight check: refuse migration if any `residuum serve --agent <name>` process is running.

### Documentation

A migration guide lives at `docs/migration-hub-architecture.md` (to be written with the cutover release). Existing `docs/multi-agent-setup.md` is rewritten to reflect the new model; its current form is preserved in the `docs/` directory as a historical record per `docs/CLAUDE.md` convention.

## Phasing

Per earlier discussion, the refactor is additive until a single cutover release.

### Phase 1 — Additive build

Runs on a long-lived feature branch. No release until phase 3.

- New `residuum/src/hub/` module: HTTP server, reverse proxy, supervisor, lifecycle API.
- New hidden subcommand `residuum __agent-worker` that runs the existing `GatewayRuntime` bound to `127.0.0.1:0` and emits the readiness JSON line.
- New `residuum/src/hub/config.rs` with the hub-only config schema.
- New web UI agent switcher and node view pages. Existing agent-scoped UI paths remain functional for hub-routed traffic.
- New `/api/hub/*` endpoints.
- Existing `residuum serve --agent <name>` path stays working during this phase. Existing gateway module untouched.
- Integration test: boot hub + 2 agents, exercise chat, memory, file upload, agent switch.

### Phase 2 — Parity verification

- Dogfood locally. Run hub mode for real work for at least a week.
- Identify and fix any feature-parity gaps.
- Final decision point: cutover, or iterate further.

### Phase 3 — Cutover (single breaking release)

All in one release with clear notes:

- `residuum serve` becomes the hub. `--agent` is removed from `serve`.
- Standalone agent mode deleted. `residuum serve --agent <name>` errors with a migration hint.
- Legacy layout migration prompt runs on first post-update serve.
- Per-agent tunnel (`[cloud]` in agent config) is dropped.
- Agent binary stripped: web UI assets, setup wizard, `gateway/file_server.rs` external routing (file serving stays, just no external binding), tunnel code, update checker.
- `GatewayRuntime` refactored to `AgentRuntime`: remove HTTP-surface concerns (or leave as-is if it's not worth touching — decision deferred to implementation).
- `docs/multi-agent-setup.md` rewritten. `docs/systems-usage/` updated. Migration guide written.
- CalVer release tag.

## Testing strategy

- **Unit tests** for the supervisor: spawn/exit/restart/backoff/process-group kill. Run against a fake worker binary that can be instructed to exit with various signals and delays.
- **Integration tests** for the hub: boot hub + N agents, exercise lifecycle API, verify proxy behavior (HTTP + WS upgrade + multipart), verify crash-looping detection, verify graceful shutdown kills children within the grace window.
- **Migration tests**: golden fixtures of legacy layouts, verify post-migration filesystem and config state; verify rollback on injected failure.
- **End-to-end test**: boot hub, create agent via CLI, send chat through the web UI WS path, verify response, delete agent, verify cleanup.

Existing agent-level tests are unaffected — the agent runtime is unchanged.

## Open questions / deferred items

- **Whether to refactor `GatewayRuntime` to `AgentRuntime` at cutover.** Mechanical, large diff, but not strictly required for correctness — the agent's HTTP surface works fine on loopback as-is. Deferred to implementation judgement.
- **Log aggregation UI.** Out of scope per decision. Future work if user feedback demands it.
- **Parallel agent startup with concurrency cap.** Out of scope per decision. Add if serial startup becomes painful.
- **Parity feature flags.** Not planned. Additive-then-cutover avoids needing flags.
- **Windows service integration.** The user's platform targets include Windows x86_64, but supervisor/process-group semantics differ (job objects vs. process groups). First-pass implementation should target Unix; Windows support added in a follow-up.
- **Resource pooling** (shared HTTP client, shared embedding model across agents). Deferred until resource cost is measured.

## Decisions log

| # | Decision | Date |
|---|---|---|
| 1 | Hub↔agent transport: loopback HTTP + WS, reuse existing protocol | 2026-04-18 |
| 2 | No standalone agent mode post-cutover | 2026-04-18 |
| 3 | Default-agent migration prompts on first post-update serve | 2026-04-18 |
| 4 | New agents default to `autostart = true` | 2026-04-18 |
| 5 | Hub owns the tunnel; per-agent `[cloud]` removed at cutover | 2026-04-18 |
| 6 | Relay protocol is unchanged (slug-opaque) | 2026-04-18 |
| 7 | Hub death kills all children; no orphan adoption | 2026-04-18 |
| 8 | `residuum connect` opens an interactive picker; `--agent` skips | 2026-04-18 |
| 9 | Timezone is hub-level; propagated to children via `TZ` env var + mirrored into agent config at creation | 2026-04-18 |
| 10 | Agent subcommands require a running hub (hub-required, not dual-mode) | 2026-04-18 |
| 11 | File registry stays per-agent; hub proxies transparently | 2026-04-18 |
| 12 | Startup fan-out is serial; parallel is a future optimization | 2026-04-18 |
| 13 | Crash policy: 3 retries with 30s/2m/10m backoff; healthy-for-5min resets counter | 2026-04-18 |
| 14 | Dedicated logs UI is out of scope for this refactor | 2026-04-18 |
| 15 | Agent worker drops auth on loopback; hub is the trust boundary | 2026-04-18 |
| 16 | Phasing: additive build → dogfood → single breaking cutover release | 2026-04-18 |
