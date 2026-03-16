# Plan: OpenClaw → Residuum Migration Tool

## Context

OpenClaw is the most popular open-source AI agent framework (214k+ GitHub stars). Users who want to switch to Residuum currently have no migration path — they'd lose their agent persona, skills, memory, and conversation history. This tool provides `residuum migrate openclaw <path>` to ingest an OpenClaw config directory (or `.tar.gz` export) and translate it into Residuum's workspace format, preserving as much context as possible.

**Validated against a real OpenClaw installation** at `flinn@aether:~/.openclaw/` with 4 agents (andromeda, nova, sable, vex), each with their own workspaces, sessions, and memory.

## Real OpenClaw Directory Layout (validated)

```
~/.openclaw/
    openclaw.json                          # Main config (JSON, not JSON5 in practice)
    agents/{id}/
        agent/                             # Auth, model configs
            auth.json
            auth-profiles.json
            models.json
        sessions/                          # Conversation transcripts
            {uuid}.jsonl                   # Active sessions
            {uuid}.jsonl.reset.{ts}        # Reset (daily boundary) sessions
            {uuid}.jsonl.deleted.{ts}      # Soft-deleted sessions
    memory/
        {agent-id}.sqlite                  # Per-agent memory database (283MB+ each)
    skills/
        {name}.skill                       # ZIP archives containing SKILL.md
        {name}/                            # Or plain directories
    credentials/
    cron/
    identity/
        device.json
        device-auth.json

# Agent workspaces are EXTERNAL paths (not under ~/.openclaw/):
/home/user/agents/{name}/
    SOUL.md                                # Agent personality
    AGENTS.md                              # Agent capabilities/rules
    USER.md                                # User profile
    IDENTITY.md                            # Name, emoji, appearance
    MEMORY.md                              # Curated long-term memory
    TOOLS.md                               # Environment config, API notes
    HEARTBEAT.md                           # Periodic task instructions
    BOOTSTRAP.md                           # Session initialization
    memory/
        YYYY-MM-DD.md                      # Daily memory logs (narrative style)
        YYYY-MM-DD-{topic}.md              # Topic-specific logs
        observations/                      # Auto-generated observations
            obs-{timestamp}-{id}.md        # YAML frontmatter + content
        daily-logs/
    skills/
        {name}/SKILL.md                    # YAML frontmatter + Markdown body
    dossiers/                              # Crew member profiles
    scripts/                               # Custom scripts
    generated/                             # Generated images
    inbox/                                 # Incoming messages
    .config/                               # Agent-specific config
```

## Session JSONL Format (validated)

Each line is a typed JSON event:
```jsonl
{"type":"session","version":3,"id":"uuid","timestamp":"ISO","cwd":"/path"}
{"type":"model_change","id":"...","provider":"anthropic","modelId":"claude-sonnet-4-6"}
{"type":"thinking_level_change","id":"...","thinkingLevel":"medium"}
{"type":"custom","customType":"model-snapshot","data":{...}}
{"type":"message","id":"...","message":{"role":"user","content":[{"type":"text","text":"..."}]}}
{"type":"message","id":"...","message":{"role":"assistant","content":[{"type":"text","text":"..."}]}}
```

Session states: `.jsonl` (active), `.jsonl.reset.{ts}` (daily reset), `.jsonl.deleted.{ts}` (soft-deleted).

## Architecture Mapping

| OpenClaw | Residuum | Migration Strategy |
|----------|----------|--------------------|
| `openclaw.json` | `config.toml` + `providers.toml` + `channels.toml` | Translate schema |
| `SOUL.md` (workspace) | `SOUL.md` | Direct copy |
| `AGENTS.md` (workspace) | `AGENTS.md` | Direct copy |
| `USER.md` (workspace) | `USER.md` | Direct copy |
| `IDENTITY.md` (workspace) | Merge into `SOUL.md` header | Extract name/emoji, prepend |
| `MEMORY.md` (workspace) | `MEMORY.md` | Direct copy |
| `TOOLS.md` (workspace) | `ENVIRONMENT.md` | Rename + adapt references |
| `HEARTBEAT.md` (workspace) | Pulse config or `HEARTBEAT.md` | Copy as-is (review needed) |
| `memory/YYYY-MM-DD.md` | `observations.json` | Convert paragraphs → observations |
| `memory/observations/*.md` | `observations.json` | Parse YAML frontmatter → observations |
| `agents/{id}/sessions/*.jsonl` | `episodes/YYYY-MM/DD/ep-NNN.jsonl` + `.idx.jsonl` | Filter `type:"message"`, convert |
| `memory/{id}.sqlite` | `observations.json` + episodes | Query & extract (if sqlite3 available) |
| `skills/*/SKILL.md` (workspace) | `skills/*/SKILL.md` | Direct copy |
| `skills/*.skill` (global) | `skills/*/SKILL.md` | Unzip, extract SKILL.md |
| `agents.defaults.models` + `models.providers` | `providers.toml` | Translate to TOML |
| `channels.telegram.accounts.*` | `config.toml` [telegram] | Map per-agent bot tokens |
| `channels.discord.accounts.*` | `config.toml` [discord] | Map per-agent bot tokens |
| `env.vars` (API keys) | `SecretStore` | Store securely |
| `bindings[]` | Channel routing config | Map agent↔channel bindings |

