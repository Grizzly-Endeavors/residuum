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
      "headers": { "Authorization": "Bearer ${API_TOKEN}" },
      "timeout_secs": 30
    }
  }
}
```

Same `mcpServers` map format used by Claude Code/Desktop. `${VAR}` /
`${VAR:-default}` expansion applies to HTTP header values only, not stdio
`env`. A bad entry (missing `command`/`url`, unrecognized transport) drops
just that server — never a hard failure.

A tool call has no automatic cutoff by default: it runs until it finishes or
the turn is stopped (Cancel / `stop_agent` interrupts an in-flight call
immediately). Setting `timeout_secs` on a server opts that server's calls
into a fixed timeout instead; a call that runs past it returns a plain-language
error naming the tool, the server, and the number of seconds.

To give a server a credential without writing it into `mcp.json`, reference
an agent key: `"env": { "GITHUB_TOKEN": "${agent-key:github_token}" }` or
`"headers": { "Authorization": "Bearer ${agent-key:remote_api}" }`. It is
resolved when the server connects; an unknown key fails that server's
connection. See [agent-keys](agent-keys.md).

## Tools

No dedicated tools — once a server is running, its tools are merged directly
into the agent's available tool set.

See the authoritative reference: `docs/systems-usage/mcp.md`.
