# Secure Tunnel and Certificates

Residuum terminates TLS itself for its own addresses. The relay forwards raw encrypted bytes and sees only host names, IP addresses, timing and byte counts. The connection to the relay is described in [Residuum Cloud Tunnel](cloud-tunnel.md). Which browsers may use the secure tunnel is [Remote access](remote-access.md).

The code is in `src/remote_access/` (the manager, certificates, pin service client, engine) and `src/tunnel/v2/` (the tunnel client).

## Addresses

For user `bear` and instance slug `laptop` under the base domain `agent-residuum.com`, one certificate covers three names:

| Name | Serves |
|---|---|
| `bear.agent-residuum.com` | the web app, behind the device gate (Settings follows the user's active instance) |
| `bear.workbench.agent-residuum.com` | workbench artifacts, behind the device gate |
| `laptop.bear.agent-residuum.com` | this instance's public A2A (`/a2a/{agent}/...`) and Teams (`/teams/{agent}`), which authenticate their own callers |

## Identity is local

An install stores its user and slug in `hub/remote-access/state.json` when it enrolls with the pin service. From then on it derives every host name from those and the base domain (`[cloud] base_domain`, default `agent-residuum.com`). It never takes a host name, URL or `card_url` from a relay frame for anything it sends a credential to.

- A `Connected` frame whose user or instance differs from the stored identity, or whose `hosts` differ from the derived names, makes the install refuse the tunnel. The status says `refused`, a notice appears, and the client retries only after a long pause.
- Before the first enrollment the announced user and instance are taken once (trust on first use); enrolling is what stores them.
- For each stream the engine reads the SNI from the ClientHello itself and closes the stream when it differs from `StreamOpen.host` or isn't one of the three names.
- The pairing identity (cookie name and the origins in pairing links) comes from this local identity too, and what the relay announces is ignored once it exists.
- A relay that answers `404` or `426` on `/tunnel/v2/register` doesn't offer the secure tunnel. Nothing is served through it: the status says `error` with the reason and the client keeps retrying.

## Enrollment and the pin service

The first instance of a user enrolls: it generates a recovery code (20 characters, `A-Z2-7`), asks the relay for an `enroll` grant over the tunnel, and sends the pin service an `enroll` request signed with its ACME account key. The pin service then publishes CAA records that restrict certificate issuance for the user's names to the pinned accounts and to TLS-ALPN-01 (see the relay repository's `pins/README.md`).

