# Tool PATH — Runtime-Extensible CLIs

Residuum spawns external programs in two places:

- **The `exec` tool** — runs shell commands (`sh -c` on Unix, `cmd /C` on Windows).
- **MCP stdio servers** — each server is a child process launched by its `command`.

Both resolve the program name against the child process's `PATH`. The `[tools]`
config section lets you extend that `PATH` at runtime so new CLIs become
available **without rebuilding** the binary or the container image.

## How it works

Residuum prepends one or more directories to the `PATH` of every child it
spawns. Effective order (first match wins):

1. Directories listed in `[tools].path`, in the order given.
2. The default persistent dir `~/.residuum/bin`.
3. The inherited system `PATH`.

Drop a binary into any of these directories and the agent can invoke it —
`exec`-ing it directly, or configuring an MCP stdio server whose `command` is
that binary.

## The default dir: `~/.residuum/bin`

Created at first run, always on the tool `PATH`. It lives next to the workspace
under `~/.residuum`, which in the container image is the mounted state volume
(`VOLUME /home/residuum/.residuum`). Anything placed here therefore **persists**
across restarts, recreations, and image upgrades — install once, not every
start.

## Configured dirs: `[tools].path`

```toml
[tools]
# Extra dirs prepended to the PATH of spawned children. Prepended in order,
# ahead of ~/.residuum/bin.
path = ["/opt/residuum-tools"]
```

These entries take precedence over `~/.residuum/bin` on a name collision, so an
externally-managed directory can pin or override a tool.

### Container pattern: a read-only tools volume

The intended self-hosting pattern is to mount a **dedicated tools volume**
(e.g. at `/opt/residuum-tools`) that is managed independently of agent state:

- Mount it **read-only** so the agent can *use* the tools but not modify its own
  toolbox — a meaningful boundary given the `exec` tool can write to
  `~/.residuum/bin`.
- **Share** it across multiple agents/instances.
- Add tools by dropping a binary into the mounted dir — no custom image, no
  rebuild.

## Live reload

Changing `[tools].path` and reloading config takes effect without a restart:

- The **`exec` tool** reads the effective `PATH` on every call, so it applies
  immediately.
- **MCP stdio servers** bake their environment at spawn. A server started
  *after* the reload uses the new `PATH`; an already-running server keeps the
  `PATH` it launched with until it next (re)connects.

## Per-server MCP `PATH`

If a stdio MCP server sets its own `PATH` in its `env`, that value wins — the
tool `PATH` is applied first and the server's explicit `env` overrides it.

## Scope & limitations

- **Single-file static binaries only.** `gh`, `kubectl`, `flux`, `bao`, `uv`,
  and most Go/Rust CLIs work perfectly this way.
- **Interpreter/runtime tools are out of scope.** `node` and `python` aren't
  single files and still need their runtime present in the base image (or route
  around them — e.g. `uv`, itself a single static binary, for Python-based MCP
  servers). `git` is the exception: the published image ships it (see below).

## What the container image already ships

The published image installs two things beyond the binary itself, because
neither can be supplied through the tool `PATH`:

- **`git`** — the common agent workflow (clone → edit → commit → push) needs a
  real `git`, not a dropped-in static binary.
- **`ca-certificates`** — the HTTP client verifies TLS against the *system*
  trust store, so every HTTPS model provider is unreachable without a CA
  bundle in the image.

Everything else is expected to arrive via `~/.residuum/bin` or a mounted
`[tools].path` dir.
