# Systems Usage — Intent & Ownership

This directory documents how each Residuum system is **intended to be used**, by both the agent and the user. It serves as the authoritative reference when writing onboarding content, reference skills, or default workspace files.

## Ownership Model

Everything inside the workspace directory is **agent-owned by default**. The agent creates, reads, updates, and evolves these files as part of normal operation. The user provides initial guidance during onboarding and occasional course corrections, but the goal is that users rarely need to intervene after the first conversation.

### Agent-owned files

| File | Churn | Notes |
|------|-------|-------|
| `wiki/` | High | Open Knowledge Format bundle — one concept per page, plus `index.md` files and an append-only `wiki/log.md`. Agent maintains pages and indexes by hand. |
| `USER.md` | Medium | Core facts only (capped list, replace-don't-append) — user preferences, communication style, active interests. Longer-form knowledge lives in wiki pages. |
| `workbench/` | Medium | Interactive artifacts the agent builds for the user, each a page or a folder, plus each artifact's `<name>.*` data files. See [Workbench](workbench.md). |
| `HEARTBEAT.yml` | Medium | Agent creates during onboarding, evolves autonomously (adds/removes pulses, adjusts schedules, moves routing). |
| `SOUL.md` | Rare | Foundational identity. Agent may refine wording but shouldn't overhaul without user input. |
| `AGENTS.md` | Rare | Behavioral rules. Same as SOUL.md — low-churn, foundational. |
| `memory/OBSERVER.md` | Low | Observer extraction prompt. Agent can improve over time via self-analysis. |
| `memory/REFLECTOR.md` | Low | Reflector compression prompt. Same — agent self-improves. |
| `scheduled_actions.json` | Managed via tools | Never edited directly. Created/removed by `schedule_action` / `cancel_action`. |
| `pulse_state.json` | Managed by gateway | Pulse last-run timestamps and run counts. Persisted across restarts. Never edited directly. |
| `teams_state.json` | Managed by the Teams interface | Teams owner and conversation references for proactive messages. Edit only to reset the owner (see [Microsoft Teams](teams.md)). |
| `discord_state.json` | Managed by the Discord interface | Discord owner and the conversations the bot has seen. Edit only to reset the owner (see [Discord](discord.md)). |
| `telegram_state.json` | Managed by the Telegram interface | Telegram owner and the chats the bot has seen. Edit only to reset the owner (see [Telegram](telegram.md)). |
| `config/agent-card.json` | Low | What this agent advertises to other agents over A2A: name, description, and skills. Bootstrapped with a generic placeholder and an empty skill list. See [A2A](a2a.md). |

### User-owned files

| File | Notes |
|------|-------|
| `config.toml`, `providers.toml` | Live outside the workspace directory. The agent may edit both directly with the file tools on the user's behalf — a reload picks up the change like any other edit — and should prefer a targeted edit over rewriting the whole file, per the residuum-system skill's config guidance. Both the agent's `write_file`/`edit_file` and the `POST /api/workspace/validate` endpoint surface diagnostics from the same validators the loader uses (`src/diagnostics/mod.rs`). The web UI's Settings page edits both through `PATCH /api/config/patch` / `PATCH /api/providers/patch` (`src/config/patch.rs`), which merge only the fields the form changed into the file already on disk — comments, unmodeled sections, and unmodeled keys survive. The Settings page's Raw tab PUTs whole-file text the user typed directly through `PUT /api/config/raw` / `PUT /api/providers/raw`, which always save the file even when it fails validation — a config reload that can't load the new file keeps the gateway running on its current config and publishes a notice, so the save is reported with diagnostics rather than rejected. |
| `config.example.toml`, `providers.example.toml` | Reference templates, regenerated from Residuum's compiled-in defaults on every startup. Write-blocked by `PathPolicy` — an edit would be silently overwritten at the next restart, so the refusal points at `config.toml`/`providers.toml` instead. |
| `config.last-known-good.toml`, `providers.last-known-good.toml` | Gateway-owned, next to `config.toml`/`providers.toml`. Not user-edited — the gateway copies the live files here after a successful start or reload, and falls back to these copies if the live files later fail to load or start the gateway. See [Config Loading & Startup Fallback](config-loading.md). |
| `agent-keys.toml.enc` | Encrypted agent key store, outside the workspace directory. Managed with `residuum agent-keys` or Settings → Agent keys; the agent adds only keys it mints. Write-blocked by `PathPolicy`, like `secrets.toml.enc`. See [Agent keys](agent-keys.md). |
| `a2a-keys.toml` | A2A caller-key store (hashes only), outside the workspace directory. Managed with `residuum a2a keys` or Settings → A2A. Write-blocked by `PathPolicy`. See [A2A](a2a.md). |

### Key principle

All workspace `.md` files are presented to the agent as markdown files it owns. The observer and reflector prompts (`OBSERVER.md`, `REFLECTOR.md`) exist specifically so the agent can set up a heartbeat to analyze its own past episodes and extracted observations, then iteratively improve those prompts over time.

## Design Principles

These are drawn from [design-philosophy.md](../design-philosophy.md) and inform how every system should be documented:

1. **File-first**: System state lives in files the user can inspect, edit, and version control. No opaque databases (exception: `vectors.db` for embeddings, since raw vectors aren't human-parsable).

2. **Gateway schedules, LLM evaluates**: The gateway handles timing, concurrency, file watching, and schema validation. The LLM is only invoked when judgment is needed.

3. **Agent autonomy with transparency**: The agent acts on its own for routine operations. Every action is visible in the filesystem. Users can always see what the agent did by looking at files.

4. **No silent failures**: Every failure must be visible. Debug/trace logging is not sufficient. Partial failures must be reported, not ignored.

5. **Simple composition**: Systems are independent and compose through shared data (the workspace filesystem and observation log). No system depends on another system's internals.

## System Index

| System | Doc | Primary tools | Config |
|--------|-----|---------------|--------|
| [Config](config.md) | Global settings in `config.toml`/`providers.toml`, editable by the agent on the user's behalf | `write_file`, `edit_file` | `config.toml`, `providers.toml` |
| [Memory](memory.md) | Automatic observation pipeline + searchable index | `memory_search`, `memory_get` | `memory/OBSERVER.md`, `memory/REFLECTOR.md` |
| [Wiki](wiki.md) | Curated knowledge base of concept pages, distilled from episodes | `read_file`, `write_file`, `edit_file` | `wiki/` |
| [Checkpoints](checkpoints.md) | Hidden git history of the workspace and root config files, for recovery | `workspace_history`, `workspace_restore` | `~/.residuum/checkpoints/` |
| [Workbench](workbench.md) | Interactive artifacts the user opens in the web UI, served from their own origin with an injected SDK | `write_file`, `edit_file` (plus the `workbench` skill) | `workbench/` |
| [Heartbeats](heartbeats.md) | Ambient scheduled monitoring | *(automatic — no tools)* | `HEARTBEAT.yml` |
| [Inbox](inbox.md) | Capture and triage items | `inbox_list`, `inbox_read`, `inbox_archive`, `user_inbox_add` | *(none)* |
| [Scheduled Actions](scheduled-actions.md) | One-off future tasks | `schedule_action`, `list_actions`, `cancel_action` | `scheduled_actions.json` |
| [Skills](skills.md) | Loadable instruction modules | `skill_activate`, `skill_deactivate` | per-skill `SKILL.md` |
| [Tool PATH](tools.md) | Runtime-extensible PATH for spawned CLIs (exec + MCP stdio) | *(automatic — no tools)* | `[tools]` in `config.toml`, `~/.residuum/bin` |
| [Agent Keys](agent-keys.md) | Credentials the agent passes to commands and MCP servers without seeing their values | `agent_keys_list`, `agent_key_delete`, `exec` (`keys`, `store_output_as`) | `residuum agent-keys`, `~/.residuum/agent-keys.toml.enc` |
| [MCP](mcp.md) | External tool servers (stdio + HTTP), reconciled against desired state | *(automatic — surfaced as regular tools)* | `config/mcp.json` |
| [Notifications](notifications.md) | Result routing from background tasks | `list_endpoints`, `list_conversations`, `switch_endpoint`, `send_message` | `config/channels.toml` |
| [Idle](idle.md) | Deactivates skills, switches notification channel, and injects a continuity message after user inactivity | *(automatic — no tools)* | `[idle]` in `config.toml` |
| [Background Tasks](background-tasks.md) | Sub-agents and scripts | `subagent_spawn`, `list_agents`, `stop_agent`, `message_agent` | `[background]` in `config.toml`, role skills in `skills/` |
| [Subconscious](subconscious.md) | Instruction-drift classifier that steers the agent | *(automatic — no tools)* | `[subconscious]` in `config.toml`, `SUBCONSCIOUS.md` |
| [Turn Control](turn-control.md) | Stop the running main-agent turn from any interface | *(no tools — a protocol/command control, not a tool)* | *(none)* |
| [Self-Update, Rollback, and Startup Health](self-update.md) | Self-update with automatic rollback, and the readiness signal `residuum serve`/the rollback watchdog wait on | `residuum update`, `residuum serve`, `residuum stop` | *(none)* |
| [Residuum Cloud Tunnel and Remote Control Safety](cloud-tunnel.md) | Remote access via the cloud relay, and the guard that refuses a remote shutdown or cloud-disconnect | *(none — Settings → Residuum Cloud)* | `[cloud]` in `config.toml` |
| [Microsoft Teams](teams.md) | Chat with the agent in Teams DMs, group chats, and channels | *(interface — no tools)* | `[teams]` in `config.toml`, `teams_state.json` |
| [Discord](discord.md) | Chat with the agent in Discord DMs and server channels | *(interface — no tools)* | `[discord]` in `config.toml`, `discord_state.json` |
| [Telegram](telegram.md) | Chat with the agent in Telegram private chats and groups | *(interface — no tools)* | `[telegram]` in `config.toml`, `telegram_state.json` |
| [A2A](a2a.md) | Lets other agents (including a user's own other instances) delegate tasks to this agent, and lets this agent delegate to them, over the Agent2Agent protocol | `list_agents`, `message_agent`, `stop_agent` (addresses `a2a:<name>`); `a2a_task_update` (A2A sessions only) | `[a2a]` in `config.toml`, `residuum a2a keys`, `config/agent-card.json`, `config/a2a.json` |
| [Config Loading & Startup Fallback](config-loading.md) | How `config.toml`/`providers.toml` load, what's fatal vs. skipped with a notice, and the last-known-good fallback when startup can't come up on the live files | *(operations — no tools)* | `config.toml`, `providers.toml`, `config.last-known-good.toml`, `providers.last-known-good.toml` |

## What This Is Not

- Not API documentation. Tool parameter schemas live in the code.
- Not onboarding content. The `residuum-getting-started` skill handles first-run UX.
- Not a design rationale. The `docs/*.md` design docs explain *why* decisions were made. This directory explains *how things are meant to work*.
