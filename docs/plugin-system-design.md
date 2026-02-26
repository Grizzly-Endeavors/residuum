# Personal AI Agent — Plugin System Design

## Overview

This document describes a plugin system for Ironclaw that closes the primary ecosystem gap with OpenClaw. The goal is not feature parity — it's covering the capabilities that actually drive ecosystem adoption: channels, hooks, and a lightweight plugin lifecycle.

Ironclaw already covers significant ground through existing mechanisms — MCP for external tools, SKILL.md for agent instructions, file-based identity and pulse customization, OpenAI-compatible provider config. What's missing is the ability to add new *channels* and *behavioral modifications* without recompiling the binary.

---

## Design Philosophy

Same principles as the rest of Ironclaw, plus one new one:

1. **One protocol, one pattern.** MCP proved that subprocess communication over stdio works. The plugin system extends this pattern rather than inventing a new one. A plugin is a process that speaks JSON-RPC.

2. **Plugins are processes, not libraries.** No dynamic linking, no WASM runtime, no embedded scripting language. Plugins run as child processes with natural crash isolation, language independence, and the same lifecycle model MCP servers already use. The tradeoff — IPC overhead on hook invocations — is acceptable for the hook frequencies involved (per-turn, not per-token).

3. **Small protocol surface.** OpenClaw's plugin API has 12+ registration methods. This design has four capabilities: channels, hooks, tools, and HTTP routes. A plugin declares what it provides in a manifest. The protocol has ~12 message types total.

4. **File-first discovery.** No package registry, no database. Plugins are directories on disk with a manifest file. Discovery is directory scanning, same as projects and skills.

5. **Channels are the ecosystem driver.** Most OpenClaw plugins are channels. The protocol is designed around making channels easy to write — a Telegram bot plugin should be ~200 lines of Python, not a project.

---

## What This Covers (and What It Doesn't)

### Ecosystem gap analysis

| OpenClaw capability | Current Ironclaw equivalent | Gap |
|---|---|---|
| Tools | MCP servers | **None** — fully covered |
| Skills | SKILL.md files | **None** — fully covered |
| Channels (Discord, Telegram, Slack, ...) | Compiled-in (Discord only) | **Large** — biggest ecosystem driver |
| Hooks (modify agent behavior) | None | **Large** — enables composable plugins |
| Custom providers (OAuth, auth flows) | `[providers]` config (OpenAI-compatible URLs) | **Small** — covers most cases |
| Services (background processes) | Pulse + cron | **Small** — different model, similar outcomes |
| HTTP route handlers | N/A | **Small** — axum is already in use, easy to expose |
| CLI command registration | N/A (thin connect client) | **Minimal** — low ecosystem value |
| Plugin distribution (npm install) | N/A | **Moderate** — convenience, not capability |

### What this design adds

- **Channels as plugins** (~50% of the ecosystem gap). Discord moves from a compile-time feature to a bundled plugin. Telegram, Slack, Signal, Matrix — anyone can write one.
- **Hook system** (~20%). Plugins can intercept and modify the agent's behavior at well-defined points in the turn loop and message pipeline.
- **HTTP route registration** (~5%). Plugins can register HTTP endpoints on the gateway's existing axum server — webhooks, OAuth callbacks, health endpoints, custom APIs.
- **Plugin lifecycle** (~10%). Manifest-based discovery, config validation, enable/disable, install from git.

### What this design does not add

