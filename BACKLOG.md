# Backlog

Organized into parallel-safe groups ordered by priority. Items within a group that share files are marked with sequencing constraints.

---

## Group 1 — Low-Risk Removals & Fixes

Quick wins touching isolated modules. All four items are parallel-safe with each other.

### #13 — Remove vestigial script execution

Delete the `Script` execution path from `background/`. It is fully implemented but unreachable — no tool, pulse, or action ever constructs `Execution::Script`. The agent handles scripts via `write_file`/`exec`.

**Remove:**
- `background/script.rs` — entire file
- `background/mod.rs` — `mod script;` declaration
- `background/types.rs` — `Script(ScriptConfig)` variant from `Execution` enum, `ScriptConfig` struct, `Execution::Script` arm in `execution_info()`, and test helpers that construct `ScriptConfig`
- `background/spawner.rs` — `use super::script::execute_script;` import, `Execution::Script(config) => execute_script(...)` match arm, and all tests using `ScriptConfig`/`Execution::Script`
- `gateway/server/mod.rs` — dead `Execution::Script(_) => BackgroundModelTier::Small` match arm
- `background/README.md` — remove Script references

### #16 — Remove `hooks/` from workspace layout and bootstrap

The `hooks/` directory was planned but never used. Remove it from `workspace/layout.rs` (the path constant and any `required_dirs` entry) and `workspace/bootstrap.rs` (the `create_dir_all` call). Grep for any other references.

### #18 — Warn on zero-channel notification routing

In `notify/router.rs`, when a notification (pulse name or action result) routes to zero channels, the result is silently discarded. Add `tracing::warn!(name = %pulse_name, "notification routed to zero channels")` so operators can see misconfigured pulse→channel mappings.

### #14 — Fix `memory_search` source filter values

