# Residuum Cloud Tunnel and Remote Control Safety

Residuum Cloud (`[cloud]` in `hub/config.toml`, Settings → All agents → Residuum Cloud) keeps one persistent WebSocket open from this gateway to a relay, so the web UI and the workbench reach it from anywhere without port forwarding. The tunnel client (`tunnel::start_tunnel`) registers at the relay's `/tunnel/v2/register` endpoint with `Authorization: Bearer {token}` and the capability `tls-passthrough` in `x-residuum-capabilities`. The relay hands this instance one raw byte stream per browser connection, and Residuum terminates TLS itself, so the relay sees only host names, addresses, timing and byte counts. An install with `[cloud]` configured always uses this tunnel. To stay off the cloud, leave `[cloud]` out, set `enabled = false` in it, or disconnect in Settings.

How the streams are served, the certificates and the identity checks are in [Secure Tunnel and Certificates](secure-tunnel.md). Which browsers may use the tunnel is [Remote access](remote-access.md). The code is in `src/tunnel/`.

`[cloud] relay_url` names the relay. The client always registers at `/tunnel/v2/register` on it: a URL that ends in `/tunnel/register` is rewritten to that path, and a URL that ends in neither is logged at `error` and never connected. A relay that answers `404` or `426` on the endpoint doesn't offer the secure tunnel; the status says so (`error`) and the client keeps retrying with backoff. `local_port` and `remote_access` in `[cloud]` are still accepted so an existing config loads, and have no effect (a `remote_access = false` or a `local_port` is logged at `warn`).

## Agents on the relay

One tunnel connection is one hub, and the hub can host several agents. The hub sends the relay its full agent list as an `agents_update` frame, `{ "agents": [{ "name", "display_name", "a2a_enabled", "a2a_private" }] }`:

- right after the relay's `connected` frame has been accepted, on every (re)connect;
- again whenever the list changes: an agent is created or deleted, starts or stops, or changes A2A visibility, or the hub's `[a2a] enabled` flips.

The frame always carries the whole list and the relay replaces its stored copy, so resending is harmless. Changes that arrive together are sent as one update carrying the latest list: the hub gathers a burst for a quarter of a second before publishing, and the tunnel sends the newest list when it next writes.

- `name` is the agent's name, and `display_name` is the label people see.
- `a2a_enabled` is true when the hub's A2A listener is enabled and the agent is running. A stopped or failed agent can't answer, so the relay hides it from its directory. An agent whose stop has begun is hidden the same way, from the moment the stop is requested rather than when it finishes (which can take up to the stop timeout): the same moment the hub's team router stops accepting messages for it. Starting it again re-lists it.
- `a2a_private` is true when the agent's A2A visibility is private. The relay lists a private agent only for your own installs.

The relay carries no A2A or Teams traffic. Callers reach `https://{slug}.{user}.{base}/a2a/{agent}/...` and `/teams/{agent}` on the instance host, which Residuum serves itself and forwards to its A2A listener and to the agent's Teams listener (see [A2A](a2a.md) and [Microsoft Teams](teams.md)).

## Telling a Remote Request Apart From a Local One

A request is remote when the secure tunnel's engine marked it with the `RemoteTransport` request extension (`pairing::remote`) before it entered a router. The extension carries the peer address the relay reported and the origin the browser used. A client cannot send an extension, so nothing a client sends — over the tunnel or directly to a local port, in any header — can make a local request remote or hide a remote one. The engine also removes any client-supplied `x-residuum-a2a-caller`, `x-real-ip` and `x-forwarded-for` from inbound requests.

Everything that treats remote requests differently asks that one question:

- **The device gate** (`pairing::device_gate`), which refuses a remote request to the main gateway or the workbench listener unless it carries a paired device's credential, and applies the cross-site rule to it. The A2A and Teams listeners are not gated. See [Remote access](remote-access.md).
- **The remote-control guard** (`gateway::remote_control_guard`), described below.
- **`viewed_via_tunnel`** in the cloud status, below.

## Remote-Control Guard

