# Config Files

Residuum's global settings live in two files outside the workspace directory, both in the config directory (`~/.residuum/` by default): `config.toml` (everything except providers/models) and `providers.toml` (provider credentials and `[models]` role assignments). Per-system files inside the workspace's `config/` directory (`mcp.json`, `channels.toml`, `agent-card.json`, `a2a.json`) hold narrower, system-specific settings — see each system's own doc.

## Who can write what

| File | Writable by the agent? |
|------|------------------------|
| `config.toml`, `providers.toml` | Yes — the file tools (`write_file`, `edit_file`) allow it, same as any other file. |
| `config.example.toml`, `providers.example.toml` | No. `PathPolicy` (`src/tools/path_policy.rs`) refuses every write. |
| `config/mcp.json`, `config/channels.toml` | Yes, unless `agent.modify_mcp`/`agent.modify_channels` is turned off in `config.toml`. |
| Credential stores (`secrets.toml.enc`, `agent-keys.toml.enc`, `a2a-keys.toml`, plus their key/lock files) | No, always. |

The `.example.toml` files are reference templates, not user config: `config::bootstrap::bootstrap_at` regenerates both from the binary's compiled-in defaults on every startup, so a write to either would appear to succeed and then be silently overwritten at the next restart. `PathPolicy::check_write` gives a write refusal for one of them a distinct message naming this and pointing at the writable file instead of the generic "user-managed configuration" refusal every other blocked path gets.

## How the agent edits config.toml / providers.toml

The bundled `residuum-system` skill's [config reference](../../assets/bundled-skills/residuum-system/references/config.md) is the agent-facing guidance: which file holds a given setting, editing with a targeted diff rather than a full rewrite (both files carry comments and commented-out examples), matching the file's existing style, and using a `secret:<name>` or `${ENV_VAR}` reference rather than a plaintext credential when the field already holds one or the user has a matching secret stored.

## Reload

A background poller (`src/gateway/watcher.rs`) watches `config.toml`/`providers.toml` by mtime and signals a reload on change, regardless of what wrote the file — the web UI's Settings form, the Raw tab, or the agent's own file tools all take the same path. `handle_root_reload` (`src/gateway/reload.rs`) diffs old vs. new config, rebuilds the changed subsystems in place, and publishes a `NoticeEvent` on the system notification channel: `"configuration reloaded: {summary}"` on success, or `"config reload failed (keeping current config): {err}"` on a parse or validation failure — the previous config keeps running until the file is fixed.

That notice reaches the user's interfaces (the web UI, Discord, Teams, Telegram) as a system notification; it is **not** injected into the agent's own conversation or turn transcript, so the agent has no tool-level way to confirm a reload it triggered. This is why the config reference above tells the agent to double-check the TOML is well-formed before writing, and to tell the user what changed and to expect the change to apply automatically — the user, not the agent, sees the reload notice if something goes wrong.
