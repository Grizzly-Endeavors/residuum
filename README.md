# IronClaw

One Agent, for *You*.

Memory the lasts, capabilities that grow with you, proactivity without the pricetag. Built to be an assistant that learns with you, providing support in the areas you need it. 

## What Is IronClaw?

IronClaw is a *personal* assistant. Not multi-user, not a swarm, not an automation platform. The design explicitly rejects the kitchen sink approach in favor of depth in the areas that matter for an always-on personal assistant. 

Continuity is the biggest area, most frameworks try to carry context across sessions, and while some approaches almost bridge the gap, it can still feel like you have to re-explain things the agent should know every time a new session starts. IronClaw does not have sessions, it has one feed. Whether you message your agent via the CLI or Discord (or any other platform), it picks up right where you left off. (More on how this works later)

The second area is *efficient* proactivity. Instead of burning frontier tokens to ckeck if there's anything to do every thirty minutes. HEARTBEAT.yml allows the agent or user to define individual Pulses, each with their own schedule and set of tasks. Want the agent to check your email every two hours? Have it create a pulse. Want it to check for action items in your conversations from that day? Make a pulse. If you don't mind the token spend, and like the freeform "is there anything to do?" checks, make a pulse and point it at a markdown file.

Lastly, IronClaw prioritizes giving your agent everything it needs to keep up with the full scope of your life, *without* buring thousands of tokens to do it. Projects allow your agent to always have a lightweight index of the important areas and work in your life. Instead of trying to juggle everything all the time, it can activate a project when it matters, and deactivate it when it doesn't. Your agent never has to guess where to look for important information.

### Key Capabilities