## File Structure

```
src/migrate/
    mod.rs           -- Pipeline orchestration: load → translate → write → report
    types.rs         -- OpenClaw deserialization structs + MigrateOptions
    source.rs        -- Load from directory or .tar.gz archive
    session.rs       -- Session JSONL parser (typed events → messages)
    translate.rs     -- Pure translation: OpenClaw types → Residuum types/strings
    writer.rs        -- Write translated artifacts, handle conflicts
    report.rs        -- MigrationReport: written/skipped/warnings/errors
```

## Implementation

### Phase 1: Scaffolding

1. Add dependencies to `Cargo.toml`: `json5`, `tar`, `flate2`, `zip` (for `.skill` archives)
2. Add `Migration(String)` variant to `ResiduumError` in `src/error.rs`
3. Create `src/migrate/mod.rs` with module declarations
4. Add `pub mod migrate;` to `src/lib.rs`
5. Add `Some("migrate")` match arm to `src/main.rs` (following `run_setup_command` pattern)

### Phase 2: Source Loading (`types.rs` + `source.rs`)

**OpenClaw deserialization structs** — all fields `Option` with `#[serde(default)]`:
- `OpenClawConfig` — top-level with `meta`, `env`, `agents`, `channels`, `models`, `bindings`, `tools`, `gateway`, etc.
- `AgentsConfig` — `defaults` (model, models, workspace, contextTokens, heartbeat, subagents) + `list[]` (per-agent: id, workspace, agentDir, model overrides, heartbeat)
- `ModelsConfig` — `providers.{name}` with `baseUrl`, `apiKey`, `models[]` (each with id, name, reasoning, contextWindow, maxTokens, cost)
- `AgentDefaults.model` — `primary` + `fallbacks[]`
- `ChannelConfig` — per-channel with `enabled`, `accounts.{name}` (each with `botToken`/`token`, `allowFrom`, policies)
- `Bindings[]` — `agentId` + `match` (channel, accountId)
- `EnvVars` — `env.vars` map of API keys and secrets

**Source loading** (two modes):
1. **Directory mode**: Point at `~/.openclaw/` directory
   - Parse `openclaw.json` (standard JSON, despite docs saying JSON5 — support both)
   - For each agent in `agents.list[]`:
     - Resolve workspace path from `agents.list[].workspace`
     - Resolve agentDir from `agents.list[].agentDir` (sessions, auth)
     - Read workspace: `SOUL.md`, `AGENTS.md`, `USER.md`, `IDENTITY.md`, `MEMORY.md`, `TOOLS.md`, `HEARTBEAT.md`
     - Collect `{workspace}/memory/YYYY-MM-DD*.md` daily logs
     - Collect `{workspace}/memory/observations/*.md` observation files
     - Collect `{workspace}/skills/*/SKILL.md` workspace skills
     - Collect `{agentDir}/sessions/*.jsonl` (skip `.deleted.` files, include `.reset.`)
   - Collect global skills from `~/.openclaw/skills/` (`.skill` ZIP or dirs)
2. **Archive mode**: `.tar.gz` → extract to tempdir, then process as directory

### Phase 3: Session Parsing (`session.rs`)

Dedicated module for the complex session JSONL format:

```rust
enum SessionEvent {
    SessionMeta { id: String, timestamp: String, cwd: String },
    ModelChange { provider: String, model_id: String },
    ThinkingLevel { level: String },
    Message { id: String, parent_id: Option<String>, message: SessionMessage },
    Custom { custom_type: String, data: Value },
    Other(Value),  // Forward-compat for unknown types
}

struct SessionMessage {
    role: String,           // "user", "assistant", "toolResult"
    content: Vec<ContentBlock>,
    timestamp: Option<u64>,
}

struct ContentBlock {
    r#type: String,         // "text", "thinking", "tool_use", "tool_result", "image"
    text: Option<String>,
    // ... other variant fields
}
```

**Parsing pipeline**:
1. Read JSONL, deserialize each line as `SessionEvent`
2. Extract session metadata (id, start time, model used)
3. Filter for `Message` events only
4. Map to Residuum `Message` format:
   - `role: "user"` → `Role::User`
   - `role: "assistant"` → `Role::Assistant` (extract `type: "text"` content blocks)
   - `role: "toolResult"` → `Role::Tool`
5. Determine episode date from session metadata timestamp
6. Generate episode context from session cwd or agent id

### Phase 4: Translation (`translate.rs`)

**Providers**: Map OpenClaw model references like `"anthropic/claude-sonnet-4-6"` directly (same format as Residuum). Extract custom providers from `models.providers` (e.g., Ollama with custom `baseUrl`). Translate `agents.defaults.model.primary` → `[models] main`. Translate `fallbacks[]` → failover arrays.

**Channels**: Per-agent Telegram bot tokens → Residuum `[telegram]` config. Per-agent Discord tokens → Residuum `[discord]` config. Multi-account handling depends on Residuum multi-agent support.

**Secrets**: Extract all API keys from `env.vars` and channel configs. Store via `SecretStore`. Replace inline values with `secret:` references.

**Identity files**: `SOUL.md` → direct copy. `IDENTITY.md` → prepend key identity fields to `SOUL.md`. `AGENTS.md`, `USER.md` → direct copy. `TOOLS.md` → copy as `ENVIRONMENT.md` (Residuum equivalent).

**Skills**:
- Workspace skills (`skills/*/SKILL.md`): Copy as-is. Validate names.
- Global `.skill` archives: Unzip, extract `SKILL.md`, place in workspace skills dir.
- Validate names against Residuum rules (1-64 chars, lowercase alphanumeric + hyphens).

**Memory — daily logs**: Parse each `memory/YYYY-MM-DD.md`. These are narrative-style entries (not bullet lists). Split by `##` sections. Each section becomes an `Observation` with:
- `timestamp`: derive from filename date + section ordering
- `project_context`: `"imported/openclaw/{agent-id}"`
- `source_episodes`: empty
- `visibility`: `Visibility::User`
- `content`: section text (truncated to reasonable length)

**Memory — workspace observations**: Parse `memory/observations/obs-*.md` files. Each has YAML frontmatter with `session_key`, `agent_id`, `channel`, `timestamp`, `type`. Map directly to `Observation` entries.

**Memory — session transcripts**:
For each session JSONL (see Phase 3 parsing):
1. Build episode from parsed messages
2. Write as Residuum episode transcript (`ep-NNN.jsonl`)
3. Extract interaction-pair chunks → `ep-NNN.idx.jsonl`
4. Generate observations from session content (LLM summary or simple extraction)
5. Skip `.deleted.` sessions; include `.reset.` sessions (they represent completed days)
6. Assign sequential episode IDs starting after any existing episodes

**Memory — SQLite database**: Best-effort. If the migrator can read the SQLite file:
- Query for conversation history and memory entries
- Cross-reference with session JSONL to avoid duplicates
- Extract any observations/memories not captured in Markdown files
- If SQLite reading fails (no driver, corrupt, etc.), log warning and continue with Markdown + JSONL sources

### Phase 5: Writer (`writer.rs` + `report.rs`)

**Per-agent output**: Each OpenClaw agent gets its own Residuum workspace directory.

**Conflict handling**:
- Default: skip existing files (no overwrite)
- `--overwrite`: replace existing files
- `--merge`: additive for `mcp.json`, `observations.json`, `providers.toml`

**Dry-run**: `--dry-run` runs all translation/validation but writes nothing.

**MigrationReport**: Tracks `written`, `skipped`, `warnings`, `errors` with paths and actions. Printed to stderr after completion.

**Extra workspace files**: Copy `HEARTBEAT.md`, `dossiers/`, `scripts/`, `constitution.md`, and other workspace files into a `imported/` subdirectory to preserve them for reference without interfering with Residuum's expected layout.

