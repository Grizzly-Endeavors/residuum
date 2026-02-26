# OpenClaw vs IronClaw: Comparative Analysis

## Executive Summary

IronClaw is a from-scratch Rust re-implementation of the OpenClaw personal AI agent gateway. While sharing OpenClaw's core architectural DNA — gateway pattern, channel normalization, file-first workspace, model-agnostic runtime — IronClaw makes deliberate tradeoffs: it sacrifices OpenClaw's ecosystem breadth and extensibility for a tighter, single-binary Rust implementation with significantly improved memory continuity, proactive scheduling, and structured project/context management.

OpenClaw is a **platform**. IronClaw is a **personal tool**.

---

## 1. Language & Runtime

| Dimension | OpenClaw | IronClaw |
|-----------|----------|----------|
| Language | TypeScript (ESM) | Rust (2024 edition) |
| Runtime | Node.js | Tokio async runtime |
| Binary | `node` + bundled JS (tsdown/rolldown) | Single compiled binary |
| Package management | pnpm monorepo workspaces | Cargo single crate |
| Config format | JSON5 (Zod schema validation) | TOML (serde + custom validators) |
| Error handling | try/catch, error types | `anyhow` + `thiserror`, `IronclawError` enum |
| Lint enforcement | oxlint | Clippy pedantic (warnings = errors) |
| Test framework | Vitest (unit, e2e, live, gateway configs) | `cargo test` with wiremock + tempfile |

**Key implications**: IronClaw's single binary means zero runtime dependencies, simpler deployment, and no node_modules. The Rust type system and strict Clippy lints (denying `unwrap_used`, `expect_used`, `panic`) enforce correctness at compile time that OpenClaw handles at runtime via Zod schemas and TypeScript types.

---

## 2. Architecture

Both share the same fundamental architecture: a central gateway server that receives messages from channels, routes them through an agent turn loop, and delivers responses back. The similarities are intentional — IronClaw's design documents explicitly state that OpenClaw's gateway pattern is sound and doesn't need rethinking.

### Gateway

| Dimension | OpenClaw | IronClaw |
|-----------|----------|----------|
| Server framework | Express 5 + ws | Axum 0.8 + tokio-tungstenite |
| Protocol | WebSocket JSON-RPC + REST (OpenAI-compatible) | WebSocket JSON protocol |
| REST API | `/v1/chat/completions`, `/v1/responses` | `/webhook` (POST) |
| OpenAI compatibility | Yes (serves as OpenAI proxy) | No |
| mDNS/Bonjour discovery | Yes (for companion apps) | No |
| Tailscale exposure | Yes | No |
| Control UI | Yes (Lit web components, served from `/`) | No |
| Hot reload | Config file watch, channel/plugin reload without restart | Config file watch, full gateway restart loop (`GatewayExit::Reload`) |
| Event loop | WebSocket event callbacks | 8-arm `tokio::select!` (messages, pulse, cron, observer, reflector, reload, manual observe, manual reflect) |

OpenClaw's gateway is a full-featured server platform with REST APIs, a web control panel, and device discovery. IronClaw's gateway is a focused message-processing engine.

### Agent Turn Loop

The core loop is structurally identical in both:

1. Receive normalized inbound message
2. Assemble context (system prompt + identity files + memory + tools)
3. Send to model provider
4. Execute tool calls if any, loop back
5. Deliver final response
6. Update memory pipeline

**Differences in context assembly**:

| Context Section | OpenClaw | IronClaw |
|-----------------|----------|----------|
| Identity files | SOUL.md, AGENTS.md, USER.md, MEMORY.md, TOOLS.md | SOUL.md (with Identity section), AGENTS.md, USER.md, MEMORY.md, ENVIRONMENT.md |
| Memory | Session transcripts (JSONL), MEMORY.md, last 2 daily logs | Observation log (always in context), recent context narrative, episode transcripts on demand |
| Projects | No project system | Projects index + active project context + file manifest |
| Skills | Per-agent skill list, loaded at config time | Progressive disclosure: metadata always visible, full body on activation |
| MCP tools | No (planned via ACP) | Full MCP client with reconcile/diff lifecycle |
| Time context | Not injected | Time tag inserted before last user message |