- **Observational Memory** — Two-tier compression system (Observer + Reflector) that keeps a compressed event history always in the agent's context window. No "memory cliff" where yesterday's context disappears.
- **Projects** — Structured context management with progressive disclosure. The agent sees a lightweight index of all projects, loads full context on activation, and reads specific files on demand. Write-scoped to the active project's workspace.
- **Pulse Scheduling** — Machine-parseable `HEARTBEAT.yml` replaces freeform heartbeat files. The gateway handles scheduling; the LLM only fires when something is actually due. Zero token cost when idle.
- **Cron Jobs** — The agent can schedule its own wake-ups: one-shot reminders, recurring tasks, deferred follow-ups.
- **Skills** — Compatible with the [Agent Skills spec](https://agentskills.io). Dynamic runtime activation — metadata is always visible, full instructions load only when the agent decides a skill is relevant.
- **MCP** — Native Model Context Protocol client. Connects to tool servers defined globally or per-project. Full lifecycle management.

## Requirements

- Rust 1.85+ (2024 edition)
- An API key for at least one supported LLM provider

### Supported Providers

| Provider | Protocol | Example models |
|----------|----------|----------------|
| Anthropic | Messages API | `claude-sonnet-4-6`, `claude-haiku-4-5` |
| OpenAI | Chat Completions | `gpt-4o`, `gpt-4o-mini` (also any OpenAI-compatible API: vLLM, LM Studio, Cerebras, etc.) |
| Google Gemini | generateContent | `gemini-2.5-flash`, `gemini-2.5-pro` |
| Ollama | Ollama REST API | `llama3`, `mistral`, any local model |

## Quick Start

```bash
# Build
cargo build --release

# First run — creates ~/.ironclaw/ with config template and workspace scaffolding
cargo run --release

# Or with Discord support
cargo run --release --features discord
```

On first run, IronClaw creates `~/.ironclaw/config.toml` with sensible defaults. Set your API key and model:

```bash
# Option 1: environment variable
export ANTHROPIC_API_KEY="sk-ant-..."

# Option 2: named provider in config.toml
# [providers]
# claude = { type = "anthropic", api_key = "sk-ant-..." }
```

### Running

IronClaw has two modes:

```bash
# Start the gateway server (default)
ironclaw serve

# Connect a CLI client to a running gateway
ironclaw connect                        # defaults to ws://127.0.0.1:7700/ws
ironclaw connect ws://host:port/ws      # custom URL
ironclaw connect -v                     # verbose mode (shows tool calls)
```

### CLI Commands

While connected, the following slash commands are available:

| Command | Description |
|---------|-------------|
| `/verbose` | Toggle verbose mode (show tool calls and internal events) |
| `/reload` | Hot-reload gateway configuration |
| `/observe` | Manually trigger the memory Observer |
| `/reflect` | Manually trigger the memory Reflector |
| `/help` | Show available commands |
| `/quit` | Disconnect |

## Configuration

Single TOML file at `~/.ironclaw/config.toml`. An annotated example with all options is generated at `~/.ironclaw/config.example.toml` on every boot.

```toml
timezone = "America/New_York"       # REQUIRED — IANA timezone for all timestamps

[models]
main = "anthropic/claude-sonnet-4-6"            # Primary agent model
# observer = "gemini/gemini-2.5-flash"          # Cheap model for memory compression
# reflector = "gemini/gemini-2.5-flash"          # Cheap model for log compaction
# pulse = "anthropic/claude-haiku-4-5"          # Model for pulse evaluations

[providers]
# cerebras = { type = "openai", api_key = "csk-...", url = "https://api.cerebras.ai/v1" }

[memory]
observer_threshold_tokens = 30000               # Soft threshold: start cooldown
observer_force_threshold_tokens = 60000         # Hard threshold: observe immediately
observer_cooldown_secs = 120
reflector_threshold_tokens = 40000

[gateway]
bind = "127.0.0.1"
port = 7700

[pulse]
enabled = true

[cron]
enabled = true

# [discord]
# token = "${IRONCLAW_DISCORD_TOKEN}"

# [webhook]
# enabled = true
# secret = "your-secret"

# [mcp.servers.filesystem]
# command = "mcp-server-filesystem"
# args = ["/home/user/documents"]

# [skills]
# dirs = ["~/extra-skills"]
```

### Environment Variables

| Variable | Purpose |
|----------|---------|
| `ANTHROPIC_API_KEY` | Anthropic API key |
| `OPENAI_API_KEY` | OpenAI API key |
| `GEMINI_API_KEY` | Google Gemini API key |
| `IRONCLAW_API_KEY` | Fallback API key for any provider |
| `IRONCLAW_DISCORD_TOKEN` | Discord bot token |
| `IRONCLAW_WORKSPACE` | Override workspace directory |
| `RUST_LOG` | Logging level (`debug`, `trace`, etc.) |

## Workspace

All agent state lives in human-readable files under `~/.ironclaw/workspace/`:

```
workspace/
├── SOUL.md                 # Agent persona, boundaries, and identity
├── AGENTS.md               # Operating instructions
├── USER.md                 # User info and preferences
├── MEMORY.md               # Curated long-term memory
├── ENVIRONMENT.md          # Local environment notes (optional)
├── HEARTBEAT.yml           # Pulse schedule definitions
├── NOTIFY.yml              # Notification routing
│
├── memory/
│   ├── observations.json           # Compressed event history (always in context)
│   ├── recent_messages.json        # Unobserved messages (persists across restarts)
│   ├── recent_context.json         # Latest observer narrative
│   └── episodes/                   # Raw episode transcripts
│
├── projects/
│   └── my-project/
│       ├── PROJECT.md              # Frontmatter (name, description, tools, MCP servers)
│       ├── notes/                  # Agent-maintained notes + session logs
│       ├── references/             # User-provided reference material
│       ├── workspace/              # Agent work output (write-scoped)
│       └── skills/                 # Project-scoped skills
│
├── archive/                        # Completed projects
├── skills/                         # Workspace-global skills (SKILL.md)
└── cron/
    └── jobs.json                   # Agent-created scheduled jobs
```

Every file is plain text (Markdown, JSON, YAML, TOML). If you can open it in a text editor, the system is working as intended.

## Architecture

```
                    ┌──────────────────────────────────────────┐
  CLI ──────────┐   │              Gateway Server               │
  Discord ──────┤   │                                          │
  Webhook ──────┘   │  ┌─────────┐  ┌──────────┐  ┌────────┐  │
        ─── WS ───► │  │  Agent  │  │  Memory  │  │ Pulse  │  │
                    │  │  Turn   │  │ Observer │  │Sched.  │  │
        ◄── WS ──── │  │  Loop   │  │Reflector │  │        │  │
                    │  └────┬────┘  └──────────┘  └────────┘  │
                    │       │                                  │
                    │  ┌────┴────────────────────────────────┐ │
                    │  │  Tools   MCP   Projects   Skills    │ │
                    │  └─────────────────────────────────────┘ │
                    │                                          │
                    │  ┌─────────────────────────────────────┐ │
                    │  │  LLM Providers                      │ │
                    │  │  Anthropic · OpenAI · Gemini · Ollama│ │
                    │  └─────────────────────────────────────┘ │
                    └──────────────────────────────────────────┘
```

The gateway runs an 8-arm `tokio::select!` event loop handling: inbound messages, pulse timer, cron timer, cron notify, observer cooldown, manual observe/reflect triggers, and reload signals.

Subsystems are independent and compose through shared data — the workspace filesystem and the observation log. Memory doesn't import Projects. Skills doesn't import Pulse. They all compose at the Agent layer.

## Tools

Built-in tools available to the agent:

| Tool | Description |
|------|-------------|
| `read` | Read file contents |
| `write` | Write files (scoped to active project workspace) |
| `edit` | String replacement in files |
| `exec` | Shell command execution (requires project opt-in) |
| `memory_search` | BM25 search over workspace files |
| `project_activate` | Activate a project context |
| `project_deactivate` | Deactivate with mandatory session log |
| `project_create` | Create a new project |
| `project_archive` | Archive a completed project |
| `project_list` | List all projects |
| `skill_activate` | Load skill instructions into the system prompt |
| `skill_deactivate` | Remove skill instructions |
| `cron_add` | Schedule a one-shot or recurring job |
| `cron_list` | List scheduled jobs |
| `cron_update` | Modify an existing job |
| `cron_remove` | Delete a job |

MCP server tools are automatically unioned with built-in tools when servers are connected.

## Testing

```bash
# Run all tests (quiet mode — only shows failures and summary)
cargo test --quiet

# Run a specific integration test
cargo test --test memory_integration --quiet
cargo test --test gateway_integration --quiet
```

Pre-commit hooks enforce `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` on every commit.

## Project Structure

```
src/
├── main.rs              # Entry point: serve/connect subcommands
├── lib.rs               # Module declarations
├── error.rs             # IronclawError enum
├── agent/               # Agent runtime, context assembly, turn loop
├── channels/            # CLI, Discord, WebSocket, webhook adapters
├── config/              # TOML config loading and validation
├── models/              # LLM providers (Anthropic, OpenAI, Gemini, Ollama)
├── gateway/             # WebSocket server and main event loop
├── memory/              # Observer, Reflector, search, episode store
├── projects/            # Project scanning, activation, lifecycle
├── pulse/               # HEARTBEAT.yml scheduling and execution
├── cron/                # Job persistence and scheduling
├── skills/              # SKILL.md discovery, parsing, activation
├── mcp/                 # MCP client and server registry
├── tools/               # Tool trait, built-in tools, policy enforcement
└── workspace/           # Layout conventions, identity files, bootstrap
```

## Design Documents

- [Design Philosophy](design-philosophy.md) — Guiding principles
- [Architecture Design](ironclaw-design.md) — Full system architecture
- [Memory & Proactivity](personal-agent-design.md) — Observational Memory and Pulse system design
- [Projects System](projects-context-design.md) — Context management design
- [Background Tasks](background-tasks-design.md) — Background task execution and turn loop interrupts
- [Notification Routing](notification-routing-design.md) — NOTIFY.yml channel-based result delivery

## License

Private project. Not currently licensed for redistribution.
