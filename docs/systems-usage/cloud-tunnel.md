# Residuum Cloud Tunnel and Remote Control Safety

Residuum Cloud (`[cloud]` in `config.toml`, Settings → Residuum Cloud) opens a persistent WebSocket tunnel from this gateway to a relay, so the web UI, the workbench, and A2A requests reach it from anywhere without port forwarding. The tunnel client (`tunnel::start_tunnel`) forwards each proxied HTTP request to the appropriate local listener with a real loopback HTTP call — from the gateway's own perspective, a tunnel-forwarded request looks like any other request arriving on its port.

## Telling a Tunnel-Forwarded Request Apart From a Local One

Two things need to reliably tell a request that arrived through the tunnel apart from one made directly against a local port, and both reuse the same mechanism: a per-process nonce (`tunnel::tunnel_nonce`, 32 random characters, generated once at startup and never persisted) sent as the `x-residuum-tunnel` header (`tunnel::TUNNEL_NONCE_HEADER`) on every request the tunnel forwards. Because the nonce is unpredictable and lives only in this process's memory, nothing a client sends — over the tunnel or directly to a local port — can forge a match; the forwarder strips any client-supplied value for that header name before adding its own.

- **A2A sibling attestation** (`a2a::auth`): a sibling instance's request is only trusted when the nonce matches, proving it came from this instance's own tunnel connection rather than a caller hitting the A2A port directly and forging the sibling header.
- **The remote-control guard** (`gateway::remote_control_guard`), described below.

## Remote-Control Guard

**Observed failure this guards against:** a remote shutdown or cloud-disconnect executed through the tunnel leaves nothing that can bring the gateway back, since both actions cut off the only channel a remote caller has to reach it.

`POST /api/shutdown` and `POST /api/cloud/disconnect` refuse any request carrying a matching tunnel nonce, with a plain-language message telling the caller to do it on the machine running Residuum instead. Every other route stays reachable remotely, including restart and update — those don't cut off the tunnel, so there's a way back even if something goes wrong (see [Self-Update, Rollback, and Startup Health](self-update.md)). The guard is mounted with `route_layer` on exactly those two routes, not the whole router, so nothing else is affected.

A local request — whether it's the web UI open on the same machine, `residuum stop`'s own HTTP call, or a plain `curl` against the local port — never carries a matching nonce and is unaffected.

`GET /api/cloud/status` includes `viewed_via_tunnel: true` whenever the request that fetched it arrived through the tunnel. The Settings → Residuum Cloud page uses this to hide the Disconnect/Cancel control and show the same explanation, rather than letting someone click a button that the gateway will refuse anyway.
