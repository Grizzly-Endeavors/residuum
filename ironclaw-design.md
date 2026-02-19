# IronClaw — Personal AI Agent Gateway

## What This Is

A Rust implementation of a personal AI agent gateway, sharing OpenClaw's core architecture — gateway pattern, channel normalization, file-first workspace, model-agnostic runtime, self-evolving behavior — while making targeted improvements to memory continuity, proactive scheduling, context management, and skill/tool interoperability.

This is not a fork. It's a from-scratch implementation in Rust that preserves architectural compatibility with the OpenClaw ecosystem where it matters (skills, MCP, workspace conventions) and diverges where the language port enables meaningful improvements.

---

## Design Philosophy

Carried forward from the existing design work, restated here as project-wide constraints:

1. **Start from what works.** OpenClaw's gateway pattern, channel normalization, and file-first workspace are sound. Every change targets a specific observed failure mode.
2. **Simplicity that stays practical.** Directory scanning over registries. Flat files over knowledge graphs. If you can understand the system by looking at the filesystem, it's working.
3. **Put the right work in the right place.** The gateway handles scheduling, file watching, schema validation, and protocol mechanics. The LLM handles judgment — what's relevant, what to alert on, what to write.
4. **Independent systems that compose through shared data.** Memory, proactivity, PARA, and skills are designed independently. They share the workspace filesystem and observation log. Each is valuable on its own.
5. **File-first, always.** System state lives in files the user can inspect, edit, and version control. No databases, no opaque embeddings. The filesystem is the source of truth.

---

## Project Structure

```
ironclaw/
├── Cargo.toml
├── src/
│   ├── main.rs                       # Entry point, CLI arg parsing
│   ├── config.rs                     # Configuration loading & validation
│   ├── server.rs                     # WebSocket/HTTP server
│   ├── shutdown.rs                   # Graceful shutdown coordination
│   │
│   ├── channels/
│   │   ├── mod.rs                    # Channel trait definition
│   │   ├── discord.rs                # Serenity-based Discord adapter
│   │   ├── webhook.rs                # Generic incoming webhook channel
│   │   └── cli.rs                    # Local CLI channel (dev/debug)
│   │
│   ├── agent/
│   │   ├── mod.rs                    # Agent runtime orchestration
│   │   ├── context.rs                # Context window assembly
│   │   ├── prompt.rs                 # System prompt builder
│   │   ├── session.rs                # Session state & history
│   │   └── compaction.rs             # Context overflow handling
│   │
│   ├── models/
│   │   ├── mod.rs                    # Provider trait definition
│   │   ├── anthropic.rs              # Claude API
│   │   ├── openai.rs                 # OpenAI-compatible API
│   │   ├── ollama.rs                 # Ollama local models
│   │   ├── gemini.rs                 # Google Gemini API
│   │   └── failover.rs               # Model failover & rotation
│   │
│   ├── memory/
│   │   ├── mod.rs                    # Memory system coordination
│   │   ├── observer.rs               # Tier 1: conversation → observations
│   │   ├── reflector.rs              # Tier 2: observation compaction
│   │   ├── search.rs                 # Hybrid BM25 + vector retrieval
│   │   ├── index.rs                  # Search index management
│   │   └── daily_log.rs              # Legacy daily log compatibility
│   │
│   ├── para/
│   │   ├── mod.rs                    # PARA system coordination
│   │   ├── scanner.rs                # Directory discovery & context.yml parsing
│   │   ├── activation.rs             # Context activation/deactivation logic
│   │   ├── lifecycle.rs              # Create, archive, promote entries
│   │   └── manifest.rs               # Generate file listings for active entries
│   │
│   ├── pulse/
│   │   ├── mod.rs                    # Pulse system coordination
│   │   ├── scheduler.rs              # HEARTBEAT.yml parsing & pulse scheduling
│   │   ├── executor.rs               # Pulse task execution
│   │   └── alerts.rs                 # Alerts.md behavior parsing
│   │
│   ├── cron/
│   │   ├── mod.rs                    # Cron system coordination
│   │   ├── store.rs                  # Job persistence (jobs.json)
│   │   ├── scheduler.rs              # Schedule evaluation (at/every/cron)
│   │   └── executor.rs              # Job execution & delivery
│   │
│   ├── skills/
│   │   ├── mod.rs                    # Skills system coordination
│   │   ├── loader.rs                 # SKILL.md discovery & frontmatter parsing
│   │   ├── registry.rs               # In-memory skill index
│   │   ├── resolver.rs               # Skill selection for prompt injection
│   │   └── validator.rs              # Agent Skills spec validation
│   │
│   ├── mcp/
│   │   ├── mod.rs                    # MCP system coordination
│   │   ├── client.rs                 # JSON-RPC 2.0 client
│   │   ├── transport.rs              # stdio & HTTP/SSE transports
│   │   ├── registry.rs               # Active server tracking & tool union
│   │   └── lifecycle.rs              # Server spawn, health check, teardown
│   │
│   ├── tools/
│   │   ├── mod.rs                    # Tool trait definition
│   │   ├── exec.rs                   # Shell command execution
│   │   ├── read.rs                   # File reading
│   │   ├── write.rs                  # File writing
│   │   ├── web_search.rs             # Web search
│   │   ├── web_fetch.rs              # URL fetching
│   │   ├── browser.rs                # Browser automation (headless)
│   │   └── policy.rs                 # Tool allow/deny + write scope enforcement
│   │
│   └── workspace/
│       ├── mod.rs                    # Workspace management
│       ├── layout.rs                 # Workspace directory conventions
│       ├── watcher.rs                # Filesystem change notifications
│       ├── identity.rs               # SOUL.md, AGENTS.md, USER.md loading
│       └── bootstrap.rs              # First-run workspace scaffolding
│
├── config/
│   └── default.toml              # Default gateway configuration
│
└── docs/
    ├── architecture.md
    ├── design-philosophy.md
    ├── memory-design.md
    ├── para-design.md
    └── skills-mcp.md
```

