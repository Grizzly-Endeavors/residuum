# Agent Keys

Agent keys are credentials (API keys, tokens) the agent can hand to the commands it runs and to MCP servers, without the value ever entering its context, the transcript, memory, or trace exports. The agent works with key names; the value is added only in the spawned process's environment, and scrubbed from everything that comes back.

## Storage

Agent keys live in their own encrypted store, separate from the system secret store that holds provider keys and chat tokens:

| File | Holds |
|------|-------|
| `~/.residuum/agent-keys.toml.enc` | The keys: value, description, and who created each one. AES-256-GCM-SIV. |
| `~/.residuum/agent-keys.key` | The store's own 32-byte machine key, mode 0600. |
| `~/.residuum/agent-keys.lock` | Lock file that serializes writes from the CLI, the web UI, and the running agent. |

No agent tool or MCP reference opens the system secret store (`secrets.toml.enc`), so the agent-key machinery never hands out a provider key. All of these files are write-blocked for `write_file` and `edit_file`.

Every key has:

- **A name**: lowercase letters, digits, and underscores, starting with a letter, at most 64 characters. Names that would map onto process-critical environment variables (`PATH`, `HOME`, `USER`, `SHELL`, `PWD`, `TMPDIR`, `IFS`, `ENV`, `BASH_ENV`, `LANG`, and anything starting `LD_` or `DYLD_`) are rejected.
- **An environment variable**: the name uppercased. `github_token` is exposed as `$GITHUB_TOKEN`.
- **A value** of at least 8 characters. Shorter values can't be redacted by substring match without also mangling unrelated output.
- **A description**, shown to the agent. Say what the key is for and what it can reach.
- **A creator**: `user` or `agent`, shown in listings and used to decide when to notify. Either the agent or the user may replace or delete any key, whoever created it. Overwriting or deleting a key through the agent's tools (`exec`'s `store_output_as`, `agent_key_delete`) checkpoints the config repository first, so it's always undoable from checkpoint history; when the key being overwritten or deleted was created by the user, the agent also publishes a notice naming the key so it doesn't happen invisibly.

Keys have no expiry; they live until deleted.

## Managing keys

**CLI:**

```bash
residuum agent-keys set github_token -d "Fine-grained PAT, repo read/write"   # masked prompt for the value
echo "$TOKEN" | residuum agent-keys set github_token --stdin
residuum agent-keys list
residuum agent-keys delete github_token
```

**Web UI:** Settings → Agent keys lists every key with its environment variable and description, marks the ones the agent saved itself, and adds or removes keys. Values are write-only there too.

**HTTP:** `GET /api/agent-keys` (metadata only), `POST /api/agent-keys` with `{ "name", "value", "description" }`, `DELETE /api/agent-keys/{name}`. Like the rest of the config API, it is unauthenticated and meant to stay on loopback.

Changes from any surface take effect on the agent's next tool call; no restart is needed.

## How the agent uses them

**Discovery.** `agent_keys_list` returns each key's name, environment variable, creator, and description, never values.

**Use.** `exec` takes a `keys` parameter naming the keys a command needs. Each named key is set as its environment variable in that child process only, and the agent writes the variable in the command:

```json
{ "command": "curl -sH \"Authorization: Bearer $GITHUB_TOKEN\" https://api.github.com/user", "keys": ["github_token"] }
```

A key not named in `keys` is not in the child's environment. An unknown name fails the call before anything runs. Pulses, scheduled actions, and sub-agents all run as agent turns using the same `exec`, so they use keys the same way.

**Minting.** `exec` takes a `store_output_as: { name, description }` parameter. When the command exits 0 with non-empty stdout, stdout (trailing newline trimmed) is stored as an agent-created key and the tool reports only the key's name, length, and environment variable; stdout is never returned. On a non-zero exit or empty stdout nothing is stored and stdout is discarded. stderr is still returned, redacted. Naming an existing key — the agent's own or one the user created — replaces it; replacing a user-created key publishes a notice naming it.

**Cleanup.** `agent_key_delete` removes a key, whoever created it; deleting one the user created publishes a notice naming it.

## MCP servers

`mcp.json` stdio `env` values and HTTP `headers` values accept `${agent-key:<name>}` anywhere in the value, resolved from the agent key store when the server connects:

```json
{
  "mcpServers": {
    "github": {
      "command": "github-mcp",
      "env": { "GITHUB_TOKEN": "${agent-key:github_token}" }
    },
    "remote": {
      "type": "http",
      "url": "https://mcp.example.com",
      "headers": { "Authorization": "Bearer ${agent-key:remote_api}" }
    }
  }
}
```

An unknown key fails that server's connection with an error naming the key. MCP references resolve only against the agent key store, never the system secret store: the agent can edit `mcp.json` when `agent.modify_mcp` is on, and a system-secret reference there would let it route a provider key into a server of its choosing.

## Redaction

Every key value, and its standard base64, URL-safe base64 (padded and unpadded), and percent-encoded forms, is replaced with `[agent-key:<name>]`:

- in every tool result, built-in and MCP, before it reaches the web UI activity feed, the conversation history, transcripts, episodes, the search index, or the next model call;
- in every span field and event message of a trace export (OTEL dump, streaming, bug reports), whether or not `sanitize_content` is on;
- in the `exec` command text written to the debug log.

A value embedded mid-stream inside a larger encoded blob (HTTP basic auth's base64 of `user:token`, for example) is not matched.

## What this does and doesn't protect against

Agent keys stop **accidental** leakage: a verbose `curl`, an `env` dump, an API or MCP server that echoes the token back, an error message that includes it.

They don't stop a hijacked agent. `exec` runs as the same OS user as Residuum, so an agent following injected instructions can transform a value before printing it, send it to a remote host, or read the key file and decrypt the store itself. Treat anything you store here as reachable by the agent, and scope tokens to what the agent should be able to do.