IronClaw's context assembly is the integration point where its independent subsystems compose. The 12-section layered prompt is more structured than OpenClaw's approach.

---

## 3. Channel Ecosystem

This is the most dramatic difference between the two projects.

### OpenClaw: Platform-scale channel support

**Built-in (8)**:
- Telegram, WhatsApp, Discord, IRC, Google Chat, Slack, Signal, iMessage

**Extensions (20+)**:
- BlueBubbles, Matrix, Mattermost, Microsoft Teams, Nextcloud Talk, Nostr, Tlon, Twitch, Zalo, Feishu/Lark, Line, and more

**Companion apps**:
- iOS (Swift), Android (Kotlin), macOS (implied)

**Special protocols**:
- ACP (Agent Client Protocol) — stdio bridge for IDE integration (Zed)
- Canvas/A2UI — real-time collaborative visual workspace for mobile apps
- Voice/TTS — Edge TTS + telephony integration

### IronClaw: Focused channel set

- **CLI** — WebSocket client with readline, markdown rendering, slash commands
- **Discord** — Serenity-based, DM-only, with presence, slash commands, attachments, chunking
- **Webhook** — HTTP POST endpoint with bearer token auth
- **WebSocket** — Raw JSON protocol for custom clients

IronClaw has no messaging platform integrations beyond Discord, no companion apps, no ACP bridge, no voice support, and no visual workspace. This is by design — the project explicitly scopes out multi-channel breadth in favor of depth on core agent capabilities.

---

## 4. LLM Provider Support

| Provider | OpenClaw | IronClaw |
|----------|----------|----------|
| Anthropic | Yes | Yes |
| OpenAI / compatible | Yes | Yes |
| Google Gemini | Yes | Yes |
| Ollama | Yes (auto-discovery) | Yes |
| AWS Bedrock | Yes | No |
| GitHub Copilot | Yes (token exchange) | No |
| Together AI | Yes | No |
| Hugging Face | Yes | No |
| Venice.ai | Yes (dynamic) | No |
| MiniMax | Yes | No |
| Moonshot/Kimi | Yes | No |
| Qwen | Yes (OAuth) | No |
| node-llama-cpp | Yes (local) | No |
| Cloudflare AI Gateway | Yes (proxy) | No |
| ~15 more providers | Yes | No |

**OpenClaw**: 25+ providers with auth profile rotation, round-robin, cooldown on failure, and per-provider OAuth flows.

**IronClaw**: 4 providers with retry/exponential backoff. Uses `provider/model` format (e.g., `anthropic/claude-sonnet-4-6`). Different subsystems can use different models (main agent, observer, reflector, pulse, cron).

IronClaw's approach is sufficient for personal use. OpenClaw's approach targets a multi-user platform where provider diversity and redundancy matter.

---

## 5. Memory System

This is where IronClaw makes its most significant architectural departure from OpenClaw.

### OpenClaw: Hybrid search with session persistence

- **Storage**: SQLite with FTS5 (BM25) + sqlite-vec (vector similarity)
- **Sources**: `MEMORY.md`, `memory/*.md`, session transcripts
- **Embedding providers**: OpenAI, Gemini, Voyage AI, local (node-llama-cpp)
- **Features**: MMR diversity, temporal decay, LRU embedding cache, batch embedding
- **Auto-loading**: Identity files + last 2 daily logs
- **Retrieval**: Agent must actively call `memory_search` for anything older