### Module Boundaries

The project is a single crate compiled to one binary. Module visibility enforces the same boundaries that separate crates would:

- Each module directory exposes its public API through `mod.rs`. Internal types stay private.
- `agent` is the integration point — it imports from `memory`, `para`, `skills`, `mcp`, and `tools` to assemble context.
- `memory` doesn't import `para`. `skills` doesn't import `pulse`. Subsystems are independent and compose at the `agent` layer.
- Shared types (message types, config structs, error types) live at the crate root or in dedicated modules that any subsystem can import.

If any module later needs to become a standalone library (e.g., the MCP client is useful in another project), it can be extracted into its own crate at that point. Start simple, promote when there's a reason.

---

## Gateway Configuration

A single TOML configuration file replaces OpenClaw's `openclaw.json`. TOML over JSON5 because Rust's serde ecosystem handles it cleanly and it supports comments natively.

```toml
# ~/.ironclaw/config.toml

[identity]
name = "Samantha"
emoji = "🦥"
theme = "helpful sloth"

[agent]
workspace = "~/.ironclaw/workspace"
model = "anthropic/claude-sonnet-4-5"

[agent.fallbacks]
models = ["anthropic/claude-haiku-4-5", "ollama/llama3"]

[agent.session]
scope = "per-sender"          # "main" | "per-sender" | "per-channel-peer"

[channels.discord]
token = "${DISCORD_BOT_TOKEN}"
guild_id = "123456789"

[channels.cli]
enabled = true                # Always-available local CLI for dev/debug

[memory]
observer_model = "ollama/gemma3"
observer_threshold_tokens = 30000
reflector_threshold_tokens = 40000

[memory.search]
provider = "local"            # "local" | "openai" | "voyage"

[pulse]
enabled = true
# Pulse definitions live in HEARTBEAT.yml, not here.
# This just controls whether the scheduler runs.

[mcp]
# MCP server definitions can live here or in PARA context.yml entries.
[mcp.servers.filesystem]
command = "mcp-server-filesystem"
args = ["/home/user/documents"]

[skills]
dirs = ["~/.ironclaw/skills", "~/.ironclaw/workspace/skills"]
```

Validation happens at startup via serde + custom validators. Invalid config prevents boot with clear error messages.

### Hot Reload

The gateway watches `config.toml`, workspace identity files, `HEARTBEAT.yml`, `Alerts.md`, and the `para/` directory tree using `notify`. Changes are classified as:

- **Hot-applicable**: Identity file changes, HEARTBEAT.yml updates, skill additions, PARA entry changes. Applied without restart.
- **Infrastructure**: Channel config changes, model provider changes, MCP server config. Require gateway restart (or a targeted subsystem restart).

---

## Workspace Layout

```
~/.ironclaw/workspace/
├── SOUL.md                       # Agent persona, tone, boundaries
├── AGENTS.md                     # Operating instructions for the agent
├── USER.md                       # User info & preferences
├── MEMORY.md                     # Curated long-term memory
├── TOOLS.md                      # Local tool notes
├── HEARTBEAT.yml                 # Structured pulse schedule
├── Alerts.md                     # Alert behavior playbook
│
├── memory/
│   ├── observations.json           # Global observation log (episode-based timeline)
│   ├── episodes/                 # Raw episode transcripts (persisted by Observer)
│   └── YYYY-MM-DD.md             # Daily logs (for explicit note-taking)
│
├── skills/                       # User-defined workspace skills
│   └── my-skill/
│       ├── SKILL.md
│       ├── scripts/
│       └── references/
│
├── para/
│   ├── projects/
│   │   └── aerohive-setup/
│   │       ├── context.yml
│   │       ├── notes/
│   │       ├── references/
│   │       └── workspace/
│   ├── areas/
│   │   └── homelab/
│   │       ├── context.yml
│   │       ├── notes/
│   │       └── references/
│   ├── resources/
│   │   └── ansible-patterns/
│   │       ├── context.yml
│   │       ├── notes/
│   │       └── references/
│   └── archive/
│       └── proxmox-migration/
│           ├── context.yml
│           ├── notes/
│           ├── references/
│           └── workspace/
│
├── cron/
│   └── jobs.json                 # Agent-created scheduled jobs
│
└── hooks/                        # Optional user-defined hooks
```