- Custom OAuth provider flows (the `[providers]` config with OpenAI-compatible URLs is sufficient for nearly all cases)
- CLI command registration (the connect client is thin by design)
- A plugin SDK with re-exported types (plugins speak JSON-RPC — they import their own language's JSON-RPC library, not Ironclaw types)
- A package registry or marketplace (install from git URL or local path; a registry can come later if the ecosystem warrants it)

---

## Plugin Structure

### Filesystem layout

```
~/.ironclaw/plugins/
├── telegram/
│   ├── plugin.toml          # manifest (required)
│   ├── main.py              # entry point
│   └── ...
├── rag-injector/
│   ├── plugin.toml
│   └── target/release/rag-injector
└── usage-logger/
    ├── plugin.toml
    └── index.js
```

Plugins can also live in project-scoped directories:

```
~/.ironclaw/workspace/projects/my-project/plugins/
└── project-specific-hook/
    ├── plugin.toml
    └── hook.py
```

### Manifest: plugin.toml

Every plugin has a `plugin.toml` in its root directory. This is the only file the gateway reads before deciding whether to start the plugin process.

```toml
id = "telegram"
name = "Telegram Channel"
version = "0.1.0"
description = "Telegram bot channel adapter"

# How to start the plugin process
command = "python3"
args = ["main.py"]

# What this plugin provides
provides = ["channel"]

# What hooks this plugin subscribes to (empty = none)
hooks = ["message_sending"]

# JSON Schema for plugin-specific configuration
# Validated against config.toml [plugins.<id>] before the plugin starts
[config_schema]
type = "object"
required = ["bot_token"]

[config_schema.properties.bot_token]
type = "string"
description = "Telegram Bot API token"

[config_schema.properties.allowed_users]
type = "array"
description = "Telegram user IDs allowed to interact"
items = { type = "integer" }
```

#### Manifest fields

| Field | Type | Required | Description |
|---|---|---|---|
| `id` | string | yes | Unique plugin identifier. Used in config references. |
| `name` | string | no | Human-readable display name |
| `version` | string | no | SemVer version string |
| `description` | string | no | Brief description |
| `command` | string | yes | Executable to run |
| `args` | list | no | Arguments passed to the command |
| `env` | table | no | Additional environment variables |
| `provides` | list | yes | Capabilities: `"channel"`, `"hooks"`, `"tools"`, `"http_routes"` |
| `hooks` | list | no | Hook names this plugin subscribes to |
| `config_schema` | table | no | JSON Schema for plugin config validation |

### Configuration in config.toml

Plugin configuration integrates into the existing config file:

```toml
[plugins]
enabled = true                    # master toggle (default: true)

# Discovery paths (in addition to default ~/.ironclaw/plugins/)
paths = ["~/my-plugins"]

# Per-plugin configuration
[plugins.telegram]
enabled = true
bot_token = "${TELEGRAM_BOT_TOKEN}"
allowed_users = [12345678]

[plugins.rag-injector]
enabled = true
vector_db_url = "http://localhost:6333"
collection = "documents"

[plugins.usage-logger]
enabled = false                   # explicitly disabled
```

Config values support the same `${ENV_VAR}` interpolation as the rest of `config.toml`.

---

## The Protocol

JSON-RPC 2.0 over stdio, same transport as MCP. The gateway is the client; the plugin is the server. Messages are newline-delimited JSON.

### Lifecycle

#### Startup

```
Gateway spawns plugin process
    │
    ▼
Gateway → Plugin:  initialize
    {
        "config": { ... },         // validated plugin config from config.toml
        "workspace_dir": "/home/user/.ironclaw/workspace",
        "plugin_dir": "/home/user/.ironclaw/plugins/telegram"
    }
    │
    ▼
Plugin → Gateway:  initialized
    {
        "channels": [
            {
                "id": "telegram",
                "capabilities": ["text", "images", "replies"]
            }
        ],
        "tools": [
            {
                "name": "send_telegram_sticker",
                "description": "Send a sticker to the current chat",
                "parameters": { ... }
            }
        ],
        "http_routes": [
            {
                "path": "/plugins/telegram/webhook",
                "methods": ["POST"]
            }
        ]
    }
```

The `initialized` response declares what the plugin actually provides. The gateway uses this — not the manifest — as the authoritative capability list. The manifest's `provides` field is for pre-start filtering only (don't start a hooks-only plugin if hooks are disabled).

#### Shutdown

```
Gateway → Plugin:  shutdown {}
    │
    ▼
Plugin → Gateway:  shutdown_ack {}
    │
    ▼
Gateway waits up to 5 seconds, then SIGTERM, then SIGKILL
```

#### Health

```
Gateway → Plugin:  ping {}
Plugin → Gateway:  pong {}
```

Periodic health checks (default: 30 seconds). Three consecutive missed pongs trigger a restart.