**Known weakness** (documented in IronClaw's design docs): "OpenClaw's current memory has a day-boundary cliff. Context from even the previous day can get dropped if it wasn't promoted to MEMORY.md or if the agent doesn't recognize it should search."

### IronClaw: Observational Memory (OM) — always-in-context compressed history

- **Storage**: JSON files (observations.json, episodes/*.jsonl), no database
- **Search**: tantivy 0.22 BM25 (no vector embeddings currently)
- **Always in context**: The entire observation log is loaded into every prompt
- **Two-tier compression**:
  - **Observer (Tier 1)**: Fires when unobserved messages exceed ~30k tokens. Uses a cheap model (e.g., Gemini Flash) to extract structured episodes. Produces dated observations + raw episode transcripts. Has soft threshold with cooldown and force threshold for immediate fire.
  - **Reflector (Tier 2)**: Fires when observation log exceeds ~40k tokens. Reorganizes/compresses the log while preserving episode structure. Carries `source_episodes` references for retrieval trail.
- **Persistence across restarts**: `recent_messages.json` (unobserved messages), `recent_context.json` (last observer narrative), `observations.json` (global log)
- **Visibility tagging**: `User` vs `Background` — pulse/cron observations don't pollute the user conversation record

**Key difference**: OpenClaw requires the agent to *decide* to search for old context. IronClaw keeps compressed history *always visible* in the context window, eliminating the retrieval dependency. The agent never has to guess that it should look for something — relevant history is there by default.

**Tradeoff**: IronClaw's approach consumes more baseline tokens (the observation log is always present), but the design argues this is a net cost reduction because it eliminates the expensive raw message history that would otherwise fill the context window.

---

## 6. Projects & Context Management

### OpenClaw: Multi-agent isolation

OpenClaw handles context separation through **agents** — each agent has its own workspace directory, model config, skill list, and session history. Switching context means talking to a different agent. There is no project abstraction within a single agent.

### IronClaw: Single-agent with structured project contexts

IronClaw introduces a dedicated **Projects system**:

- **Discovery**: Filesystem scanning of `projects/` and `archive/` for `PROJECT.md` files (no registry)
- **Progressive disclosure**: Index always in prompt (name + description, ~50-100 tokens each), full context on activation (manifest, tools, MCP servers, skills), file contents on agent request
- **Single active project**: One project at a time; activating a new project deactivates the current one
- **Deactivation logging**: Mandatory non-empty log entry on deactivation, written to `notes/log/YYYY-MM/log-DD.md`
- **Write scoping**: Active project constrains writes to its `workspace/` subdirectory; global files (MEMORY.md, memory/) always writable
- **Tool gating**: Projects opt in to tools (like `exec`) via `PROJECT.md` frontmatter
- **Project-scoped MCP servers**: Started on activation, torn down on deactivation
- **Project-scoped skills**: Discovered from project's `skills/` subdirectory on activation
- **Archiving**: Status change + date stamp + move to `archive/`; observation history stays in global log (tagged by `context` field)

This is one of IronClaw's most distinctive features. It gives the agent structured context management without requiring multiple agent instances, and the progressive disclosure model keeps baseline token cost predictable.

---

## 7. Proactivity (Pulse & Cron)

### OpenClaw: Freeform heartbeat + cron

- **Heartbeat**: `HEARTBEAT.md` — freeform markdown checklist. Full agent turn fires every N minutes (default 30). LLM reads the file and decides what needs attention. Burns tokens on scheduling logic.
- **Cron**: `croner` crate, `cron.json` persistence. Schedule types: at, every, cron expression. Standard implementation.

### IronClaw: Structured pulse scheduling + cron

- **Pulse**: `HEARTBEAT.yml` — machine-parseable YAML with named pulses, per-task schedules, and active hours windows. The gateway handles scheduling logic; the LLM only fires when a pulse is actually due. **Zero token cost when nothing is due.**
- **Notification routing**: `NOTIFY.yml` maps channels to task names. Agent self-evolves this file based on user feedback. Supports built-in channels (agent_wake, agent_feed, inbox) and external channels (ntfy, webhook).
- **Cron**: Nearly identical to OpenClaw. `cron` crate (0.12), JSON persistence, At/Every/Cron schedules, UserVisible/Background delivery modes.

**Key difference**: IronClaw moves scheduling decisions from the LLM to the gateway. OpenClaw's heartbeat fires a full LLM turn every 30 minutes to ask "is anything due?" — most of which return HEARTBEAT_OK. IronClaw's pulse system only invokes the LLM when the gateway's scheduler determines a pulse is actually due based on timestamps and active hours.

---

## 8. Skills System

Both support the Agent Skills spec (agentskills.io) with SKILL.md files, but the activation models differ.

| Dimension | OpenClaw | IronClaw |
|-----------|----------|----------|
| Discovery | Configured dirs + bundled + npm packages + ClawHub | Configured dirs + workspace + active project |
| Loading | Per-agent skill list, loaded at config time | Progressive disclosure: metadata always visible, full body on `skill_activate` tool call |
| Persistence | Part of agent config | Active skills persist in system prompt section (survive message truncation) |
| Scoping | Per-agent | Global + project-scoped |
| Dynamic activation | Not clear | Agent activates/deactivates via tool calls at runtime |

IronClaw's approach is more dynamic — the agent decides at runtime which skills to load based on the current task, rather than having a fixed skill set per agent.

---

## 9. MCP (Model Context Protocol)

| Dimension | OpenClaw | IronClaw |
|-----------|----------|----------|
| Implementation | ACP (Agent Client Protocol) bridge, not native MCP | Native MCP client via `rmcp` crate |
| Transport | stdio (NDJSON) over WebSocket proxy | stdio (child process) via `TokioChildProcess` |
| Server lifecycle | Manual via ACP bridge | `McpRegistry` with reconcile/diff, Pending/Running/Failed states |
| Tool integration | Via ACP proxy | Direct: MCP tools unioned with built-in tools in agent turn loop |
| Scoping | N/A | Global (`config.toml`) + project-scoped (`PROJECT.md` frontmatter) |
| Pagination | N/A | Automatic tool list pagination |

IronClaw has a more complete MCP integration — native client, lifecycle management, project-scoped servers, and direct tool union. OpenClaw's ACP is a different protocol (Agent Client Protocol) that bridges to the gateway over WebSocket; it doesn't natively speak MCP.

---

## 10. Tool System

### Shared tools

Both have: file read, file write, file edit, shell exec, memory search, cron management.

### OpenClaw-only tools

| Tool | Description |
|------|-------------|
| `web_search` | DuckDuckGo / configured engine |
| `web_fetch` | URL fetching with Firecrawl, SSRF guard |
| `browser` | Full Playwright browser automation |
| `sessions_*` | Send/list/spawn conversation sessions |
| `message_send` | Direct channel messaging |
| `canvas_tool` | A2UI visual workspace rendering |
| `tts_tool` | Text-to-speech generation |
| `subagents_tool` | Spawn/manage child agents |
| `discord/slack/telegram/whatsapp_actions` | Per-channel messaging/moderation |
| `nodes_tool` | Mobile companion node control |
| `gateway_tool` | Gateway state queries |

### IronClaw-only tools

| Tool | Description |
|------|-------------|
| `project_activate/deactivate/create/archive/list` | Full project lifecycle management |
| `skill_activate/deactivate` | Runtime skill loading/unloading |

### Tool gating

| Dimension | OpenClaw | IronClaw |
|-----------|----------|----------|
| Policy model | Per-channel, per-sender allowlist/denylist | Project-level opt-in via `PROJECT.md` `tools` field |
| Write scoping | Sandbox (Docker-based) for exec | `PathPolicy` enforces workspace directory boundaries |
| Approval workflow | Gateway can require user approval for exec | Tool filter (gated tools invisible unless project opts in) |

IronClaw's tool gating is tighter and more structural — gated tools don't even appear in tool definitions unless the active project explicitly opts them in.

---

## 11. Configuration

| Dimension | OpenClaw | IronClaw |
|-----------|----------|----------|
| Format | JSON5 | TOML |
| Schema validation | Zod | serde + custom validators |
| Env var override | Yes (`OPENCLAW_*`) | Yes (`IRONCLAW_*`, provider-specific) |
| Config CLI | `openclaw config get/set/unset` | None (edit file directly) |
| Migration system | Multi-phase migration scripts | None |
| Hot reload | Watch + reload without restart | Watch + full restart loop |
| Secrets | Auth profiles with file locking | Env var interpolation (`${VAR}`) in TOML |
| Backup rotation | Configurable | None |

OpenClaw's config system is more mature with migration tooling, CLI manipulation, and backup rotation. IronClaw's is simpler — edit the TOML file, the gateway restarts.

---

## 12. Extensibility & Plugin System

### OpenClaw: Full plugin ecosystem

- **Plugin SDK** (`openclaw/plugin-sdk`): register tools, CLI commands, gateway methods, HTTP routes, channel adapters, provider auth flows, hooks
- **Plugin manifest**: `openclaw.plugin.json` with id, configSchema, kind
- **Plugin discovery**: bundled, global npm, workspace, config paths
- **Plugin types**: channel, provider, memory, hooks
- **Extensions**: 30+ workspace packages (channels, memory backends, voice)

### IronClaw: Compiled-in, no plugin system

- All channels, providers, and tools are compiled into the single binary
- Feature flags (e.g., `discord`) for conditional compilation
- No dynamic plugin loading (explicitly scoped out; WASM/subprocess mentioned as future possibility)

This is the fundamental extensibility tradeoff. OpenClaw can gain new capabilities at runtime via npm packages. IronClaw requires recompilation for any new capability. For a personal tool used by one person, this is acceptable. For a platform, it wouldn't be.

---

## 13. Testing

| Dimension | OpenClaw | IronClaw |
|-----------|----------|----------|
| Runner | Vitest | cargo test |
| Configs | 4 separate (unit, e2e, live, gateway) | Single cargo test with integration tests in `tests/` |
| Coverage target | 70% lines/functions, 55% branches | No explicit target, but "testing is first-class" |
| Mocking | Standard JS mocking | `wiremock` for HTTP, `MockMemoryProvider` for agent/observer |
| E2E | Docker-based full install/onboard/gateway | `tempfile::TempDir` isolated workspace tests |
| Integration tests | Co-located (`*.e2e.test.ts`, `*.live.test.ts`) | Separate `tests/` directory (gateway, memory, proactivity, projects, skills, MCP) |
| Pre-commit | oxlint | `cargo fmt --check` + `cargo clippy -- -D warnings` + `cargo test` |

Both projects take testing seriously. IronClaw's Clippy-as-error approach means many classes of bugs that OpenClaw catches at test time are caught at compile time.

---

## 14. What OpenClaw Has That IronClaw Doesn't

1. **Channel breadth**: 28+ messaging platforms vs 3
2. **Companion apps**: iOS, Android, macOS
3. **Multi-agent support**: Isolated agents with independent configs
4. **Plugin ecosystem**: Dynamic extension via npm packages
5. **Web control UI**: Browser-based dashboard
6. **OpenAI-compatible REST API**: Serves as an LLM proxy
7. **ACP (Agent Client Protocol)**: IDE integration (Zed)
8. **Canvas/A2UI**: Visual collaborative workspace
9. **Voice/TTS**: Text-to-speech and telephony
10. **Browser automation**: Full Playwright integration
11. **Web search/fetch**: Built-in web tools
12. **Subagent spawning**: Child agent management
13. **Auth profile rotation**: Multi-key round-robin with cooldown
14. **Vector embeddings**: sqlite-vec for semantic search
15. **Docker sandboxing**: Isolated exec environment
16. **Config migration tooling**: Version-to-version schema migrations
17. **25+ LLM providers**: vs 4

---

## 15. What IronClaw Has That OpenClaw Doesn't

1. **Observational Memory (OM)**: Always-in-context compressed history eliminating the retrieval dependency
2. **Two-tier compression**: Observer + Reflector with episode-based structure and retrieval trails
3. **Projects system**: Structured context management with progressive disclosure, write scoping, and tool gating
4. **Structured pulse scheduling**: `HEARTBEAT.yml` with machine-parseable schedules, active hours, and zero-cost-when-idle
5. **Notification routing**: `NOTIFY.yml` channel-based result routing with external channel support
6. **Native MCP client**: Full lifecycle management with project-scoped servers
7. **Dynamic skill activation**: Runtime activate/deactivate via tool calls with persistent prompt injection
8. **Single-binary deployment**: No runtime dependencies
9. **Compile-time safety**: Strict Clippy lints, no unwrap/expect/panic allowed
10. **Memory persistence across restarts**: Unobserved messages and observer narrative survive gateway restarts
11. **Deactivation logging**: Mandatory session logs when switching project contexts
12. **Visibility tagging**: Background observations (pulse/cron) separated from user conversation observations

---

## 16. Philosophical Differences

### OpenClaw: Platform-first

OpenClaw is designed as a **multi-user, multi-channel platform**. Its architecture optimizes for:
- Maximum provider and channel coverage
- Dynamic extensibility via plugins
- Multiple agent instances with isolated configs
- REST API compatibility (OpenAI proxy)
- Companion apps across platforms
- Community contributions and marketplace (ClawHub, SkillsMP)

### IronClaw: Personal-tool-first

IronClaw is designed as a **single-user personal agent**. Its architecture optimizes for:
- Memory continuity and context management
- Proactive behavior with minimal token waste
- Structured project management with safety guardrails
- File-first transparency (no databases, no opaque state)
- Simplicity and inspectability over extensibility
- Compile-time correctness over runtime flexibility

### The core insight

IronClaw's design documents articulate this clearly: "Every change targets a specific failure mode observed in real usage, not a theoretical weakness." The project identifies two specific problems in OpenClaw — the memory day-boundary cliff and the token-wasteful heartbeat system — and builds focused solutions around them. Everything else is either preserved as-is or deliberately scoped out.

The result is a much smaller codebase with deeper solutions to fewer problems. OpenClaw is wider; IronClaw is deeper.

---

## 17. Maturity Comparison

| Dimension | OpenClaw | IronClaw |
|-----------|----------|----------|
| Project age | Mature (version 2026.2.19) | Active development (Phase 6 of 6) |
| Codebase size | Very large (monorepo, 200+ files in agents/ alone) | Moderate (single crate, ~60 source files) |
| Production readiness | Yes (multi-user, multi-channel) | Personal use (single-user, limited channels) |
| Documentation | Mintlify docs site | Design documents + CLAUDE.md |
| Community | Active (extensions ecosystem, ClawHub, SkillsMP) | Solo project |
| Deployment | Docker, npm global, systemd service | Cargo build, single binary |

---

## 18. Summary Matrix

| Category | OpenClaw | IronClaw | Winner |
|----------|----------|----------|--------|
| Channel coverage | 28+ platforms | 3 channels | OpenClaw |
| Provider coverage | 25+ providers | 4 providers | OpenClaw |
| Memory continuity | Day-boundary cliff | Always-in-context OM | **IronClaw** |
| Proactive scheduling | Token-wasteful heartbeat | Zero-cost structured pulses | **IronClaw** |
| Project management | None (multi-agent isolation) | Full project system | **IronClaw** |
| Skill management | Static per-agent | Dynamic activate/deactivate | **IronClaw** |
| MCP integration | ACP bridge (different protocol) | Native MCP client | **IronClaw** |
| Extensibility | Full plugin SDK | Compiled-in only | OpenClaw |
| Deployment simplicity | Complex (Node.js + dependencies) | Single binary | **IronClaw** |
| Companion apps | iOS, Android, macOS | None | OpenClaw |
| Web UI | Full control dashboard | None | OpenClaw |
| Type safety | TypeScript + Zod | Rust + strict Clippy | **IronClaw** |
| File transparency | Good (file-first) | Excellent (zero databases) | **IronClaw** |
| Tool ecosystem | 20+ built-in tools | 16 built-in tools | OpenClaw |
| Testing rigor | 4 test configs, 70% coverage | Compile-time + integration tests | Comparable |
| Maturity | Production-grade platform | Personal-use active development | OpenClaw |
