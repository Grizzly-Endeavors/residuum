# Agent Keys

**Status:** in implementation on `feat/agent-keys`.

## Goal

Give the agent credentials (API keys, tokens) it can use in `exec` calls and MCP servers, including from pulses and scheduled actions, without the value ever entering the model's context, the transcript, memory, or trace exports. The agent can discover which keys exist, and can mint new keys itself by capturing a command's stdout straight into the store.

## Threat model

Agent keys defend against **accidental leakage**: a verbose `curl`, an `env` dump, an API that echoes the token, an MCP server that includes it in an error. Every such path runs through one redaction point before anything is recorded or sent.

They do **not** defend against a hijacked agent. `exec` runs as the same OS user as the daemon, so a prompt-injected agent can transform a value before printing it (`rev`, `base64` of a substring), send it to a remote host, or read the key file and decrypt the store directly. Closing that gap needs a credential-injecting egress proxy (placeholder in the environment, real value substituted only for allowlisted hosts) and OS-level isolation of spawned commands; both are tracked separately.

## Storage

A second encrypted store, separate from the system secret store (`secrets.toml.enc`, which holds provider keys and chat tokens). The separation is structural: the agent-facing code paths only ever open the agent store, so no tool call can reach a provider key.

- `~/.residuum/agent-keys.toml.enc` — AES-256-GCM-SIV, `nonce || ciphertext`, same scheme as `secrets.toml.enc`.
- `~/.residuum/agent-keys.key` — its own random 32-byte key, mode 0600.

Plaintext format:

```toml
[keys.github_token]
value = "ghp_..."
description = "Fine-grained PAT, repo read/write on Grizzly-Endeavors"
created_by = "user"   # "user" | "agent"
```

The encryption primitives are shared with `SecretStore` (`src/config/secrets.rs`); the file format and entry type are separate.

### Rules

- **Name:** `[a-z][a-z0-9_]*`, at most 64 characters.
- **Environment variable:** the name uppercased (`github_token` → `$GITHUB_TOKEN`). Names that map onto process-critical variables (`PATH`, `HOME`, `USER`, `SHELL`, `PWD`, `TMPDIR`, `LD_*`, `DYLD_*`) are rejected.
- **Value:** at least 8 characters, no NUL. Shorter values cannot be redacted by substring without mangling unrelated output.
- **Ownership:** every entry records `created_by`. The agent may create keys, and may overwrite or delete only keys it created. The user (CLI, web UI) may do anything.
- **No expiry.** Keys live until deleted.

## Runtime handle

`AgentKeys` (`src/agent_keys/`) is the one runtime handle, shared as `Arc<AgentKeys>` by the main agent's tools, every sub-agent's tools, the MCP spawner, the trace exporter, and the web API. It caches the decrypted store and reloads when the encrypted file's modification time changes, so a `residuum agent-keys set` from the CLI takes effect on the next tool call without a restart. Writes made through the handle update the cache directly and are serialized by an in-process lock.

The handle also caches a `Redactor` built from the current values (see below), so redaction never decrypts on the hot path.

## Discovery: `agent_keys_list`

A built-in tool with no parameters. Returns one line per key: name, environment variable, `created_by`, and description. Never returns values. Registered for the main agent and every sub-agent.

The `HARNESS` prompt block carries a short "Agent keys" bullet, and the residuum-system skill carries the full reference, so the agent knows the tool exists without the key list occupying every prompt.

## Use: `exec` with `keys`

`exec` gains an optional `keys: [string]` parameter. Each named key is set as its environment variable in the spawned child only. The model writes `$GITHUB_TOKEN` in the command and never sees the value.

- An unknown name fails the call before anything is spawned, listing the available names.
- Keys are named per call rather than injected wholesale: the transcript records which key each call used, a stray `env` exposes only what that call asked for, and the future egress proxy needs to know which placeholders to substitute.
- The command text itself is redacted before it is written to the debug log.

## Minting: `exec` with `store_output_as`

`exec` gains an optional `store_output_as: { name, description }` parameter. When set:

- On exit code 0 with non-empty stdout, stdout (trailing newline trimmed) is stored as an agent-created key. The tool returns a confirmation naming the key, its byte length, and its environment variable. **Stdout is never returned.**
- stderr is still returned (redacted, and redacted against the new value too).
- On a non-zero exit or empty stdout, nothing is stored and stdout is discarded — a partial token is still a token.
- Storing over a user-created key is refused, and stdout is discarded.
- `keys` and `store_output_as` combine, so one key can be used to mint another.

The agent deletes keys it created with `agent_key_delete { name }`, which refuses user-created keys.

## Redaction

`Redactor` holds, for every key, the raw value plus its standard base64, URL-safe base64 (with and without padding), and percent-encoded forms, longest first. Each match is replaced with `[agent-key:<name>]`.

It is applied at:

1. **`execute_tool`** (`src/agent/turn.rs`) — every tool result, built-in and MCP, before the `ToolActivityEvent::Result` publish and before the message enters `recent_messages`. Episodes, session transcripts, observations, and the search index are all derived from that history, so they inherit the redaction.
2. **Trace export** (`src/tracing_service/sanitize.rs`) — every span field value and event message, before OTEL export and bug-report submission. This runs whether or not `sanitize_content` is on, since the content sanitizer is name-based and would miss a value in an unlisted field.
3. **The `exec` debug log** — the command text.

Base64 of a value embedded mid-stream in a larger encoded blob (e.g. HTTP basic auth `user:token`) does not match; that is within the documented limit of substring redaction.

## MCP

`mcp.json` stdio `env` values and HTTP `headers` values accept `${agent-key:<name>}` anywhere in the value (e.g. `Bearer ${agent-key:github_token}`), resolved from the agent store when the server connects. An unknown name fails that server's connection visibly. MCP servers resolve from the agent store only, never the system store — `mcp.json` is agent-editable when `agent.modify_mcp` is on, and a `secret:` reference there would let the agent route a provider key into a server it controls.

## Scheduled work

Pulses and scheduled actions run as agent turns and call the same `exec`, so they use keys the same way. When script pulses (#108) land, a pulse's script declares `keys: [...]` in `HEARTBEAT.yml` and runs with no model call at all.

## Management surfaces

- **CLI:** `residuum agent-keys set <name> [value] [--description <text>]` (masked prompt when the value is omitted), `residuum agent-keys list`, `residuum agent-keys delete <name>`.
- **HTTP:** `GET /api/agent-keys` (metadata only), `POST /api/agent-keys` (`{ name, value, description }`), `DELETE /api/agent-keys/{name}`. Same loopback-only posture as `/api/secrets`.
- **Web UI:** an "Agent keys" settings section listing keys with add and delete.

## Related fixes

- Sub-agent registries never received the tool `PATH` handle, so sub-agent `exec` ran with a different environment from main's. `SubagentToolDeps` now carries both the tool `PATH` and the agent-key handle.
- `agent-keys.toml.enc`, `agent-keys.key`, `secrets.toml.enc`, and `secrets.key` are added to the file tools' write-blocked path policy, so `write_file`/`edit_file` cannot corrupt or replace them.