- The recovery code is stored in `state.json` only until it has been saved. Settings, All agents, Residuum Cloud, Remote access shows it on the machine Residuum runs on, and `residuum remote status` prints it; `residuum remote saved` (or the button) makes Residuum forget it. Only the pin service's hash of it remains. A request that arrived through Residuum Cloud never sees it.
- A second instance of an enrolled user is told by the relay that pins exist, shows `needs_join`, and serves nothing remotely until it joins an existing instance (see [Joining a sibling](#joining-a-sibling)).
- `residuum remote reset-pins` (or Reset in Settings) takes the address back: it presents the recovery code and a relay `reset` grant, replaces every pin with this instance's account, and shows a new recovery code.
- `residuum remote email-reset` (or Lost your recovery code? Email me a reset link in the reset dialog in Settings) takes the address back without the recovery code. It needs a live relay session. The install generates a new recovery code and stores it before the request, asks the relay for an `email_reset` grant, and sends the pin service `POST /v1/reset/email` signed with its ACME account key. The pin service emails the address Residuum Cloud verified for the account, as `b***@gmail.com` in the reply, with a confirm link and a cancel link. After the person confirms, the reset waits out a hold period (24 hours) and then replaces every pin with this instance's account and the new recovery code's hash. If the request fails, the new code is forgotten and any earlier pending code comes back.
- While a reset by email is waiting, `state.json` keeps the new code and the status reports `pending_reset` (the requesting slug, whether it was confirmed, when it takes effect). The code is not shown, and not discarded by the needs-join check, until the reset takes effect; then it appears for saving like any other recovery code. If the pin service stops listing the reset without this account becoming the only pin, it was cancelled or expired: the code is discarded and a notice says so. While waiting, the install reads the pin set every five minutes.
- Every hour the install reads `GET /v1/pins/{user}`, which carries the pins and any `pending_reset`. A pending reset requested by another account raises a notice naming the requesting slug and the time it takes effect, and the status marks it `cancellable` on an instance that is already pinned. `residuum remote cancel-reset` (or Cancel the reset in Settings) sends `POST /v1/reset/cancel` signed with the install's pinned account. `residuum remote status` shows a pending reset either way.
- A pinned account that this install never pinned or approved is an unknown pin: the status lists it and a warning notice appears once per account. Accounts this install pinned, accounts of siblings it approved, and the account of an instance it joined count as approved.
- A pin whose instance slug is no longer in the relay's list of the user's instances (deleting an instance in the dashboard can't remove its pin) shows `removable` in the status. Settings, Remote access offers to remove it: the install signs `pins/remove` with its own account. Certificates that account already holds stay valid until they expire. Removing a joined sibling's account also forgets the join.

## Joining a sibling

A second or later instance can't enroll, because pins exist. It joins an existing instance A, and the person approves on A.

1. On the new instance B (Settings, Remote access, or `residuum remote join {slug}`), B derives A's instance host `{slug}.{user}.{base}` itself and fetches a single-use nonce from `GET /_sibling/join/nonce` (10 minutes).
2. B sends `POST /_sibling/join`, a JWS signed with B's certificate account key. It carries the nonce, B's slug and display name, B's account URL and key, and a fresh A2A key B issues for A. A checks the signature against the included key and that it was signed for A's own address. The answer is a random 128-bit join id only B knows.
3. Both sides compute the confirmation code: SHA-256 of A's account key thumbprint, B's thumbprint and the nonce, read as a number from its first four bytes and reduced to six digits. B shows it. A's Settings (or `residuum remote joins`) lists the request with the same code, B's claimed slug and name, and whether Residuum Cloud lists that slug (a hint from the relay, not proof). A request from a different key shows a different code.
4. On approval A issues an A2A key for B, records the key B issued, and, if B's account isn't pinned yet, calls `pins/add` signed with A's account. The pin is B's first join pinning it, whichever instance approves.
5. B polls `GET /_sibling/join/{join id}`, receives A's key and account, checks the pin service lists A's account under A's slug, and stores the join. If this was B's first join B then waits for DNS and orders its certificate.

A holds at most 5 pending requests, which expire after 10 minutes; the join endpoints answer 10 requests a minute per peer address. They are served on the instance host without the device gate. B reaches A only at the derived host over HTTPS with a publicly trusted certificate (plus the root named by `acme_root_ca`), and never follows redirects.

Joins are pairwise: B joins each instance whose agents it should reach. The keys of joins are in `hub/remote-access/siblings.json` (see [A2A](a2a.md#siblings)).

## Switching instances

The relay sends `InstancesUpdate` on connect and on every change; the status lists it as `instances` (names are plain text, entries whose slug fails the slug rules are dropped). The web UI shows a switcher in the rail when there is more than one instance. Switching sends `ActivateInstance`; the relay then closes the old instance's UI and workbench streams, and the page reloads onto the new instance, which may ask to pair.

## Certificates

The certificate manager (`src/remote_access/manager.rs`) keeps one certificate for the three names.

1. It loads or creates a persistent ACME account (a P-256 key in `hub/remote-access/acme-account-*.json`, one per directory URL, mode 0600). The account URL is what the pin service pins, and the same key signs the pin service requests.
2. It waits until public DNS (`[cloud] caa_resolver`, default `1.1.1.1:53`) shows CAA records that allow its account at both `{user}.{base}` and `{user}.workbench.{base}`, for up to 10 minutes.
3. It sends `ChallengeClaim` for the three names and waits for `ChallengeGranted` (a busy answer is retried for about a minute), so the relay routes `acme-tls/1` validation here even when another instance of the user is active.
4. It orders the certificate and answers TLS-ALPN-01 from the certificate resolver: a connection that offers only `acme-tls/1` gets the challenge certificate for its SNI; every other connection gets the instance certificate.
5. It releases the claim, stores the certificate and key (`hub/remote-access/certificate-*.json`, mode 0600) and installs it without a restart.

Renewal starts when one third of the certificate's lifetime remains, or earlier when the certificate authority's renewal information (ARI) says so. The window is looked at every six hours. A failed renewal keeps the old certificate serving and retries with backoff (15 seconds doubling to 15 minutes); the status shows `ready` with the failure in `detail`.

## The engine

`src/remote_access/engine.rs` serves each tunnel stream: TLS with ALPN `h2`, `http/1.1` and `acme-tls/1`, then HTTP. It picks the handler by the request's Host (`:authority`), not the SNI, because browsers may reuse one connection for the UI and workbench names. Requests for the UI and workbench names go in-process to the same routers the local listeners serve, marked remote with the real peer address the relay reported, so the device gate and rate limits apply. An unknown Host gets `421 Misdirected Request`. On the instance name, `/a2a/{agent}/...` goes to the hub's A2A listener and `/teams/{agent}` to that agent's Teams listener. `x-residuum-a2a-caller`, `x-real-ip` and `x-forwarded-for` are removed from every inbound request, and the instance name's A2A path keeps the relay's old per-address limit of 300 requests a minute with a burst of 60.

## Status and settings

`GET /api/hub/remote-access/status` reports `instances`, `siblings`, the join this instance started (`join`) and the requests waiting for it (`pending_joins`), besides `state` (`disabled`, `connecting`, `enrolling`, `needs_join`, `waiting_for_dns`, `ordering`, `ready`, `refused`, `error`), a plain-language `detail` (`disabled` means Residuum Cloud isn't configured), the user, slug and addresses, the certificate's expiry and renewal time, the pins, any reset by email waiting to take effect (`pending_reset`), whether a recovery code is waiting, and `checked_at`, when the last check finished (failed checks included; absent until the first ends). `POST /api/hub/remote-access/retry` looks again now, and `checked_at` moves once that check is done. The recovery code, `POST .../cancel-reset`, `POST .../join`, `POST .../joins/{id}/approve|deny`, `POST .../pins/remove` and `POST .../instances/{slug}/activate` work from the paired browsers too. `POST .../recovery-code/saved`, `POST .../reset-pins` and `POST .../email-reset` (which answers `{"email": "b***@gmail.com"}`) work only for requests made on the machine Residuum runs on.

`[cloud]` in `hub/config.toml`:

| Key | Default | Meaning |
|---|---|---|
| `base_domain` | `agent-residuum.com` | Domain every host name is derived under. |
| `acme_directory` | `production` | `production`, `staging` (Let's Encrypt staging), or a directory URL such as a local Pebble. |
| `acme_root_ca` | none | PEM file with an extra root the directory's TLS certificate may chain to (a private test CA). Calls to sibling instances trust it too. |
| `pin_service_url` | `https://pins.agent-residuum.com` | The pin service. |
| `caa_resolver` | `1.1.1.1:53` | The public resolver used to see which CAA records the world sees. |

A staging account is a different account from a production one, so it has its own pin: switching `acme_directory` on an enrolled install needs the old pin replaced with a reset.

## Tests

`cargo test --quiet remote_access:: tunnel::v2` runs the tests that need nothing outside the process: a fake relay speaking tunnel v2 with a TCP front door, a fake pin service, and the engine behind both. The tests marked `#[ignore]` start Pebble, the Let's Encrypt test CA (images pinned in `src/remote_access/pebble_support.rs`), and its challenge test server in Docker with host networking. `just pebble` runs them, and CI runs them in the Rust Tests job, on pull requests that change Rust code and on release tags. They cover issuance and renewal through the tunnel, the recovery code reset, the reset by email (a kept recovery code, a failed request, a cancel signed by a pinned account), and the alert for an unknown pin.

## Files

`hub/remote-access/` holds `state.json` (identity, a recovery code until saved, the reset by email being waited on, accounts this install pinned or approved), `siblings.json` (the keys exchanged with joined siblings), the ACME account file and the certificate file per directory. All are mode 0600, never checkpointed, and write-blocked for agents.
