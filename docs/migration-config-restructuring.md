# Migration Guide: Config Restructuring

This release restructures Residuum's configuration from a single monolithic `config.toml` into multiple purpose-specific files. This is a **breaking change** — existing configurations must be updated.

## New File Layout

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

## Step-by-Step Migration

### 1. Split providers and models out of config.toml

**Before** (in `config.toml`):
```toml
[providers.anthropic]
type = "anthropic"
api_key = "${ANTHROPIC_API_KEY}"

[providers.gemini]
type = "openai"
api_key = "${GEMINI_API_KEY}"
url = "https://generativelanguage.googleapis.com/v1beta/openai"

[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
observer = "gemini/gemini-2.5-flash"
```

**After**: Move these sections to a new `providers.toml` file alongside `config.toml`:

```toml
# ~/.residuum/providers.toml
[providers.anthropic]
type = "anthropic"
api_key = "${ANTHROPIC_API_KEY}"

[providers.gemini]
type = "openai"
api_key = "${GEMINI_API_KEY}"
url = "https://generativelanguage.googleapis.com/v1beta/openai"

[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"
observer = "gemini/gemini-2.5-flash"
```

Remove the `[providers.*]` and `[models]` sections from `config.toml`. Also move `[background.models]` to `providers.toml`:

```toml
# In providers.toml
[background.models]
small = "gemini/gemini-2.5-flash"
medium = "anthropic/claude-haiku-4-5"
```

### 2. Convert MCP servers from TOML to JSON

**Before** (in `config.toml`):
```toml
[mcp.servers.filesystem]
command = "mcp-server-filesystem"
args = ["/home/user/documents"]

[mcp.servers.git]
command = "mcp-server-git"
args = ["--repo", "/home/user/project"]
```

**After**: Create `~/.residuum/workspace/config/mcp.json` using Claude Code compatible format:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "mcp-server-filesystem",
      "args": ["/home/user/documents"]
    },
    "git": {
      "command": "mcp-server-git",
      "args": ["--repo", "/home/user/project"]
    }
  }
}
```

The JSON format is compatible with Claude Code and Claude Desktop. You can copy MCP configs between these tools directly.

Optional Residuum extension: add `"transport": "http"` for HTTP-based MCP servers (default is `"stdio"`).

Remove the `[mcp]` and `[mcp.servers.*]` sections from `config.toml`.

### 3. Move notification channels to channels.toml

**Before** (in `config.toml`):
```toml
[notifications.channels.ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "residuum"

[notifications.channels.ops_webhook]
type = "webhook"
url = "https://hooks.example.com/residuum"
method = "POST"
```

**After**: Create `~/.residuum/workspace/config/channels.toml`:

```toml
[channels.ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "residuum"

[channels.ops_webhook]
type = "webhook"
url = "https://hooks.example.com/residuum"
method = "POST"
```

Note the table prefix changes from `[notifications.channels.X]` to `[channels.X]`.

Remove the `[notifications]` section from `config.toml`.

### 4. Update PROJECT.md frontmatter (per-project MCP)

**Before** (inline MCP server definitions):
```yaml
---
name: my-project
mcp_servers:
  - command: mcp-server-filesystem
    args: ["/home/user/project"]
  - command: mcp-server-git
    args: ["--repo", "/home/user/project"]
---
```

**After** (name references resolved against `mcp.json`):
```yaml
---
name: my-project
mcp_servers:
  - filesystem
  - git
---
```

Server names are resolved first against a project-local `mcp.json` (if one exists alongside `PROJECT.md`), then against the global `~/.residuum/workspace/config/mcp.json`. Project-local definitions take precedence for the same name.

### 5. New `[agent]` section (optional)

A new optional section in `config.toml` gates the agent's ability to modify workspace config files:

```toml
[agent]
modify_mcp = true          # allow agent to add/remove MCP servers (default: true)
modify_channels = true     # allow agent to add/remove notification channels (default: true)
```

Both default to `true` when omitted — no action needed unless you want to restrict the agent.

## What Stays in config.toml

After migration, `config.toml` should contain only:

- `timezone`, `name`, `workspace_dir`, `timeout_secs`, `max_tokens`
- `[memory]` and `[memory.search]`
- `[pulse]`
- `[gateway]`
- `[discord]` and `[telegram]`
- `[skills]`
- `[webhook]`
- `[agent]`
- `[background]` (without `[background.models]`, which moves to `providers.toml`)
- `[retry]`

## Behavioral Changes

### In-place reload replaces restart

Configuration changes no longer tear down and restart the gateway. The gateway watches all config files and applies changes in place:

- **`config.toml` or `providers.toml` changes**: Subsystem diff determines what changed. Provider chains are swapped between turns, memory thresholds update immediately, adapter tokens trigger graceful reconnect.
- **`mcp.json` changes**: MCP servers are reconciled (started/stopped) without interrupting the agent.
- **`channels.toml` changes**: Notification channels are hot-swapped.

WebSocket connections survive all reload types.

### Degraded mode removed

The degraded mode (limited web UI served when config is invalid) has been removed. On startup:

- If config is invalid and this is the first boot, the setup wizard runs.
- If config is invalid on restart, the gateway attempts to roll back from the `.bak` backup files. If rollback succeeds, the gateway starts normally. If not, the process exits with a clear error message explaining what to fix.

At runtime, invalid config changes are rejected and the gateway continues with the previous configuration.

## Quick Checklist

- [ ] Create `providers.toml` with `[providers.*]`, `[models]`, and `[background.models]` from old config
- [ ] Create `workspace/config/mcp.json` from old `[mcp.servers.*]` sections
- [ ] Create `workspace/config/channels.toml` from old `[notifications.channels.*]` sections
- [ ] Remove `[providers.*]`, `[models]`, `[mcp.*]`, `[notifications.*]`, `[background.models]` from config.toml
- [ ] Update any `PROJECT.md` files: replace inline MCP definitions with server name strings
- [ ] Optionally add `[agent]` section to config.toml

## Notes

- The `secret:` prefix for encrypted API keys continues to work in `providers.toml` (not supported in `mcp.json`).
- The web UI will be updated in the same release to reflect the new config structure.
- Example files ship with the release: `config.example.toml`, `providers.example.toml`, `mcp.example.json`, `channels.example.toml`.
