# Tool PATH — Runtime-Extensible CLIs

Residuum spawns external programs from the **`exec` tool** (shell commands) and
from **MCP stdio servers** (each server's `command`). Both resolve the program
name against an extended `PATH` that you can grow at runtime — no rebuild.

## Where binaries are found

Effective search order (first match wins):

1. Directories listed in `[tools].path` (config.toml), in order.
2. The default persistent dir `~/.residuum/hub/bin` (created at first run).
3. The inherited system `PATH`.

Drop a binary into any of these and it becomes runnable — via `exec` directly,
or as an MCP stdio server's `command`.

## Installing a tool

The default dir `~/.residuum/hub/bin` is writable and **persists** across restarts
and (in containers) image upgrades, because it lives under the state volume. To
add a CLI, place an executable there (e.g. `exec` a download into
`~/.residuum/hub/bin/mytool` and `chmod +x` it). It's immediately resolvable.

Works for **single-file static binaries** (`gh`, `kubectl`, `flux`, `bao`, `uv`,
most Go/Rust CLIs). Interpreter/runtime tools (`git`, `node`, `python`) aren't
single files and still need their runtime in the base image.

## Config

```toml
[tools]
# Extra dirs prepended ahead of ~/.residuum/hub/bin. Often a read-only, shared,
# admin-managed toolbox volume.
path = ["/opt/residuum-tools"]
```

Changing `[tools].path` and reloading config applies to `exec` immediately;
already-running MCP stdio servers pick up the change on their next reconnect.

## Large tool results

A tool result over 32,000 characters (any tool, built-in or MCP) is saved whole, with agent-key values redacted, to `~/.residuum/hub/tool-results/<your agent>/`. You see its first and last few thousand characters around a note naming the file, its size, the lines left out, and the `read_file` offset they start at. Page through the rest with `read_file` from that offset (each page names the next offset), or search it with `exec` (`grep`, `jq`, `cut`). `read_file` cuts lines over 2,000 characters, so use `exec` for a result that is one long line. `exec`, `read_file` and `web_fetch` size their own pages to fit under this limit, so their results are kept whole unless you pass `read_file` a large `limit`. Saved results are deleted after seven days. See `docs/systems-usage/turn-control.md`, Large Tool Results.

See the authoritative reference: `docs/systems-usage/tools.md`.
