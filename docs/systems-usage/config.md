# Config Files

Residuum's global settings live in two files outside the workspace directory, both in the config directory (`~/.residuum/` by default): `config.toml` (everything except providers/models) and `providers.toml` (provider credentials and `[models]` role assignments). Per-system files inside the workspace's `config/` directory (`mcp.json`, `channels.toml`, `agent-card.json`, `a2a.json`) hold narrower, system-specific settings — see each system's own doc.

## Who can write what

| File | Writable by the agent? |
|------|------------------------|
| `config.toml`, `providers.toml` | Yes — the file tools (`write_file`, `edit_file`) allow it, same as any other file. |
| `config.example.toml`, `providers.example.toml` | No. `PathPolicy` (`src/tools/path_policy.rs`) refuses every write. |
| `config/mcp.json`, `config/channels.toml` | Yes, unless `agent.modify_mcp`/`agent.modify_channels` is turned off in `config.toml`. |
| `config/a2a.json`, `HEARTBEAT.yml` | Yes, same as any other workspace file — no ability gate. |
| Credential stores (`secrets.toml.enc`, `agent-keys.toml.enc`, `a2a-keys.toml`, plus their key/lock files) | No, always. |

The `.example.toml` files are reference templates, not user config: `config::bootstrap::bootstrap_at` regenerates both from the binary's compiled-in defaults on every startup, so a write to either would appear to succeed and then be silently overwritten at the next restart. `PathPolicy::check_write` gives a write refusal for one of them a distinct message naming this and pointing at the writable file instead of the generic "user-managed configuration" refusal every other blocked path gets.

## How the agent edits config.toml / providers.toml

The bundled `residuum-system` skill's [config reference](../../assets/bundled-skills/residuum-system/references/config.md) is the agent-facing guidance: which file holds a given setting, editing with a targeted diff rather than a full rewrite (both files carry comments and commented-out examples), matching the file's existing style, and using a `secret:<name>` or `${ENV_VAR}` reference rather than a plaintext credential when the field already holds one or the user has a matching secret stored.

## Reload

Two background pollers (`src/gateway/watcher.rs`) watch these files by mtime and signal a reload on change, regardless of what wrote the file — the web UI's Settings form, the Raw tab, a manual edit, or the agent's own file tools all take the same path. `spawn_root_config_watcher` covers `config.toml`/`providers.toml` and signals `ReloadSignal::Root`; `spawn_workspace_watcher` covers `mcp.json`/`channels.toml`/`agent-card.json`/`a2a.json` and signals `ReloadSignal::Workspace`. `handle_root_reload` (`src/gateway/reload.rs`) diffs old vs. new root config, rebuilds the changed subsystems in place, and publishes a `NoticeEvent` on the system notification channel: `"configuration reloaded: {summary}"` on success, or `"config reload failed (keeping current config): {err}"` on a parse or validation failure — the previous config keeps running until the file is fixed. `handle_workspace_reload` (`src/gateway/event_loop/run_loop.rs`) does the equivalent for the workspace files, one subsystem at a time, and publishes its own notice per subsystem plus a final `"workspace configuration reloaded"` notice. `HEARTBEAT.yml` reloads on a different path entirely — the pulse scheduler re-parses it on every scheduler tick (see `heartbeats.md`), not through `ReloadSignal` at all.

**When the reload was caused by the agent's own write**, its outcome also reaches the agent directly. `ConfigWriteWatch` (`src/tools/config_reload_tracker.rs`), wired only into main's `write_file`/`edit_file` — never a session's — recognizes a successful write to one of the six paths above and marks it on a `SharedConfigReloadTracker` shared with the gateway event loop. The first reload of the matching kind (root, workspace, or the pulse scheduler's next tick for `HEARTBEAT.yml`) consumes that mark and, in addition to the usual notice on the system notification channel, calls `Agent::inject_system_message` with the same outcome text — so it shows up as a system note in the agent's own transcript on its next step, the same mechanism used for other system-originated notes. A mark older than 30 seconds is dropped rather than attributed to a later, unrelated reload (e.g. a manual edit made shortly after). This is why the config reference above no longer tells the agent to just hope for the best after writing — it can check the note that follows and tell the user if something went wrong.