### Files the gateway parses structurally

These are YAML/TOML files the Rust gateway validates and acts on:

| File | Format | Gateway action |
|------|--------|---------------|
| `config.toml` | TOML | Full gateway configuration |
| `HEARTBEAT.yml` | YAML | Pulse scheduling, task definitions |
| `para/**/context.yml` | YAML | PARA entry metadata, tool/skill/MCP resolution |
| `cron/jobs.json` | JSON | Agent-created scheduled wake-ups |
| `memory/observations.json` | JSON | Global observation log (episode-based) |

### Files injected as system prompt content

These are markdown files the gateway loads verbatim and inserts into the LLM context window:

| File | When loaded |
|------|-------------|
| `SOUL.md` | Always |
| `AGENTS.md` | Always |
| `USER.md` | Always (DM sessions) |
| `MEMORY.md` | Always (DM sessions) |
| `TOOLS.md` | Always |
| `Alerts.md` | When pulse tasks are being evaluated |
| `memory/observations.json` | Always (global timeline) |

### Files available via agent tool calls (progressive disclosure)

These files are never auto-loaded into context. The agent knows they exist (via PARA manifests or skill metadata) and reads them on demand via the `read` tool:

| File | When available |
|------|---------------|
| `para/**/notes/*` | When PARA entry is active (listed in manifest) |
| `para/**/references/*` | When PARA entry is active (listed in manifest) |
| `para/**/workspace/*` | When PARA entry is active (listed in manifest) |
| `skills/**/SKILL.md` (body) | When skill metadata is in prompt (agent reads to activate) |
| `memory/episodes/*.md` | Always (via `read` tool or `memory_search`) |
| `skills/**/scripts/*` | After agent has read the SKILL.md |
| `skills/**/references/*` | After agent has read the SKILL.md |

The distinction matters: parsed files have schemas and validation. Prompt files are opaque markdown the gateway doesn't interpret.

---

## Subsystem Designs

### 1. Channel System (`channels/`)

Channels are the inbound/outbound message interface. Each channel adapter implements a trait:

```rust
#[async_trait]
pub trait Channel: Send + Sync {
    /// Unique identifier for this channel type
    fn id(&self) -> &str;

    /// Start receiving messages. Sends normalized messages to the gateway.
    async fn start(&self, tx: mpsc::Sender<InboundMessage>) -> Result<()>;

    /// Send a message out through this channel.
    async fn send(&self, msg: OutboundMessage) -> Result<()>;

    /// Graceful shutdown.
    async fn shutdown(&self) -> Result<()>;
}
```

**Initial channels:**

- **Discord** — via serenity. Primary channel. Supports rich embeds, threads, reactions.
- **CLI** — stdin/stdout local channel. Always available. Essential for development and debugging.
- **Webhook** — HTTP endpoint for incoming messages. Enables integration with arbitrary services.

Additional channels (Telegram, Signal, etc.) can be added later as separate adapter implementations. The trait boundary means the gateway doesn't care.

**Message normalization:**

All inbound messages are converted to a common `InboundMessage` type:

```rust
pub struct InboundMessage {
    pub channel: String,          // "discord", "cli", "webhook"
    pub sender: Sender,           // Normalized sender identity
    pub content: MessageContent,  // Text, attachments, etc.
    pub metadata: MessageMeta,    // Channel-specific metadata
    pub timestamp: DateTime<Utc>,
}
```

**Routing:**

In single-agent mode (the default and expected primary use case), all messages route to the one agent. Multi-agent routing via bindings can be added later without changing the channel trait.

### 2. Model Providers (`models/`)

Model providers handle LLM communication. Each implements:

```rust
#[async_trait]
pub trait ModelProvider: Send + Sync {
    fn id(&self) -> &str;

    async fn complete(&self, request: CompletionRequest) -> Result<CompletionResponse>;

    /// Whether this provider supports streaming responses.
    fn supports_streaming(&self) -> bool;

    /// Streaming completion (optional).
    async fn complete_stream(
        &self,
        request: CompletionRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamChunk>> + Send>>>;
}
```

**Initial providers:**

- **Anthropic** — Claude API via reqwest. Existing Rust connector.
- **OpenAI** — OpenAI-compatible API. Existing Rust connector. Also covers any OpenAI-compatible local server.
- **Ollama** — Ollama REST API. Existing Rust connector.
- **Gemini** — Google Gemini API. Existing Rust connector.

**Failover:**

The `failover` module wraps providers with retry logic. Configuration defines a primary model and ordered fallbacks. On rate limit or error, the next provider in the chain is tried. Auth profile rotation (multiple API keys for the same provider) is handled within each provider adapter.

**Model selection for subsystems:**

