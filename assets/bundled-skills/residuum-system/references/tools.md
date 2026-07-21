# Tool PATH — Runtime-Extensible CLIs

Residuum spawns external programs from the **`exec` tool** (shell commands) and
from **MCP stdio servers** (each server's `command`). Both resolve the program
name against an extended `PATH` that you can grow at runtime — no rebuild.

## Where binaries are found

Effective search order (first match wins):

1. Directories listed in `[tools].path` (config.toml), in order.
2. The default persistent dir `~/.residuum/bin` (created at first run).
3. The inherited system `PATH`.

Drop a binary into any of these and it becomes runnable — via `exec` directly,
or as an MCP stdio server's `command`.

## Installing a tool

The default dir `~/.residuum/bin` is writable and **persists** across restarts
and (in containers) image upgrades, because it lives under the state volume. To
add a CLI, place an executable there (e.g. `exec` a download into
`~/.residuum/bin/mytool` and `chmod +x` it). It's immediately resolvable.

Works for **single-file static binaries** (`gh`, `kubectl`, `flux`, `bao`, `uv`,
most Go/Rust CLIs). Interpreter/runtime tools (`git`, `node`, `python`) aren't
single files and still need their runtime in the base image.

## Config

```toml
[tools]
# Extra dirs prepended ahead of ~/.residuum/bin. Often a read-only, shared,
# admin-managed toolbox volume.
path = ["/opt/residuum-tools"]
```

Changing `[tools].path` and reloading config applies to `exec` immediately;
already-running MCP stdio servers pick up the change on their next reconnect.

See the authoritative reference: `docs/systems-usage/tools.md`.