### Phase 6: CLI Integration + Multi-Agent

**Usage**:
```
residuum migrate openclaw <path> [--dry-run] [--overwrite] [--merge] [--agent <name>]
```

- `<path>`: `~/.openclaw/` directory or `.tar.gz` archive
- `--agent <name>`: migrate a specific agent from `agents.list[]`; if omitted, migrate all agents
- Multi-agent: iterate over `agents.list[]`, create per-agent Residuum workspace

### Phase 7: Indexing

After all files are written, trigger search index rebuild:
- Call `rebuild()` on Tantivy search index
- If embedding provider configured, generate vectors
- Uses existing `incremental_sync()` path

## Key Files to Modify

| File | Change |
|------|--------|
| `Cargo.toml` | Add `json5`, `tar`, `flate2`, `zip` |
| `src/error.rs` | Add `Migration(String)` variant |
| `src/lib.rs` | Add `pub mod migrate;` |
| `src/main.rs` | Add `Some("migrate")` subcommand dispatch |

## Key Files to Reference (read, not modify)

| File | Why |
|------|-----|
| `src/config/types.rs` | Config struct for generating TOML |
| `src/config/provider.rs` | ProviderSpec for provider translation |
| `src/config/wizard.rs` | Pattern for config file generation |
| `src/memory/types.rs` | Episode, Observation, IndexChunk structs |
| `src/memory/episode_store.rs` | Episode JSONL read/write |
| `src/memory/log_store.rs` | Observation log + episode ID generation |
| `src/memory/chunk_extractor.rs` | Interaction-pair extraction logic |
| `src/memory/search.rs` | Tantivy indexing (rebuild/sync) |
| `src/memory/vector_store.rs` | SQLite-vec embedding storage |
| `src/workspace/layout.rs` | WorkspaceLayout paths |
| `src/workspace/config.rs` | MCP server + channel config loading |
| `src/skills/types.rs` | SkillIndexEntry for skill validation |
| `src/subagents/parser.rs` | Name validation rules |
| `src/notify/types.rs` | Channel type definitions |

## Not Migrated (Documented Limitations)

- **Cron jobs** (`~/.openclaw/cron/`) — Different scheduling model; warn user to recreate
- **Heartbeat schedules** — OpenClaw heartbeats use per-agent intervals; warn user to configure Residuum pulse
- **Browser config** — No equivalent in Residuum
- **Voice call config** (Twilio/ElevenLabs) — No equivalent; preserve in `imported/` for reference
- **Diagnostics/OTEL config** — No equivalent
- **`.skill` ZIP archives** with embedded dependencies — Unzip SKILL.md but warn about missing deps
- **Memory SQLite** — Best-effort extraction; primary data comes from Markdown + JSONL
- **Delivery queue, exec approvals** — Internal OpenClaw state, not migrated
- **Canvas, media directories** — Copy to `imported/` for reference only

## Real-World Test Data (flinn@aether)

Available for validation during development:
- 4 agents: andromeda (default/XO), vex (engineer), nova (research), sable (comms)
- ~40 session files per agent, 200KB-4MB each
- 50+ daily memory logs per agent
- Per-agent memory SQLite (170MB-283MB)
- Skills: reclaim, gog, gemini-deep-research, gws
- Channels: Telegram (4 bot accounts), Discord (4 bot accounts)

## Testing

**Unit tests** (in each module's `#[cfg(test)]`):
- Source loading: parse real `openclaw.json` structure, handle missing fields
- Session parsing: typed events, message extraction, session states
- Provider translation: Anthropic, OpenAI, Ollama with custom providers
- Channel translation: Telegram multi-account, Discord multi-account
- Memory translation: narrative daily logs → observations, observation .md files, session JSONL → episodes
- Skills: workspace SKILL.md copy, .skill ZIP extraction, name validation
- Writer: empty target, existing target + overwrite, merge mode, dry-run

**Integration test** (`tests/migrate_integration.rs`):
- Create fixture OpenClaw directory matching real structure
- Run full pipeline against empty Residuum workspace
- Verify all output files exist with correct content
- Verify report accuracy

## Verification

1. Build: `cargo build`
2. Unit tests: `cargo test --quiet`
3. Dry-run against real data: `residuum migrate openclaw /path/to/openclaw-copy --dry-run`
4. Full migration: Run without `--dry-run`, start gateway, verify memory search returns imported content
