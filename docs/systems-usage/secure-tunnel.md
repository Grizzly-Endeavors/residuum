# Secure Tunnel and Certificates

With the secure tunnel (tunnel v2), Residuum terminates TLS itself for its own addresses. The relay forwards raw encrypted bytes and sees only host names, IP addresses, timing and byte counts. The legacy tunnel ([Residuum Cloud Tunnel](cloud-tunnel.md)) is the fallback for a relay that doesn't offer it yet. Which browsers may use the secure tunnel is [Remote access](remote-access.md).

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
- An install that has a stored identity never falls back to the legacy tunnel when the relay answers `404` or `426` on `/tunnel/v2/register`: a relay that refused would otherwise read everything. The status says so and the client keeps retrying the secure tunnel. An install with no stored identity does fall back, and the status says `legacy`.

## Enrollment and the pin service

The first instance of a user enrolls: it generates a recovery code (20 characters, `A-Z2-7`), asks the relay for an `enroll` grant over the tunnel, and sends the pin service an `enroll` request signed with its ACME account key. The pin service then publishes CAA records that restrict certificate issuance for the user's names to the pinned accounts and to TLS-ALPN-01 (see the relay repository's `pins/README.md`).

- The recovery code is stored in `state.json` only until it has been saved. Settings, All agents, Residuum Cloud, Remote access shows it on the machine Residuum runs on, and `residuum remote status` prints it; `residuum remote saved` (or the button) makes Residuum forget it. Only the pin service's hash of it remains. A request that arrived through Residuum Cloud never sees it.
- A second instance of an enrolled user is told by the relay that pins exist, shows `needs_join`, and serves nothing remotely. Joining another instance is a separate feature; until it exists the instance stays local-only.
- `residuum remote reset-pins` (or Reset in Settings) takes the address back: it presents the recovery code and a relay `reset` grant, replaces every pin with this instance's account, and shows a new recovery code.
- Every hour the install reads `GET /v1/pins/{user}`. A pinned account that this install never pinned or approved is an unknown pin: the status lists it and a warning notice appears once per account.

## Certificates

The certificate manager (`src/remote_access/manager.rs`) keeps one certificate for the three names.

1. It loads or creates a persistent ACME account (a P-256 key in `hub/remote-access/acme-account-*.json`, one per directory URL, mode 0600). The account URL is what the pin service pins, and the same key signs the pin service requests.
2. It waits until public DNS (`[cloud] caa_resolver`, default `1.1.1.1:53`) shows CAA records that allow its account at both `{user}.{base}` and `{user}.workbench.{base}`, for up to 10 minutes.
3. It sends `ChallengeClaim` for the three names and waits for `ChallengeGranted` (a busy answer is retried for about a minute), so the relay routes `acme-tls/1` validation here even when another instance of the user is active.
4. It orders the certificate and answers TLS-ALPN-01 from the certificate resolver: a connection that offers only `acme-tls/1` gets the challenge certificate for its SNI; every other connection gets the instance certificate.
5. It releases the claim, stores the certificate and key (`hub/remote-access/certificate-*.json`, mode 0600) and installs it without a restart.

Renewal starts when one third of the certificate's lifetime remains, or earlier when the certificate authority's renewal information (ARI) says so. The window is looked at every six hours. A failed renewal keeps the old certificate serving and retries with backoff (15 seconds doubling to 15 minutes); the status shows `ready` with the failure in `detail`.

## The engine

`src/remote_access/engine.rs` serves each tunnel stream: TLS with ALPN `h2`, `http/1.1` and `acme-tls/1`, then HTTP. It picks the handler by the request's Host (`:authority`), not the SNI, because browsers may reuse one connection for the UI and workbench names. Requests for the UI and workbench names go in-process to the same routers the local listeners serve, marked remote with the real peer address the relay reported, so the device gate and rate limits apply. An unknown Host gets `421 Misdirected Request`. On the instance name, `/a2a/{agent}/...` goes to the hub's A2A listener and `/teams/{agent}` to that agent's Teams listener. The tunnel and sibling attestation headers are removed from every inbound request, and the instance name's A2A path keeps the relay's old per-address limit of 300 requests a minute with a burst of 60.

## Status and settings

`GET /api/hub/remote-access/status` reports `state` (`disabled`, `legacy`, `connecting`, `enrolling`, `needs_join`, `waiting_for_dns`, `ordering`, `ready`, `refused`, `error`), a plain-language `detail`, the user, slug and addresses, the certificate's expiry and renewal time, the pins, and whether a recovery code is waiting. `POST /api/hub/remote-access/retry` looks again now. The recovery code, `POST .../recovery-code/saved` and `POST .../reset-pins` work only for requests made on the machine Residuum runs on.

`[cloud]` in `hub/config.toml`:

| Key | Default | Meaning |
|---|---|---|
| `remote_access` | `true` | Try the secure tunnel. `false` keeps the legacy tunnel only. |
| `base_domain` | `agent-residuum.com` | Domain every host name is derived under. |
| `acme_directory` | `production` | `production`, `staging` (Let's Encrypt staging), or a directory URL such as a local Pebble. |
| `acme_root_ca` | none | PEM file with an extra root the directory's TLS certificate may chain to (a private test CA). |
| `pin_service_url` | `https://pins.agent-residuum.com` | The pin service. |
| `caa_resolver` | `1.1.1.1:53` | The public resolver used to see which CAA records the world sees. |

A staging account is a different account from a production one, so it has its own pin: switching `acme_directory` on an enrolled install needs the old pin replaced with a reset.

## Tests

`cargo test --quiet remote_access:: tunnel::v2` runs the tests that need nothing outside the process: a fake relay speaking tunnel v2 with a TCP front door, a fake pin service, and the engine behind both. The tests marked `#[ignore]` start Pebble, the Let's Encrypt test CA (images pinned in `src/remote_access/pebble_support.rs`), and its challenge test server in Docker with host networking. `just pebble` runs them, and CI runs them in the Rust job. They cover issuance and renewal through the tunnel, the recovery code reset, and the alert for an unknown pin.

## Files

`hub/remote-access/` holds `state.json` (identity, a recovery code until saved, accounts this install pinned or approved), the ACME account file and the certificate file per directory. All are mode 0600, never checkpointed, and write-blocked for agents.
