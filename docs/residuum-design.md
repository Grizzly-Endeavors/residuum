# Residuum — Personal AI Agent Gateway

## What This Is

A Rust implementation of a personal AI agent gateway, sharing OpenClaw's core architecture — gateway pattern, channel normalization, file-first workspace, model-agnostic runtime, self-evolving behavior — while making targeted improvements to memory continuity, proactive scheduling, context management, and skill/tool interoperability.

This is not a fork. It's a from-scratch implementation in Rust that preserves architectural compatibility with the OpenClaw ecosystem where it matters (skills, MCP, workspace conventions) and diverges where the language port enables meaningful improvements.

---

## Design Philosophy

Carried forward from the existing design work, restated here as project-wide constraints:

1. **Start from what works.** OpenClaw's gateway pattern, channel normalization, and file-first workspace are sound. Every change targets a specific observed failure mode.
2. **Simplicity that stays practical.** Directory scanning over registries. Flat files over knowledge graphs. If you can understand the system by looking at the filesystem, it's working.
3. **Put the right work in the right place.** The gateway handles scheduling, file watching, schema validation, and protocol mechanics. The LLM handles judgment — what's relevant, what to alert on, what to write.
4. **Independent systems that compose through shared data.** Memory, proactivity, Projects, and skills are designed independently. They share the workspace filesystem and observation log. Each is valuable on its own.
5. **File-first, always.** System state lives in files the user can inspect, edit, and version control. No databases, no opaque embeddings (exception: `vectors.db` — a SQLite + sqlite-vec store for vector embeddings, since raw vectors aren't human-parsable). The filesystem is the source of truth.

---

## Project Structure

```
residuum/
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
│   │   ├── recent_messages.rs        # In-memory message history
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
│   │   ├── search.rs                 # BM25 full-text retrieval (tantivy)
│   │   ├── index.rs                  # Search index management
│   │   └── episode_store.rs           # Episode transcript persistence
│   │
│   ├── projects/
│   │   ├── mod.rs                    # Projects system coordination
│   │   ├── scanner.rs                # Directory discovery & PROJECT.md frontmatter parsing
│   │   ├── activation.rs             # Context activation/deactivation logic
│   │   ├── lifecycle.rs              # Create and archive entries
│   │   └── manifest.rs               # Generate file listings for the active entry
│   │
│   ├── pulse/
│   │   ├── mod.rs                    # Pulse system coordination
│   │   ├── scheduler.rs              # HEARTBEAT.yml parsing & pulse scheduling
│   │   ├── executor.rs               # Pulse task execution
│   │   └── router.rs                 # CHANNELS.yml parsing & channel dispatch
│   │
│   ├── actions/
│   │   ├── mod.rs                    # Scheduled actions coordination
│   │   ├── store.rs                  # Action persistence (scheduled_actions.json)
│   │   └── types.rs                  # Action types and serialization
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
│   │   ├── transport.rs              # stdio transport (HTTP/SSE planned)
│   │   ├── registry.rs               # Active server tracking & tool union
│   │   └── lifecycle.rs              # Server spawn, health check, teardown
│   │
│   ├── tools/
│   │   ├── mod.rs                    # Tool trait definition
│   │   ├── exec.rs                   # Shell command execution
│   │   ├── read.rs                   # File reading
│   │   ├── write.rs                  # File writing
│   │   ├── actions.rs                # Scheduled action tools
│   │   ├── memory_search.rs          # Memory search tool
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
    ├── projects-design.md
    └── skills-mcp.md
```

### Module Boundaries

The project is a single crate compiled to one binary. Module visibility enforces the same boundaries that separate crates would:

- Each module directory exposes its public API through `mod.rs`. Internal types stay private.
- `agent` is the integration point — it imports from `memory`, `projects`, `skills`, `mcp`, and `tools` to assemble context.
- `memory` doesn't import `projects`. `skills` doesn't import `pulse`. Subsystems are independent and compose at the `agent` layer.
- Shared types (message types, config structs, error types) live at the crate root or in dedicated modules that any subsystem can import.

If any module later needs to become a standalone library (e.g., the MCP client is useful in another project), it can be extracted into its own crate at that point. Start simple, promote when there's a reason.

---

## Gateway Configuration

A single TOML configuration file replaces OpenClaw's `openclaw.json`. TOML over JSON5 because Rust's serde ecosystem handles it cleanly and it supports comments natively.

```toml
# ~/.residuum/config.toml

# Top-level user settings
name = "Samantha"
timezone = "America/New_York"
workspace_dir = "~/.residuum/workspace"

# Named provider definitions
[providers.anthropic]
type = "anthropic"
api_key = "${ANTHROPIC_API_KEY}"

[providers.gemini]
type = "openai"               # Gemini uses OpenAI-compatible endpoint
api_key = "${GEMINI_API_KEY}"
url = "https://generativelanguage.googleapis.com/v1beta/openai"

# Role → model assignments
[models]
main = "anthropic/claude-sonnet-4-6"
default = "anthropic/claude-haiku-4-5"   # fallback for unset roles
observer = "gemini/gemini-2.5-flash"
# reflector, pulse default to `default` if unset
# embedding = "openai/text-embedding-3-small"  # optional, enables vector search

# Memory thresholds (provider assignments come from [models])
[memory]
observer_threshold_tokens = 30000
reflector_threshold_tokens = 40000
observer_cooldown_secs = 120             # cooldown after soft threshold
observer_force_threshold_tokens = 60000  # bypasses cooldown

[memory.search]
vector_weight = 0.7
text_weight = 0.3
min_score = 0.35

[discord]
token = "${DISCORD_BOT_TOKEN}"

[pulse]
enabled = true
# Pulse definitions live in HEARTBEAT.yml, not here.

[mcp]
# MCP server definitions can live here or in project PROJECT.md frontmatter.
[mcp.servers.filesystem]
command = "mcp-server-filesystem"
args = ["/home/user/documents"]

[skills]
dirs = ["~/.residuum/skills"]

[notifications.channels.ntfy]
type = "ntfy"
url = "https://ntfy.sh"
topic = "residuum"

# External notification channels are defined here.
# Built-in channels (agent_wake, agent_feed, inbox) need no config.
# CHANNELS.yml in the workspace defines the channel registry.

[background]
max_concurrent = 3

[background.models]
small = "gemini/gemini-2.5-flash"
medium = "anthropic/claude-haiku-4-5"
# large defaults to main agent model

[retry]
max_retries = 3
initial_delay_ms = 500
```

Validation happens at startup via serde + custom validators. Invalid config prevents boot with clear error messages.

### Hot Reload

The gateway watches `config.toml`, workspace identity files, `HEARTBEAT.yml`, `CHANNELS.yml`, and the `projects/` directory tree using `notify`. Changes are classified as:

- **Hot-applicable**: Identity file changes, HEARTBEAT.yml updates, skill additions, project entry changes. Applied without restart.
- **Infrastructure**: Channel config changes, model provider changes, MCP server config. Require gateway restart (or a targeted subsystem restart).

---

## Workspace Layout

```
~/.residuum/workspace/
├── SOUL.md                       # Agent persona, tone, boundaries
├── AGENTS.md                     # Operating instructions for the agent
├── USER.md                       # User info & preferences
├── MEMORY.md                     # Curated long-term memory
├── ENVIRONMENT.md                # Local environment notes
├── HEARTBEAT.yml                 # Structured pulse schedule
├── CHANNELS.yml                  # Channel registry
├── PRESENCE.toml                 # Hot-reloadable Discord presence
│
├── memory/
│   ├── observations.json         # Global observation log (episode-based timeline)
│   ├── recent_messages.json      # Unobserved messages persisted across restarts
│   └── episodes/                 # Episode storage (persisted by Observer)
│       └── YYYY-MM/DD/           # Date-organized episode files
│
├── projects/
│   └── aerohive-setup/
│       ├── PROJECT.md
│       ├── notes/
│       ├── references/
│       ├── skills/
│       └── workspace/
│
├── archive/
│   └── proxmox-migration/
│       ├── PROJECT.md
│       ├── notes/
│       ├── references/
│       ├── skills/
│       └── workspace/
│
├── skills/                       # User-defined workspace skills
│   └── my-skill/
│       ├── SKILL.md
│       ├── scripts/
│       └── references/
│
├── scheduled_actions.json        # Agent-created scheduled actions
├── pulse_state.json              # Pulse last-run timestamps and run counts
│
├── inbox/                        # Incoming attachments and files
│
└── subagents/                    # Sub-agent preset definitions
```

### Files the gateway parses structurally

These are YAML/TOML files the Rust gateway validates and acts on:

| File | Format | Gateway action |
|------|--------|---------------|
| `config.toml` | TOML | Full gateway configuration |
| `HEARTBEAT.yml` | YAML | Pulse scheduling, task definitions |
| `NOTIFY.yml` | YAML | Notification routing (hot-reloaded on every route call) |
| `projects/**/PROJECT.md` | Markdown+YAML frontmatter | Project entry metadata, tool/MCP resolution |
| `scheduled_actions.json` | JSON | Agent-created scheduled actions |
| `pulse_state.json` | JSON | Pulse last-run timestamps and run counts (persisted across restarts) |
| `memory/observations.json` | JSON | Global observation log (episode-based) |

### Files injected as system prompt content

These are markdown files the gateway loads verbatim and inserts into the LLM context window:

| File | When loaded |
|------|-------------|
| `SOUL.md` | Always |
| `AGENTS.md` | Always |
| `USER.md` | Always (DM sessions) |
| `MEMORY.md` | Always (DM sessions) |
| `ENVIRONMENT.md` | When present |
| `memory/observations.json` | Always (global timeline) |

### Files available via agent tool calls (progressive disclosure)

These files are never auto-loaded into context. The agent knows they exist (via project manifests or skill metadata) and reads them on demand via the `read` tool:

| File | When available |
|------|---------------|
| `projects/<n>/notes/*` | When project is active (listed in manifest) |
| `projects/<n>/references/*` | When project is active (listed in manifest) |
| `projects/<n>/workspace/*` | When project is active (listed in manifest) |
| `projects/<n>/skills/**/SKILL.md` (body) | When project is active (agent activates via `skill_activate`) |
| `skills/**/SKILL.md` (body) | When skill metadata is in prompt (agent reads to activate) |
| `memory/episodes/YYYY-MM/DD/*` | Always (via `read` tool or `memory_search`) |
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

**Retry:**

Each provider is wrapped with configurable retry logic (`[retry]` config section): exponential backoff with max retries, initial delay, max delay, and backoff multiplier. Retries handle transient failures (rate limits, network errors) transparently.

**Failover** (not yet implemented):

Model failover (primary → ordered fallback chain) and auth profile rotation (multiple API keys per provider) are planned but not yet built. Currently, each role (main, observer, reflector, pulse) is assigned a single provider/model via the `[models]` config section, with a `default` fallback for unset roles.

**Model selection for subsystems:**

Different subsystems can use different models:

| Subsystem | Default model | Rationale |
|-----------|--------------|-----------|
| Agent (main conversation) | User-configured primary | Frontier reasoning |
| Observer | Cheap/fast (e.g., Gemini Flash, local) | Extraction, not reasoning |
| Reflector | Cheap/fast | Reorganization, not generation |
| Pulse evaluation | Small model tier (cheap/fast) | Extraction, not reasoning |

### 3. Agent Runtime (`agent/`)

The agent runtime is the core orchestration loop. On each turn:

1. **Receive** normalized inbound message.
2. **Assemble context** — system prompt + identity files + observation log + active project context + relevant skills + recent message history.
3. **Send** to model provider.
4. **Execute** any tool calls returned by the model.
5. **Loop** if tool results need to be sent back to the model.
6. **Deliver** final response through the originating channel.
7. **Update** — append to recent messages, trigger Observer if token threshold reached.

#### Context Assembly (`context.rs`)

Context assembly is the critical integration point. It builds the full prompt from independent subsystems:

```
┌─────────────────────────────────────┐
│ System prompt                       │
│ ├── SOUL.md                         │
│ ├── AGENTS.md                       │
│ ├── ENVIRONMENT.md                  │
│ ├── Available skill metadata        │  ← names + descriptions only
│ └── Active skill instructions       │  ← full SKILL.md body, per activated skill
├─────────────────────────────────────┤
│ User context                        │
│ ├── USER.md                         │
│ └── MEMORY.md                       │
├─────────────────────────────────────┤
│ Observation log                     │  ← from memory crate
│ └── memory/observations.json        │  ← global timeline (always loaded)
├─────────────────────────────────────┤
│ Projects index (always present)     │  ← from projects crate
│ └── name + description per entry   │
├─────────────────────────────────────┤
│ Active project context              │  ← from projects crate
│ ├── PROJECT.md (frontmatter + body) │
│ └── File manifest (not contents)    │
├─────────────────────────────────────┤
│ Available tools                     │
│ ├── Built-in tools                  │  ← from tools crate
│ ├── MCP server tools                │  ← from mcp crate
│ └── Project-scoped tools            │  ← from projects + tools
├─────────────────────────────────────┤
│ Recent messages                     │
│ └── Raw messages (unobserved)       │
│     (includes any file contents     │
│      the agent has read via tools)  │
└─────────────────────────────────────┘
```

**Token budget management:**

The context assembler tracks token usage across sections. Progressive disclosure means the baseline context cost is predictable and lean:

1. System prompt + identity files — always loaded (non-negotiable).
2. Global observation log — always loaded (high-level timeline).
3. Projects index — always loaded (lightweight, ~50-100 tokens per entry).
4. Active project context — loaded when a project is active (`PROJECT.md` body + file manifest).
5. Skill metadata — always loaded (names + descriptions only, ~100 tokens each).
6. Active skill instructions — loaded when a skill is activated (`skill_activate` tool call), persisted in the system prompt section (not the recent messages window, so they don't age out).
7. Recent messages — loaded newest-first, truncated from oldest.

File contents and reference material enter the context window only when the agent reads them via tool calls. Activated skill instructions are the exception — they're injected as a persistent system prompt section so they survive message window truncation.

Token counting uses tiktoken-rs or a provider-specific tokenizer.

### 4. Memory System (`memory/`)

Implements the Observational Memory design from `personal-agent-design.md`.

#### Observer (`observer.rs`)

A background task that watches accumulated raw messages. When unobserved tokens exceed the configured threshold (~30k default):

1. Collect all unobserved messages, tool calls, and results.
2. Send to the observer model with extraction instructions.
3. Receive a dated, structured episode — an ID, time range, and extracted observations.
4. Persist the episode files under `memory/episodes/YYYY-MM/DD/` (`.jsonl` transcript, `.obs.json` observations, `.idx.jsonl` index chunks).
5. Append the episode to `memory/observations.json`.
6. Mark processed messages as observed.

The Observer always writes to the single global observation log. If a project is active at the time of compression, the episode is tagged with a `context` field identifying it. This is metadata for searchability, not a routing mechanism.

The observer model is configured separately from the main agent model. Default: cheap, fast, high-throughput.

**Observation format** (`observations.json` is a flat list of individual observations):

```json
{
  "observations": [
    {
      "timestamp": "2026-02-18 12:15",
      "project_context": "aerohive-setup",
      "source_episodes": ["ep-001"],
      "visibility": "user",
      "content": "Decided to use host_vars over group_vars for per-AP channel assignment"
    },
    {
      "timestamp": "2026-02-18 12:30",
      "project_context": "aerohive-setup",
      "source_episodes": ["ep-001"],
      "visibility": "user",
      "content": "User correction: AeroHive uses HiveManager CLI, not aoscli"
    }
  ]
}
```

Each observation is self-describing: `timestamp`, `project_context` (active project or workspace label), `source_episodes` (which episode transcripts produced it), `visibility` (`"user"` or `"background"`), and `content` (a single concise sentence). The `project_context` field enables filtered search by project via the `project_context` parameter on `memory_search`.

**Episode files** (persisted to `memory/episodes/YYYY-MM/DD/`):

Each episode produces three files in a date-organized directory:

- **`ep-NNN.jsonl`** — The raw episode transcript. Line 1 is meta JSON (id, date, time range, context tag). Subsequent lines are the serialized messages, tool calls, and results that were compressed into this episode.
- **`ep-NNN.obs.json`** — The extracted observations for this episode, indexed individually by the search system.
- **`ep-NNN.idx.jsonl`** — Interaction-pair chunks extracted from the episode, indexed individually for fine-grained retrieval.

This is the full record the agent can retrieve via `read` tool or `memory_search` when the observation log's compressed view isn't enough detail.

**Prompt cache optimization:** The observation log is a prefix that grows append-only between Observer runs. This enables prompt cache hits on the stable prefix across turns within a run.

#### Reflector (`reflector.rs`)

When `observations.json` exceeds its token threshold (~40k default):

1. Back up the current `observations.json` to `observations.json.bak`.
2. Send the full observation list to the reflector model.
3. Receive a compressed version — same flat `Observation` structure, just denser. The reflector merges related observations, drops superseded information, and deduplicates.
4. Write the compressed result back to `observations.json`. Empty LLM responses are rejected (the reflector will not destroy existing content).

**Critical**: The reflector reads from and writes to `observations.json` only. It does **not** touch `MEMORY.md`. These are completely separate systems. Original episode transcripts remain in `memory/episodes/` — the reflector compresses the observation log, not the raw record.

The Reflector is the only operation that fully invalidates the prompt cache for the observation prefix. Acceptable given its infrequency.

**Key constraint:** The Reflector does not summarize into prose. It reorganizes, merges, and compresses while preserving the same per-observation structure. The `source_episodes` fields on observations preserve the retrieval trail back to original episode transcripts.

#### Search (`search.rs`)

Hybrid retrieval over workspace files using BM25 + vector similarity:

- Indexes observations (from `.obs.json` files) and interaction-pair chunks (from `.idx.jsonl` files) individually, not raw episode transcripts as bulk blobs. Also indexes project notes and references, and archived entries.
- Observations and interaction-pair chunks are the primary search targets — they provide fine-grained retrieval into the detail that the observation log compressed away. The agent can follow episode IDs from the observation log to retrieve specific transcripts, or search across indexed chunks when the observation log doesn't have enough context.
- Available as a tool the agent can invoke for deep retrieval beyond the observation window.
- Vector embeddings stored in `vectors.db` (SQLite + sqlite-vec).

### 5. Projects Context Management (`projects/`)

Implements the Projects design from `projects-context-design.md`.

#### Scanner (`scanner.rs`)

On startup and on filesystem change:

1. Walk `projects/` and `archive/`.
2. For each subfolder containing a `PROJECT.md`, parse the YAML frontmatter. Validate against schema.
3. Build in-memory index: name, description, status, tools, MCP servers.

The frontmatter is a few lines of YAML per entry, so scanning is cheap and always current. The body of `PROJECT.md` is not read during scanning — only on activation.

#### PROJECT.md frontmatter schema

```yaml
name: aerohive-setup
description: "AeroHive AP network configuration using Ansible"
status: active                    # active | archived
created: 2026-02-10

# Optional: capabilities loaded when this context activates
tools:
  - exec
  - read
  - write

mcp_servers:
  - name: filesystem
    command: "mcp-server-filesystem"
    args: ["/home/user/ansible/aerohive"]

# Archive-only fields
archived: null
```

#### Activation (`activation.rs`)

Project activation follows a progressive disclosure model. The gateway never bulk-loads file contents on activation. Instead, it tells the agent what's available and lets the agent decide what to read.

**Single-project constraint:** Only one project may be active at a time. Activating a new project automatically deactivates the current one.

**Workspace write scoping:**

When a project is active, project output stays within the project's `workspace/` folder. However, the agent can always write to global files — MEMORY.md and the global observation log — without requiring project deactivation.

| State | Scoped writes | Always writable | Read-only |
|-------|---------------|-----------------|-----------|
| Project active | `projects/<n>/workspace/**` | `MEMORY.md`, `memory/**`, project notes | Identity files (SOUL.md, AGENTS.md), archive |
| No project active | — | Global workspace (memory, MEMORY.md, etc.) | Archive |

**Three tiers of progressive disclosure:**

1. **Always present** — The lightweight projects index (name + description + status for every entry) is always in the system prompt. The agent always knows what projects exist. Cost: ~50-100 tokens per entry.

2. **On activation** — When the agent activates a project (via tool call or conversational cue), the gateway:
   - Loads the entry's **manifest**: a listing of what files exist in `notes/`, `references/`, `skills/`, and `workspace/` (filenames and sizes, not contents).
   - Loads the **recent session log** (~2000 tokens from the most recent `notes/log/` entries) to give the agent immediate continuity context for the project.
   - Starts any **MCP servers** defined in the entry's `PROJECT.md` frontmatter.
   - Adds any **tools** defined in the entry's `PROJECT.md` frontmatter to the active tool set.
   - Discovers any **skills** in the project's `skills/` subdirectory and adds their metadata to the available skills list (not full SKILL.md bodies).

   The manifest tells the agent "here's what's in this project folder." Full file contents are not loaded, but the recent log provides immediate context about what the agent was last doing in this project.

3. **On agent request** — The agent reads specific files by invoking the `read` tool. This applies to:
   - Notes files (`notes/decisions.md`, `notes/current-state.md`)
   - Reference files (`references/topology.png`, `references/ap01.conf`)
   - Workspace files (`workspace/playbooks/configure-aps.yml`)
   - Skill instructions (the full SKILL.md body, loaded when the agent decides the skill is relevant)

**Why this matters:**

A project might have 20 files across notes, references, and workspace. Loading all of them on activation would consume thousands of tokens — most of which are irrelevant to the current question. Progressive disclosure lets the agent read `notes/current-state.md` when it needs project status, or `references/ap01.conf` when debugging a specific AP, without carrying everything else.

This also aligns with how the Agent Skills spec works: skill metadata is always visible (~100 tokens), but the full SKILL.md body (~5000 tokens) loads only when the agent decides to activate that skill.

**Tool and MCP server auto-loading is the exception.** Tools and MCP servers load immediately on activation because they define *capabilities*, not *knowledge*. The agent needs to know what it can do — it doesn't need to read every file to know that.

#### Lifecycle (`lifecycle.rs`)

- **Create**: `mkdir` + write `PROJECT.md` (frontmatter + blank body) + create subfolders. No registry.
- **Archive**: Update status, add archived date, move to `archive/`. Archived projects retain their notes, references, and workspace. Observation history is not carried with the project — it lives in the global observation log and is retrievable via the `context` tag on episodes.

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
      - name: pr_review
        prompt: "Any PRs waiting on my review?"

  - name: daily_review
    enabled: true
    schedule: "24h"
    active_hours: "08:00-09:00"
    tasks:
      - name: morning_brief
        prompt: "Summarize today's calendar and top priorities"
```

The scheduler runs on a tokio interval timer. Each tick:

1. Check which pulses are due based on schedule, active_hours, `enabled` flag, and last-run timestamps.
2. If a pulse is due, invoke the agent with the pulse's tasks as context.
3. Track the result. If HEARTBEAT_OK (nothing actionable), log silently.
4. If actionable, route result to the channels declared on the pulse in HEARTBEAT.yml.
5. Update last-run timestamp.

**Zero cost when idle.** If no pulses are due, no LLM invocation happens.

#### Notification routing

Result routing is handled by the gateway based on the `channels` field declared on each pulse in `HEARTBEAT.yml`. When a pulse evaluation produces a result (not HEARTBEAT_OK), the gateway dispatches to every channel listed on the pulse. See [Notification Routing Design](notification-routing-design.md) for the full specification.

### 7. Scheduled Actions System (`actions/`)

Scheduled actions give the agent the ability to schedule its own wake-ups — one-shot reminders and deferred follow-ups. Where pulses are user-defined ambient monitoring (declarative YAML, LLM-evaluated), scheduled actions are agent-created (created via tool calls, persisted as JSON, executed by the gateway).

Scheduled actions are fire-once: each action fires at a specified timestamp and is then removed. For recurring tasks, use heartbeat pulses instead. Actions persist at the workspace root as `scheduled_actions.json` and survive gateway restarts.

The agent manages actions via `schedule_action`, `list_actions`, and `cancel_action` tool calls. There is no update operation — cancel and re-create instead.

### 8. Skills System (`skills/`)

Implements Agent Skills spec compatibility (agentskills.io).

#### Loader (`loader.rs`)

Discovers skills from configured directories:

1. Walk each skills directory.
2. For each subdirectory containing a `SKILL.md`, parse YAML frontmatter.
3. Validate against the Agent Skills spec: name format, required fields, constraints.
4. Build in-memory skill index: name, description, metadata, file path.

**Skill sources** (precedence, highest first):

1. Project-scoped skills: `projects/<active>/skills/` (only when project is active)
2. Workspace skills: `~/.residuum/workspace/skills/`
3. User-global skills: `~/.residuum/skills/`
4. Bundled skills (shipped with the binary)

#### Resolver (`resolver.rs`)

Skills use the same activation/deactivation model as the Projects system:

1. **Always present**: All available skill metadata (name + description, ~100 tokens each) is in the system prompt. The agent always knows what skills exist.
2. **Explicit activation**: When the agent decides a skill is relevant, it calls `skill_activate` with the skill name. The gateway loads the full `SKILL.md` body into the system prompt and makes any `allowed-tools` available. The agent deactivates skills via `skill_deactivate` when they're no longer needed.
3. **Supporting files**: While a skill is active, the agent can read files from `scripts/`, `references/`, and `assets/` via the `read` tool as needed.

The gateway tracks which skills are currently active and maintains their instructions as a persistent section of the system prompt (not part of the recent messages window). This means activated skill instructions don't age out of context the way read-tool results do.

The gateway's role is indexing, making skills discoverable, and managing the active skill set. The agent's role is deciding which skills to activate and when.

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

Currently supports one transport mechanism:

- **stdio**: Spawn MCP server as child process, communicate over stdin/stdout. Used for all local servers.

HTTP/SSE transport for remote MCP servers is planned but not yet implemented.

#### Lifecycle (`lifecycle.rs`)

MCP servers are managed as child processes:

- **Spawn**: Start the server process when its context activates (either globally configured or via project entry).
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
2. **Project-scoped** (`PROJECT.md` frontmatter `mcp_servers`): Available only when the project is active.

### 10. Tool System (`tools/`)

Built-in tools the agent can invoke directly.

#### Core tools

| Tool | Description |
|------|-------------|
| `exec` | Execute shell commands |
| `read` | Read file contents |
| `write` | Write/create files |
| `edit` | String replacement in files |
| `memory_search` | Hybrid retrieval over workspace |
| `schedule_action` | Schedule a one-shot agent wake-up at a specific timestamp |
| `cancel_action` | Cancel a scheduled action |
| `list_actions` | List scheduled actions |
| `skill_activate` | Load a skill's full instructions into the system prompt |
| `skill_deactivate` | Remove a skill's instructions from the system prompt |
| `project_activate` | Activate a project context |
| `project_deactivate` | Deactivate the current project context; requires a `log` field which is written to the project's dated session log before deactivating |
| `project_create` | Create a new project entry |
| `project_archive` | Archive a completed project |
| `project_list` | List all projects and their status |

**Web capabilities** (search, fetch, browser automation) are not built-in tools. They are provided through MCP servers configured by the user. This keeps the core tool set focused on workspace operations while allowing flexible web access via the MCP ecosystem.

#### Policy (`policy.rs`)

Cascading tool policy resolution:

1. **Global defaults** from config.
2. **Per-project** overrides from `PROJECT.md` frontmatter `tools` field.
3. **Per-skill** additions from `allowed-tools` in SKILL.md frontmatter (global or project-scoped skills).
4. **MCP server tools** from active MCP connections.

The active tool set at any moment is the union of all sources, filtered by deny lists.

**Write scope enforcement:** The `write`, `edit`, and `exec` tools enforce workspace write scoping. When a project is active, project output (generated files, build artifacts) is scoped to the project's `workspace/` directory. Global files (MEMORY.md, observation log) remain writable. Identity files and archive are always read-only. The gateway enforces these constraints via path validation in the tool implementation, not by relying on LLM judgment.

---

## Data Flow

### Inbound message → response

```
Discord ──→ Channel adapter ──→ Normalized message
                                      │
                                      ▼
                              Feed resolution
                                      │
                                      ▼
                              Context assembly
                              ├── Identity files
                              ├── Observation log
                              ├── Active project context
                              ├── Activated skills
                              ├── Available tools (built-in + MCP)
                              └── Recent messages
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
              Log silently   Route via pulse channels
                             (dispatch to all channels
                              declared on this pulse)
```

### Observer compression

```
Raw messages accumulate
            │
            ▼
Token count exceeds threshold (~30k)
            │
            ▼
Observer model extracts episode
(id, date, time range, context tag, observations)
            │
            ▼
Persist episode files
to memory/episodes/YYYY-MM/DD/
            │
            ▼
Append episode to
memory/observations.json
(tagged with active project context, if any)
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
| `reqwest` | HTTP client for model APIs |
| `notify` | Filesystem watching |
| `walkdir` | Directory traversal for projects/skills scanning |
| `tantivy` | BM25 full-text search for memory |
| `axum` | HTTP server for webhook channel & API |
| `tracing` | Structured logging |
| `tiktoken-rs` | Token counting |

---

## What's Not Included (Yet)

Things deliberately scoped out of the initial implementation:

- **Multi-agent routing** — Single-agent mode only. The architecture supports it (the channel trait and message routing are agent-agnostic) but the routing layer isn't built.
- **Companion apps** — No macOS menu bar, iOS/Android nodes. CLI channel covers the dev use case.
- **Canvas / A2UI** — No visual workspace. Text-only interactions.
- **Voice** — No TTS/STT integration. Text channels only.
- **ClawHub integration** — Skills are loaded from local directories. No registry API client.
- **Plugin system** — Channels, providers, and tools are compiled in. A dynamic plugin system was explored and abandoned; the design doc (`docs/plugin-system-design.md`) is being deleted. MCP servers and skills cover the extensibility use cases adequately.
- **Migration tooling** — No automated migration from an existing OpenClaw workspace. Manual setup or a one-time script.

---

## Implementation Priorities

Ordered by "what gets you a usable agent fastest":

### Phase 1: Core loop (COMPLETE)
1. Shared types — Message types, config types, error handling (crate-root modules).
2. `workspace` — Layout conventions, identity file loading, bootstrap.
3. `models` — Anthropic + Ollama providers (use existing connectors).
4. `channels/cli` — Local CLI channel.
5. `agent` — Basic runtime: context assembly from identity files + recent messages, model call, tool execution loop.
6. `tools` — `read`, `write`, `exec` (minimum viable tool set).
7. `main.rs` + `config.rs` — Config loading, startup, wire everything together.

**Milestone: You can talk to your agent via CLI.**

### Phase 2: Memory & continuity (COMPLETE)
8. `memory/observer` — Tier 1 compression.
9. `memory/reflector` — Tier 2 compression.
10. `memory/search` — Hybrid retrieval (tantivy + embeddings).

**Milestone: Agent remembers context across restarts.**

### Phase 3: Proactivity (COMPLETE)
12. `pulse/scheduler` — HEARTBEAT.yml parsing, scheduling loop.
13. `pulse/executor` — Pulse task execution via agent runtime.
14. `notifications/` — CHANNELS.yml parsing, channel dispatch, `NotificationChannel` trait.
15. `actions/store` — Action persistence, `actions/scheduler` — schedule evaluation.
16. `actions/executor` — Action execution, background threads, delivery.

**Milestone: Agent proactively checks on things, notifies you, and can schedule its own wake-ups.**

### Phase 4: Discord & channels (COMPLETE)
17. `channels/discord` — Serenity integration, DM support, message chunking.
18. `channels/webhook` — Incoming webhook support.
19. Channel abstraction — `ReplyHandle` trait, `RoutedMessage`, message source injection.
20. `channels/presence` — Hot-reloadable Discord presence via PRESENCE.toml.
21. `channels/discord` — Slash commands (help, status, reload, observe, reflect).
22. `channels/attachment` — Attachment downloading to inbox with metadata injection.

Guild channels, mention gating, and threads are deferred to Phase 5+.

**Milestone: Agent is fully accessible via Discord with presence, commands, and media.**

### Phase 5: Projects (COMPLETE)
19. `projects/scanner` — Directory discovery, PROJECT.md frontmatter parsing.
20. `projects/activation` — Context activation/deactivation via agent tool calls.
21. `projects/manifest` — Generate file listings for the active entry.
22. `projects/lifecycle` — Create and archive.

**Milestone: Agent manages structured project contexts with progressive disclosure.**

### Phase 6: Skills & MCP
23. `skills/loader` — SKILL.md discovery and parsing.
24. `skills/resolver` — Skill activation and prompt injection.
25. `mcp/client` — JSON-RPC client, stdio transport.
26. `mcp/lifecycle` — Server spawn and teardown.
27. `mcp/transport` — HTTP/SSE transport for remote servers (planned).
28. Integration: project `skills/` subdirectory discovery on activation, MCP server activation from frontmatter.

**Milestone: Agent can use OpenClaw-compatible skills and connect to MCP servers.**

### Phase 7: Background tasks & sub-agents (COMPLETE)
29. `background/types` — `BackgroundTask`, `SubAgentConfig`, `BackgroundResult`, `Execution::SubAgent`.
30. `background/subagent` — Sub-agent execution with minimal context, project-aware forced deactivation, full message transcript capture.
31. `background/spawner` — Semaphore-bounded concurrency, cancellation tokens, result routing.
32. `background/spawn_context` — Resource gathering at spawn time (provider, tools, filter, MCP, identity).
33. Pulse and scheduled action migration to `BackgroundTaskSpawner`.
34. Agent-facing tools: `subagent_spawn` (async-only), `list_agents`, `stop_agent`.
35. Sub-agent presets: `subagents/` directory, YAML frontmatter, built-in `general-purpose` preset.

**Milestone: Background work runs independently; main agent can delegate and manage tasks.**

### Phase 8: CLI & UX (COMPLETE)
36. CLI subcommands: `serve` (default), `connect`, `logs` (with `--watch`), `setup`.
37. CLI logging: dual stderr + daily rolling file appender via `tracing-appender`.
38. Config setup wizard: interactive terminal wizard (`run_interactive`) and flag-driven mode (`from_flags`).
39. First-launch welcome message when `config.toml` doesn't exist.
40. Config file protection: `PathPolicy.blocked_paths` prevents agent writes to `config.toml`.
41. Gateway resilience: config backup/rollback on reload failure, degraded mode with config editor.
