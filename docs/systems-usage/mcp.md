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
      "env": { "SOME_TOKEN": "${MY_TOKEN:-}" }
    },
    "hosted-search": {
      "type": "http",
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer ${API_TOKEN}" }
    }
  }
}
```

The loader (`crate::workspace::config::load_mcp_servers_map`, in `src/workspace/config.rs`) accepts either the Residuum-native `transport` field (`"stdio"` | `"http"`) or the Claude Code/Desktop `type` field (`"stdio"` | `"streamable-http"` | `"http"` | `"sse"`); `type` takes priority when both are present. `"sse"` is recognized but skipped with a warning (deprecated by the MCP spec), as is any unrecognized transport value. For HTTP servers, `url` is preferred over `command` as the address; a server missing both is skipped with a warning. A stdio server missing `command` is likewise skipped. None of these are hard failures — a bad entry drops that one server, not the whole file.

Project frontmatter references servers by name rather than embedding them (see [projects.md](projects.md)):

```yaml
mcp_servers:
  - filesystem
  - git
```

`resolve_mcp_references` (same file) resolves each name against a project-local `mcp.json` (`<project_root>/mcp.json`) first, then the global workspace `mcp.json`, so a project can shadow a global server definition by reusing its name. An unresolvable reference is an error for the whole activation — surfaced to the agent as a warning rather than aborting activation (see `ProjectActivateTool` in `src/tools/projects.rs`).

## Transports

`McpServerEntry::transport` (`src/projects/types.rs`) selects the connection strategy in `McpClient::connect` (`src/mcp/client.rs`):

- **Stdio** (default): spawns `entry.command` with `entry.args` as a child process and speaks MCP over stdin/stdout (`TokioChildProcess`). The tool-PATH handle (see [tools.md](tools.md)) is applied to the child's `PATH` first, then the entry's own `env` — so an explicit `PATH` in the entry still wins.
- **Http**: connects to `entry.command` (the URL) via Streamable HTTP (`StreamableHttpClientTransport`). If `headers` is non-empty, header values are expanded for env interpolation and attached as custom headers.

Both paths perform the MCP handshake via `rmcp`'s `ServiceExt::serve`; a spawn/dial failure or a handshake failure both surface as connection errors, and the caller (the registry) marks the server `Failed` with that reason.

## `${VAR}` / `${VAR:-default}` expansion

`expand_env_vars` (`src/mcp/client.rs`) expands `${VAR}` and `${VAR:-default}` patterns against the process's own environment. It is currently applied only to HTTP server **header values** at connect time (`expand_header_env_vars`) — a missing variable with no default resolves to an empty string; an empty (but set) variable does **not** trigger the default, which diverges from POSIX shell semantics. Stdio `env` entries are passed through to the child process as literal strings without this expansion.

## Registry and reconciliation

`McpRegistry` (`src/mcp/registry.rs`) tracks servers as a flat list of `TrackedServer { name, command, args, status, client, tools }`. Status is one of `Pending`, `Running`, or `Failed(reason)`.

- **`reconcile(desired)`** — pure diff, no I/O. Servers in `desired` that aren't already `Running`/`Pending` go into `to_start` (and are immediately re-tracked as `Pending`, replacing any stale entry). Tracked servers not in `desired` and currently `Running`/`Pending` go into `to_stop`. A `Failed` server whose name is still in `desired` is treated as absent and restarted.
- **`reconcile_and_connect(desired)`** — runs `reconcile`, then connects everything in `to_start` and disconnects everything in `to_stop`, returning an `McpReconcileReport` (`started`, `stopped`, `failures: Vec<(name, error)>`). This is the workspace-level reconciliation path: it runs at startup against `config/mcp.json` and again on a live config reload (`handle_workspace_reload` in `src/gateway/event_loop/run_loop.rs`) — a config edit that removes a server tears it down, one that adds a server starts it, without a restart.
- **`connect_servers(entries)`** — purely additive; never stops or removes existing servers. Used for one-off attachments outside the desired-state model, e.g. the standalone web-search MCP servers (Brave/Tavily, wired in `connect_web_search_mcp` in `src/gateway/startup/mod.rs`) that aren't declared in `mcp.json` at all.
- **`connect(entry)`** — the low-level step: builds an `McpClient`, lists its tools, and marks the server `Running` with the discovered `ToolDefinition`s cached. On any failure it marks the tracked entry `Failed` with the error text (visible via `servers()`).

## Per-project reference counting

Multiple sub-agents can have the same project active at once, so MCP servers declared in project frontmatter are shared rather than started per-agent. `McpRegistry` keeps a separate map, `project_refs: HashMap<lowercased-name, ProjectMcpState>`, alongside the flat server list:

- **`activate_project(name, servers)`** — first activation (ref count 0→1) reconciles and connects `servers`, then records the count and the resolved entries. Subsequent activations (count N→N+1) just increment and return an empty report — servers already running are reused, not restarted.
- **`deactivate_project(name)`** — decrements the count; only at 0 does it disconnect the project's servers and return their names. `project_activate`/`project_deactivate` tool calls and the idle-timeout project deactivation (`src/gateway/idle.rs`) both go through this path.
- **`force_deactivate_project(name)`** — ignores the ref count entirely: removes the tracking entry and disconnects immediately. This is the crash-recovery path — a sub-agent whose turn loop ends without calling `project_deactivate` (crash, cancellation, or exhausted retry) leaves its project active, so `background/subagent.rs`'s `force_deactivate_project` helper calls this to tear the MCP servers down alongside clearing the path policy and tool filter, regardless of how many other agents still think the project is active.

Project names are lowercased for the ref-count key, so activation/deactivation is case-insensitive with respect to matching.

## Consumption

- **`tool_definitions()`** returns the flat union of `ToolDefinition`s from every `Running` server — this is what gets merged into the agent's available tool set. Names aren't guaranteed unique across servers or against built-ins, so a deterministic collision policy applies before this union is built: a built-in tool always wins, and among MCP servers the first-registered running one wins; the losing tool is shadowed (excluded from the union, never dispatched) and the collision is logged once at `warn`. See [`src/mcp/CLAUDE.md`](../../src/mcp/CLAUDE.md) for the implementation.
- **`call_tool(name, args)`** finds the first `Running` server whose tool list contains `name` and routes the call to its `McpClient`. Unknown tool names return `ToolError::NotFound`; a found-but-broken client (an internal-consistency bug, not a user error) returns `ToolError::Execution` and is logged at `error`. Each call has a one-minute timeout (`TOOL_CALL_TIMEOUT`); a timeout is reported as an execution error naming the tool, server, and elapsed seconds.
- Tool results only preserve text content blocks (`extract_text_content` joins them with newlines); non-text blocks (e.g. images) are silently dropped from the MCP path today.

## Interaction with Other Systems

- **Tool PATH**: stdio servers resolve their `command` against the effective tool `PATH` (configured dirs + `~/.residuum/bin` + inherited `PATH`), injected into the registry via `McpRegistry::new_shared_with_tools_path`. See [tools.md](tools.md).
- **Projects**: project frontmatter's `mcp_servers` list is names, not inline definitions; resolution and reference-counted lifecycle are described above. See [projects.md](projects.md).
- **Background tasks**: sub-agents share the same `SharedMcpRegistry` as the main agent — there is no per-agent isolation. If a sub-agent's turn loop ends with a project still active, force-deactivation tears down that project's MCP servers even though other agents may still be relying on the ref count. See [background-tasks.md](background-tasks.md).