Different subsystems can use different models:

| Subsystem | Default model | Rationale |
|-----------|--------------|-----------|
| Agent (main conversation) | User-configured primary | Frontier reasoning |
| Observer | Cheap/fast (e.g., Gemini Flash, local) | Extraction, not reasoning |
| Reflector | Cheap/fast | Reorganization, not generation |
| Pulse evaluation | User-configured primary | Needs judgment |

### 3. Agent Runtime (`agent/`)

The agent runtime is the core orchestration loop. On each turn:

1. **Receive** normalized inbound message.
2. **Assemble context** — system prompt + identity files + observation log + active PARA contexts + relevant skills + session history.
3. **Send** to model provider.
4. **Execute** any tool calls returned by the model.
5. **Loop** if tool results need to be sent back to the model.
6. **Deliver** final response through the originating channel.
7. **Update** — append to session history, trigger Observer if token threshold reached.

#### Context Assembly (`context.rs`)

Context assembly is the critical integration point. It builds the full prompt from independent subsystems:

```
┌─────────────────────────────────────┐
│ System prompt                       │
│ ├── SOUL.md                         │
│ ├── AGENTS.md                       │
│ ├── TOOLS.md                        │
│ └── Available skill metadata        │  ← names + descriptions only
├─────────────────────────────────────┤
│ User context                        │
│ ├── USER.md                         │
│ └── MEMORY.md                       │
├─────────────────────────────────────┤
│ Observation log                     │  ← from memory crate
│ └── memory/observations.json        │  ← global timeline (always loaded)
├─────────────────────────────────────┤
│ PARA index (always present)         │  ← from para crate
│ └── name + description per entry    │
├─────────────────────────────────────┤
│ Active PARA manifests               │  ← from para crate
│ └── File listings (not contents)    │
│     for each activated entry        │
├─────────────────────────────────────┤
│ Available tools                     │
│ ├── Built-in tools                  │  ← from tools crate
│ ├── MCP server tools                │  ← from mcp crate
│ └── PARA-scoped tools               │  ← from para + tools
├─────────────────────────────────────┤
│ Session history                     │
│ └── Raw messages (current session)  │
│     (includes any file contents     │
│      the agent has read via tools)  │
└─────────────────────────────────────┘
```

**Token budget management:**

The context assembler tracks token usage across sections. Progressive disclosure means the baseline context cost is predictable and lean:

1. System prompt + identity files — always loaded (non-negotiable).
2. Global observation log — always loaded (high-level timeline).
3. PARA index — always loaded (lightweight, ~50-100 tokens per entry).
4. Active PARA manifests — loaded when entries are active (file listings, not contents).
5. Available skill metadata — always loaded (names + descriptions only).
6. Session history — loaded newest-first, truncated from oldest.

File contents, skill instructions, and reference material enter the context window only when the agent reads them via tool calls — at which point they become part of the session history and are subject to the same truncation rules as any other message. This keeps the baseline context small and gives the agent control over what fills the remaining budget.

Token counting uses tiktoken-rs or a provider-specific tokenizer.

### 4. Memory System (`memory/`)

Implements the Observational Memory design from `personal-agent-design.md`.

#### Observer (`observer.rs`)

A background task that watches accumulated raw messages. When unobserved tokens exceed the configured threshold (~30k default):

1. Collect all unobserved messages, tool calls, and results.
2. Send to the observer model with extraction instructions.
3. Receive a dated, structured episode — an ID, time range, and extracted observations.
4. Persist the raw transcript as an episode file under `memory/episodes/<id>.md`.
5. Append the episode to `memory/observations.json`.
6. Mark processed messages as observed.

The Observer always writes to the single global observation log. If a PARA project is active at the time of compression, the episode is tagged with a `context` field identifying it. This is metadata for searchability, not a routing mechanism.

The observer model is configured separately from the main agent model. Default: cheap, fast, high-throughput.

**Observation format** (appended to `observations.json`):

```json
{
  "id": "ep-001",
  "date": "2026-02-18",
  "start": "12:10",
  "end": "12:45",
  "context": "aerohive-setup",
  "observations": [
    "Working on Ansible playbook for AeroHive AP configuration",
    "Decided to use host_vars over group_vars for per-AP channel assignment",
    "Hit issue: aoscli module not recognizing enable mode — workaround using raw shell",
    "User correction: AeroHive uses HiveManager CLI, not aoscli"
  ]
}
```

The `context` field is optional — episodes generated outside any active project have no context tag. This lets the agent filter the observation log and episode transcripts by PARA entry when searching for project-specific history.

**Episode transcript** (persisted to `memory/episodes/ep-001.md`):

Episode transcripts use markdown with YAML frontmatter, consistent with skills and other human-facing files in the workspace. The frontmatter carries the episode metadata; the body contains the raw messages, tool calls, and results that were compressed into this episode.

```markdown
---
id: ep-001
date: 2026-02-18
start: "12:10"
end: "12:45"
context: aerohive-setup
---

## Messages

[Raw conversation transcript, tool calls, and results...]
```

