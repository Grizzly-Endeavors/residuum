# MCP (Model Context Protocol)

Residuum extends the agent's tool set with external MCP servers — spawned
stdio children or remote HTTP endpoints — reconciled against desired state
and exposed alongside the agent's built-in tools.

## Config: `config/mcp.json`

```json
{
  "mcpServers": {
    "filesystem": {
      "command": "npx",
      "args": ["-y", "@modelcontextprotocol/server-filesystem", "/data"]
    },
    "hosted-search": {
      "type": "http",
      "url": "https://example.com/mcp",
      "headers": { "Authorization": "Bearer ${API_TOKEN}" }
    }
  }
}
```

Same `mcpServers` map format used by Claude Code/Desktop. `${VAR}` /
`${VAR:-default}` expansion applies to HTTP header values only, not stdio
`env`. A bad entry (missing `command`/`url`, unrecognized transport) drops
just that server — never a hard failure.

Editing `mcp.json` via `write_file`/`edit_file`, the workspace editor, or
`POST /api/workspace/validate` reports invalid JSON, a missing
`command`/`url`, or an unrecognized/deprecated transport as a diagnostic
alongside the save — the write always goes through on these surfaces rather
than being rejected.

To give a server a credential without writing it into `mcp.json`, reference
an agent key: `"env": { "GITHUB_TOKEN": "${agent-key:github_token}" }` or
`"headers": { "Authorization": "Bearer ${agent-key:remote_api}" }`. It is
resolved when the server connects; an unknown key fails that server's
connection. See [agent-keys](agent-keys.md).

## Tools

No dedicated tools — once a server is running, its tools are merged directly
into the agent's available tool set.

See the authoritative reference: `docs/systems-usage/mcp.md`.
