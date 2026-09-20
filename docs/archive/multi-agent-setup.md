# Multi-Agent Setup

Residuum supports running multiple fully independent agents, each with its own workspace, memory, tools, integrations, and configuration. Each agent runs as a separate gateway process on its own port.

## Concepts

A **named agent** is an independent Residuum instance stored under `~/.residuum/agent_registry/<name>/`. Each agent has:

- Its own `config.toml` (timezone, port, workspace path)
- Its own workspace directory (memory, inbox, projects, skills, etc.)
- Its own PID file and log directory
- Optionally, its own `providers.toml` — if absent, the agent inherits the global one from `~/.residuum/providers.toml`

The **default agent** (unnamed) continues to use `~/.residuum/` directly. All existing behavior is preserved — multi-agent support is purely additive.

## CLI Reference

### Create an agent

```bash
residuum agent create <name>
```

Creates a new named agent with a ready-to-run configuration. The command:

1. Validates the name (alphanumeric + hyphens, 1–32 characters, not `"default"`)
2. Assigns the next available port (starting from 7701)
3. Copies the timezone from the default agent's config
4. Creates the directory structure with `config.toml`, workspace, and starter files
5. Registers the agent in `~/.residuum/agent_registry/registry.toml`

The agent is immediately runnable — no manual editing required.

```
$ residuum agent create researcher
agent 'researcher' created (port 7701)
  config: /home/user/.residuum/agent_registry/researcher
  start:  residuum serve --agent researcher
```

### List agents

```bash
residuum agent list
```

Shows all agents (default + named) with their port and current status.

```
NAME             PORT    STATUS
(default)        7700    running (pid 12345)
researcher       7701    stopped
coder            7702    running (pid 12346)
```

### Show agent details

```bash
residuum agent info <name>
```

Displays configuration paths, port, status, and whether the agent uses local or inherited providers.

```
agent: researcher
  port:      7701
  status:    running (pid 12345)
  config:    /home/user/.residuum/agent_registry/researcher
  workspace: /home/user/.residuum/agent_registry/researcher/workspace
  logs:      /home/user/.residuum/agent_registry/researcher/logs
  providers: /home/user/.residuum/providers.toml (inherited)
```

### Delete an agent

```bash
residuum agent delete <name>
```

Removes the agent's directory and registry entry. Refuses if the agent is still running — stop it first.

### Start an agent

```bash
residuum serve --agent <name>
```

Starts the named agent as a background daemon on its assigned port. Works the same as `residuum serve` for the default agent, including daemon mode and `--foreground` flag.

```bash
# Background (default)
residuum serve --agent researcher

# Foreground mode
residuum serve --agent researcher --foreground
```

### Stop an agent

```bash
residuum stop --agent <name>
```

Sends SIGTERM to the named agent's process and waits for it to exit.

### View agent logs

```bash
residuum logs --agent <name>
residuum logs --agent <name> --watch
```

Shows the most recent log file from the agent's log directory. Named agent logs use the prefix `serve-<name>` (e.g., `serve-researcher.2026-03-16.log`).

### Connect to an agent

```bash
residuum connect --agent <name>
```

Opens a CLI session to the named agent's gateway, looking up the port from the registry automatically.

## Directory Layout

```
~/.residuum/
├── config.toml                          # default agent config
├── providers.toml                       # global provider/model config
├── residuum.pid                         # default agent PID
├── workspace/                           # default agent workspace
├── logs/                                # default agent logs
└── agent_registry/
    ├── registry.toml                    # name → port mapping
    ├── researcher/
    │   ├── config.toml                  # pre-filled: timezone, port, workspace_dir
    │   ├── residuum.pid
    │   ├── workspace/                   # independent workspace
    │   │   ├── config/
    │   │   │   ├── mcp.json
    │   │   │   └── channels.toml
    │   │   ├── SOUL.md
    │   │   ├── memory/
    │   │   └── ...
    │   └── logs/
    │       └── serve-researcher.2026-03-16.log
    └── coder/
        ├── config.toml
        ├── workspace/
        └── logs/
```

## Provider Inheritance

Named agents do **not** get a `providers.toml` by default. They inherit from `~/.residuum/providers.toml`, which means all agents share the same LLM provider configuration unless explicitly overridden.

To give an agent its own provider config, create `~/.residuum/agent_registry/<name>/providers.toml`. The agent will use it instead of the global one.

## Configuration

Each agent's `config.toml` is a standard Residuum config file. The `agent create` command pre-fills:

- `timezone` — copied from the default agent
- `workspace_dir` — set to `~/.residuum/agent_registry/<name>/workspace`
- `[gateway] port` — auto-assigned unique port

All other config options (memory, pulse, skills, integrations, etc.) can be customized per agent. See `config.example.toml` in the agent directory for the full reference.

## Running Multiple Agents

Each agent is a fully independent process. Start as many as you need:

```bash
# Create agents
residuum agent create researcher
residuum agent create coder
residuum agent create monitor

# Start them all
residuum serve --agent researcher
residuum serve --agent coder
residuum serve --agent monitor

# Check status
residuum agent list

# Connect to a specific agent
residuum connect --agent researcher

# Stop one
residuum stop --agent coder
```

Agents do not communicate with each other directly. Each has its own workspace, memory, and conversation history. Cross-agent coordination would be handled at the user or integration level (e.g., via webhooks or shared files).
