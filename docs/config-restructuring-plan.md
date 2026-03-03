# Config Restructuring Plan

**Issue**: #17 (subsumes #3, #4, #10)
**Branch**: `refactor/config`
**Breaking change**: Yes — migration guide included with release

## Context

The current config is a single `~/.residuum/config.toml` containing all concerns: providers, models, channels, MCP servers, memory settings, notifications, background tasks. This makes it hard to add new provider capabilities, configure integrations, or let the agent manage its own config at runtime.

The current reload mechanism (`GatewayExit::Reload`) tears down the entire gateway — killing WebSocket connections, orphaning Discord/Telegram adapter tasks, disconnecting all MCP servers — and rebuilds everything from scratch. This causes visible disruption to users.

This plan restructures config into multiple files with clear ownership, introduces a file watcher for hot-reloading dynamic config, and replaces the full-restart reload with in-place subsystem updates.

## Target File Layout

```
~/.residuum/
├── config.toml              # general settings, interface tokens, agent abilities
├── providers.toml           # provider definitions + model role assignments
├── secrets.key
├── secrets.toml.enc
└── workspace/
    └── config/
        ├── mcp.json         # MCP servers (Claude Code/Desktop compatible)
        └── channels.toml    # notification channels (ntfy, webhooks)
```

### Ownership Model

| File | Who edits | When it changes | Reload behavior |
|------|-----------|-----------------|-----------------|
| `config.toml` | User | Rare (setup, manual tuning) | In-place subsystem update |
| `providers.toml` | User | Rare (add/change providers) | Swap provider chains between turns |
| `mcp.json` | User or agent | Runtime (agent adds servers) | Hot-reload via file watcher |
| `channels.toml` | User or agent | Runtime (agent adds channels) | Hot-reload via file watcher |

### Design Decisions

- **MCP as JSON, not TOML**: Users can drag-and-drop existing Claude Code / Claude Desktop configs. Format: `{ "mcpServers": { "name": { "command": "...", "args": [...], "env": {...} } } }`.
- **No `secret:` resolution for MCP**: Too many variations in MCP config style. Plain values only.
- **`secret:` resolution stays for `providers.toml`**: API keys benefit from encryption.
- **No backward compatibility**: Breaking change. Migration guide ships with the release. The web UI will be broken until the separate web refactor lands; both ship in the same release.
- **Per-project MCP becomes name references**: `PROJECT.md` frontmatter changes from inline `McpServerEntry` objects to `Vec<String>` of server names resolved against `mcp.json`.
- **Agent abilities**: New `[agent]` section in `config.toml` with `modify_mcp` and `modify_channels` booleans (default `true`). Gates whether the agent's tools can write to workspace config files.

---

## Phase 1: Shared Gateway Core

**Goal**: Replace the tear-down-and-rebuild reload with a long-lived gateway core that supports in-place subsystem updates. This is the architectural foundation everything else builds on.

**PR**: `refactor/config` → `dev` — "feat: shared gateway core with in-place reload"

### Problem

Currently `run_gateway()` creates all channels (`inbound_tx`, `broadcast_tx`, `reload_sender`) locally, passes clones to interface adapters via `tokio::spawn`, then returns `GatewayExit::Reload` on config change. The caller in `main.rs` loops and calls `run_gateway()` again with fresh config, creating new channels. The spawned Discord/Telegram tasks still hold references to the old, dropped channels — their messages go nowhere.

### Design

Introduce a `GatewayCore` that owns the long-lived shared channels and persists across config reloads:

```rust
/// Long-lived state that survives config reloads.
struct GatewayCore {
    // Channels — created once, never recreated
    inbound_tx: mpsc::Sender<RoutedMessage>,
    inbound_rx: mpsc::Receiver<RoutedMessage>,
    broadcast_tx: broadcast::Sender<ServerMessage>,
    reload_tx: watch::Sender<ReloadSignal>,
    reload_rx: watch::Receiver<ReloadSignal>,
    command_tx: mpsc::Sender<ServerCommand>,
    command_rx: mpsc::Receiver<ServerCommand>,
    sigterm: tokio::signal::unix::Signal,
}
```

