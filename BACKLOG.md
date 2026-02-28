- Nothing the LLM or user (via an external channel) does should be able to disrupt/break/otherwise disable the gateway in any way shape or form.
- Disallow the LLM from editing the main config files under any circumstance.
- `/inbox` command for sending a message or file directly to the agent's inbox without triggering a new agent turn.
- `send_file` tool for the agent to send attachments to the user.
- HTTP/SSE transport support for MCP servers.
- Add a trigger count option for heartbeat pulses that can be provided in place of interval. It would schedule a number of triggers equal to the count across the active period. Triggers would be roughly evenly spaced throughout the active period, with added randomization to make the triggers feel less rigid.
- Persist pulse `last_run` timestamps to disk (e.g. `pulse_state.json`). Currently in-memory only — every pulse fires on gateway restart and every new pulse fires immediately on creation. Timestamps should survive restarts so pulses resume their schedule.
- Vestigial script execution code in `src/background/` (script.rs, ScriptConfig, Execution::Script) should be removed. Scripts are handled by the agent via write_file/exec.
- `memory_search` source filter values in code (`"observation"`, `"chunk"`) don't match design doc (`"observations"`, `"episodes"`, `"both"`). Fix code to match design doc.
- Sub-agent context should include active skills (currently excluded).
- ~~Remove `docs/plugin-system-design.md` (plugin system abandoned).~~ DONE
- Remove `hooks/` directory from workspace layout and bootstrap (artifact, never used).
- Background task transcripts only capture the final text response, not the full turn. `execute_subagent` returns `texts.last()` and `write_transcript` writes that single string. The full conversation (tool calls, tool results, intermediate messages) accumulates in `RecentMessages` during the turn but is dropped without serialization. Fix: serialize `recent_messages` to the transcript file instead of (or in addition to) the summary.
- Add `tracing::warn!` when a notification routes to zero channels (pulse name not in any NOTIFY.yml entry). Currently silent — result is discarded with no log line.
- Config internals cleanup: resolve.rs is `.and_then()` soup, defaults are defined in three places (constants.rs, types.rs Default impls, resolve.rs unwrap_or), 16 deserialize structs for ~30 knobs. Loosely coupled from the web UI — clean up whenever.
- Skill priority is wrong. Project skills > Workspace >  User-global > Bundled
- Implement model failover: primary model → ordered fallback chain on rate limit or error. Design doc describes a `failover` module and auth profile rotation (multiple API keys per provider). Currently each role gets a single provider/model with no fallback beyond the `default` role in `[models]`.
- Auto-load most recent project logs on activation. Currently only PROJECT.md (frontmatter + body) is loaded; notes/logs/references require explicit `read_file`. The intended behavior is that recent session logs are loaded automatically so the agent has immediate context about where things left off.
- Rename NOTIFY.yml → CHANNELS.yml and move pulse routing into HEARTBEAT.yml. Currently NOTIFY.yml serves double duty as both a channel registry (what channels exist) and pulse routing config (which pulses go where). Split these concerns: CHANNELS.yml becomes the source of truth for available channels (built-in + external, referenced by subagent_spawn, schedule_action, and heartbeat pulses). Each pulse in HEARTBEAT.yml gets a `channels: [...]` array for its own routing, co-located with its schedule and tasks. This eliminates the indirect pulse-name-to-channel mapping and makes pulse config self-contained.
- remove the wait parameter on the `subagent_spawn` tool, setting the channel to `agent_wake` does the same thing.

## OAuth Provider Support (OpenAI Codex & Google Gemini)

Both OpenAI and Google have subscriber-level OAuth flows that OpenClaw supports. Unlike Anthropic's OAuth (simple header swap, same endpoint), both require new providers — different endpoints and API formats.

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

### Anthropic OAuth — Consider Adopting pi-ai's Approach
Our current Anthropic OAuth is minimal: prefix detection (`sk-ant-oat01-`) + header swap. pi-ai's implementation is significantly more complete:
- **Full PKCE login flow** via `claude.ai/oauth/authorize` + token exchange at `console.anthropic.com/v1/oauth/token`
- **Automatic token refresh** (short-lived access tokens + refresh tokens, 5-min expiry buffer)
- **Additional headers** we're missing: `claude-code-20250219` beta, `user-agent: claude-cli/{version}`, `x-app: cli`
- **Anthropic SDK** uses `authToken` (Bearer) vs `apiKey` (x-api-key) — cleaner than our manual header swap
- Reference: `~/Projects/pi-mono/packages/ai/src/utils/oauth/anthropic.ts` (OAuth flow), `~/Projects/pi-mono/packages/ai/src/providers/anthropic.ts:546-565` (client creation)

### Implementation Notes
- Each would need a **new provider** in `src/models/`, not just auth header detection
- OpenAI Codex uses a completely different request/response format (Responses API vs Chat Completions)
- Google Gemini CLI uses an internal Google endpoint with a custom request wrapper, not the public Gemini API
- All three (including Anthropic) need proper token refresh logic (short-lived access tokens + refresh tokens)
- OpenClaw's full OAuth registry is in `~/Projects/pi-mono/packages/ai/src/utils/oauth/index.ts` — supports anthropic, github-copilot, google-gemini-cli, google-antigravity, openai-codex
