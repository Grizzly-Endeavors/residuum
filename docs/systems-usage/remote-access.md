# Remote Access and Device Pairing

Residuum reached through Residuum Cloud answers only browsers that have been paired with it. A browser that arrives through Residuum Cloud carries a device credential, a cookie only that browser holds, or it is sent to the pairing page. Opening Residuum on the machine it runs on needs no pairing. The A2A and Teams listeners are not part of this and authenticate their own callers (see [A2A](a2a.md) and [Microsoft Teams](teams.md)).

The code is in `src/pairing/`; the pairing routes are in `src/hub/http/pairing.rs`.

## What counts as remote

A request is remote when the transport that terminates TLS inside Residuum marked it by adding a `RemoteTransport` extension (`pairing::remote`) to it before it enters a router. The extension carries the peer address the relay reported and the origin the browser used (`https://{host}`), and a client can't send one. The secure tunnel's engine ([Secure Tunnel and Certificates](secure-tunnel.md)) is that transport. The device gate, the cross-site rule and the remote-control guard all ask the same question, so they apply to every remote request alike. Nothing a client sends in a header makes a request remote or local. Everything else arrived on a local port and is not gated.

**The peer address** the rate limits use is the one the relay reported for the browser's connection. IPv4-mapped IPv6 addresses count as the IPv4 address. A compromised relay can name any address, which only lets it dodge or trigger a rate limit.

## The gate

The device gate (`pairing::device_gate`) sits in front of the main gateway router and the workbench listener. For a remote request it checks, in order:

1. **The cross-site rule**, for requests that change state (anything but `GET`, `HEAD` or `OPTIONS`) and for every WebSocket upgrade. `Sec-Fetch-Site` must be `same-origin` or `none`. A browser that doesn't send it must send an `Origin` equal to the origin of the host the request is for. A request with neither is refused with `403`. Other users' hosts are the same site as this one, so a `SameSite=Lax` cookie alone doesn't tell a request from this page from one from another user's page.
2. **The pre-auth routes**, which answer without a credential.
3. **The credential.** A valid device cookie for that host lets the request through. Without one, a browser navigation is redirected to `/pair` and anything else gets `401` with `{ "error", "code": "device_required" }`. The web app moves to the pairing page on that code.

The pre-auth routes on the UI host are the pairing page (`GET /pair`), the app's static files it loads (`/assets/...`, `/icons/...`, `/favicon.svg`, `/manifest.webmanifest`) and the pairing API: `GET /api/hub/pairing/state` and `POST /api/hub/pairing/requests`, `.../requests/poll`, `.../redeem` and `.../recovery`. On the workbench host they are `GET /_handoff` and `POST /api/hub/pairing/handoff`. Nothing else answers before pairing, including `/sw.js`, `/mcp-catalog.json` and webhooks.

On the workbench host an unpaired navigation to an artifact is redirected to the same artifact in the Residuum app (`{ui origin}/team/workbench/{artifact}`), which opens it through the handoff once the browser is paired. The artifacts listener hands `/api` requests to the main router in-process, and the main router's gate skips a request the workbench listener already gated.

## The credential

A paired browser holds a random 256-bit secret in a cookie named `__Host-residuum_device_{slug}`: `HttpOnly`, `Secure`, `SameSite=Lax`, `Path=/`, `Max-Age` 400 days. The `__Host-` prefix keeps the cookie on the one host that set it. `{slug}` is this instance's slug, so cookies for several instances of one user coexist on the shared UI host; the active instance also receives the others' cookies and ignores them.