The `memory_search` tool accepts source filter values `"observation"` and `"chunk"` in code, but the design doc specifies `"observations"`, `"episodes"`, and `"both"`. Update `tools/memory_search.rs` (the tool's schema and argument parsing) and `memory/search.rs` (the filtering logic) to accept and document the design doc values. Check `tools/TOOLS.md` for consistency.

---

## Group 2 — Background Task & Subagent Improvements

Shares the `background/` cluster. Must run after Group 1 merges (item 13 changes `background/`). Items within this group are sequential.

### #17 — Full transcript capture for background tasks

Currently `execute_subagent()` in `background/subagent.rs` returns only `texts.last().cloned().unwrap_or_default()` — the final text response. The full conversation (tool calls, tool results, intermediate messages) accumulates in a local `RecentMessages` during the turn but is dropped. `write_transcript()` in `background/spawner.rs` writes that single string.

**Fix:** Change `execute_subagent()` to return the full `RecentMessages` buffer (or a serializable representation). Update `write_transcript()` to accept structured message data and serialize it as JSON (array of messages with role, content, tool_call fields). Keep the existing summary as a top-level field alongside the full messages for quick scanning.

**Files:** `background/subagent.rs` (return type), `background/spawner.rs` (`write_transcript` signature and serialization), `agent/recent_messages.rs` (add `Serialize` derive or a `to_json()` method if not already present).

### #24 — Remove `wait` parameter from `subagent_spawn`

The `wait: true` option on `subagent_spawn` (in `tools/background.rs`) runs the sub-agent synchronously and returns the output directly. This is redundant — setting the channel to `agent_wake` achieves the same result asynchronously without blocking the tool call.

**Remove from `tools/background.rs`:** the `"wait"` field from the tool `definition()` schema, the `let wait = ...` parsing line, the `if wait { ... }` branch (which calls `execute_subagent` directly and bypasses the spawner), and the `channels_ignored_in_sync_mode` test. Keep only the async spawn path. Update `tools/TOOLS.md` to remove the `wait` parameter from the `subagent_spawn` schema.

---

## Group 3 — Pulse & Scheduling

Isolated to `pulse/` and `actions/`. Parallel-safe with Groups 1, 2, 4–9. Run 12 before 11 (both modify `scheduler.rs` and `types.rs`).

### #12 — Persist pulse `last_run` timestamps to disk

`PulseScheduler` in `pulse/scheduler.rs` tracks `last_run: HashMap<String, NaiveDateTime>` in memory only. On every restart, all enabled pulses fire immediately regardless of when they last ran.

**Fix:** Add a `pulse_state.json` path to `workspace/layout.rs`. On scheduler construction, load existing timestamps from this file (missing file = fresh start, which is fine). After each `due_pulses()` call that updates timestamps, save the map to disk (atomic write via temp file + rename). Use `serde_json` for the format. The file should be a simple `{ "pulse_name": "2026-02-28T14:30:00" }` map.

**Files:** `pulse/scheduler.rs` (add `state_path: PathBuf` field, load/save methods), `pulse/types.rs` (if any type changes needed for serialization), `workspace/layout.rs` (add the path constant).

### #11 — Trigger count option for heartbeat pulses

Add an optional `trigger_count: Option<u32>` field to `PulseDef` in `pulse/types.rs`. When set, the pulse fires exactly N times across its active period, roughly evenly spaced, with randomization (jitter) so triggers don't feel rigid. After N firings, the pulse stops firing for that active period (resets next period).

**Implementation:** In `pulse/scheduler.rs`, track a `run_count: HashMap<String, u32>` alongside `last_run`. In `due_pulses()`, if a pulse has `trigger_count` set, check `run_count < trigger_count` before marking as due. Calculate spacing as `active_period_duration / trigger_count` with ±15% random jitter. Persist `run_count` in the same `pulse_state.json` from item 12 (this is why 12 must land first). Reset counts when the active period rolls over.

**Files:** `pulse/types.rs` (new field), `pulse/scheduler.rs` (counting + spacing logic), `pulse/executor.rs` (if any changes needed for the new scheduling mode).

---

## Group 4 — Skill Priority Fix

Isolated to skills module. Parallel-safe with all groups.

### #20 — Fix skill priority order

Current skill loading order is wrong. Correct priority (highest to lowest): Project skills > Workspace skills > User-global skills > Bundled skills. When skills share a name, higher-priority sources should shadow lower ones. Check `skills/index.rs` for the loading/merge order and fix it. Update `skills/types.rs` if the `SkillSource` enum or ordering logic lives there.

---

## Group 5 — CLI & UX Improvements

Touches `channels/cli/` and `main.rs`. Parallel-safe with all other groups. Run 1 before 2 (both touch CLI internals).

### #1 — CLI onboarding and logging

**Current state:** The CLI prints a single banner line on connect: `ironclaw v0.1.0 — connected to ws://127.0.0.1:7700/ws`. No logging to disk exists. The gateway outputs tracing logs to stdout but the CLI client has none.

**Changes needed:**
1. **Debug log on disk:** Write CLI session logs (sent messages, received responses, connection events, errors) to `~/.ironclaw/logs/cli-YYYY-MM-DD.log`. Rotate daily. Use `tracing_appender` or a simple file writer.
2. **`ironclaw logs` command:** New subcommand that reads and displays the log file. Add a `-w`/`--watch` flag that tails the file (like `tail -f`).
3. **Welcome message on first launch:** Detect first launch (no config file exists or a `.first_run` marker). Print a welcome message with a clickable `http://` URL pointing to the gateway web UI. Currently the banner shows `ws://ip:port/ws` which isn't clickable and ctrl-c to copy kills the process.
4. **Clickable link in banner:** Always include `http://127.0.0.1:7700` (or the configured address) as a proper URL in the connect banner so terminals can make it clickable.

**Files:** `main.rs` (new `logs` subcommand, first-launch detection), `channels/cli/mod.rs` (`print_banner` changes, log file setup), `channels/cli/render.rs` (if welcome message formatting lives here).

### #2 — CLI config/onboarding wizard

**Current state:** `bootstrap_at()` in `config/bootstrap.rs` silently writes a minimal `config.toml` with a commented-out timezone and a single model line. No interactive prompts. When config is invalid, the system falls back to a web-based setup UI (`--setup` flag).

**Changes needed:** Add a terminal-based config wizard that runs when `config.toml` doesn't exist (or via `ironclaw setup` subcommand). Two modes:
1. **Interactive:** Prompt for timezone (detect system TZ as default), API provider (anthropic/openai/ollama/gemini), API key (masked input), model selection. Write the result to `config.toml`.
2. **Non-interactive:** Accept all values via CLI flags (`--timezone`, `--provider`, `--api-key`, `--model`) for scripted/agent use. Exit non-zero if required values are missing.

**Files:** `config/bootstrap.rs` (wizard logic), `main.rs` (new `setup` subcommand), `channels/cli/` (input helpers for masked key entry, selection menus).

---

## Group 6 — Notification & Channel Architecture

Heavy overlap across `notify/`, `channels/`, `pulse/`, and `config/`. Sequential within the group. Should run after Group 3 merges (item 23 changes pulse routing).

### #23 — Rename NOTIFY.yml → CHANNELS.yml, split pulse routing

NOTIFY.yml currently serves double duty: channel registry (what channels exist) and pulse routing (which pulses go where). Split these:
- **CHANNELS.yml** becomes the source of truth for available channels (built-in + external). Referenced by `subagent_spawn`, `schedule_action`, and heartbeat pulses.
- **HEARTBEAT.yml** gets a `channels: [...]` array on each pulse definition for its own routing, co-located with schedule and tasks.

This eliminates the indirect pulse-name-to-channel mapping. Update `notify/*`, `pulse/types.rs`, `workspace/layout.rs`, `workspace/bootstrap.rs`, and any `config/` references.

### #8 — Differentiate internal vs external channels

Create a clear boundary between internal channels (`agent_wake`, `agent_feed`, `inbox` — used for inter-component communication) and external channels (outbound webhook, ntfy, http endpoints — user-facing notifications). This may involve separate enums, traits, or module organization in `notify/` and `channels/`.

### #9 — `send_message` tool

New agent tool that sends a message to any configured channel in CHANNELS.yml. Support text content and file attachments (if the channel supports it). The tool should validate the channel exists, format the message appropriately for the channel type, and report delivery success/failure.

### #5 — Unified slash command interface

Common slash command parsing and dispatch for all channel types (CLI, Discord, WebSocket, future web). Consistent error message formatting across channels. Currently each channel has its own command parsing.

### #6 — Improve inbox/external channel interface

Clarify the interface for sending items to the agent's inbox and supporting external channels. Make it easier to route messages from external sources into the agent's inbox without triggering a new agent turn.

### #7 — `/inbox` command for Discord

Already implemented for CLI and WebSocket. Extend to Discord: allow users to send a message or file directly to the agent's inbox via a Discord slash command without triggering a new agent turn.

---

## Group 7 — Config & Gateway Hardening

Touches `config/` and `gateway/`. Item 3 is independent; run 4 before 19. Conflicts with Group 6 on `config/` — schedule after Group 6 or before it starts.

### #3 — Disallow LLM from editing config files

**Current state:** `PathPolicy::check_write()` in `tools/path_policy.rs` enforces: `archive/` is read-only, writes inside `projects/` must be in the active project, all other workspace-level writes are unrestricted (`Ok(())`). Config files (`~/.ironclaw/config.toml`) are outside the workspace entirely and have no protection. Identity files (`SOUL.md`, `USER.md`, `ENVIRONMENT.md`, `AGENTS.md`), `HEARTBEAT.yml`, `NOTIFY.yml`, and `MEMORY.md` inside the workspace are also unprotected.

**Fix:** Add a blocked paths mechanism to `PathPolicy` — either a `blocked_paths: HashSet<PathBuf>` field or an explicit check for config file patterns. Block writes to: `config.toml`, `config.example.toml`, `HEARTBEAT.yml`, `CHANNELS.yml`/`NOTIFY.yml`, identity files (`SOUL.md`, `ENVIRONMENT.md`, `USER.md`, `AGENTS.md`). Populate during `PathPolicy::new_shared()` in `gateway/server/mod.rs`. The existing `WriteTool` and `EditTool` already call `check_write()` so no changes needed there beyond the policy logic.

### #4 — Improve secret handling

API keys are stored in plaintext in `config.toml`. Explore alternatives: environment variable references (`$ANTHROPIC_API_KEY`), integration with `infisical` CLI (already installed), or a separate encrypted secrets file. At minimum, support env var expansion in the API key fields during config resolution.

**Files:** `config/resolve.rs` (env var expansion), `config/types.rs` (document the feature), `config/deserialize.rs` (if parsing changes needed).

### #19 — Config internals cleanup

`resolve.rs` is `.and_then()` soup. Defaults are defined in three places: `constants.rs`, `types.rs` `Default` impls, and `resolve.rs` `unwrap_or` calls. There are 16 deserialize structs for ~30 config knobs. Consolidate defaults into one location, simplify the resolution pipeline, reduce struct count where possible. This is a refactor — no behavior changes.

---

## Group 8 — Model & Provider Improvements

Isolated to `models/` and `config/`. Items 21 and 10 are parallel-safe with each other. Slight conflict with Group 7 on `config/` — schedule accordingly.

### #21 — Model failover chain

Implement ordered fallback: primary model → fallback chain on rate limit or error. The design doc describes a `failover` module and auth profile rotation (multiple API keys per provider). Currently each role gets a single provider/model with no fallback beyond the `default` role in `[models]`.

**Files:** `models/factory.rs` (failover wrapper), `models/retry.rs` (extend to support fallback providers), `config/types.rs` and `config/resolve.rs` (new config schema for fallback chains and multiple auth profiles).

### #10 — HTTP/SSE transport for MCP servers

Currently MCP servers only support stdio transport. Add HTTP and SSE transport options so MCP servers can be reached over the network. Self-contained in `mcp/`.

---

## Group 9 — Agent Context & Memory

Touches `agent/context/` and `projects/`. Parallel-safe with most groups.

### #22 — Auto-load recent project logs on activation

**Current state:** When a project is activated, `format_active_context_for_prompt()` in `projects/activation.rs` includes the project name, `PROJECT.md` body, and a file manifest (just filenames and sizes). The agent sees log files listed as `notes/log/2026-02/log-23.md (2.3 KB)` but not their contents. To get continuity from prior sessions, the agent must manually `read_file`.

**Fix:** On activation, read the most recent log entries (e.g., last 2-3 session logs or current month's log file, capped at a token budget) and include their content in the activation context. Add a `recent_log: Option<String>` field to `ActiveProject` in `projects/types.rs`. Populate it in `activate()` by reading from `notes/log/`. Include it in `format_active_context_for_prompt()` output.

**Files:** `projects/activation.rs` (read log files during activation), `projects/types.rs` (`ActiveProject` struct), `projects/manifest.rs` (if log reading logic is shared with manifest building). The change surfaces to the system prompt via `agent/context/loading.rs` → `build_project_context_strings()` which calls `format_active_context_for_prompt()`.

---

## Group 10 — OAuth Provider Support (lowest priority)

Depends on Group 8 item 21 (failover) so new providers plug into the fallback chain.

### OpenAI Codex OAuth
- **Endpoint:** `https://chatgpt.com/backend-api/codex/responses` (NOT `api.openai.com`)
- **API format:** OpenAI Responses API (not Chat Completions)
- **Auth:** Bearer JWT + `chatgpt-account-id` (extracted from JWT claims) + `OpenAI-Beta: responses=experimental`
- **OAuth flow:** PKCE via `auth.openai.com`, callback on `localhost:1455`, refresh via `auth.openai.com/oauth/token`
- **Tied to:** ChatGPT Plus/Pro subscription credits, not OpenAI Platform billing
- **Reference code:** `~/Projects/pi-mono/packages/ai/src/providers/openai-codex-responses.ts` (HTTP layer), `~/Projects/pi-mono/packages/ai/src/utils/oauth/openai-codex.ts` (OAuth flow)

### Google Gemini CLI OAuth
- **Endpoint:** `https://cloudcode-pa.googleapis.com/v1internal:streamGenerateContent?alt=sse` (internal Cloud Code Assist, NOT public `generativelanguage.googleapis.com`)
- **API format:** Gemini content generation format with Cloud Code Assist wrapper
- **Auth:** Bearer `ya29.*` token + `projectId` (auto-provisioned during OAuth), spoofed IDE user-agent headers
- **OAuth flow:** Standard Google OAuth with PKCE, callback on `localhost:8085`, auto-discovers/provisions GCP project
- **Reference code:** `~/Projects/pi-mono/packages/ai/src/providers/google-gemini-cli.ts` (HTTP layer), `~/Projects/pi-mono/packages/ai/src/utils/oauth/google-gemini-cli.ts` (OAuth flow)

### Anthropic OAuth — Adopt pi-ai's Approach
Our current Anthropic OAuth is minimal: prefix detection (`sk-ant-oat01-`) + header swap. pi-ai's implementation is significantly more complete:
- **Full PKCE login flow** via `claude.ai/oauth/authorize` + token exchange at `console.anthropic.com/v1/oauth/token`
- **Automatic token refresh** (short-lived access tokens + refresh tokens, 5-min expiry buffer)
- **Additional headers** we're missing: `claude-code-20250219` beta, `user-agent: claude-cli/{version}`, `x-app: cli`
- **Anthropic SDK** uses `authToken` (Bearer) vs `apiKey` (x-api-key) — cleaner than our manual header swap
- Reference: `~/Projects/pi-mono/packages/ai/src/utils/oauth/anthropic.ts` (OAuth flow), `~/Projects/pi-mono/packages/ai/src/providers/anthropic.ts:546-565` (client creation)

### Implementation Notes
- Each needs a **new provider** in `src/models/`, not just auth header detection
- OpenAI Codex uses a completely different request/response format (Responses API vs Chat Completions)
- Google Gemini CLI uses an internal Google endpoint with a custom request wrapper, not the public Gemini API
- All three (including Anthropic) need proper token refresh logic (short-lived access tokens + refresh tokens)
- OpenClaw's full OAuth registry is in `~/Projects/pi-mono/packages/ai/src/utils/oauth/index.ts` — supports anthropic, github-copilot, google-gemini-cli, google-antigravity, openai-codex

---

## Parallelism Map

```
         ┌── Group 1 (removals/fixes) ──────────────────────┐
         ├── Group 3 (pulse/scheduling)                      │
         ├── Group 4 (skill priority)                        │
         ├── Group 5 (CLI/UX)                                │
  START ─┤── Group 9 (auto-load logs)                        ├─→ MERGE
         ├── Group 7.3 (config protection)                   │
         └── Group 8.10 (MCP HTTP/SSE)                       │
                                                             │
         After Group 1 merges:                               │
         ├── Group 2 (background/subagent) ──────────────────┤
                                                             │
         After Groups 3, 5 merge:                            │
         ├── Group 6 (notification architecture) ────────────┤
                                                             │
         After Group 7.3+4:                                  │
         ├── Group 7.19 (config cleanup) ────────────────────┤
         ├── Group 8.21 (model failover) ────────────────────┤
                                                             │
         After Group 8.21:                                   │
         └── Group 10 (OAuth) ───────────────────────────────┘
```
