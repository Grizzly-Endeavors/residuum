# Agent2Agent (A2A)

A2A ([Agent2Agent protocol](https://a2a-protocol.org/), spec v1.0.1) is how other agents — including a user's own other Residuum instances — reach this agent. Residuum runs a dedicated listener implementing the protocol's server side: an Agent Card, JSON-RPC and REST bindings, and an auth layer that gates every request behind a caller key or a sibling attestation.

The listener currently answers every JSON-RPC/REST call with `UNSUPPORTED_OPERATION` — there is no session executor wired up yet, so it accepts connections and proves identity but cannot yet run a task. The Agent Card, the caller-key store, and the auth layer described below are otherwise fully live.

## Configuration

```toml
[a2a]
enabled = true
port = 7702
public_url = ""          # this instance's own tunnel/reverse proxy origin
visibility = "public"    # "public" or "private"
```

- **`enabled`** (default `true`): whether the listener runs at all. Every request is still authenticated, so leaving it on costs nothing until a caller key or sibling instance exists to use it.
- **`port`** (default `7702`): the dedicated listener's port, bound on `[gateway] bind`. Kept separate from the gateway port for the same reason Teams is: a public tunnel pointed at it exposes only the A2A endpoints, never the unauthenticated config API.
- **`public_url`**: the base URL other agents should use to reach this instance, when it runs its own tunnel or reverse proxy. Left empty, the Agent Card falls back to `http://{bind}:{port}` — a placeholder good for same-host and same-network callers, not for callers over the public internet.
- **`visibility`**: `"public"` (default) or `"private"`. See [Visibility](#visibility).

Changing any `[a2a]` value, or the gateway `bind` it shares, restarts the A2A listener on reload — the config API's reload path, `residuum a2a` commands, and manual edits to `config.toml` (picked up by the running gateway) all take effect the same way.

## Caller keys

Other agents authenticate with a bearer token minted for them:

```bash
residuum a2a keys create laptop -d "my other instance, before siblings exist"
# A2A caller key 'laptop' created.
#
#   rsdm_a2a_Ax7f...   (32 base62 characters)
#
# Store this now — it is shown only once and is not recoverable. ...
residuum a2a keys list
residuum a2a keys revoke laptop
```

The caller gives the token as `Authorization: Bearer <token>`.

**Storage.** `~/.residuum/a2a-keys.toml`, mode 0600 on Unix, holds each key's name, description, a `sha256:<hex>` hash of the token, and its creation time — never the token itself. The store is unencrypted because there is nothing in it worth encrypting at rest: the hash's only job is to reject a stolen credential's replay, which it already does. `~/.residuum/a2a-keys.lock` serializes writes from the CLI, the web UI, and the running listener so none of them lose a concurrent change. Both files are write-blocked for `write_file` and `edit_file`, like the other credential stores.

**Web UI:** `GET /api/a2a/keys` (metadata only), `POST /api/a2a/keys` with `{ "name", "description" }` (returns the token once, in the response body — never again), `DELETE /api/a2a/keys/{name}`. Like the rest of the config API, this is unauthenticated and meant to stay on loopback.

A key name is lowercase letters, digits, and underscores, starting with a letter, at most 64 characters.

## The Agent Card

`{workspace}/config/agent-card.json` is what this agent advertises to other agents that reach it over A2A:

```json
{
  "name": "Residuum agent",
  "description": "A personal AI agent, reachable over the Agent2Agent (A2A) protocol.",
  "skills": []
}
```

- **`name`**, **`description`**: required, non-empty.
- **`skills`**: an array of `{ "id", "name", "description", "tags": [], "examples"?: [] }`. Every `id` must be unique. An empty list is valid.
- **`default_input_modes`**, **`default_output_modes`**: optional; default to `["text/plain"]` when omitted.

Everything else in the wire Agent Card — the JSON-RPC and REST interface URLs, capabilities, version, and security scheme — is filled in by the server from the file plus runtime facts (the configured base URL and visibility), never edited in the file directly.

The file is bootstrapped on first run with a generic placeholder and an empty skill list (`write_if_missing`, so a user edit is never overwritten). Residuum watches it alongside `mcp.json` and `channels.toml`; a change is picked up within a few seconds without restarting the listener. If the file becomes invalid JSON or fails validation, the listener keeps serving the last good card, logs a warning, and posts a system notice — it never starts serving a broken or stale-to-empty card.

## Endpoints

The listener serves, on `[gateway] bind`:`[a2a] port`:

| Path | What |
|------|------|
| `GET /.well-known/agent-card.json` | The Agent Card |
| `GET /_a2a/auth-check` | `204` if the request's credentials are currently valid, `404` otherwise — used by directory probes and health checks, nothing else |
| `POST /` | JSON-RPC binding (`message/send`, `tasks/get`, …) |
| `POST /rest/...` | HTTP+JSON (REST) binding, mirroring the same operations |

The Agent Card's `supportedInterfaces` names the JSON-RPC interface at the base URL itself and the REST interface at `{base}/rest`.

## Auth layer

Every request passes through an axum middleware before it reaches anything else:

1. Any client-supplied `x-residuum-a2a-caller`, `x-residuum-tunnel`, or `x-residuum-sibling` header is stripped before it is ever inspected.
2. If `x-residuum-tunnel` matches this process's own tunnel nonce **and** `x-residuum-sibling` names a slug, the caller is `sibling:<slug>` — an attestation only this instance's own tunnel forwarder can produce, never something a client can present directly. No tunnel is wired up yet in this stream, so no sibling request currently authenticates this way.
3. Otherwise, an `Authorization: Bearer <token>` that matches a live caller key authenticates as `key:<name>`.
4. A caller resolved either way gets `x-residuum-a2a-caller: <key:name|sibling:slug>` injected for the handler to read.

An unauthenticated request is refused according to [visibility](#visibility). `GET /_a2a/auth-check` is the one path that never refuses outright: it reports `204` (authenticated) or `404` (not) in both visibility modes, so a directory probe or health check can ask "is this credential currently valid" without needing a real operation to fail against.

## Visibility

- **`public`** (default): the Agent Card is open to everyone; every other route still requires a valid caller key or sibling attestation, and an unauthenticated request there gets `401` with `WWW-Authenticate: Bearer`.
- **`private`**: every route, including the Agent Card, answers a plain `404` with no `WWW-Authenticate` header to a caller without a valid key or attestation — a private agent is indistinguishable from one that doesn't exist. Present a valid key and everything (card included) answers normally.

## Code

`src/a2a/`: `keys.rs` and `keys_runtime.rs` (the caller-key store and its shared runtime handle), `card.rs` (the workspace agent-card file, validation, and the live `CardState`), `auth.rs` (the middleware, `Caller`, and the `TunnelNonceSource` seam a later stream fills from the real tunnel connection), `listener.rs` (the axum listener and the `StubHandler` placeholder). `src/commands/a2a.rs` is the CLI; `src/gateway/web/a2a.rs` is the web API.
