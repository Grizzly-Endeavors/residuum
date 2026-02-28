# Backlog

Organized into parallel-safe groups ordered by priority. Items within a group that share files are marked with sequencing constraints.

---

## Group 1 — Low-Risk Removals & Fixes

Quick wins touching isolated modules. All four items are parallel-safe with each other.

| # | Item | Files Touched |
|---|------|---------------|
| **13** | Remove vestigial script execution code (`script.rs`, `ScriptConfig`, `Execution::Script`) | `background/script.rs`, `background/types.rs`, `background/mod.rs` |
| **16** | Remove `hooks/` directory from workspace layout and bootstrap (artifact, never used) | `workspace/layout.rs`, `workspace/bootstrap.rs` |
| **18** | Add `tracing::warn!` when a notification routes to zero channels (currently silent) | `notify/router.rs` |
| **14** | Fix `memory_search` source filter values to match design doc (`"observations"`, `"episodes"`, `"both"`) | `tools/memory_search.rs`, `memory/search.rs` |

---

## Group 2 — Background Task & Subagent Improvements

Shares the `background/` and `tools/background.rs` cluster. Must run after Group 1 merges (item 13 changes `background/`). Items within this group are sequential.

| # | Item | Files Touched |
|---|------|---------------|
| **17** | Full transcript capture — serialize `recent_messages` instead of `texts.last()` | `background/subagent.rs`, `background/spawner.rs`, `agent/recent_messages.rs` |
| **24** | Remove `wait` parameter from `subagent_spawn` (setting channel to `agent_wake` does the same thing) | `tools/background.rs`, `tools/TOOLS.md` |
| **15** | Include active skills in sub-agent context | `background/spawn_context.rs`, `background/subagent.rs`, `agent/context/assembly.rs` |

---

## Group 3 — Pulse & Scheduling

Isolated to `pulse/` and `actions/`. Parallel-safe with Groups 1, 2, 4–9. Run 12 before 11 (both modify `scheduler.rs` and `types.rs`).

| # | Item | Files Touched |
|---|------|---------------|
| **12** | Persist pulse `last_run` timestamps to disk (`pulse_state.json`). Currently in-memory only — every pulse fires on restart | `pulse/scheduler.rs`, `pulse/types.rs`, `workspace/layout.rs` |
| **11** | Add trigger count option for heartbeat pulses — schedule N triggers evenly across the active period with randomization | `pulse/types.rs`, `pulse/scheduler.rs`, `pulse/executor.rs` |

---

## Group 4 — Skill Priority Fix

Isolated to skills module. Parallel-safe with all groups.

| # | Item | Files Touched |
|---|------|---------------|
| **20** | Fix skill priority order: Project > Workspace > User-global > Bundled | `skills/index.rs`, `skills/types.rs` |

---

## Group 5 — CLI & UX Improvements

Touches `channels/cli/` and `main.rs`. Parallel-safe with all other groups. Run 1 before 2 (both touch CLI internals).

| # | Item | Files Touched |
|---|------|---------------|
| **1** | CLI onboarding: save logs to debug file on disk, add `ironclaw logs` command with `-w`/watch flag, welcome message with clickable `http://` link on first launch | `main.rs`, `channels/cli/mod.rs`, `channels/cli/render.rs`, new logging module |
| **2** | CLI config/onboarding wizard for headless environments — interactive (for users) and non-interactive (for coding agents) | `config/bootstrap.rs`, `channels/cli/` |

---

## Group 6 — Notification & Channel Architecture

Heavy overlap across `notify/`, `channels/`, `pulse/`, and `config/`. Sequential within the group. Should run after Group 3 merges (item 23 changes pulse routing).

| # | Item | Files Touched |
|---|------|---------------|
| **23** | Rename NOTIFY.yml → CHANNELS.yml, move pulse routing into HEARTBEAT.yml. Split channel registry from pulse routing | `notify/*`, `pulse/types.rs`, `workspace/layout.rs`, `workspace/bootstrap.rs`, `config/` |
| **8** | Stronger differentiation between internal channels (`agent_wake`, `agent_feed`, `inbox`) and external (webhook, ntfy, http) | `notify/`, `channels/` |
| **9** | `send_message` tool — agent sends to any configured channel, with file attachments if supported | `tools/` (new), `notify/router.rs`, `channels/` |
| **5** | Unified slash command interface for web/discord/other channel support, including error handling | `channels/`, `gateway/server/` |
| **6** | Improve clarity and interface for sending items to the agent's inbox and supporting external channels | `channels/`, `inbox/` |
| **7** | `/inbox` command for Discord (already implemented for CLI/WS) | `channels/discord/` |

---

## Group 7 — Config & Gateway Hardening

Touches `config/` and `gateway/`. Item 3 is independent; run 4 before 19. Conflicts with Group 6 on `config/` — schedule after Group 6 or before it starts.

| # | Item | Files Touched |
|---|------|---------------|
| **3** | Disallow LLM from editing main config files — enforce at gateway | `gateway/server/mod.rs`, `tools/path_policy.rs`, `tools/write.rs`, `tools/edit.rs` |
| **4** | Improve secret handling so keys are not in a plain file | `config/resolve.rs`, `config/types.rs`, `config/deserialize.rs` |
| **19** | Config internals cleanup — consolidate defaults, simplify `resolve.rs`, reduce deserialize struct count | `config/*` |

---

## Group 8 — Model & Provider Improvements

Isolated to `models/` and `config/`. Items 21 and 10 are parallel-safe with each other. Slight conflict with Group 7 on `config/` — schedule accordingly.

| # | Item | Files Touched |
|---|------|---------------|
| **21** | Model failover: primary → ordered fallback chain on rate limit/error, auth profile rotation (multiple API keys per provider) | `models/factory.rs`, `models/retry.rs`, `config/types.rs`, `config/resolve.rs` |
| **10** | HTTP/SSE transport support for MCP servers | `mcp/` (self-contained) |

---

## Group 9 — Agent Context & Memory

Touches `agent/context/` and `projects/`. Parallel-safe with most groups.

| # | Item | Files Touched |
|---|------|---------------|
| **22** | Auto-load most recent project logs on activation (currently only PROJECT.md frontmatter + body) | `agent/context/loading.rs`, `agent/context/assembly.rs`, `projects/activation.rs` |

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
