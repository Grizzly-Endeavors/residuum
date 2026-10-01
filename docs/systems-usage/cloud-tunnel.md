# Residuum Cloud Tunnel and Remote Control Safety

Residuum Cloud (`[cloud]` in `hub/config.toml`, Settings → All agents → Residuum Cloud) opens a persistent WebSocket tunnel from this gateway to a relay, so the web UI and the workbench reach it from anywhere without port forwarding. The tunnel client (`tunnel::start_tunnel`) forwards each proxied HTTP request and WebSocket channel to the appropriate local listener with a real loopback call — from the gateway's own perspective, a tunnel-forwarded request looks like any other request arriving on its port. The hub also carries each agent's A2A traffic through it, and tells the relay which agents exist (see below).

## Capabilities

The tunnel declares what it can handle in the `x-residuum-capabilities` header of its WebSocket upgrade: `workbench-surface`, `workbench-sockets`, `http-streaming` and `agents` always, plus `a2a` while the hub's A2A listener is enabled (`[a2a] enabled`). Each agent's A2A visibility isn't a capability; it travels with the agent list.

- `workbench-surface`: an `http_request` tagged `"surface": "workbench"` is sent to the workbench artifacts listener.
- `workbench-sockets`: a `ws_open` tagged `"surface": "workbench"` is connected to the workbench artifacts listener (see [Sockets through the tunnel](#sockets-through-the-tunnel)). Both workbench capabilities are advertised even while that listener isn't running, so the relay still forwards and the visitor gets an explanation instead of a generic "update Residuum" answer.

## Agents on the relay

One tunnel connection is one hub, and the hub can host several agents. The hub sends the relay its full agent list as an `agents_update` frame, `{ "agents": [{ "name", "display_name", "a2a_enabled", "a2a_private" }] }`:

- right after the relay's `Connected` frame, on every (re)connect;
- again whenever the list changes: an agent is created or deleted, starts or stops, or changes A2A visibility, or the hub's `[a2a] enabled` flips.

The frame always carries the whole list and the relay replaces its stored copy, so resending is harmless. Changes that arrive together are sent as one update carrying the latest list: the hub gathers a burst for a quarter of a second before publishing, and the tunnel sends the newest list when it next writes. A frame that fails to send is logged at `warn`; the next change or reconnect sends the list again.

- `name` is the agent's name, and `display_name` is the same.
- `a2a_enabled` is true when the hub's A2A listener is enabled and the agent is running. A stopped or failed agent can't answer, so the relay hides it from its directory and answers `404` for it. An agent whose stop has begun is hidden the same way, from the moment the stop is requested rather than when it finishes (which can take up to the stop timeout): the same moment the hub's team router stops accepting messages for it. Starting it again re-lists it.
- `a2a_private` is true when the agent's A2A visibility is private. The relay lists a private agent only for your own installs (or a caller the agent's own auth-check accepts) and forwards every request to the hub, whose auth layer answers `404` to anyone it doesn't recognize.

Until the relay has received the first list on a connection it answers `503` to A2A requests for the instance, and an instance that never sends one has no A2A entries.

## A2A through the tunnel

The relay forwards `/a2a/{instance}/{agent}/<rest>` as an HTTP request on the A2A surface with the frame's `agent` field set. The tunnel sends it to the hub's A2A listener as `/agents/<agent>/<rest>` and streams the response back frame by frame (`http_response_start`, `http_response_chunk`, `http_response_end`), so a long streaming call flows through unbuffered. A request on the A2A surface that names no agent, or a name that isn't a valid agent name, is answered `404` and reaches no agent. So is a request whose path contains a `.` or `..` segment (plain or percent-encoded, in any case) or a backslash, since after normalization it could reach a different agent's routes and skip the relay's per-agent gating. If the A2A listener isn't running, the answer is `503` saying so. See [A2A](a2a.md) for addresses and sibling discovery.

## Sockets through the tunnel

The relay opens a WebSocket channel to a local listener with a `ws_open` frame, `{ "channel_id", "path", "headers", "surface" }`, where `surface` is optional and `path` carries any query string. The tunnel connects to that listener on loopback and answers with `ws_open_result`, `{ "channel_id", "success", "reason" }`. A successful open then carries `ws_message` frames (text only) in both directions until either side sends `ws_close`; when the local socket closes, the tunnel sends `ws_close` to the relay.

`surface` picks the listener:

- **Absent** connects to the main gateway listener, which is what a relay that never sends the field gets.
- **`"workbench"`** connects to the workbench artifacts listener, which forwards `/api` sockets to the hub router, so `/api/hub/ws` and `/api/agents/{name}/ws` are reachable on that surface (see [Workbench](workbench.md)). The open never falls back to the main listener.
- **`"a2a"`** is refused: the A2A listener serves no sockets.

`reason` is set only on a failed open, in plain language the relay can pass on to the browser. A workbench open while the artifacts listener isn't running fails with a reason saying so; an `a2a` open fails with a reason saying the A2A endpoint doesn't serve WebSockets; a listener that answers the upgrade with a refusal (a route guard's `403`, for instance) fails with a reason carrying that status; any other connection failure points to Residuum's logs. A failed open answers promptly, never hangs, and each one is logged at `warn` with the channel and surface. `reason` is absent from a successful open, and a relay that ignores the field still sees `success: false`.

The relay sends a `ws_open` tagged `"workbench"` only to a hub that advertised `workbench-sockets` (see [Capabilities](#capabilities)). A hub without it ignores the unknown field and would connect the socket to its main listener.

## Telling a Tunnel-Forwarded Request Apart From a Local One

Two things need to reliably tell a request that arrived through the tunnel apart from one made directly against a local port, and both reuse the same mechanism: a per-process nonce (`tunnel::tunnel_nonce`, 32 random characters, generated once at startup and never persisted) sent as the `x-residuum-tunnel` header (`tunnel::TUNNEL_NONCE_HEADER`) on every request and every socket open the tunnel forwards, on every surface. Because the nonce is unpredictable and lives only in this process's memory, nothing a client sends — over the tunnel or directly to a local port — can forge a match; the forwarder strips any client-supplied value for that header name before adding its own, for HTTP requests and socket opens alike, so a client can neither forge the mark nor erase it. The artifacts listener passes the header on to the hub router unchanged, so a workbench-surface request is marked the same way.

- **A2A sibling attestation** (`a2a::auth`): a sibling instance's request is only trusted when the nonce matches, proving it came from this instance's own tunnel connection rather than a caller hitting the A2A port directly and forging the sibling header.
- **The remote-control guard** (`gateway::remote_control_guard`), described below.

## Remote-Control Guard

**Observed failure this guards against:** a remote shutdown or cloud-disconnect executed through the tunnel leaves nothing that can bring the gateway back, since both actions cut off the only channel a remote caller has to reach it.

`POST /api/hub/shutdown` and `POST /api/hub/cloud/disconnect` refuse any request carrying a matching tunnel nonce, with a plain-language message telling the caller to do it on the machine running Residuum instead. Every other route stays reachable remotely, including restart and update — those don't cut off the tunnel, so there's a way back even if something goes wrong (see [Self-Update, Rollback, and Startup Health](self-update.md)). The guard is mounted with `route_layer` on exactly those two routes, not the whole router, so nothing else is affected. Stopping agents stays reachable remotely, one at a time (`POST /api/hub/agents/{name}/stop`) or all at once (`POST /api/hub/stop-all`): the hub keeps running and every agent can be started again through the tunnel.

A local request — whether it's the web UI open on the same machine, `residuum stop`'s own HTTP call, or a plain `curl` against the local port — never carries a matching nonce and is unaffected.

`GET /api/hub/cloud/status` includes `viewed_via_tunnel: true` whenever the request that fetched it arrived through the tunnel. The Settings → All agents → Residuum Cloud section uses this to hide the Disconnect/Cancel control and show the same explanation, rather than letting someone click a button that the gateway will refuse anyway. The section's other actions: Connect opens the relay's sign-in page (`<relay origin>/connect?port=<gateway port>`, with the origin taken from `[cloud] relay_url`, `ws` read as `http` and `wss` as `https`, or Residuum Cloud's when it is empty), which ends at this gateway's `/cloud/callback`; Reconnect writes `enabled = true` to `[cloud]`; Disconnect and Cancel call `POST /api/hub/cloud/disconnect`. Each acts at once. The relay URL, the local port and removing the account are staged settings, saved with Save changes. The section reads the status again when the window regains focus, when the hub's config reloads, and every few seconds while the tunnel is connecting or one of these actions is under way.
