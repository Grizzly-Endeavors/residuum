# Agent Keys

Agent keys are credentials (API keys, tokens) you can hand to commands without ever seeing their values. You work with key names; the value exists only in the environment of the command that asked for it, and every value is replaced with `[agent-key:<name>]` in anything you read back.

## Find what exists

`agent_keys_list` shows each key's name, the environment variable it is exposed as (the name uppercased: `github_token` → `$GITHUB_TOKEN`), who created it, and its description. Check it before telling the user you have no credential for something.

## Use a key

Name it in `exec`'s `keys` parameter and reference the variable in the command:

```json
{ "command": "curl -sH \"Authorization: Bearer $GITHUB_TOKEN\" https://api.github.com/user", "keys": ["github_token"] }
```

- Only keys named in `keys` are in the command's environment.
- An unknown name fails the call before anything runs, listing the available keys.
- Reference the variable (`$GITHUB_TOKEN`); never try to print, echo, or decode a value. You'd only see the marker, and it wastes a call.
- Pulses, scheduled actions, and sub-agents use keys the same way.

## Save a token a command creates

When a command prints a new credential (an OAuth exchange, a short-lived token), set `store_output_as` so stdout goes straight into the store instead of into your context:

```json
{
  "command": "curl -s -X POST https://auth.example.com/token -d \"client_secret=$CLIENT_SECRET\" | jq -r .access_token",
  "keys": ["client_secret"],
  "store_output_as": { "name": "example_access_token", "description": "Access token for example.com API, from client_secret" }
}
```

- The command must exit 0 and print only the credential on stdout. Send progress and diagnostics to stderr, which you still see.
- On failure nothing is stored and stdout is discarded.
- Naming an existing key replaces it, including one the user created — that's checkpointed and reported to them, so prefer a fresh name unless replacing that specific key is really what you mean.
- Write a description that says what the key grants and where it came from; the user sees it too.
- Delete keys you no longer need with `agent_key_delete` — this too works on a key the user created, checkpointed and reported to them.

Never ask the user to paste a credential into chat. Anything typed into the conversation is already in the transcript and has already reached the model provider. Point them to `residuum agent-keys set <name>` or Settings → Agent keys in the web UI instead. If they already pasted one, tell them to revoke or rotate it and store the new value that way.

## MCP servers

In `config/mcp.json`, reference a key from a stdio server's `env` or an HTTP server's `headers` with `${agent-key:<name>}`, anywhere in the value:

```json
"env": { "GITHUB_TOKEN": "${agent-key:github_token}" }
"headers": { "Authorization": "Bearer ${agent-key:remote_api}" }
```

It is resolved when the server connects. An unknown key fails that server's connection.

## Limits

Redaction matches each value and its base64 and percent-encoded forms. A value transformed any other way, or embedded inside a larger encoded blob, is not caught, so don't transform keys in ways that print them.

See the authoritative reference: `docs/systems-usage/agent-keys.md`.
