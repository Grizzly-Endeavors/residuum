# Residuum Cloud Tunnel and Remote Control Safety

Residuum Cloud (`[cloud]` in `hub/config.toml`, Settings → All agents → Residuum Cloud) keeps one persistent WebSocket open from this gateway to a relay, so the web UI and the workbench reach it from anywhere without port forwarding. The tunnel client (`tunnel::start_tunnel`) registers at the relay's `/tunnel/v2/register` endpoint with `Authorization: Bearer {token}` and the capability `tls-passthrough` in `x-residuum-capabilities`. The relay hands this instance one raw byte stream per browser connection, and Residuum terminates TLS itself, so the relay sees only host names, addresses, timing and byte counts. An install with `[cloud]` configured always uses this tunnel. To stay off the cloud, leave `[cloud]` out, set `enabled = false` in it, or disconnect in Settings.

How the streams are served, the certificates and the identity checks are in [Secure Tunnel and Certificates](secure-tunnel.md). Which browsers may use the tunnel is [Remote access](remote-access.md). The code is in `src/tunnel/`.

`[cloud] relay_url` names the relay. The client always registers at `/tunnel/v2/register` on it: a URL that ends in `/tunnel/register` is rewritten to that path, and a URL that ends in neither is logged at `error` and never connected. A relay that answers `404` or `426` on the endpoint doesn't offer the secure tunnel; the status says so (`error`) and the client keeps retrying with backoff. A relay that answers `401` or `403` refused the token: the tunnel's status becomes `refused` (logged at `error` once, not on each retry), `GET /api/hub/cloud/status` reports `"status": "refused"`, and the client keeps trying once a minute. The status stays `refused` across those retries until one gets further; a tunnel started afresh, such as after a new sign-in reloads the hub, reads `connecting` on its first attempt. `local_port` and `remote_access` in `[cloud]` are still accepted so an existing config loads, and have no effect (a `remote_access = false` or a `local_port` is logged at `warn`).

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

`GET /api/hub/cloud/status` includes `viewed_via_tunnel: true` whenever the request that fetched it arrived through the tunnel. The Settings → All agents → Residuum Cloud section uses this to hide the Disconnect/Cancel control and show the same explanation, rather than letting someone click a button that the gateway will refuse anyway. The section's other actions: Connect starts a sign-in (see [Starting a sign-in](#starting-a-sign-in)) and opens the relay's sign-in page the hub answers with in a new tab, which ends at this gateway's `/cloud/callback` (see [The sign-in callback](#the-sign-in-callback)); while the relay refuses the saved token the section says so and offers Sign in again, which does the same; Reconnect writes `enabled = true` to `[cloud]`; Disconnect and Cancel call `POST /api/hub/cloud/disconnect`. Each acts at once. The disconnect sets `enabled = false` in `[cloud]` through the same path as the sign-in callback, leaving the token and every other line in place, checkpointed as `cloud disconnect`. The relay URL and removing the account are staged settings, saved with Save changes. The section reads the status again when the window regains focus, when the hub's config reloads, and every few seconds while the tunnel is connecting or one of these actions is under way.

## Starting a sign-in

`/cloud/callback` saves the token it is handed and switches the tunnel on, and it is a plain `GET`, so any web page could send the browser there with a token of its own and sign the install in to someone else's account. A `state` ties each callback to a sign-in this app started (`gateway::web::cloud_sign_in`):

- Connect calls `POST /api/hub/cloud/connect` with `{ relay_url, port }`: the relay URL in the form (empty for Residuum Cloud's) and the saved gateway port (empty for 7700). It is a `POST`, so the cross-site guard refuses it from another site. The hub generates a `state` (256 random bits as 43 characters of URL-safe base64, inside the relay's `^[A-Za-z0-9_-]{16,128}$`), remembers it, and answers `{ url }`: `<relay origin>/connect?port=<port>&state=<state>`, the origin taken from the relay URL with `ws` read as `http` and `wss` as `https`. A relay URL that isn't a `ws://` or `wss://` address, or a port that isn't a port number, is answered `400` with an `{ error }` the section shows.
- The relay carries `state` through its sign-in page and redirects to `http://localhost:<port>/cloud/callback?token=<token>&state=<state>`.
- The hub remembers a `state` for 15 minutes and for one callback. It keeps the eight most recent, so someone who clicked Connect a few times can finish in any of the tabs it opened; a ninth pushes out the oldest. They live in memory, so a hub restart forgets them and a sign-in started before it has to be started again.

