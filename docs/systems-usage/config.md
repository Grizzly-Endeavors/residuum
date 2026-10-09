# Config Files

`~/.residuum/` holds `hub/` (hub-level state: never any one agent's workspace), `team/` (the shared team layer), and one directory per agent, each of which *is* that agent's workspace root.

Hub-level settings live in `hub/config.toml`: the timezone (shared by every agent), the gateway bind/port, the cloud tunnel, the A2A listener's enablement/port/public URL, tracing, the Web Push contact, the decision model, and the shared background session budget and cross-agent hop limits. `hub/` also holds the encrypted secret store (`secrets.toml.enc`), the agent-key store (`agent-keys.toml.enc`), A2A caller keys (`a2a-keys.toml`), the Web Push signing key and device list (`push-vapid.key`, `push-devices.json`; see [Notifications](notifications.md#web-push)), and the browsers paired for remote access (`remote-access.json`; see [Remote access](remote-access.md)) — all shared by every agent.

Everything else lives in the agent's own `config/` directory (`~/.residuum/<agent-name>/config/`): `config.toml` (memory, pulse, subconscious, adapters, agent abilities, idle, `autostart`, this agent's A2A `visibility`, and more), `providers.toml` (provider credentials and `[models]` role assignments), plus the narrower per-system files `mcp.json`, `channels.toml`, `agent-card.json`, and `a2a.json` — see each system's own doc.

## Layout

```
~/.residuum/
├── hub/                       # hub-level state; never an agent's workspace
│   ├── config.toml            # hub config
│   ├── secrets.toml.enc, secrets.key
│   ├── agent-keys.toml.enc, agent-keys.key, agent-keys.lock
│   ├── a2a-keys.toml, a2a-keys.lock
│   ├── push-vapid.key, push-devices.json
│   ├── remote-access.json
│   ├── logs/, bin/, checkpoints/
│   ├── residuum.pid, residuum.lock, residuum.ready, residuum.startup-error, crash.log
│   └── *.last-known-good.toml (hub's and each agent's), update markers
└── <agent-name>/              # the agent's directory, and its workspace root
    └── config/                # config.toml, providers.toml, channels.toml, mcp.json, agent-card.json, a2a.json
```

`config::paths` (`src/config/paths.rs`) is the only place that resolves the literal `~/.residuum` path (`residuum_root`); everything else takes a hub directory or an agent directory as a parameter. `HubPaths` names every file under `hub/`.

An agent is any directory directly under `~/.residuum/` that holds `config/config.toml`; a symlink to such a directory counts. `hub`, `team`, `agents`, and any name starting with `.` are never agents. A missing `~/.residuum/` is a fresh install, but a `~/.residuum/` that exists and cannot be read is reported as an error (never treated as empty), so an unreadable install is not mistaken for a fresh one. One process hosts every agent it finds (see [Hub](hub.md)). A fresh install, with nothing under `hub/` and no agent directory, goes through onboarding (the web setup wizard, or `residuum setup`), which asks for the user's name, the first agent's name, the timezone, and the model configuration. Onboarding writes each file atomically and writes the agent's `config/config.toml` last, since that file is what makes the directory an agent: a failure partway through never leaves a discoverable half-configured agent, and setup can be run again. The setup endpoint refuses with a 409 when an agent already exists, so a running gateway's live agent is never overwritten. The name chosen in onboarding is written into that agent's `SOUL.md`. The identity section under the name is left empty. A `SOUL.md` that is still that bundled template is brought up to date the next time the workspace is prepared; one that has been edited is left as it is.

### Agent names

The name a person types is what the team list, setup, and `agent:` addresses use. It is up to 32 characters: letters from any language, numbers, spaces, hyphens, and apostrophes. It can't start or end with a hyphen or an apostrophe, and it can't be `hub`, `team`, or `agents` (compared ignoring case). Two names that differ only by case are the same agent. `Research Desk` and `research-desk` are different names.

The directory, the URL, and the A2A path are a short folder name derived from that name: lowercase ASCII letters, digits, and hyphens, at most 24 characters, with no leading or trailing hyphen. The person never types the folder name. A name with no ASCII letters gets a stable folder name starting with `n`. When two different names would use the same folder, the second is `…-2`. `config::validate_agent_name` is the folder-name check. The typed name is stored as `display_name` in the agent's `config.toml`; when that key is absent, the folder name is shown.