This is the full record the agent can retrieve via `read` tool or `memory_search` when the observation log's compressed view isn't enough detail. The `observations.json` log itself remains JSON since the gateway parses it structurally, but the persisted transcripts benefit from the more readable format.

**Prompt cache optimization:** The observation log is a prefix that grows append-only between Observer runs. This enables prompt cache hits on the stable prefix across turns within a session.

#### Reflector (`reflector.rs`)

When the global observation log exceeds its threshold (~40k tokens default):

1. Send the full observation log to the reflector model.
2. Receive a reorganized, compressed version — still dated, still episode-based, but denser. Each reflected episode includes a `source_episodes` field listing the IDs of the episodes it was compacted from. The `context` tags from source episodes are preserved on reflected episodes.
3. Replace the observation log contents. Original episode transcripts remain in `memory/episodes/` — the Reflector compresses the log, not the raw record.

The Reflector operates on the single global observation log. There are no per-project logs to manage independently.

The Reflector is the only operation that fully invalidates the prompt cache for the observation prefix. Acceptable given its infrequency.

**Key constraint:** The Reflector does not summarize. It reorganizes, merges related items, and drops superseded information while preserving the chronological episode-based structure. The `source_episodes` field on reflected episodes preserves the retrieval trail back to the original transcripts.

#### Search (`search.rs`)

Hybrid retrieval over workspace files using BM25 + vector similarity:

- Indexes `memory/` daily logs, `memory/episodes/` episode transcripts, PARA notes and references, archived PARA entries.
- Episode transcripts are a primary search target — they contain the full raw detail that the observation log compressed away. The agent can follow episode IDs from the observation log to retrieve specific transcripts, or search across all transcripts when the observation log doesn't have enough context.
- Available as a tool the agent can invoke for deep retrieval beyond the observation window.
- Vector embeddings stored as local files (no external database).

### 5. PARA Context Management (`para/`)

Implements the PARA design from `para-context-design.md`.

#### Scanner (`scanner.rs`)

On startup and on filesystem change:

1. Walk `para/projects/`, `para/areas/`, `para/resources/`, `para/archive/`.
2. Parse each `context.yml` with serde_yaml, validate against schema.
3. Build in-memory index: name, description, status, category, tools, skills, MCP servers.

The index is cheap to build (a few lines of YAML per entry) and always current.

#### context.yml schema

```yaml
name: aerohive-setup
description: "AeroHive AP network configuration using Ansible"
category: project                 # project | area | resource
status: active                    # active | available | archived
created: 2026-02-10

# Optional: capabilities loaded when this context activates
tools:
  - exec
  - read
  - write

skills:
  - ansible-playbooks

mcp_servers:
  - name: filesystem
    command: "mcp-server-filesystem"
    args: ["/home/user/ansible/aerohive"]

# Archive-only fields
archived: null
original_category: null
```

Note the addition of `skills` and `mcp_servers` fields beyond the original PARA design. These bind scoped capabilities to specific contexts — when the project activates, its skills load into the prompt and its MCP servers spin up. When it deactivates, they unload.

#### Activation (`activation.rs`)

PARA activation follows a progressive disclosure model. The gateway never bulk-loads file contents on activation. Instead, it tells the agent what's available and lets the agent decide what to read.

**Single-project constraint:** Only one project may be active at a time. Activating a new project automatically deactivates the current one. Areas and resources have no such constraint — multiple can be active simultaneously alongside the active project.

**Workspace write scoping:**

When a project is active, project output stays within the project's `workspace/` folder. However, the agent can always write to global files — MEMORY.md, area notes, resource notes, and the global observation log — without requiring project deactivation.

| State | Scoped writes | Always writable | Read-only |
|-------|---------------|-----------------|-----------|
| Project active | `para/projects/<n>/workspace/**` | `MEMORY.md`, `memory/**`, area notes, resource notes, project notes | Identity files (SOUL.md, AGENTS.md), archive |
| No project active | — | Global workspace (memory, MEMORY.md, area notes, etc.) | Archive |

This means if you're working on the AeroHive project and say "add a note about my dentist appointment to my health area," the agent writes the note to the health area directly. The episode generated during that interaction gets tagged with `"context": "aerohive-setup"` in the global observation log, preserving the record of what was active at the time.

**Three tiers of progressive disclosure:**

1. **Always present** — The lightweight PARA index (name + description + status for every entry) is always in the system prompt. The agent always knows what contexts exist. Cost: ~50-100 tokens per entry.

2. **On activation** — When the agent activates a PARA entry (via tool call or conversational cue), the gateway:
   - Loads the entry's **manifest**: a listing of what files exist in `notes/`, `references/`, and `workspace/` (filenames and sizes, not contents).
   - Starts any **MCP servers** defined in the entry's `context.yml`.
   - Adds any **tools** defined in the entry's `context.yml` to the active tool set.
   - Adds the entry's **skills** to the available skills list (metadata only, not full SKILL.md bodies).
   
   The manifest tells the agent "here's what's in this project folder." Contents are not loaded.

