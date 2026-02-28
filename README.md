# IronClaw

One Agent, for *You*.

Memory that lasts, capabilities that grow with you, proactivity without the pricetag. Built to be an assistant that learns with you, providing support in the areas you need it.

## Table of Contents

- [What Is IronClaw?](#what-is-ironclaw)
- [Design Principles](#design-principles)
- [Why Rust?](#why-rust)
- [The Workspace](#the-workspace)
- [Extensibility](#extensibility)
- [Supported Providers](#supported-providers)
- [Getting Started](#getting-started)
- [Configuration Reference](#configuration-reference)
- [Architecture](#architecture)
- [Contributing](#contributing)
- [License](#license)

## What Is IronClaw?

IronClaw is a *personal* assistant. Not multi-user, not a swarm, not an automation platform. The design explicitly rejects the kitchen sink approach in favor of depth in the areas that matter for an always-on personal assistant.

Continuity is the biggest area, most frameworks try to carry context across sessions, and while some approaches almost bridge the gap, it can still feel like you have to re-explain things the agent should know every time a new session starts. IronClaw does not have sessions, it has one feed. Whether you message your agent via the CLI or Discord (or any other platform), it picks up right where you left off. (More on how this works later)

The second area is *efficient* proactivity. Instead of burning frontier tokens to check if there's anything to do every thirty minutes. HEARTBEAT.yml allows the agent or user to define individual Pulses, each with their own schedule and set of tasks. Want the agent to check your email every two hours? Have it create a pulse. Want it to check for action items in your conversations from that day? Make a pulse. If you don't mind the token spend, and like the freeform "is there anything to do?" checks, make a pulse and point it at a markdown file.

Lastly, IronClaw prioritizes giving your agent everything it needs to keep up with the full scope of your life, *without* burning thousands of tokens to do it. Projects allow your agent to always have a lightweight index of the important areas and work in your life. Instead of trying to juggle everything all the time, it can activate a project when it matters, and deactivate it when it doesn't. Your agent never has to guess where to look for important information.

### Key Capabilities

**Observational Memory** — Most agent frameworks punt on long-term memory, or try to solve it with RAG over old transcripts. IronClaw runs a two-tier compression pipeline that keeps the agent aware of what's been happening without carrying raw conversation history. The Observer distills recent messages into compact observations, each pointing back to the raw transcript (episode) it came from. These observations stay in the agent's context window permanently. When they accumulate, the Reflector condenses them further — dropping things that are outdated or no longer relevant, merging the rest, preserving episode references throughout. The result: the agent always knows what you've been working on recently, and knows exactly where to find the stuff you haven't touched in a while through RAG-supported memory search.

**Projects** — Your agent keeps a lightweight index of every project in its context at all times: just a name and a one-liner. When the conversation shifts to something relevant, the agent activates that project — loading the overview, a file manifest, and scoped tools/MCP servers. When the conversation moves on, it deactivates and logs a session summary. One project active at a time, no token waste carrying context for things that aren't relevant right now. The agent can create, archive, and maintain projects on its own, or you can drop files into a project's `references/` folder and the agent sees them in the manifest.

**Pulse Scheduling** — `HEARTBEAT.yml` defines named groups of tasks on independent schedules. The gateway handles all the timing; the LLM only runs when a pulse is actually due, and only receives the tasks it needs to evaluate. Active hours keep your agent from checking work email at 2am. Results that find nothing actionable are logged silently — zero token cost when there's nothing to do. The agent can create and modify pulses on its own, or you can edit the YAML directly.

**Notifications** — `NOTIFY.yml` controls where results from background work end up. Route a pulse result to `agent_wake` (starts a turn immediately), `agent_feed` (appears passively at the next conversation), `inbox` (stored silently for later), or an external channel like ntfy push notifications. The agent maintains this file — tell it "stop pinging me about PR reviews" and it updates the routing. Everything is human-readable and overridable.

**Inbox** — The agent's "deal with it later" queue. Drop files, notes, or task results here for the agent to pick up when it gets to them. The agent sees an unread count at every turn but never loads the contents unless it needs to, so inbox items don't eat your token budget. Items are individual files on disk, so external tools can drop things in too.

## Design Principles

**File-first transparency.** All agent state lives in plain text files — Markdown, YAML, JSON, TOML. If you can open it in a text editor, the system is working as intended. This isn't just a convenience; it's the trust model. You can always see exactly what your agent knows, what it's scheduled to do, and why it made the decisions it made. The filesystem is the source of truth. There *are* derived indexes — a tantivy search index and an optional sqlite-vec vector store — but they exist to make memory searchable, not to hold state. They're rebuilt from the plain text files on startup and can be deleted without losing anything.

**Right work in the right place.** LLMs are expensive and inconsistent at deterministic tasks. The gateway handles scheduling, file watching, and schema validation — work that a YAML parser and a timestamp comparison can do for free. The LLM handles judgment: what's worth alerting about, which context is relevant, what to write in a project's notes. Most agent frameworks burn tokens to say "nothing to do." IronClaw only wakes the model when there's something worth thinking about.

**Independent systems that compose through shared data.** Memory, Projects, Pulse scheduling, and Notifications are designed independently. They share a data layer — the workspace filesystem and the observation log — which means improvements to one naturally benefit the others, but they don't depend on each other. You can run Observational Memory without Projects. You can run Pulses without Memory. Each system is valuable on its own. Tight coupling creates fragility; shared data creates opportunity without obligation.

**Autonomy with visibility.** The agent should act on its own — activating project contexts, archiving completed work, adjusting notification behavior, creating scheduled tasks. Requiring permission for routine organizational decisions defeats the purpose of having an agent. But every autonomous action is visible: files the user can read, alerts when something gets archived, routing rules in a YAML file instead of buried in code. The agent has broad autonomy; the user has full visibility.

### Why Rust?

> Single binary, no runtime dependencies, lightweight, reliable, blah blah blah. Rust wasn't the right choice for this project. I had to rebuild boilerplate that there are existing libraries for, cut myself off from a lot of extensibility options, and the single user target means it doesn't get most of the benefits of using the language. But I ***Really*** hate Python and Typescript. I've tried both in many projects, but I was cursed by a computer science major who convinced me to write my first coding project in Rust, and now I can't stand anything else. 
>

## The Workspace

The workspace at `~/.ironclaw/workspace/` is your agent's home. It owns and manages this space — maintaining its own notes, memory, project state, and schedules as it works with you over time.

Everything in the workspace is agent-owned. The agent maintains all of these files as part of normal operation — identity, memory, project notes, pulse schedules, notification routing. You provide initial guidance during onboarding (who you are, what you need, how you communicate), and the agent takes it from there. You can always read and edit any file to correct course, but the goal is that you rarely need to after the first conversation. The only file the agent *can't* touch is `config.toml`, which lives outside the workspace.

```
workspace/
├── SOUL.md                 # Agent persona, boundaries, and identity
├── AGENTS.md               # Operating instructions
├── USER.md                 # User info and preferences
├── MEMORY.md               # Curated long-term memory (agent scratchpad)
├── ENVIRONMENT.md          # Local environment notes
├── HEARTBEAT.yml           # Pulse schedule definitions
├── NOTIFY.yml              # Pulse result routing
├── PRESENCE.toml           # Discord presence configuration
├── scheduled_actions.json  # One-off future tasks (managed via tools)
│
├── memory/                 # Observation log, episodes, search index
│   ├── OBSERVER.md         # Customizable extraction guidance
│   └── REFLECTOR.md        # Customizable compression guidance
│
├── projects/
│   └── my-project/
│       ├── PROJECT.md      # Frontmatter (name, description, tools, MCP servers)
│       ├── notes/          # Agent-maintained notes + session logs
│       ├── references/     # Reference material
│       ├── workspace/      # Agent work output (write-scoped)
│       └── skills/         # Project-scoped skills
│
├── archive/                # Completed projects
├── skills/                 # Workspace-global skills
├── subagents/              # Sub-agent preset definitions
└── inbox/                  # Queued items for later processing
```

Every file is plain text, human-readable, and human-editable. That's a transparency guarantee, not an invitation to manually configure everything. You *can* read and edit anything — and sometimes you'll want to — but the intended workflow is telling your agent what you need and letting it manage the details.

## Extensibility

**Skills** — Skills are packaged instructions that extend the agent's capabilities without code changes. Each skill is a Markdown file (`SKILL.md`) with a name, description, and instructions. The agent sees an index of all available skills (name + description) and activates them dynamically when they're relevant. Skills can live at the workspace level or scoped to a specific project. Want your agent to follow a specific code review process? Write a skill. Want project-specific deployment steps? Drop a skill in that project's `skills/` folder.

**MCP** — IronClaw is a native MCP (Model Context Protocol) client. Configure MCP servers per-project or globally, and their tools are automatically available to the agent alongside built-in tools. This is how IronClaw connects to external services — web search, file systems, APIs, databases — without baking integrations into the core.

**Scheduled Actions** — The agent can schedule its own one-shot tasks: reminders, deferred follow-ups, timed checks. Actions fire once at a specified time, then auto-remove. They persist across restarts. Unlike Pulses (which are recurring task groups on a heartbeat), scheduled actions are lightweight, individual tasks the agent creates on the fly via `schedule_action`.

## Supported Providers

| Provider | Example Models |
|----------|----------------|
| Anthropic | `claude-sonnet-4-6`, `claude-haiku-4-5` |
| OpenAI | `gpt-4o`, `gpt-4o-mini` (also any OpenAI-compatible API: vLLM, LM Studio, Cerebras, etc.) |
| Google Gemini | `gemini-2.5-flash`, `gemini-2.5-pro` |
| Ollama | `llama3`, `mistral`, any local model |

## Getting Started

### Requirements

- Rust 1.85+ (2024 edition)
- An API key for at least one supported provider

### Build & First Run

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

## Configuration Reference

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

## Architecture

```
                    ┌───────────────────────────────────────────┐
  CLI ──────────┐   │              Gateway Server               │
  Discord ──────┤   │                                           │
  Webhook ──────┘   │  ┌─────────┐   ┌──────────┐   ┌────────┐  │
        ─── WS ───► │  │  Agent  │   │  Memory  │   │ Pulse  │  │
                    │  │  Turn   │   │ Observer │   │Sched.  │  │
        ◄── WS ──── │  │  Loop   │   │Reflector │   │        │  │
                    │  └────┬────┘   └──────────┘   └────────┘  │
                    │       │                                   │
                    │  ┌────┴────────────────────────────────┐  │
                    │  │  Tools   MCP   Projects   Skills    │  │
                    │  └─────────────────────────────────────┘  │
                    │                                           │
                    │  ┌──────────────────────────────────────┐ │
                    │  │  LLM Providers                       │ │
                    │  │  Anthropic · OpenAI · Gemini · Ollama│ │
                    │  └──────────────────────────────────────┘ │
                    └───────────────────────────────────────────┘
```

The gateway runs a `tokio::select!` event loop handling: inbound messages, pulse timer, action timer, background task results, observer cooldown, manual observe/reflect triggers, and reload signals.

Subsystems are independent and compose through shared data — the workspace filesystem and the observation log. Memory doesn't import Projects. Skills doesn't import Pulse. They all compose at the Agent layer.

## Contributing

### Testing

```bash
# Run all tests (quiet mode — only shows failures and summary)
cargo test --quiet

# Run a specific integration test
cargo test --test memory_integration --quiet
cargo test --test gateway_integration --quiet
```

Pre-commit hooks enforce `cargo fmt --check`, `cargo clippy -- -D warnings`, and `cargo test` on every commit.

### Project Structure

```
src/
├── main.rs              # Entry point: serve/connect subcommands
├── lib.rs               # Module declarations
├── error.rs             # IronclawError enum
├── agent/               # Agent runtime, context assembly, turn loop
├── background/          # Sub-agent spawning, concurrency, transcripts
├── channels/            # CLI, Discord, WebSocket, webhook adapters
├── config/              # TOML config loading and validation
├── models/              # LLM providers (Anthropic, OpenAI, Gemini, Ollama)
├── gateway/             # WebSocket server and main event loop
├── inbox/               # Inbox item persistence and tools
├── memory/              # Observer, Reflector, search, episode store
├── notify/              # NOTIFY.yml routing and notification channels
├── projects/            # Project scanning, activation, lifecycle
├── pulse/               # HEARTBEAT.yml scheduling and execution
├── actions/             # Scheduled action persistence and scheduling
├── skills/              # SKILL.md discovery, parsing, activation
├── mcp/                 # MCP client and server registry
├── tools/               # Tool trait, built-in tools, policy enforcement
└── workspace/           # Layout conventions, identity files, bootstrap
```

### Design Documents

- [Systems Usage](docs/systems-usage/) — Authoritative reference for how each system is intended to work
- [Design Philosophy](docs/design-philosophy.md) — Guiding principles
- [Architecture Design](docs/ironclaw-design.md) — Full system architecture
- [Memory & Proactivity](docs/personal-agent-design.md) — Observational Memory and Pulse system design
- [Projects System](docs/projects-context-design.md) — Context management design
- [Background Tasks](docs/background-tasks-design.md) — Background task execution and turn loop interrupts
- [Memory Search](docs/memory-search-design.md) — Hybrid BM25 + vector search design
- [Notification Routing](docs/notification-routing-design.md) — NOTIFY.yml channel-based result delivery

## License

Private project. Not currently licensed for redistribution.