**Observed failure this guards against:** a remote shutdown or cloud-disconnect executed through the tunnel leaves nothing that can bring the gateway back, since both actions cut off the only channel a remote caller has to reach it.

`POST /api/hub/shutdown` and `POST /api/hub/cloud/disconnect` refuse any remotely delivered request (one the secure tunnel's engine marked with its `RemoteTransport` request extension, which no client can send), with a plain-language message telling the caller to do it on the machine running Residuum instead. Every other route stays reachable remotely, including restart and update — those don't cut off the tunnel, so there's a way back even if something goes wrong (see [Self-Update, Rollback, and Startup Health](self-update.md)). The guard is mounted with `route_layer` on exactly those two routes, not the whole router, so nothing else is affected. Stopping agents stays reachable remotely, one at a time (`POST /api/hub/agents/{name}/stop`) or all at once (`POST /api/hub/stop-all`): the hub keeps running and every agent can be started again through the tunnel.

A local request — whether it's the web UI open on the same machine, `residuum stop`'s own HTTP call, or a plain `curl` against the local port — never carries the extension, and no header it sends can add one, so it is unaffected.

`GET /api/hub/cloud/status` includes `viewed_via_tunnel: true` whenever the request that fetched it arrived through the tunnel. The Settings → All agents → Residuum Cloud section uses this to hide the Disconnect/Cancel control and show the same explanation, rather than letting someone click a button that the gateway will refuse anyway. The section's other actions: Connect opens the relay's sign-in page (`<relay origin>/connect?port=<gateway port>`, with the origin taken from `[cloud] relay_url`, `ws` read as `http` and `wss` as `https`, or Residuum Cloud's when it is empty), which ends at this gateway's `/cloud/callback`; Reconnect writes `enabled = true` to `[cloud]`; Disconnect and Cancel call `POST /api/hub/cloud/disconnect`. Each acts at once. The relay URL and removing the account are staged settings, saved with Save changes. The section reads the status again when the window regains focus, when the hub's config reloads, and every few seconds while the tunnel is connecting or one of these actions is under way.

## Running Against a Local Dev Relay

The tunnel connects to the production relay (`wss://agent-residuum.com/tunnel/v2/register`) unless `[cloud] relay_url` says otherwise, so running against a relay on the same machine takes four things.

- **The relay URL**, in `hub/config.toml`: `[cloud]` with `relay_url = "ws://127.0.0.1:<relay port>/tunnel/v2/register"`.
- **The token**, from the relay's `/connect?port=<gateway port>` flow. Open `http://localhost:<relay port>/connect?port=<gateway port>` in a browser (the port must be 1024 or higher). The relay signs you in if needed, then redirects to `http://localhost:<gateway port>/cloud/callback?token=...` on this gateway, which stores the token as the `cloud_token` secret, sets `enabled = true` and `token = "secret:cloud_token"` in `[cloud]`, and reloads the hub. The callback keeps a `relay_url` that is already set. Settings → All agents → Residuum Cloud's Connect opens that same `/connect` page on the relay that `relay_url` names, so once `relay_url` is saved it works for a local relay too; with `relay_url` empty it opens the production relay's. `RESIDUUM_CLOUD_TOKEN` is the other way to supply a token, and it covers only the token: it overrides `token` in the config but not `relay_url`, and the `[cloud]` section still has to exist, so without a `relay_url` there the tunnel dials production with it.
- **The relay's origin**: run the relay with `BASE_URL=http://localhost:<relay port>`. The relay builds the web UI and workbench origins it announces to the tunnel from `BASE_URL`, so this makes them `http://<username>.localhost:<relay port>` and `http://<username>.workbench.localhost:<relay port>` instead of `https://<username>.agent-residuum.com` and `https://<username>.workbench.agent-residuum.com`. The relay's default is the production URL, which makes a local relay announce origins that don't reach it.
- **Resolving `*.localhost`**: browsers and `curl` resolve `*.localhost` to loopback themselves. glibc doesn't on some Linux systems, so a program that uses the system resolver (Node's `fetch`, `getent hosts`) can fail to find `<username>.localhost`; use the browser or `curl`, or add the hostname to `/etc/hosts`.
