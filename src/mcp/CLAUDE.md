# MCP Module

Lifecycle management for user-configured MCP servers (declared in the global
`mcp.json`) and the tools they expose to the agent.

## Tool-name collision policy

MCP servers can expose a tool with any name, including one that collides with a
built-in tool (`read_file`, `exec`, `web_fetch`, …) or with another connected
server's tool. Collisions are resolved **deterministically and visibly** — never
silently.

**Precedence (which tool keeps the name):**
1. A **built-in** tool always wins. It is dispatched first in the turn loop
   (`agent/turn.rs::execute_tool`), so a colliding MCP tool could never be
   reached anyway.
2. Among MCP servers, the **first-registered running server** wins (matches the
   iteration order in `call_tool`).

The losing tool is **shadowed**: excluded from the definitions offered to the
model and never dispatched to. The winner and the losing server's *other*
(non-colliding) tools keep working — shadowing is per-tool, not per-server.

**Every shadowing is logged once at `warn`**, naming the shadowed tool, its
server, and the winner. The log fires at server-connect time (or when the
built-in namespace is first reserved), not per turn, to avoid log spam.

### Where this lives (`registry.rs`)

- `set_reserved_tool_names` — the gateway calls this once after the built-in
  tool registry is built, handing over the built-in names so MCP tools that
  reuse them can be detected. Re-scans already-connected servers.
- `warn_shadowed_tools` — called from `connect`; emits the collision warnings
  for a newly connecting server.
- `tool_definitions` — returns the de-duplicated union actually offered to the
  model: reserved (built-in) names dropped, later MCP duplicates dropped.
- `resolve_tool` — resolves a name to the first running server owning it,
  consistent with the de-duplication above, and returns a cheap-to-clone
  `McpClientHandle` that doesn't borrow the registry. This is what
  `agent/turn.rs::execute_mcp_tool` calls while holding the registry's read
  lock, dropping that guard before awaiting the call on the handle — so a
  slow or hung MCP server never holds the lock for the call's duration.
  `call_tool` is a `resolve_tool` + await convenience wrapper for callers
  that aren't holding the registry behind a lock they need to release
  first (tests, and any other owned-registry caller).

Because the registry hands `agent/turn.rs` an already-clean union, the turn
loop's merge and built-in-first dispatch need no collision logic of their own —
this module is the single source of truth for the policy.
