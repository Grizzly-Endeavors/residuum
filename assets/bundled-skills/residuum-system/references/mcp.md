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

## Projects

Project frontmatter references servers by name rather than embedding them:

```yaml
mcp_servers:
  - filesystem
  - git
```

Names resolve against a project-local `mcp.json` first, then the workspace
one. Servers are reference-counted across sub-agents sharing a project, so
multiple agents with the same project active reuse one running server
instead of starting duplicates.

## Tools

No dedicated tools — once a server is running, its tools are merged directly
into the agent's available tool set.

See the authoritative reference: `docs/systems-usage/mcp.md`.