A relay that doesn't carry `state` back can't sign an install in: every sign-in through it ends in `cloud=not-started-here`.

## The sign-in callback

After sign-in the relay sends the browser tab to `http://localhost:<gateway port>/cloud/callback?token=...&state=...` (`gateway::web::cloud::cloud_callback`). The callback first checks `state`: when it is missing, or isn't a `state` the hub handed out (never handed out, already used, pushed out by newer ones, or older than 15 minutes), it saves nothing, logs why at `warn` (never the token), and hands off with `cloud=not-started-here`. Otherwise it uses the `state` up and builds the change to `hub/config.toml`: `enabled = true` and `token = "secret:cloud_token"` in `[cloud]`, through the same path as the other Settings saves. Every other line and comment in the file stays as it is (a `relay_url` that is already set included), and the patched text must pass validation, with an unset timezone allowed as on first run. If validation fails, the callback answers `save-failed` and stores nothing. Otherwise it takes a checkpoint of `config.toml` and the secret store as they stand, named `cloud sign-in` so History can restore them, and only then stores the `cloud_token` secret and replaces `config.toml` atomically. Then the hub reloads. Whatever happens, it answers `303` into the web app at Settings → All agents → Residuum Cloud, `/home?settings=_all/cloud&cloud=<outcome>`:

| `cloud=` | When | What the section shows |
|---|---|---|
| `connecting` | The token is saved and the hub is reloading | Nothing of its own: the status, which reads `connecting` and then `connected` or `refused` |
| `missing-token` | The query has no usable `token`, including a malformed or repeated one | An error banner saying the sign-in didn't come back, with Try connecting again |
| `save-failed` | The secret or `config.toml` couldn't be written; the cause is logged at `error` | An error banner saying the sign-in couldn't be saved, with Try connecting again |
| `not-started-here` | The `state` is missing or isn't one the hub handed out; nothing was saved | An error banner saying the sign-in link didn't come from this app or expired, with Try connecting again |

The web app reads `cloud=` before its router rewrites the address (`web/src/lib/cloud-handoff.ts`), shows it once, and the address that stays is `/home?settings=_all/cloud`. The callback never answers with a page of its own, so the tab always lands in the app.

## Running Against a Local Dev Relay

The tunnel connects to the production relay (`wss://agent-residuum.com/tunnel/v2/register`) unless `[cloud] relay_url` says otherwise, so running against a relay on the same machine takes four things.

- **The relay URL**, in `hub/config.toml`: `[cloud]` with `relay_url = "ws://127.0.0.1:<relay port>/tunnel/v2/register"`.
- **The token**, from the relay's `/connect` flow, started from Settings → All agents → Residuum Cloud's Connect with the local relay's URL in `relay_url` (under More options; save it first, so the tunnel dials the same relay). Connect opens `http://localhost:<relay port>/connect?port=<gateway port>&state=...` (the gateway port must be 1024 or higher). The relay signs you in if needed, then redirects to `http://localhost:<gateway port>/cloud/callback?token=...&state=...` on this gateway, which saves the token, switches the tunnel on and lands the tab in Settings → Residuum Cloud (see [The sign-in callback](#the-sign-in-callback)). The callback keeps a `relay_url` that is already set. Opening the relay's `/connect` page by hand doesn't sign the install in: the callback refuses a token without a `state` Connect started (see [Starting a sign-in](#starting-a-sign-in)). `RESIDUUM_CLOUD_TOKEN` is the other way to supply a token, and it covers only the token: it overrides `token` in the config but not `relay_url`, and the `[cloud]` section still has to exist, so without a `relay_url` there the tunnel dials production with it.
- **The relay's origin**: run the relay with `BASE_URL=http://localhost:<relay port>`. The relay builds the web UI and workbench origins it announces to the tunnel from `BASE_URL`, so this makes them `http://<username>.localhost:<relay port>` and `http://<username>.workbench.localhost:<relay port>` instead of `https://<username>.agent-residuum.com` and `https://<username>.workbench.agent-residuum.com`. The relay's default is the production URL, which makes a local relay announce origins that don't reach it.
- **Resolving `*.localhost`**: browsers and `curl` resolve `*.localhost` to loopback themselves. glibc doesn't on some Linux systems, so a program that uses the system resolver (Node's `fetch`, `getent hosts`) can fail to find `<username>.localhost`; use the browser or `curl`, or add the hostname to `/etc/hosts`.