### Channel Messages

#### Inbound (plugin → gateway)

When a user sends a message to the plugin's platform:

```json
{
    "jsonrpc": "2.0",
    "method": "channel/inbound",
    "params": {
        "channel_id": "telegram",
        "sender": {
            "id": "12345678",
            "display_name": "Alice"
        },
        "content": "What's on my calendar today?",
        "attachments": [],
        "metadata": {
            "chat_id": 456789,
            "message_id": 1234
        }
    }
}
```

This is a JSON-RPC *notification* (no `id` field) — the gateway does not respond. The message enters the same `RoutedMessage` pipeline as CLI and compiled-in channel messages.

#### Outbound (gateway → plugin)

When the agent produces a response routed to this channel:

```json
{
    "jsonrpc": "2.0",
    "id": "out-001",
    "method": "channel/outbound",
    "params": {
        "channel_id": "telegram",
        "content": "You have 3 meetings today...",
        "reply_to": {
            "chat_id": 456789,
            "message_id": 1234
        }
    }
}
```

This is a JSON-RPC *request* — the plugin must acknowledge delivery:

```json
{
    "jsonrpc": "2.0",
    "id": "out-001",
    "result": {
        "delivered": true,
        "platform_message_id": "5678"
    }
}
```

Delivery failures are reported as JSON-RPC errors. The gateway logs the failure and may retry based on the error code.

#### Typing indicators

```json
{
    "jsonrpc": "2.0",
    "method": "channel/typing",
    "params": {
        "channel_id": "telegram",
        "metadata": { "chat_id": 456789 }
    }
}
```

Notification (no response expected). Plugins that don't support typing indicators ignore this silently.

### Hook Messages

#### Modifying hooks

Modifying hooks allow the plugin to alter data before the core system processes it. They are JSON-RPC requests — the gateway waits for a response.

```
Gateway → Plugin:
{
    "jsonrpc": "2.0",
    "id": "hook-001",
    "method": "hook/before_completion",
    "params": {
        "messages": [ ... ],
        "tools": [ ... ],
        "options": { "max_tokens": 4096 }
    }
}

Plugin → Gateway:
{
    "jsonrpc": "2.0",
    "id": "hook-001",
    "result": {
        "action": "modify",
        "messages": [ ... ],      // modified message array
        "tools": [ ... ]          // modified tools (optional, omit to keep unchanged)
    }
}
```

The `action` field controls behavior:

| Action | Meaning |
|---|---|
| `"pass"` | No modifications. Equivalent to not subscribing. |
| `"modify"` | Apply the returned modifications. Only fields present in the result are changed. |
| `"block"` | Cancel the operation entirely. Only valid for `before_tool_call` and `message_sending`. |

#### Observe hooks

Observe hooks are fire-and-forget notifications. The gateway does not wait for a response.

```
Gateway → Plugin:
{
    "jsonrpc": "2.0",
    "method": "hook/after_completion",
    "params": {
        "content": "You have 3 meetings today...",
        "tool_calls": [],
        "usage": { "input_tokens": 1200, "output_tokens": 340 }
    }
}
```

No response expected or processed.

### Tool Messages

Plugins that register tools in their `initialized` response receive tool calls via JSON-RPC:

```
Gateway → Plugin:
{
    "jsonrpc": "2.0",
    "id": "tool-001",
    "method": "tool/call",
    "params": {
        "name": "send_telegram_sticker",
        "arguments": { "sticker_id": "abc123" }
    }
}

Plugin → Gateway:
{
    "jsonrpc": "2.0",
    "id": "tool-001",
    "result": {
        "output": "Sticker sent successfully",
        "is_error": false
    }
}
```

Plugin-registered tools follow the same dispatch rules as MCP tools: built-in tools take precedence, then plugin tools, then MCP tools. Within plugin tools, earlier-loaded plugins win on name conflicts.

### HTTP Route Messages

Plugins that declare `http_routes` in their `initialized` response get those routes mounted on the gateway's axum server. When a request hits a plugin route, the gateway proxies it to the plugin over stdio:

```
Gateway → Plugin:
{
    "jsonrpc": "2.0",
    "id": "http-001",
    "method": "http/request",
    "params": {
        "path": "/plugins/telegram/webhook",
        "method": "POST",
        "headers": {
            "content-type": "application/json",
            "x-telegram-bot-api-secret-token": "abc123"
        },
        "body": "{\"update_id\": 12345, ...}"
    }
}

Plugin → Gateway:
{
    "jsonrpc": "2.0",
    "id": "http-001",
    "result": {
        "status": 200,
        "headers": { "content-type": "application/json" },
        "body": "{\"ok\": true}"
    }
}
```

The gateway handles the HTTP server, TLS, and connection management. The plugin just sees request/response pairs.

#### Why this matters

Many channel APIs prefer or require webhooks over polling. Telegram supports both long-polling and webhook mode — webhook mode is more efficient and lower latency, but requires an HTTP endpoint. Without plugin HTTP routes, every channel plugin that needs webhooks would have to run its own HTTP server on a separate port, complicating deployment and firewall configuration.

With plugin HTTP routes, the channel plugin declares the paths it needs, and the gateway mounts them on its existing axum server. One port, one TLS termination point, no extra configuration.

Other use cases:
- **OAuth callback endpoints** for plugins that integrate with third-party services
- **Health/status endpoints** exposed by plugins for monitoring
- **Webhook receivers** for services that push data (GitHub, Stripe, etc.)

#### Route namespacing

All plugin HTTP routes are automatically prefixed with `/plugins/<id>/` to prevent collisions between plugins and with core routes (`/ws`, `/webhook`). If a plugin declares `path: "/webhook"`, it's mounted at `/plugins/telegram/webhook`. The plugin sees the original path in `http/request` params.

#### Route registration timing

Routes are collected from all plugins during the initialization phase and merged into the axum router before the server starts listening — the same assembly pattern already used for the webhook channel:

```rust
// Existing
let mut app = axum::Router::new()
    .route("/ws", get(ws_handler))
    .with_state(state);
if let Some(wh) = webhook_router {
    app = app.merge(wh);
}

// New — merge plugin routes
app = app.merge(plugin_host.http_routes());
```

Project-scoped plugins present a constraint: their routes can't be added after the server is already listening. Two options:

1. **Restart the HTTP server on project activation** (clean but has a brief interruption)
2. **Use a catch-all route that dispatches dynamically** (no interruption but slightly more complex)

Option 2 is preferable. A single catch-all at `/plugins/{plugin_id}/{*rest}` routes to the appropriate plugin process at runtime. Global plugin routes get static axum routes for efficiency; project-scoped plugin routes use the dynamic fallback.

---

## Hook Taxonomy

The hook set is deliberately small. Eight hooks cover the critical interception points. More can be added later as real use cases emerge — adding a hook is a backwards-compatible change.

### Modifying hooks (sequential execution)

Multiple plugins subscribing to the same modifying hook run sequentially in load order. Each receives the output of the previous plugin's modifications.

| Hook | Fires when | Payload | Can block? |
|---|---|---|---|
| `before_completion` | Before each LLM call in the turn loop | messages, tools, options | No |
| `before_tool_call` | Before dispatching a tool call | tool name, arguments | Yes |
| `message_sending` | Before outbound message delivery | content, channel, metadata | Yes |

### Observe hooks (parallel execution)

All subscribers receive the event simultaneously. The gateway does not wait for responses.

| Hook | Fires when | Payload |
|---|---|---|
| `after_completion` | After LLM response received | content, tool_calls, usage |
| `after_tool_call` | After tool execution completes | tool name, result, is_error |
| `message_received` | When inbound message arrives (any channel) | sender, content, channel, metadata |
| `observation_complete` | After the observer fires | episode_id, observations, context |
| `gateway_ready` | After gateway startup completes | (empty) |

### Hook timeout

Modifying hooks have a configurable timeout (default: 5 seconds). If a plugin does not respond within the timeout, the hook invocation is treated as a `"pass"` and the gateway logs a warning. This prevents a misbehaving plugin from blocking the agent turn loop.

### Hook execution example