The reload signal changes from `watch::Sender<bool>` to a typed enum:

```rust
enum ReloadSignal {
    None,
    /// Root config changed — re-read config.toml + providers.toml, diff and
    /// update all affected subsystems in place (swap providers, update
    /// thresholds, restart adapters if tokens changed). No connections dropped.
    Root,
    /// Workspace config changed — only re-read mcp.json and/or channels.toml,
    /// reconcile MCP servers and reload notification channels. Cheapest reload.
    Workspace,
}
```

Both variants are **graceful in-place updates**. The old tear-down-and-rebuild behavior (`GatewayExit::Reload`) is eliminated entirely.

### Changes

**`src/gateway/server/mod.rs`**:
- Extract `GatewayCore` from `GatewayRuntime`. Core is created once in `run_gateway()` and persists.
- `GatewayRuntime` keeps mutable subsystem references but borrows channels from core.
- Replace `GatewayExit::Reload` path: instead of returning from the event loop, the reload arm calls a reconfigure function that updates subsystems in place.
- Remove `rt.server_handle.abort()` from reload path. The HTTP server stays running.
- On reload: swap the axum router's state to point to updated config rather than killing the server.

**`src/main.rs`**:
- Simplify the `run_serve_foreground_inner()` loop. The gateway no longer returns `Reload` — it handles reloads internally. The loop only handles fatal errors and degraded mode entry.
- `backup_config()` / `rollback_config()` stay but only for root config files.

**Interface adapters** (`src/channels/discord/mod.rs`, `src/channels/telegram/mod.rs`):
- Adapters receive `Arc`-wrapped channel senders from `GatewayCore` instead of owned copies.
- Add a `shutdown_rx: watch::Receiver<()>` so the gateway can signal adapters to stop gracefully (e.g., when a token changes and the adapter needs to reconnect).
- On reconfigure: if the interface token changed, signal the old adapter to shut down and spawn a new one. If unchanged, the adapter continues running with the same shared channels.

**`src/gateway/server/startup.rs`**:
- Split `initialize()` into granular functions that can be called independently:
  - `init_providers(cfg) -> ProviderChains`
  - `init_memory(cfg, providers) -> MemoryComponents`
  - `init_mcp(servers) -> SharedMcpRegistry` (already exists as `reconcile_and_connect`)
  - `init_notifications(channels) -> NotificationRouter`
  - `init_tools(subsystems) -> ToolRegistry`
  - `init_agent(providers, tools, messages) -> Agent`
- Full init calls all of these in sequence (first boot). Reload calls only the relevant subset.

