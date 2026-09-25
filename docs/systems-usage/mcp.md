# MCP (Model Context Protocol)

MCP servers extend the agent's tool set with external tools served by a separate process, either a spawned stdio child or a remote HTTP endpoint. Residuum maintains a registry of running servers, reconciles it against desired state, and exposes each server's tools alongside the agent's built-in tools.

## Configuration: `config/mcp.json`

Server definitions live in `config/mcp.json` (workspace-level, via `WorkspaceLayout::mcp_json()`), in the same `mcpServers` map format used by Claude Code/Desktop:

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/data"],
      "env": { "SOME_TOKEN": "${agent-key:some_token}" }
    },
    "hosted-search": {
      "type": "http",
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer ${API_TOKEN}" },
      "timeout_secs": 30
    }
  }
}
```

The loader (`crate::workspace::config::load_mcp_servers_map`, in `src/workspace/config.rs`) accepts either the Residuum-native `transport` field (`"stdio"` | `"http"`) or the Claude Code/Desktop `type` field (`"stdio"` | `"streamable-http"` | `"http"` | `"sse"`); `type` takes priority when both are present. `"sse"` is recognized but skipped with a warning (deprecated by the MCP spec), as is any unrecognized transport value. For HTTP servers, `url` is preferred over `command` as the address; a server missing both is skipped with a warning. A stdio server missing `command` is likewise skipped. None of these are hard failures — a bad entry drops that one server, not the whole file.

The web UI's Settings → MCP panel edits `mcp.json` through `PATCH /api/mcp/patch` (`src/gateway/web/config.rs`, applied by `crate::workspace::mcp_patch::apply_mcp_patch`), which merges only the fields the form changed into the file already on disk rather than rewriting it from form state. A server's transport is shown and edited truthfully — HTTP servers expose `url`/`headers`, stdio servers expose `command`/`args`/`env` — and any field the form doesn't model (on a touched server or an untouched one) survives the edit. Removing a server in the form removes just that entry. An `mcp.json` that fails to parse is left untouched and the patch is refused with an error naming the file.

## Transports

`McpServerEntry::transport` (`src/mcp/types.rs`) selects the connection strategy in `McpClient::connect` (`src/mcp/client.rs`):

- **Stdio** (default): spawns `entry.command` with `entry.args` as a child process and speaks MCP over stdin/stdout (`TokioChildProcess`). The tool-PATH handle (see [tools.md](tools.md)) is applied to the child's `PATH` first, then the entry's own `env` — so an explicit `PATH` in the entry still wins.
- **Http**: connects to `entry.command` (the URL) via Streamable HTTP (`StreamableHttpClientTransport`). If `headers` is non-empty, header values are expanded for env interpolation and attached as custom headers.

Both paths perform the MCP handshake via `rmcp`'s `ServiceExt::serve`; a spawn/dial failure or a handshake failure both surface as connection errors, and the caller (the registry) marks the server `Failed` with that reason.

## `${agent-key:<name>}` references

Stdio `env` values and HTTP `headers` values may contain `${agent-key:<name>}` anywhere in the value. `McpRegistry::connect` resolves them against the agent key store before spawning or dialing, so the value reaches the server without ever appearing in `mcp.json`. An unknown key, or an unterminated reference, fails that server's connection with an error naming the problem; an entry with no references never touches the store. References resolve only against the agent key store, never the system secret store. See [Agent keys](agent-keys.md).

## `${VAR}` / `${VAR:-default}` expansion

`expand_env_vars` (`src/mcp/client.rs`) expands `${VAR}` and `${VAR:-default}` patterns against the process's own environment. It applies only to HTTP server **header values** at connect time (`expand_header_env_vars`), after agent key references are resolved — a missing variable with no default resolves to an empty string; an empty (but set) variable does **not** trigger the default, which diverges from POSIX shell semantics. Stdio `env` entries are passed through to the child process as literal strings without this expansion.

## Registry and reconciliation

`McpRegistry` (`src/mcp/registry.rs`) tracks servers as a flat list of `TrackedServer { name, command, args, status, client, tools }`. Status is one of `Pending`, `Running`, or `Failed(reason)`.

- **`reconcile(desired)`** — pure diff, no I/O. Servers in `desired` that aren't already `Running`/`Pending` go into `to_start` (and are immediately re-tracked as `Pending`, replacing any stale entry). Tracked servers not in `desired` and currently `Running`/`Pending` go into `to_stop`. A `Failed` server whose name is still in `desired` is treated as absent and restarted.
- **`reconcile_and_connect(desired)`** — runs `reconcile`, then connects everything in `to_start` and disconnects everything in `to_stop`, returning an `McpReconcileReport` (`started`, `stopped`, `failures: Vec<(name, error)>`). This is the workspace-level reconciliation path: it runs at startup against `config/mcp.json` and again on a live config reload (`handle_workspace_reload` in `src/gateway/event_loop/run_loop.rs`) — a config edit that removes a server tears it down, one that adds a server starts it, without a restart.
- **`connect_servers(entries)`** — purely additive; never stops or removes existing servers, and skips an entry whose name is already `Running`/`Pending` even if the entry itself (e.g. an API key) changed. Used for one-off attachments outside the desired-state model, e.g. the standalone web-search MCP servers (Brave/Tavily, wired in `connect_web_search_mcp` in `src/gateway/startup/mod.rs`) that aren't declared in `mcp.json` at all. On a config reload that changes the standalone web search backend, `gateway::reload::reload_web_search` disconnects whichever brave/tavily server was previously running by name first, then calls `connect_web_search_mcp` again — the skip-if-already-tracked behavior otherwise means a same-named backend's changed credentials would never take effect.
- **`connect(entry)`** — the low-level step: builds an `McpClient`, lists its tools, and marks the server `Running` with the discovered `ToolDefinition`s cached. On any failure it marks the tracked entry `Failed` with the error text (visible via `servers()`).

## Consumption

- **`tool_definitions()`** returns the flat union of `ToolDefinition`s from every `Running` server — this is what gets merged into the agent's available tool set. Names aren't guaranteed unique across servers or against built-ins, so a deterministic collision policy applies before this union is built: a built-in tool always wins, and among MCP servers the first-registered running one wins; the losing tool is shadowed (excluded from the union, never dispatched) and the collision is logged once at `warn`. See [`src/mcp/CLAUDE.md`](../../src/mcp/CLAUDE.md) for the implementation.
- **`call_tool(name, args)`** finds the first `Running` server whose tool list contains `name` and routes the call to its `McpClient`. Unknown tool names return `ToolError::NotFound`; a found-but-broken client (an internal-consistency bug, not a user error) returns `ToolError::Execution` and is logged at `error`. A call has no automatic cutoff by default — it runs until it finishes or the user or agent stops the turn, since `stop_agent` already interrupts an in-flight tool call immediately (the turn loop races every MCP call against the turn's stop token; see [background-tasks.md](background-tasks.md) and `agent/turn.rs::execute_tool`). Setting `timeout_secs` on a server entry in `mcp.json` (see above) opts that server's calls back into a fixed timeout: a call that runs past it fails with a plain-language error naming the tool, the server, and the configured number of seconds.
- Tool results only preserve text content blocks (`extract_text_content` joins them with newlines); non-text blocks (e.g. images) are silently dropped from the MCP path today.

## Interaction with Other Systems

- **Tool PATH**: stdio servers resolve their `command` against the effective tool `PATH` (configured dirs + `~/.residuum/bin` + inherited `PATH`), injected into the registry via `McpRegistry::new_shared_with_tools_path`. See [tools.md](tools.md).
- **Background tasks**: sub-agents share the same `SharedMcpRegistry` as the main agent — there is no per-agent isolation. See [background-tasks.md](background-tasks.md).