The slug and both origins come from the identity this install stored when it enrolled (see [Secure Tunnel and Certificates](secure-tunnel.md#identity-is-local)), and what the relay announces is ignored once that identity exists (lowercase letters, digits and hyphens, up to 24 characters). The hub keeps the slug and the UI and workbench origins in `hub/remote-access.json`, so cookies, pairing links and the cross-site rule keep working while the tunnel reconnects. When no slug has ever been announced (an install that has never connected) the name ends in `default`. A browser paired under one slug has to pair again if the slug later changes.

Only a SHA-256 of the secret is stored, with the device's name, when it was paired and when it was last seen. A device that makes no request for 400 days is no longer paired. Each use extends the cookie: it is set again on a response once a day. The UI host and the workbench host are different origins and each has its own credential; the workbench credentials belong to the same device, so revoking a device ends both.

The cookie is set with one `Set-Cookie` header per response, which the tunnel carries as one header.

## Ways to pair a browser

**The first browser, from the machine Residuum runs on.** Settings, All agents, Residuum Cloud, Paired browsers has Enable remote access, which makes a single-use pairing token that works for 10 minutes. It is shown as a link (`{ui origin}/pair#token=...`) and a QR code. The token travels in the URL fragment, which the browser never sends anywhere, and the pairing page posts it. `residuum remote pair` prints the same link and a terminal QR code, for a server that is used only remotely and reached over SSH. Both need Residuum Cloud to have announced the instance's address, which happens on the first connect. Making a link is refused for a remote request, so a paired browser can't mint first-device links.

**An additional browser.** An unpaired browser on the UI host sees the pairing page, which shows a 6-character code and asks for a device name. It polls with a separate random 128-bit request id that only it knows. Any paired browser, or Settings on the machine Residuum runs on, lists the waiting requests with their codes and names; the person approves the one whose code matches, and the waiting browser receives its credential in its next poll. A request expires after 10 minutes, and at most 10 can wait at once. The approving side never sees the request id.

**Recovery codes.** Ten single-use codes of 16 base32 characters are generated the first time a pairing link is made and shown once. Entering one on the pairing page pairs that browser. Settings has Make new recovery codes, which replaces all ten, and `GET /api/hub/devices` says how many are left.

**The workbench host.** Artifact pages run on their own origin, which holds its own credential. A paired browser on the UI host asks for a single-use token (`POST /api/hub/devices/workbench-handoff`, 60 seconds), and opens `{workbench origin}/_handoff#token=...&next=/{artifact}/`. The handoff page posts the token to its own host, which answers with that host's cookie, and then opens the artifact. Only an already-paired browser can mint a token, so an attacker can't fix a victim onto a credential of the attacker's own. The web app does this when Open is clicked on an artifact while the page is served through Residuum Cloud; a link copied from there reaches the workbench host unpaired and is redirected as above.

## Limits

Creating a pairing request, entering a recovery code, and presenting a pairing or handoff token each count against 10 attempts a minute per peer address and 60 a minute for the whole install, over a sliding minute. Attempts past a limit answer `429` with a plain-language reason and `Retry-After`. Local requests aren't counted. Requests the pairing page polls with aren't counted either: polling is authenticated by the 128-bit request id.

## Managing devices

`GET /api/hub/devices` lists the paired browsers, the waiting requests, the unused recovery code count and the instance's UI origin. `DELETE /api/hub/devices/{id}` revokes a device, which is refused on its next request. `POST /api/hub/devices/pending/{id}/approve` and `/deny` answer a waiting request. `POST /api/hub/devices/recovery-codes` makes new recovery codes. These need a paired browser or a local request, like the rest of the API, and `residuum remote devices` and `residuum remote revoke <id>` use them from a terminal.

Pages on the artifacts origin are refused every route under `/api/hub/devices` and `/api/hub/remote-access`, so an artifact page can't approve a pairing request or mint a link.

## Storage

`hub/remote-access.json`, readable only by its owner on Unix, holds the announced identity, the paired devices (hashes, names and times) and the unused recovery codes (hashes). It never holds a credential that could be replayed. Agents can't write it, and it is not checkpointed. A file that can't be parsed is moved aside to `remote-access.json.unreadable` and logged at `error`: every browser then has to pair again, which is done from the machine Residuum runs on.

## What you see

- Every refusal is a plain-language message in the response, and the gateway logs at `warn` each cross-site rejection, each recovery-code pairing and each revoke, and at `info` each browser that asks to pair, each answer to a request and each device paired by a link. A refusal of an unpaired request is logged at `debug`.
- The pairing page says why a link failed, shows the code to match, and says when a request was refused or expired.
- The Paired browsers group lists each browser with when it last made a request, and has Revoke on each row.