A `before_completion` hook fired with two subscribing plugins:

```
1. Gateway prepares LLM call (messages, tools, options)
2. Gateway sends hook/before_completion to rag-injector (priority: load order)
3. rag-injector responds with action: "modify", adding a system message with retrieved context
4. Gateway sends hook/before_completion to content-filter (with rag-injector's modifications applied)
5. content-filter responds with action: "pass"
6. Gateway proceeds with the LLM call using the modified messages
```

---

## Plugin Discovery and Lifecycle

### Discovery

Directory scanning, same pattern as projects and skills. No central registry.

**Discovery paths (in precedence order):**

1. **Config paths** — `plugins.paths` in config.toml
2. **Project plugins** — `projects/<active>/plugins/` (only when project is active)
3. **User plugins** — `~/.ironclaw/plugins/`
4. **Bundled plugins** — shipped with the binary (Discord is the first)

For each directory, the gateway:
1. Looks for subdirectories containing a `plugin.toml`
2. Parses the manifest
3. Validates the config schema against `[plugins.<id>]` in config.toml
4. Adds to the plugin registry

Duplicate IDs: first match wins (config paths > project > user > bundled). This allows users to override bundled plugins with their own versions.

### Enable state resolution

In order:
1. If `plugins.enabled = false` globally → all disabled
2. If `[plugins.<id>].enabled = false` → disabled
3. If manifest is invalid → disabled (with startup warning)
4. If config validation fails → disabled (with startup warning)
5. Otherwise → enabled

There is no allowlist/denylist system. OpenClaw needed one because of npm-installable untrusted code. Ironclaw plugins are local directories — if you don't want a plugin, don't put it in the plugins directory.

### Process lifecycle

```
Gateway startup
    │
    ├── Scan discovery paths
    ├── Parse manifests
    ├── Validate configs
    │
    ▼
For each enabled plugin:
    │
    ├── Spawn child process (command + args from manifest)
    ├── Send `initialize` with validated config
    ├── Receive `initialized` with declared capabilities
    ├── Register channels, hooks, tools, and HTTP routes in the gateway
    │
    ▼
Running (steady state)
    │
    ├── Health checks every 30 seconds
    ├── Restart on crash (up to 3 times, then disabled with error)
    │
    ▼
Gateway shutdown
    │
    ├── Send `shutdown` to each plugin
    ├── Wait up to 5 seconds for ack
    └── SIGTERM → SIGKILL
```

### Project-scoped plugin lifecycle

Plugins in a project's `plugins/` directory follow the same lifecycle as project-scoped MCP servers:

- **On project activation**: Discover, validate, spawn, initialize
- **On project deactivation**: Shutdown, remove channels/hooks/tools from registry
- **Reconciliation**: Same diff-based approach as `McpRegistry::reconcile_and_connect()` — switching projects cleanly starts/stops the right plugin processes

---

## Integration Points

### Channel integration

Plugin channels produce `RoutedMessage`s that enter the same gateway event loop as compiled-in channels. The integration is at the `ReplyHandle` trait boundary:

```rust
/// ReplyHandle implementation for plugin channels.
/// Sends outbound messages over the plugin's stdio connection.
struct PluginReplyHandle {
    plugin_id: String,
    channel_id: String,
    metadata: Value,              // opaque metadata from the inbound message
    writer: JsonRpcWriter,        // write half of the plugin's stdio
}

#[async_trait]
impl ReplyHandle for PluginReplyHandle {
    async fn send_response(&self, text: &str) {
        // Send channel/outbound JSON-RPC request to the plugin process
    }

    async fn send_typing(&self) {
        // Send channel/typing notification to the plugin process
    }
}
```

When a plugin sends `channel/inbound`, the gateway constructs a `RoutedMessage` with a `PluginReplyHandle` and feeds it into the inbound message channel. From the agent's perspective, it's indistinguishable from a CLI or Discord message.

### Turn loop integration

Hooks integrate into the existing `execute_turn()` loop in `src/agent/turn.rs`:

```
execute_turn():
    for iteration in 0..MAX_ITERATIONS:
        1. Merge built-in + MCP + plugin tool definitions
        2. Reassemble system prompt
        3. ── hook: before_completion (modifying) ──
        4. Call provider.complete()
        5. ── hook: after_completion (observe) ──
        6. If no tool calls → push assistant message, return
        7. For each tool call:
            a. ── hook: before_tool_call (modifying, can block) ──
            b. Execute tool (built-in → plugin → MCP fallback)
            c. ── hook: after_tool_call (observe) ──
        8. Push tool results, continue loop
```

The hook runner is a new struct that holds references to active plugin connections and dispatches hook events:

```rust
struct HookRunner {
    /// Plugins subscribed to each hook, in execution order
    subscriptions: HashMap<HookName, Vec<PluginHandle>>,
}

impl HookRunner {
    /// Run a modifying hook. Returns the (possibly modified) payload.
    async fn run_modifying<T: Serialize + DeserializeOwned>(
        &self,
        hook: HookName,
        payload: T,
    ) -> Result<T>;

    /// Fire an observe hook. Returns immediately.
    fn fire_observe<T: Serialize>(
        &self,
        hook: HookName,
        payload: &T,
    );
}
```

### Message pipeline integration

Outbound messages pass through `message_sending` hooks before delivery:

```
Agent produces response text
    │
    ▼
── hook: message_sending (modifying, can block) ──
    │
    ├── action: "modify" → deliver modified content
    ├── action: "pass" → deliver original content
    └── action: "block" → suppress delivery (log warning)
```

Inbound messages fire `message_received` as an observe hook after being routed but before the agent processes them. This lets logging and analytics plugins see all traffic without being in the critical path.

### MCP coexistence

The plugin system and MCP serve different purposes and coexist cleanly:

| Concern | MCP | Plugin system |
|---|---|---|
| Tools | Primary mechanism | Secondary (for tools bundled with a channel or hook) |
| Channels | Not supported | Primary mechanism |
| Hooks | Not supported | Primary mechanism |
| HTTP routes | Not supported | Supported (mounted on gateway's axum server) |
| Protocol | MCP standard (interoperable with other agents) | Ironclaw-specific |
| Discovery | Config-defined | Directory scanning + config |
| Lifecycle | Same subprocess model | Same subprocess model |

A plugin that only provides tools and doesn't need hooks or channels should be an MCP server instead. The plugin system is for capabilities MCP doesn't cover.

---

## Plugin Examples

### Minimal channel plugin (Python, ~100 lines)

```python
#!/usr/bin/env python3
"""Telegram channel plugin for Ironclaw."""

import json
import sys
import asyncio
from telegram import Update
from telegram.ext import Application, MessageHandler, filters

class TelegramPlugin:
    def __init__(self):
        self.config = None
        self.app = None

    async def handle_initialize(self, params):
        self.config = params["config"]
        self.app = Application.builder().token(self.config["bot_token"]).build()
        self.app.add_handler(MessageHandler(filters.TEXT, self.on_message))
        asyncio.create_task(self.app.run_polling())
        return {
            "channels": [{
                "id": "telegram",
                "capabilities": ["text", "images"]
            }]
        }

    async def on_message(self, update: Update, context):
        """Forward Telegram messages to the gateway."""
        if self.config.get("allowed_users"):
            if update.effective_user.id not in self.config["allowed_users"]:
                return
        send_notification("channel/inbound", {
            "channel_id": "telegram",
            "sender": {
                "id": str(update.effective_user.id),
                "display_name": update.effective_user.first_name
            },
            "content": update.message.text,
            "metadata": {
                "chat_id": update.effective_chat.id,
                "message_id": update.message.message_id
            }
        })

    async def handle_channel_outbound(self, params):
        """Deliver a message from the agent to Telegram."""
        chat_id = params["reply_to"]["chat_id"]
        msg = await self.app.bot.send_message(
            chat_id=chat_id, text=params["content"]
        )
        return {"delivered": True, "platform_message_id": str(msg.message_id)}

    async def handle_channel_typing(self, params):
        chat_id = params["metadata"]["chat_id"]
        await self.app.bot.send_chat_action(chat_id=chat_id, action="typing")
```

### Minimal hook plugin (Python, ~40 lines)

```python
#!/usr/bin/env python3
"""RAG context injector — adds retrieved documents to every LLM call."""

import json
import sys
import httpx

QDRANT_URL = None
COLLECTION = None

async def handle_initialize(params):
    global QDRANT_URL, COLLECTION
    QDRANT_URL = params["config"]["vector_db_url"]
    COLLECTION = params["config"]["collection"]
    return {"channels": [], "tools": []}

async def handle_hook_before_completion(params):
    # Extract the last user message for search
    user_messages = [m for m in params["messages"] if m["role"] == "user"]
    if not user_messages:
        return {"action": "pass"}

    query = user_messages[-1]["content"]

    # Search the vector DB
    async with httpx.AsyncClient() as client:
        resp = await client.post(f"{QDRANT_URL}/collections/{COLLECTION}/points/search", json={
            "query": query, "limit": 3
        })
        results = resp.json().get("result", [])

    if not results:
        return {"action": "pass"}

    # Inject retrieved context as a system message
    context = "\n\n".join(r["payload"]["text"] for r in results)
    injection = {
        "role": "system",
        "content": f"<retrieved-context>\n{context}\n</retrieved-context>"
    }
    messages = params["messages"].copy()
    messages.insert(-1, injection)  # before the last user message

    return {"action": "modify", "messages": messages}
```

### plugin.toml for the RAG injector

```toml
id = "rag-injector"
name = "RAG Context Injector"
version = "0.1.0"
description = "Injects retrieved documents into every LLM call"
command = "python3"
args = ["main.py"]
provides = ["hooks"]
hooks = ["before_completion"]

[config_schema]
type = "object"
required = ["vector_db_url", "collection"]

[config_schema.properties.vector_db_url]
type = "string"
description = "Qdrant REST API URL"

[config_schema.properties.collection]
type = "string"
description = "Collection name to search"
```

---

## Installation

### Local development

Drop a directory into `~/.ironclaw/plugins/` with a `plugin.toml` and an executable entry point. That's it.

### Install from git

```bash
ironclaw plugin install https://github.com/someone/ironclaw-telegram.git
```

This clones the repo into `~/.ironclaw/plugins/<id>/`, validates the manifest, and reports whether config is needed. The user then adds `[plugins.<id>]` to their config.toml.

### Install from local path

```bash
ironclaw plugin install ./my-local-plugin
```

Symlinks or copies the directory into the plugins path.

### List / remove

```bash
ironclaw plugin list          # shows all discovered plugins and their status
ironclaw plugin remove telegram   # removes the plugin directory
```

These are thin wrappers around directory operations. There is no package manager, no dependency resolution, no lockfile. If a plugin has dependencies (e.g., Python packages), it manages them itself — a `requirements.txt` or `package.json` in the plugin directory, installed by the user or by a setup script the plugin provides.

---

## Security Model

### Trust-based, like OpenClaw

Plugins run as child processes with the same permissions as the gateway. There is no sandbox. This is the same trust model as OpenClaw ("treat them as trusted code") and the same trust model as MCP servers.

The rationale: a personal agent gateway runs on the user's machine, not a shared server. The user installs plugins deliberately. Adding sandboxing (seccomp, WASM, containers) would dramatically increase complexity for a threat model that doesn't apply.

### Safety guardrails

Despite the trust model, basic safety checks prevent accidental misconfiguration:

- **Manifest validation**: Plugins without a valid `plugin.toml` are rejected at discovery
- **Config validation**: Plugin config is validated against the manifest's JSON Schema before the process is started
- **Crash isolation**: A crashing plugin is restarted up to 3 times, then disabled. It cannot bring down the gateway.
- **Hook timeouts**: Modifying hooks that don't respond within the timeout (default 5s) are skipped with a warning. A stuck plugin cannot block the agent.
- **No implicit network access**: The plugin process can make network calls (it's a regular process), but the gateway doesn't proxy or facilitate network access. What a plugin does on the network is its own responsibility.

### What a malicious plugin could do

Everything the gateway user can do. Read files, make network calls, exfiltrate data. This is identical to the threat surface of MCP servers and OpenClaw plugins. The mitigation is the same: don't install plugins you don't trust.

---

## Implementation

### New module: `src/plugins/`

```
src/plugins/
├── mod.rs              # Public API: PluginHost, PluginHandle
├── manifest.rs         # plugin.toml parsing and validation
├── discovery.rs        # Directory scanning, precedence resolution
├── host.rs             # Process spawning, stdio management, health checks
├── protocol.rs         # JSON-RPC message types for the plugin protocol
├── hooks.rs            # HookRunner: modifying and observe hook dispatch
├── channel.rs          # PluginReplyHandle, PluginTurnDisplay
├── http.rs             # HTTP route mounting, request proxying, dynamic dispatch
└── registry.rs         # PluginRegistry: tracks active plugins and capabilities
```

### Changes to existing modules

| Module | Change |
|---|---|
| `agent/turn.rs` | Add hook invocations at defined points in `execute_turn()` |
| `gateway/server/mod.rs` | Add plugin inbound messages to the `tokio::select!` loop |
| `gateway/server/startup.rs` | Initialize plugin host after MCP, before agent creation |
| `config/types.rs` | Add `PluginsConfig` to `ConfigFile` and `Config` |
| `tools/registry.rs` | Accept plugin-registered tools in the dispatch chain |
| `channels/types.rs` | No changes — `ReplyHandle` trait already fits |

### Reuse from MCP module

The MCP client module already implements:
- Child process spawning via `TokioChildProcess`
- JSON-RPC message framing over stdio
- Tool definition conversion
- Health checking and graceful shutdown

The plugin host reuses these primitives. The protocol is different (Ironclaw plugin protocol, not MCP), but the transport and process management are identical.

### Gateway event loop changes

The `tokio::select!` loop in `GatewayRuntime` gains one new arm per channel plugin:

```rust
// Existing arms: reload, inbound, pulse, cron, cron_notify, observer, ...

// New: plugin channel messages
msg = plugin_rx.recv() => {
    // msg is a RoutedMessage with a PluginReplyHandle
    // enters the same processing path as CLI/Discord messages
}
```

All plugin channels share a single `mpsc` channel to the gateway. The `PluginHost` multiplexes inbound messages from all plugin processes onto this channel.

---

## Migration: Discord as a Plugin

The first validation of this design is moving Discord from a compiled-in channel (behind a feature flag) to a bundled plugin. This proves the protocol works against a real, already-functioning channel.

### Steps

1. Implement the plugin host, protocol, and channel integration
2. Write the Discord plugin (Rust binary, using serenity, speaking the plugin protocol)
3. Ship it as a bundled plugin in the Ironclaw distribution
4. Remove the `discord` feature flag and the `src/channels/discord.rs` module
5. Existing `[discord]` config migrates to `[plugins.discord]`

### Why Discord first

- It's already working — the behavior is fully defined and tested
- It's the most complex channel Ironclaw supports (presence, slash commands, attachments, typing indicators)
- If the protocol can handle Discord cleanly, simpler channels (Telegram, Slack) will be straightforward
- Eating your own dogfood — bundled plugins use the same protocol as third-party plugins

---

## What This Gets You

With this design implemented, adding a new channel to Ironclaw looks like this:

1. Create a directory with a `plugin.toml` and an entry point script
2. Implement `initialize`, `channel/outbound`, and send `channel/inbound` notifications
3. Drop it in `~/.ironclaw/plugins/`
4. Add `[plugins.<id>]` to config.toml with the required config values
5. Restart the gateway

No Rust compilation. No PRs to the Ironclaw repo. No understanding of the gateway internals beyond "send JSON, receive JSON."

Adding behavioral modifications (hooks) is similarly straightforward. A RAG injector, a content filter, a usage logger, a translation layer — each is a standalone process that subscribes to the relevant hooks and speaks JSON-RPC.

The remaining gap with OpenClaw's ecosystem — custom OAuth provider flows and CLI extensions — can be addressed incrementally if demand materializes. But what matters — channels, hooks, HTTP routes, and plugin lifecycle — is covered here.