3. **On agent request** — The agent reads specific files by invoking the `read` tool. This applies to:
   - Notes files (`notes/decisions.md`, `notes/current-state.md`)
   - Reference files (`references/topology.png`, `references/ap01.conf`)
   - Workspace files (`workspace/playbooks/configure-aps.yml`)
   - Skill instructions (the full SKILL.md body, loaded when the agent decides the skill is relevant)

**Why this matters:**

A project might have 20 files across notes, references, and workspace. Loading all of them on activation would consume thousands of tokens — most of which are irrelevant to the current question. Progressive disclosure lets the agent read `notes/current-state.md` when it needs project status, or `references/ap01.conf` when debugging a specific AP, without carrying everything else.

This also aligns with how the Agent Skills spec works: skill metadata is always visible (~100 tokens), but the full SKILL.md body (~5000 tokens) loads only when the agent decides to activate that skill.

**Tool and MCP server auto-loading is the exception.** Tools and MCP servers load immediately on activation because they define *capabilities*, not *knowledge*. The agent needs to know what it can do — it doesn't need to read every file to know that.

**Tool union resolution:** When multiple active entries specify tools, the union is available. Deactivating an entry only removes tools no other active entry still needs.

#### Lifecycle (`lifecycle.rs`)

- **Create**: `mkdir` + write `context.yml` + create subfolders. No registry.
- **Archive**: Update status, add archived date and original_category, move to `para/archive/`. Archived projects retain their notes, references, and workspace. Observation history is not carried with the project — it lives in the global observation log and is retrievable via the `context` tag on episodes.
- **Promote**: Resource → Project (add workspace/), Project → Area (drop workspace/). Move between category directories.

### 6. Pulse Scheduling (`pulse/`)

Implements the structured pulse system from `personal-agent-design.md`.

#### Scheduler (`scheduler.rs`)

Parses `HEARTBEAT.yml` and manages per-pulse timing:

```yaml
pulses:
  - name: work_check
    enabled: true
    schedule: "30m"
    active_hours: "08:00-18:00"
    tasks:
      - name: inbox_scan
        prompt: "Check for urgent unread emails"
        alert: high
      - name: pr_review
        prompt: "Any PRs waiting on my review?"
        alert: low

  - name: daily_review
    enabled: true
    schedule: "24h"
    active_hours: "08:00-09:00"
    tasks:
      - name: morning_brief
        prompt: "Summarize today's calendar and top priorities"
        alert: medium
```

The scheduler runs on a tokio interval timer. Each tick:

1. Check which pulses are due based on schedule, active_hours, `enabled` flag, and last-run timestamps.
2. If a pulse is due, invoke the agent with the pulse's tasks as context.
3. Track the result. If HEARTBEAT_OK (nothing actionable), log silently.
4. If actionable, apply alert behavior from Alerts.md.
5. Update last-run timestamp.

**Zero cost when idle.** If no pulses are due, no LLM invocation happens.

#### Alerts (`alerts.rs`)

Parses `Alerts.md` for notification behavior at each level. This is treated as prompt content for the agent, not structurally parsed — the agent reads it and exercises judgment. The gateway only needs to know the alert level to decide delivery channel and timing.

### 7. Cron System (`cron/`)

A direct port of OpenClaw's cron job system. Cron gives the agent the ability to schedule its own wake-ups — one-shot reminders, recurring tasks, deferred follow-ups. Where pulses are user-defined ambient monitoring (declarative YAML, LLM-evaluated), cron jobs are agent-created scheduled actions (created via tool calls, persisted as JSON, executed by the gateway).

Three schedule types: `at` (one-shot at a timestamp), `every` (fixed interval), and `cron` (standard 5-field cron expressions with optional timezone). Jobs persist under the workspace at `cron/jobs.json` and survive gateway restarts.

Two execution modes: **main session** (enqueue a system event picked up on the next pulse/heartbeat — agent has full context) and **isolated** (dedicated agent turn in a fresh session — can use a different model, supports delivery to a channel or webhook). The agent manages jobs via `cron_add`, `cron_update`, `cron_remove`, and `cron_list` tool calls.