Changing `display_name` in `config.toml` changes the name people see. The folder, the checkpoint history, and the remote address stay where they are.

## What lives in which config file

| File | Holds |
|------|-------|
| `hub/config.toml` | `timezone` (shared), `[gateway]` bind and port, `[cloud]`, `[a2a]` `enabled`/`port`/`public_url`, `[tracing]`, `[push]` `contact` (a `mailto:` address or `https:` URL sent to push services; the project's address when unset), `[background]` `max_concurrent` (the session budget) and `hop_soft_limit`/`hop_hard_limit`, `[system_one]` (the shared [decision model](system-one.md)) |
| `<agent>/config/config.toml` | Everything else: `display_name` (the name people see; absent means the folder name), `autostart` (default `true`), `timeout_secs`, `max_tokens`, `temperature`, `thinking`, `[memory]`, `[pulse]`, `[subconscious]`, `[learning]`, `[retry]`, `[agent]`, `[idle]`, `[discord]`, `[telegram]`, `[teams]`, `[webhooks]`, `[skills]`, `[tools]`, `[web_search]`, `[auto_mode]` (see [Auto Mode](auto-mode.md)), `[a2a]` `visibility`, and `[background]` idle timeouts, `episode_skip_token_floor`, and `subagent_depth_cap` |
| `<agent>/config/providers.toml` | `[providers.*]`, `[models]`, `[background.models]` |

Each file is parsed strictly against its own schema: a hub-only key in an agent's `config.toml` (or the reverse) is an unknown key, dropped with a notice (see [Config Loading](config-loading.md)). There is no `workspace_dir` setting: the agent's directory is its workspace. The user's name lives in `team/USER.md`. The agent's shown name is `display_name`.

## Model calls: thinking, timeouts and streaming

These settings in the agent's `config.toml` shape every call to a model. A model role's own entry in `providers.toml` (`observer = { model = "...", thinking = "off" }`) overrides `temperature` and `thinking` for that role.

**`timeout_secs`** (default 120) bounds a call. A call that returns its answer whole fails when the exchange takes longer than this. A call that streams its answer fails only when no bytes arrive for this long, so a long answer is never cut off for its length. A stalled stream fails with "the model stopped responding for Ns".

**`[retry]`** applies to both kinds of call. A stream that fails partway counts as a transient failure and is retried: the provider sent an error event (an overloaded or rate-limited provider), the connection dropped, or the body ended early. Whatever had already streamed is discarded first, by telling the receiver to start over, so a retry's text never lands after the failed attempt's. A fallback in a provider chain does the same when it takes over (see [Provider chains](config-loading.md#provider-chains)). A failure that survives its retries reaches the user in plain language, and the details go to the log.

**Streaming.** Every provider can hand a response's text and thinking to the caller while the model is still producing them (`InferenceProvider::complete_streaming`). The response it returns when the call ends is complete, and is the same response a call that returned whole (`complete`) would have given for that output. The memory observer, the reflector and the web UI's model-test endpoint use `complete`, so they get the whole response from one non-streaming request. On the wire: Anthropic streams server-sent events, OpenAI-compatible servers stream `chat/completions` chunks with `stream_options.include_usage` so the last chunk carries token usage, Gemini uses `streamGenerateContent?alt=sse`, and Ollama streams newline-delimited JSON.

**`thinking`** is `off`, `on`, `low`, `medium` or `high`, and each provider maps it to its own request:

| Provider | `on` | `low` / `medium` / `high` | Reasoning that comes back |
|----------|------|---------------------------|---------------------------|
| Anthropic | adaptive thinking | adaptive thinking with `output_config.effort` set to that level | a summary, with the signature the API requires back; encrypted `redacted_thinking` blocks are kept unchanged |
| OpenAI-compatible | `reasoning_effort` medium | `reasoning_effort` low / medium / high | whatever the host returns: `reasoning_content` (Fireworks, DeepSeek), `reasoning` (vLLM, OpenRouter), `reasoning_details` (OpenRouter). OpenAI's own Chat Completions API returns no reasoning text |
| Gemini | dynamic thinking budget | budget of 1024 / 8192 / 32768 tokens | thought summaries, requested with `generationConfig.thinkingConfig.includeThoughts`; each thought signature is kept with the part it arrived on |
| Ollama | `think: true` | `think: true` (the level has no effect) | `message.thinking` |

Thinking text is a summary of the model's reasoning wherever the provider offers a choice; Anthropic is asked for `summarized` explicitly because current models omit it otherwise. Models that write their reasoning inline between `<think>` and `</think>` (some OpenAI-compatible servers and Ollama models) are handled for both streamed and whole replies: the tags never appear in the reply's text, and the text between them becomes thinking.

Anthropic models differ in what they accept. Current models take only adaptive thinking, and older ones only a manual token budget. Residuum asks for adaptive thinking first. When the API answers that adaptive thinking isn't supported on the model, the request is sent again with a budget of a quarter (`low`), half (`medium`, `on`) or three quarters (`high`) of `max_tokens`, held between 1024 and one less than `max_tokens`, and with the `interleaved-thinking-2025-05-14` beta header. That model keeps getting manual budgets for as long as its provider stays loaded, and the switch is logged once at `info`. When `max_tokens` is 1024 or lower there is no room for a budget, so thinking is left off for those requests and a `warn` is logged.

Some providers refuse a tool-use exchange whose reasoning is missing, so the reasoning of the exchange in progress (everything after the last assistant reply that made no tool call) is sent back with the tool calls it led to; earlier turns' reasoning is not. Anthropic gets its signed and redacted blocks unchanged, ahead of the rest of the message, and only while the request has thinking on. Gemini 3 gets each thought signature on the part it came from, with function calls before their results. OpenAI-compatible hosts other than OpenAI's own (`api.openai.com`, `*.openai.azure.com`) get the reasoning as `reasoning_content`, which DeepSeek-style hosts require; OpenAI itself never receives it, because it rejects fields it doesn't know. A reasoning block that has no signature to send back, such as one produced by a different provider before a failover, is left out.

## Environment overrides

Hub-level overrides apply: `RESIDUUM_TIMEZONE`, `RESIDUUM_GATEWAY_BIND`, `RESIDUUM_GATEWAY_PORT`, and `RESIDUUM_CLOUD_TOKEN`. Agent-scoped variables have no effect, because a hub-wide environment would apply the same value to every agent: `RESIDUUM_WORKSPACE`, `RESIDUUM_MODEL`, `RESIDUUM_PROVIDER_URL`, `RESIDUUM_API_KEY`, `RESIDUUM_OBSERVER_MODEL`, `RESIDUUM_REFLECTOR_MODEL`, `RESIDUUM_OBSERVER_API_KEY`, `RESIDUUM_REFLECTOR_API_KEY`, `RESIDUUM_DISCORD_TOKEN`, `RESIDUUM_TELEGRAM_TOKEN`, and `RESIDUUM_TEAMS_APP_PASSWORD`. If one of them is set, startup publishes a notice naming it once; config reloads do not repeat it. Set the equivalent value in the agent's `config.toml` or `providers.toml`; a `"${ENV_VAR}"` reference inside those files still reads any environment variable you choose, and the provider-specific variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and so on) still supply provider keys.

## Who can write what

| File | Writable by the agent? |
|------|------------------------|
| `config/config.toml`, `config/providers.toml` (agent's own) | Yes — the file tools (`write_file`, `edit_file`) allow it, same as any other workspace file. |
| `config/config.example.toml`, `config/providers.example.toml` | No. `PathPolicy` (`src/tools/path_policy.rs`) refuses every write. |
| `config/mcp.json`, `config/channels.toml` | Yes, unless `agent.modify_mcp`/`agent.modify_channels` is turned off in the agent's `config.toml`. |
| `config/a2a.json`, `HEARTBEAT.yml` | Yes, same as any other workspace file — no ability gate. |
| `hub/config.toml` | Yes, by absolute path, same as the agent's own `config.toml`. Its reload outcome reaches the agent the same way (see below). |
| `hub/secrets.toml.enc`, `hub/secrets.key`, `hub/agent-keys.*`, `hub/a2a-keys.*`, `hub/push-vapid.key`, `hub/push-devices.json` (the credential stores, and the Web Push key and device list) | No. `PathPolicy` refuses every write. |

The `.example.toml` files are reference templates, not user config: `config::bootstrap::bootstrap_agent_at` (and `bootstrap_hub_at` for the hub's own example) regenerate them from the binary's compiled-in defaults on every startup, so a write to either would appear to succeed and then be silently overwritten at the next restart. `PathPolicy::check_write` gives a write refusal for one of them a distinct message naming this and pointing at the writable file instead of the generic "user-managed configuration" refusal every other blocked path gets.

## How the agent edits config.toml / providers.toml

The bundled `residuum-system` skill's [config reference](../../assets/bundled-skills/residuum-system/references/config.md) is the agent-facing guidance: which file holds a given setting, editing with a targeted diff rather than a full rewrite (both files carry comments and commented-out examples), matching the file's existing style, and using a `secret:<name>` or `${ENV_VAR}` reference rather than a plaintext credential when the field already holds one or the user has a matching secret stored.

## Reload

Three background pollers (`src/gateway/watcher.rs`) watch these files by mtime and signal a reload on change, regardless of what wrote the file — the web UI's Settings form, its Raw config section, a manual edit, or the agent's own file tools all take the same path.

| Poller | Watches | Signal | Handler |
|--------|---------|--------|---------|
| `spawn_root_config_watcher` | the agent's `config.toml` and `providers.toml` | `ReloadSignal::Agent` | `handle_root_reload` |
| `spawn_hub_config_watcher` | `hub/config.toml` | `ReloadSignal::Hub` | `handle_hub_reload` |
| `spawn_workspace_watcher` | `mcp.json`, `channels.toml`, `agent-card.json`, `a2a.json` | `ReloadSignal::Workspace` | `handle_workspace_reload` |

Each handler (`src/gateway/reload.rs`, `src/gateway/event_loop/run_loop.rs`) diffs old against new, rebuilds the changed subsystems in place, and publishes a `NoticeEvent` on the system notification channel: `"configuration reloaded: {summary}"` (or `"hub configuration reloaded: {summary}"`) on success, or `"config reload failed (keeping current config): {err}"` on a parse or validation failure — the previous config keeps running until the file is fixed. The hub and the agent reload independently, and each keeps its own last-known-good copy (see [Config Loading](config-loading.md)). A hub reload rebuilds what the hub owns (gateway listener, A2A listener, tunnel, tracing). A `timezone` change is labeled `timezone` in the notice and applies live to the agent's turn timestamps and its `schedule_action`, `list_actions` and `user_inbox_add` tools, the observer and reflector, pulse, idle handling, chat commands, and newly started background sessions. The Discord, Telegram and Teams adapters restart so their message timestamps use it. Web API and webhook timestamps, inbox notification timestamps, and background sessions that are already running keep the previous timezone until the next restart, and a notice says so. A change to `[background] max_concurrent` also takes effect at the next restart, and a notice says so. Reload signals are queued rather than overwritten: when hub, agent and workspace files change close together, each kind is processed (hub first), and repeats of the same kind queued behind one another collapse into a single reload. A reload requested during a turn runs after the turn ends. `handle_workspace_reload` does the equivalent for the workspace files, one subsystem at a time, and publishes its own notice per subsystem plus a final `"workspace configuration reloaded"` notice. `HEARTBEAT.yml` reloads on a different path entirely — the pulse scheduler re-parses it on every scheduler tick (see `heartbeats.md`), not through `ReloadSignal` at all.

**When the reload was caused by the agent's own write**, its outcome also reaches the agent directly. `ConfigWriteWatch` (`src/tools/config_reload_tracker.rs`), wired only into main's `write_file`/`edit_file` — never a session's — recognizes a successful write to one of seven reload-triggering paths (the agent's `config.toml` and `providers.toml`, `hub/config.toml`, `mcp.json`, `channels.toml`, `a2a.json`, `HEARTBEAT.yml`) and marks it on a `SharedConfigReloadTracker` shared with the gateway event loop. The first reload of the matching kind (agent config, hub config, workspace, or the pulse scheduler's next tick for `HEARTBEAT.yml`) consumes that mark and, in addition to the usual notice on the system notification channel, calls `Agent::inject_system_message` with the same outcome text — so it shows up as a system note in the agent's own transcript on its next step, the same mechanism used for other system-originated notes. A mark older than 30 seconds is dropped rather than attributed to a later, unrelated reload (e.g. a manual edit made shortly after). So after writing, the agent can check the note that follows and tell the user if something went wrong.