**`src/agent/mod.rs`**:
- Agent needs a method to swap its model provider between turns: `swap_provider(&mut self, provider: Box<dyn ModelProvider>)`.
- This must only be called when no turn is in progress (enforced by the event loop's sequential structure — the agent is `&mut` borrowed during turns, so swaps can only happen between turns).

**`src/gateway/server/web.rs`**:
- The axum server handle is no longer aborted on reload. The server stays running. Config API state (`ConfigApiState`) may need to be `Arc<RwLock<>>` so it can be updated when config_dir changes (unlikely but possible).

**Config backup/rollback** (moves from `main.rs` into the gateway):
- Before each root config reload attempt, back up `config.toml` and `providers.toml`.
- If reload fails (bad provider key, invalid model spec, etc.), roll back files and keep the running subsystem state unchanged. Surface the error via broadcast as a `ServerMessage::Notice`.
- Workspace config (`mcp.json`, `channels.toml`) does NOT need backup — failed loads keep the previous state in memory without touching the files.
- The web API's validate-before-write pattern extends to `providers.toml`.

**`src/notify/router.rs`**:
- Wrap `external_channels` in `Arc<RwLock<HashMap<String, Box<dyn NotificationChannel>>>>` so channels can be added/removed at runtime.
- Add `reload_channels(&self, new_channels: HashMap<...>)` method.

### Non-Issues (Verified)

- **Tool registry and MCP hot-reload**: MCP tools live in `McpRegistry` (separate from `ToolRegistry`). The agent dynamically merges built-in + MCP tool definitions every turn via `mcp_registry.read().tool_definitions()`. When servers are added/removed via `reconcile_and_connect()`, the next turn automatically sees updated tools. No changes needed.

### Degraded Mode

Degraded mode is dropped. With in-place reload, a failed config change just keeps the running state and surfaces the error. For first-boot failures where no valid config has ever loaded, the setup server handles it (existing `run_setup_server()` flow). The degraded mode module (`src/gateway/server/degraded.rs`) is removed in Phase 6.

### Tests

- Unit test: `GatewayCore` channels survive simulated reload (send before and after).
- Unit test: `NotificationRouter` channel hot-swap (add, remove, replace).
- Unit test: Agent `swap_provider()` between turns.
- Unit test: backup/rollback within the gateway reload handler.
- Integration test: reload signal triggers subsystem update without dropping broadcast subscribers.

### Considerations

- **Race condition during provider swap**: The event loop is single-threaded (`&mut GatewayRuntime`), so the agent can only be swapped between turns. No lock needed — just don't swap during `process_message()` / `run_wake_turn()`.
- **HTTP server lifecycle**: Keeping the axum server alive across reloads means the `TcpListener` stays bound. If `gateway.bind` or `gateway.port` changes, only the HTTP server is restarted — not the gateway. Already-upgraded WebSocket connections are independent tasks communicating via shared channels, so they survive an HTTP server restart. The flow: gracefully shut down old server → bind new `TcpListener` → spawn new server task. Existing WS clients stay connected on the old port; new clients connect on the new port. Detect bind/port changes by comparing old vs new `GatewayConfig`.
- **Degraded mode**: Still needed for fatal init errors (bad provider config, missing API keys). But it's now entered without killing the HTTP server or channels.

---

## Phase 2: Config File Split

**Goal**: Split the monolithic `config.toml` into `config.toml` + `providers.toml`. Remove MCP and notification channels from the TOML config pipeline. Add `[agent]` section.

**PR**: `refactor/config` → `dev` — "feat: split config into config.toml and providers.toml"

### Changes

**`src/config/deserialize.rs`**:
- Remove `McpConfigFile`, `McpServerConfigEntry` structs entirely.
- Remove `NotificationsConfigFile`, `ChannelConfigEntry` structs.
- Remove `mcp` and `notifications` fields from `ConfigFile`.
- Add `AgentConfigFile` struct: `modify_mcp: Option<bool>`, `modify_channels: Option<bool>`.
- Add `agent` field to `ConfigFile`.
- Create new `ProvidersFile` struct with `providers` and `models` fields (extracted from `ConfigFile`).
- Remove `providers` and `models` fields from `ConfigFile`.

**`src/config/resolve.rs`**:
- Remove `resolve_mcp_config()` function and all MCP resolution logic.
- Remove notification channel resolution from `from_file_and_env()`.
- Add `resolve_agent_config()` for the new `[agent]` section.
- Load `providers.toml` alongside `config.toml` in `from_file_and_env()`.
- Provider and model resolution reads from `ProvidersFile` instead of `ConfigFile`.

**`src/config/types.rs`**:
- Remove `McpConfig` struct (MCP config will be loaded separately from JSON).
- Remove `NotificationsConfig`, `ExternalChannelConfig`, `ExternalChannelKind` from this file.
  - Note: these types are still needed by the notification system, but they'll be populated from the workspace `channels.toml` loader, not from the root config pipeline. Move them to `src/notify/` or a shared types module.
- Add `AgentAbilitiesConfig` struct: `modify_mcp: bool`, `modify_channels: bool`.
- Remove `mcp` and `notifications` fields from `Config`.
- Add `agent: AgentAbilitiesConfig` field to `Config`.

**`src/config/mod.rs`**:
- `Config::load_at()` reads both `config.toml` and `providers.toml` from the same directory.
- `Config::validate_toml()` needs a companion `validate_providers_toml()` or a combined validator.
- Update `Debug` impl to remove mcp/notifications, add agent.

**`src/config/bootstrap.rs`**:
- Write `providers.toml` alongside `config.toml` on first run.
- Create `workspace/config/` directory with empty `mcp.json` (`{ "mcpServers": {} }`) and empty `channels.toml`.
- Update `EXAMPLE_CONFIG` — split into `config.example.toml` and `providers.example.toml`.
- Add `mcp.example.json` and `channels.example.toml` to assets.

**`src/config/wizard.rs`**:
- `write_config()` writes provider/model info to `providers.toml`, everything else to `config.toml`.

**`src/config/constants.rs`**:
- Add `DEFAULT_AGENT_MODIFY_MCP = true`, `DEFAULT_AGENT_MODIFY_CHANNELS = true`.

**`assets/`**:
- Split `config.example.toml` into `config.example.toml` (without providers/models/mcp/notifications) and `providers.example.toml` (providers + models only).
- Add `mcp.example.json` and `channels.example.toml`.

### Tests

- All existing config tests updated for the split (two-file loading).
- New test: `validate_providers_toml_rejects_invalid_model_format()`.
- New test: `config_without_providers_file_uses_defaults()` (or errors — decide based on whether providers.toml is required).
- New test: `agent_abilities_default_to_true()`.
- Remove all MCP-related config tests (they move to the workspace loader in Phase 3).

---

## Phase 3: Workspace Config Loaders + Hot-Reload

**Goal**: Load MCP and notification channel config from the workspace config directory. Add a file watcher for hot-reloading these files.

**PR**: `refactor/config` → `dev` — "feat: workspace config loaders with file watcher"

### Changes

**New module: `src/workspace/config.rs`** (workspace concern, not root config pipeline):

MCP loader:
- `load_mcp_json(path: &Path) -> Result<Vec<McpServerEntry>>`
- Parses Claude Code compatible format: `{ "mcpServers": { "name": { "command": "...", "args": [...], "env": {...} } } }`
- Also supports optional `"transport": "stdio"|"http"` field (Residuum extension).
- No `secret:` resolution. Plain values only.
- Good error messages: "failed to parse mcp.json: expected object at mcpServers.filesystem.command"

Project MCP loader:
- `load_project_mcp(project_dir: &Path) -> Result<Vec<McpServerEntry>>`
- Loads `mcp.json` from the project directory (same Claude Code format as global).
- Returns empty list if file doesn't exist (project MCP is optional).

Channel loader:
- `load_channels_toml(path: &Path) -> Result<Vec<ExternalChannelConfig>>`
- Format mirrors the old `[notifications.channels]` TOML section but as a standalone file:
  ```toml
  [channels.my-ntfy]
  type = "ntfy"
  url = "https://ntfy.sh"
  topic = "my-alerts"

  [channels.my-webhook]
  type = "webhook"
  url = "https://example.com/hook"
  ```

**New module: `src/gateway/server/watcher.rs`**:
- File watcher on `workspace/config/` directory using `notify` crate (or `tokio::fs` polling as simpler alternative).
- On `mcp.json` change: send `ReloadSignal::Workspace` to reload channel.
- On `channels.toml` change: send `ReloadSignal::Workspace` to reload channel.
- Debounce: 500ms after last change (editors often write multiple times).

**`src/gateway/server/mod.rs`**:
- Event loop handles `ReloadSignal::Workspace` by calling only MCP reconciliation + channel reload (not full subsystem reinit).
- MCP: read `mcp.json`, call `mcp_registry.write().await.reconcile_and_connect(&new_servers).await`.
- Channels: read `channels.toml`, call `notification_router.reload_channels(new_channels)`.

**`src/gateway/server/startup.rs`**:
- `initialize()` loads MCP from `workspace/config/mcp.json` instead of from `Config.mcp`.
- `build_notification_router()` loads channels from `workspace/config/channels.toml` instead of from `Config.notifications`.
- Spawn the file watcher task.

**Agent abilities enforcement**:
- The agent's tools that write to `mcp.json` or `channels.toml` must check `cfg.agent.modify_mcp` / `cfg.agent.modify_channels` before writing. This gating happens in the tool implementation, not the watcher.

### Tests

- Unit test: `load_mcp_json` parses Claude Code format correctly.
- Unit test: `load_mcp_json` handles missing file (returns empty list).
- Unit test: `load_mcp_json` rejects malformed JSON with good error message.
- Unit test: `load_channels_toml` parses ntfy and webhook channels.
- Unit test: `load_channels_toml` handles missing file (returns empty list).
- Integration test: file change triggers MCP reconciliation.
- Integration test: file change triggers channel reload.

### Considerations

- **`notify` crate vs polling**: `notify` uses inotify on Linux and is event-driven. Polling is simpler but adds latency. Recommend `notify` with a debounce wrapper.
- **Atomic writes**: The agent (or user) should write to a temp file then rename, so the watcher doesn't see partial writes. Document this in the migration guide.
- **Error on watch**: If `mcp.json` is written with invalid JSON, log a warning and keep the previous config. Don't crash or enter degraded mode for workspace config errors.
- **Per-project `mcp.json`**: The watcher also covers the active project's directory. When the active project changes (activation/deactivation), the watcher adds/removes that project's directory from its watch set. This is a small addition since the watcher already exists — just one more path to watch, and only when a project is active.

---

## Phase 4: Per-Project MCP References

**Goal**: Change per-project MCP from inline definitions to name references resolved against `mcp.json`.

**PR**: `refactor/config` → `dev` — "feat: per-project MCP as name references"

### Changes

**`src/projects/types.rs`**:
- Change `ProjectFrontmatter.mcp_servers` from `Vec<McpServerEntry>` to `Vec<String>`.
- Update serde attributes (it's now a simple string list).
- Remove or reduce `McpServerEntry` usage in this file (the type itself stays, used by `mcp/registry.rs` and the workspace loader).

**`src/mcp/registry.rs`**:
- `activate_project()` signature changes: takes `Vec<String>` (server names) + a resolver function or reference to the loaded MCP config.
- Resolution: look up each name against **project-local `mcp.json` first, then global `mcp.json`**. Project-local definitions override global ones with the same name. If a name doesn't exist in either, return an error (not silent skip — no silent failures).
- `reconcile_and_connect()` stays the same — it still works with `McpServerEntry` values. The resolution from name → entry happens before calling it.

**`src/tools/projects.rs`** (or wherever project activation is called):
- Pass the current MCP config (from the workspace loader) alongside the project's server name list.
- Handle resolution errors: surface to the user via the agent's response.

**`src/gateway/server/startup.rs`**:
- On startup, resolve project MCP references against the loaded `mcp.json` entries.

### Tests

- Unit test: project frontmatter with `mcp_servers: ["filesystem", "git"]` parses correctly.
- Unit test: resolution succeeds when all names exist in mcp.json.
- Unit test: resolution fails with clear error when a name doesn't exist.
- Unit test: round-trip serialization of the new string list format.
- Update all existing project tests that use inline `McpServerEntry` objects.

### Considerations

- **Existing PROJECT.md files**: Any user who has PROJECT.md files with inline MCP definitions will need to update them. Document in migration guide.
- **Per-project `mcp.json`**: Projects can optionally have their own `mcp.json` alongside their `PROJECT.md`. When resolving server names, project-local definitions take precedence over global ones. This lets projects define project-specific servers without cluttering the global config. The workspace loader gains a `load_project_mcp(project_dir)` function that's called during project activation.
- **Hot-reload interaction**: If `mcp.json` (global or project-local) is updated and a referenced server definition changes, already-running servers are reconciled. The file watcher from Phase 3 covers the active project's directory in addition to `workspace/config/`.

---

## Phase 5: Graceful Root Config Reload

**Goal**: When `config.toml` or `providers.toml` changes, update subsystems in place without dropping connections or interrupting active conversations.

**PR**: `refactor/config` → `dev` — "feat: graceful in-place reload for root config"

### Changes

**`src/gateway/server/mod.rs`** (event loop reload arm):
- On `ReloadSignal::Root`:
  1. Load new `Config` from disk (both `config.toml` and `providers.toml`).
  2. Diff old config vs new config to determine what changed.
  3. Call granular update functions based on what changed:

| Config section | Update action |
|---------------|---------------|
| `[models]` / `[providers]` | Rebuild provider chains, call `agent.swap_provider()` |
| `[memory]` | Update observer/reflector thresholds in place |
| `[gateway]` bind/port | Rebind HTTP listener (only case that restarts the server) |
| `[discord]` / `[telegram]` | Signal adapter shutdown + respawn with new token |
| `[retry]` | Update retry config on provider chains |
| `[background]` | Update spawner config |
| `[pulse]` | Toggle pulse scheduler |
| `[agent]` | Update abilities (immediate, no restart) |
| `[skills]` | Re-scan skill directories |

**`src/gateway/server/startup.rs`**:
- The granular init functions from Phase 1 are used here for targeted updates.
- Add a `diff_config(old: &Config, new: &Config) -> ConfigDiff` function that returns which sections changed.

**`src/main.rs`**:
- The outer loop simplifies. `run_gateway()` no longer returns `GatewayExit::Reload` for config changes. It only returns on `Shutdown` or fatal error.
- Degraded mode entry: only on fatal initialization errors where no subsystem can start.
- Config backup/rollback: still needed. If a reload attempt fails (bad provider key, etc.), roll back and log the error without killing the gateway.

**Interface adapter lifecycle**:
- Each running adapter gets a `shutdown_rx: watch::Receiver<()>`.
- On token change: send shutdown signal, wait for adapter task to exit, spawn new adapter with new token and the same shared channels from `GatewayCore`.
- On token removal (e.g., user removes `[discord]`): send shutdown signal, don't respawn.
- On token addition: spawn new adapter.

### Tests

- Integration test: provider change triggers swap without dropping WebSocket connections.
- Integration test: gateway bind/port change triggers HTTP server restart.
- Integration test: Discord token change triggers adapter restart with same shared channels.
- Unit test: `diff_config()` correctly identifies changed sections.
- Unit test: failed reload rolls back without disruption.

### Considerations

- **Observer/Reflector threshold update**: These are owned values in the `Observer` and `Reflector` structs. They need setter methods or interior mutability to update without recreation.
- **BackgroundTaskSpawner update**: The spawner holds `Arc<SpawnContext>` which contains provider specs and model tiers. Updating this means swapping the inner `SpawnContext` via `ArcSwap` or similar.
- **Partial failure**: If provider chain rebuild fails (bad API key), the old provider should stay active. The error should be surfaced as a warning, not a crash.

---

## Phase 6: Cleanup + Migration Guide

**Goal**: Remove dead code, update all documentation, write migration guide.

**PR**: `refactor/config` → `dev` — "chore: cleanup and migration guide"

### Changes

**Dead code removal**:
- Remove `GatewayExit::Reload` variant (only `Shutdown` remains).
- Remove `src/gateway/server/degraded.rs` entirely — replaced by in-place error handling + setup server for first boot.
- Remove old MCP TOML deserialization structs (already done in Phase 2, verify nothing references them).
- Remove old notification channel TOML deserialization structs.
- Remove backup/rollback from `main.rs` (moved into gateway in Phase 1).
- Simplify `run_serve_foreground_inner()` — the outer loop only handles first-boot setup and fatal errors.
- Clean up any `#[allow(dead_code)]` attributes that were temporarily added.

**Example files** (`assets/`):
- Final versions of `config.example.toml`, `providers.example.toml`, `mcp.example.json`, `channels.example.toml`.
- Remove old `config.example.toml` that included everything.

**Documentation**:
- Update `CLAUDE.md` — config file references, build commands if any changed.
- Update `docs/residuum-design.md` — config architecture section.
- Update any other design docs that reference config loading.

**Migration guide** (`docs/migration/` or in release notes):
- Old format → new format mapping.
- Step-by-step: how to split your existing `config.toml`.
- MCP: how to convert `[mcp.servers]` TOML to `mcp.json`.
- Channels: how to move `[notifications.channels]` to `channels.toml`.
- Projects: how to update `PROJECT.md` frontmatter from inline MCP to name references.
- Note that the web UI will be updated in the same release.

### Tests

- Verify all tests pass with the final config layout.
- No test references old config structs or formats.

---

## Dependency Graph

```
Phase 1 (shared gateway core)
    │
    ├── Phase 2 (config file split)
    │       │
    │       └── Phase 3 (workspace loaders + hot-reload)
    │               │
    │               └── Phase 4 (per-project MCP references)
    │
    └── Phase 5 (graceful root config reload)
            │
            └── Phase 6 (cleanup + migration guide)
```

Phase 1 must land first — it's the foundation. After that, Phases 2→3→4 and Phase 5 can be developed in parallel (they touch different parts of the codebase), but Phase 5 logically builds on Phase 1's shared core. Phase 6 is last.

## Key Files Modified

| File | Phases |
|------|--------|
| `src/config/mod.rs` | 2 |
| `src/config/deserialize.rs` | 2 |
| `src/config/resolve.rs` | 2 |
| `src/config/types.rs` | 2 |
| `src/config/bootstrap.rs` | 2 |
| `src/config/wizard.rs` | 2 |
| `src/config/constants.rs` | 2 |
| `src/config/workspace.rs` (new) | 3 |
| `src/gateway/server/mod.rs` | 1, 3, 5 |
| `src/gateway/server/startup.rs` | 1, 3, 5 |
| `src/gateway/server/web.rs` | 1 |
| `src/gateway/server/watcher.rs` (new) | 3 |
| `src/gateway/server/degraded.rs` | 5 |
| `src/channels/discord/mod.rs` | 1, 5 |
| `src/channels/telegram/mod.rs` | 1, 5 |
| `src/agent/mod.rs` | 1 |
| `src/mcp/registry.rs` | 4 |
| `src/notify/router.rs` | 1 |
| `src/projects/types.rs` | 4 |
| `src/tools/projects.rs` | 4 |
| `src/main.rs` | 1, 5 |
| `assets/config.example.toml` | 2, 6 |
| `assets/providers.example.toml` (new) | 2, 6 |
| `assets/mcp.example.json` (new) | 3, 6 |
| `assets/channels.example.toml` (new) | 3, 6 |

## Verification

After each phase:
1. `cargo fmt` — formatting
2. `cargo clippy` — lint clean
3. `cargo test --quiet` — all tests pass
4. Manual smoke test: `residuum serve --foreground` boots successfully

After Phase 3+:
5. Edit `workspace/config/mcp.json` while gateway is running — verify hot-reload.
6. Edit `workspace/config/channels.toml` while gateway is running — verify hot-reload.

After Phase 5:
7. Edit `config.toml` while gateway is running — verify WebSocket connections survive.
8. Edit `providers.toml` while gateway is running — verify provider swap without disruption.
9. Send `/reload` from CLI — verify in-place update.