This is architecturally identical to OpenClaw's implementation. The design details are documented in [OpenClaw's cron docs](https://docs.openclaw.ai/automation/cron-jobs) and don't need to be restated here.

### 8. Skills System (`skills/`)

Implements Agent Skills spec compatibility (agentskills.io).

#### Loader (`loader.rs`)

Discovers skills from configured directories:

1. Walk each skills directory.
2. For each subdirectory containing a `SKILL.md`, parse YAML frontmatter.
3. Validate against the Agent Skills spec: name format, required fields, constraints.
4. Build in-memory skill index: name, description, metadata, file path.

**Skill sources** (precedence, highest first):

1. Workspace skills: `~/.ironclaw/workspace/skills/`
2. PARA-scoped skills referenced in `context.yml`
3. User-global skills: `~/.ironclaw/skills/`
4. Bundled skills (shipped with the binary)

#### Resolver (`resolver.rs`)

Skills follow the same progressive disclosure model as PARA:

1. **Always present**: All available skill metadata (name + description, ~100 tokens each) is in the system prompt. The agent always knows what skills exist.
2. **Agent-driven activation**: When the agent decides a skill is relevant, it reads the SKILL.md body via the `read` tool. The full instructions (~5000 tokens recommended max) enter the context as part of session history.
3. **Supporting files**: After reading a skill's SKILL.md, the agent can read files from `scripts/`, `references/`, and `assets/` as needed.

The gateway's role is indexing and making skills discoverable. The agent's role is deciding which skills to load and when. If a skill specifies `allowed-tools`, those tools become available once the agent has read the skill.

#### Compatibility

Skills from these sources work without modification:

- Anthropic's `anthropics/skills` repository
- OpenClaw workspace skills and ClawHub skills
- OpenAI Codex CLI skills
- SkillsMP marketplace
- Any skill following the Agent Skills spec at agentskills.io

### 9. MCP Client (`mcp/`)

Implements the MCP client protocol for connecting to external tool servers.

#### Client (`client.rs`)

JSON-RPC 2.0 client that communicates with MCP servers:

1. **Initialize**: Capability negotiation handshake with the server.
2. **List tools**: Discover available tools and their schemas.
3. **Call tool**: Invoke a tool with arguments, receive results.
4. **List resources**: Discover available data resources.
5. **Read resource**: Retrieve resource content.

#### Transport (`transport.rs`)

Two transport mechanisms:

- **stdio**: Spawn MCP server as child process, communicate over stdin/stdout. Default for local servers.
- **HTTP/SSE**: Connect to remote MCP servers via HTTP with Server-Sent Events for streaming. For hosted/remote servers.

#### Lifecycle (`lifecycle.rs`)

MCP servers are managed as child processes:

- **Spawn**: Start the server process when its context activates (either globally configured or via PARA entry).
- **Health check**: Periodic ping to verify the server is responsive.
- **Teardown**: Graceful shutdown when context deactivates or gateway shuts down.

#### Registry (`registry.rs`)

Maintains the set of active MCP servers and their combined tool lists:

- Tools from all active MCP servers are unioned with built-in tools.
- Tool name conflicts are resolved by precedence (built-in > MCP, or configurable).
- When an MCP server deactivates, its tools are removed from the active set (unless another active server provides them).

#### MCP server sources

MCP servers can be configured at two levels:

1. **Global** (`config.toml` `[mcp.servers]`): Always available.
2. **PARA-scoped** (`context.yml` `mcp_servers`): Available only when the PARA entry is active.

### 10. Tool System (`tools/`)

Built-in tools the agent can invoke directly.

#### Core tools

| Tool | Description |
|------|-------------|
| `exec` | Execute shell commands |
| `read` | Read file contents |
| `write` | Write/create files |
| `edit` | String replacement in files |
| `web_search` | Search the web |
| `web_fetch` | Fetch URL contents |
| `browser` | Headless browser automation |
| `memory_search` | Hybrid retrieval over workspace |
| `cron_add` | Schedule a one-shot or recurring agent wake-up |
| `cron_update` | Modify an existing cron job |
| `cron_remove` | Delete a cron job |
| `cron_list` | List scheduled cron jobs |

#### Policy (`policy.rs`)

Cascading tool policy resolution:

1. **Global defaults** from config.
2. **Per-PARA-entry** overrides from `context.yml` `tools` field.
3. **Per-skill** additions from `allowed-tools` in SKILL.md frontmatter.
4. **MCP server tools** from active MCP connections.

The active tool set at any moment is the union of all sources, filtered by deny lists.

**Write scope enforcement:** The `write`, `edit`, and `exec` tools enforce workspace write scoping. When a project is active, project output (generated files, build artifacts) is scoped to the project's `workspace/` directory. Global files (MEMORY.md, area/resource notes, observation log) remain writable. Identity files and archive are always read-only. The gateway enforces these constraints via path validation in the tool implementation, not by relying on LLM judgment.

---

## Data Flow

### Inbound message → response

```
Discord ──→ Channel adapter ──→ Normalized message
                                      │
                                      ▼
                              Session resolution
                                      │
                                      ▼
                              Context assembly
                              ├── Identity files
                              ├── Observation log
                              ├── Active PARA contexts
                              ├── Activated skills
                              ├── Available tools (built-in + MCP)
                              └── Session history
                                      │
                                      ▼
                              Model provider ──→ LLM
                                      │
                                      ▼
                              Response / Tool calls
                                      │
                              ┌───────┴───────┐
                              │               │
                         Tool calls      Text response
                              │               │
                         Execute tools   Deliver via
                         (built-in,      channel adapter
                          MCP, scripts)       │
                              │               ▼
                              └──→ Loop back  Discord
                                   if needed
```

### Pulse evaluation

```
Scheduler tick
      │
      ▼
Check due pulses (HEARTBEAT.yml + timestamps)
      │
      ├── Nothing due → no-op (zero cost)
      │
      └── Pulse due → Agent turn with pulse tasks
                          │
                          ▼
                    LLM evaluates tasks
                          │
                    ┌─────┴──────┐
                    │            │
              HEARTBEAT_OK   Findings
                    │            │
              Log silently   Apply alert level
                              (Alerts.md behavior)
                                   │
                              ┌────┴────┐
                              │         │
                           High      Low
                              │         │
                        Notify now  Log to observations
                        via channel  (surface later
                                     if relevant)
```

### Observer compression

```
Raw session messages accumulate
            │
            ▼
Token count exceeds threshold (~30k)
            │
            ▼
Observer model extracts episode
(id, date, time range, context tag, observations)
            │
            ▼
Persist raw transcript
to memory/episodes/<id>.md
            │
            ▼
Append episode to
memory/observations.json
(tagged with active PARA context, if any)
            │
            ▼
Raw messages marked as observed
(dropped from context on next assembly)
```

---

## Key Dependencies

| Crate | Purpose |
|-------|---------|
| `tokio` | Async runtime |
| `serde` / `serde_yaml` / `toml` | Serialization for configs |
| `serenity` | Discord gateway & API |
| `reqwest` | HTTP client for model APIs and web tools |
| `notify` | Filesystem watching |
| `walkdir` | Directory traversal for PARA/skills scanning |
| `tantivy` | BM25 full-text search for memory |
| `axum` | HTTP server for webhook channel & API |
| `tracing` | Structured logging |
| `tiktoken-rs` | Token counting |

---

## What's Not Included (Yet)

Things deliberately scoped out of the initial implementation:

- **Multi-agent routing** — Single-agent mode only. The architecture supports it (the channel trait and session system are agent-agnostic) but the routing layer isn't built.
- **Companion apps** — No macOS menu bar, iOS/Android nodes. CLI channel covers the dev use case.
- **Canvas / A2UI** — No visual workspace. Text-only interactions.
- **Voice** — No TTS/STT integration. Text channels only.
- **ClawHub integration** — Skills are loaded from local directories. No registry API client.
- **Plugin system** — Channels, providers, and tools are compiled in. A dynamic plugin system (WASM or subprocess) can be added later if extensibility is needed.
- **Migration tooling** — No automated migration from an existing OpenClaw workspace. Manual setup or a one-time script.

---

## Implementation Priorities

Ordered by "what gets you a usable agent fastest":

### Phase 1: Core loop (COMPLETE)
1. Shared types — Message types, config types, error handling (crate-root modules).
2. `workspace` — Layout conventions, identity file loading, bootstrap.
3. `models` — Anthropic + Ollama providers (use existing connectors).
4. `channels/cli` — Local CLI channel.
5. `agent` — Basic runtime: context assembly from identity files + session history, model call, tool execution loop.
6. `tools` — `read`, `write`, `exec` (minimum viable tool set).
7. `main.rs` + `config.rs` — Config loading, startup, wire everything together.

**Milestone: You can talk to your agent via CLI.**

### Phase 2: Memory & continuity (COMPLETE)
8. `memory/observer` — Tier 1 compression.
9. `memory/daily_log` — Daily log writing for explicit notes.
10. `memory/reflector` — Tier 2 compression.
11. `memory/search` — Hybrid retrieval (tantivy + embeddings).

**Milestone: Agent remembers context across sessions.**

### Phase 3: Proactivity
12. `pulse/scheduler` — HEARTBEAT.yml parsing, scheduling loop.
13. `pulse/executor` — Pulse task execution via agent runtime.
14. `pulse/alerts` — Alert level behavior.
15. `cron/store` — Job persistence, `cron/scheduler` — schedule evaluation.
16. `cron/executor` — Job execution, isolated sessions, delivery.

**Milestone: Agent proactively checks on things, notifies you, and can schedule its own wake-ups.**

### Phase 4: Discord & channels
17. `channels/discord` — Serenity integration, rich messages.
18. `channels/webhook` — Incoming webhook support.

**Milestone: Agent is accessible via Discord.**

### Phase 5: PARA
19. `para/scanner` — Directory discovery, context.yml parsing.
20. `para/activation` — Context activation/deactivation via agent tool calls.
21. `para/manifest` — Generate file listings for active entries.
22. `para/lifecycle` — Create, archive, promote.

**Milestone: Agent manages structured project/area/resource contexts with progressive disclosure.**

### Phase 6: Skills & MCP
23. `skills/loader` — SKILL.md discovery and parsing.
24. `skills/resolver` — Skill activation and prompt injection.
25. `mcp/client` — JSON-RPC client, stdio transport.
26. `mcp/lifecycle` — Server spawn and teardown.
27. `mcp/transport` — HTTP/SSE transport for remote servers.
28. Integration: PARA context.yml → skill and MCP server activation.

**Milestone: Agent can use OpenClaw-compatible skills and connect to MCP servers.**
