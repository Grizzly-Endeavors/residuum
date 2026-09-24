# Workbench Artifacts — Implementation Phases

> **Status:** archived. All phases shipped. End-to-end verification ran locally and through a local dev-mode relay. Design: [`workbench-artifacts-design.md`](./workbench-artifacts-design.md). Terms (artifact, workbench, artifacts listener, SDK, bridge, artifact identity, version token, workspace access policy) are defined there.

> Module level only. No file or line references — those get mapped in each phase's own session. Each phase is self-contained, depends only on phases before it, and is verifiable on its own. Each phase ships as its own branch and PR with passing pre-commit gates. Each phase updates the `docs/systems-usage/` pages whose behavior it changes, the bundled `workbench` skill (its `SKILL.md` and `references/api.md`), and the `residuum-system` skill's mirrors. A phase that ships a capability with a feature id (design §11) adds that id to the feature list; phases before Phase 2 have no list to add to.

## Dependency order

Phase 1 comes first. Phases 2 and 3 can proceed in parallel after it. Phases 4–8 and 10 can proceed in parallel once their listed preconditions have merged. Phase 9 follows 7 and 8. Phase 11 is last.

## Phase 1 — Rename tools to artifacts

- **Modules:** workbench module (discovery, artifacts listener, watcher, deletion); workbench HTTP API; gateway protocol types and frames; bus workbench event and topic docs; WebSocket subscriber mapping; gateway wiring for the listener; workspace layout and bootstrap docs; TypeScript export test and generated types; web UI (API client, workbench helpers, bridge, routes, router, workbench components, feed, help overlay, mock server, tests, contributor guide); SDK; bundled `workbench` and `residuum-system` skills; bootstrap `AGENTS.md`; systems-usage docs. In the relay repository: user-facing copy only.
- **Preconditions:** none.
- **Shape when done:**
  - Every identifier, route, frame, type, component, CSS class, user-facing string, and doc that calls a workbench page a "tool" says "artifact", per the design's wire-level rename table (§1).
  - Names for the place are unchanged: `/workbench` UI routes, the `workbench/` folder, the `workbench` skill name, the tunnel surface and capability, the handshake's `workbench_origin`, and the relay host.
  - Behavior is otherwise identical, including `residuum.send` (removed in Phase 7).
  - In the relay repository, strings that say "workbench tools" say "artifacts", in their own PR.
- **Verification:**
  - `cargo test --quiet` and the web checks (lint, type check, unit tests) pass; generated TypeScript types are regenerated and committed.
  - A repository-wide search for workbench-sense "tool" (for example `WorkbenchTool`, `workbench_tool`, `tools_origin`, `/api/workbench/tools`, "tools listener", "workbench tool") finds nothing outside `docs/archive/`.
  - Manually: open the Workbench page in the web UI, open an artifact, edit it on disk, and see it reload.

## Phase 2 — Artifact identity, bridge request flow, and feature detection

- **Modules:** SDK injection in the artifacts listener; SDK; bridge; web UI artifact view; status endpoint; a single feature-list definition.
- **Preconditions:** Phase 1.
- **Shape when done:**
  - The injected SDK embeds the artifact name, the Residuum version, and the feature list; `residuum.artifact`, `residuum.version`, and `residuum.features` expose them (design §6, §11).
  - The bridge stamps `X-Residuum-Artifact` on every relayed request, replacing any artifact-supplied value.
  - The bridge relays at most 8 ordinary requests at a time, queues the rest in order, and retries relay `agent overloaded` 503s as the design describes (§10). (The separate model-call limit arrives with model calls in Phase 7.)
  - The blocked-route list drops `/api/mcp/raw` and artifact deletion; the rest are unchanged (§10).
  - `GET /api/status` answers `{ mode, version, features }`. The feature list exists in one place and starts empty; later phases add their ids.
- **Verification:**
  - Unit tests: the bridge overwrites a spoofed identity header; the concurrency limit holds with more than 8 concurrent requests and preserves order; retry fires only on the relay's overloaded 503 and gives up after 3 attempts; `/api/mcp/raw` and `DELETE /api/workbench/artifacts/{name}` are no longer refused; the other blocked routes still are.
  - Rust tests: the injected SDK contains the artifact's name, version, and feature list; `/api/status` has the new fields.
  - Manually: an artifact that fires 100 parallel `residuum.fetch` calls through the relay completes all of them.

## Phase 3 — Workspace access foundation

- **Modules:** workspace HTTP API; a workspace access policy module; inbox and other handlers only where they share the blocked-path check.
- **Preconditions:** Phase 1.
- **Shape when done:**
  - One workspace access policy with segment-based matching (design §2) replaces the substring check, and every existing file endpoint uses it.
  - Workspace handlers do no blocking filesystem work on the async runtime.
  - Text writes are atomic, create missing parent directories, run the identity-file reload and `HEARTBEAT.yml` validation as before, and return `{ saved, version }`.
  - Listings carry `modified` and `version`; text reads return `ETag`; non-UTF-8 files answer `415`. Text reads and writes are capped at 8 MiB, with the write route's body limit raised to match.
  - Conditional writes (`If-Match`, `If-None-Match: *`) work on text writes with the `412` contract (§3).
- **Verification:**
  - Unit tests cover: blocked segments (`.index` at any depth, `memory/.index` itself, database files and their sidecars) and look-alikes that must not be blocked (`index.md`, `my.index.md`); version changes on write; `412` on a stale `If-Match` and on `If-None-Match: *` with an existing file; parent creation; path escape still refused; `415` on invalid UTF-8; a 3 MiB text file round-trips and an over-8 MiB write answers `413`; identity-file writes still send the reload signal; invalid `HEARTBEAT.yml` still refused.
  - Manually: the web UI's workspace file browser lists, opens, and saves files as before.

## Phase 4 — Full file handling

- **Modules:** workspace HTTP API (raw read/write, delete, mkdir, move); SDK and bridge binary bodies.
- **Preconditions:** Phases 2 and 3.
- **Shape when done:**
  - Raw read and write, delete, mkdir, and move exist with the design's contracts (§3), all through the access policy, all writes atomic, conditional headers honored, post-write hooks applied (a move onto an identity file triggers the reload signal).
  - The raw write route accepts bodies up to 8 MiB.
  - `residuum.fetch` accepts `ArrayBuffer`, typed-array, and `Blob` bodies; the bridge relays them unchanged.
  - Feature ids `workspace-raw`, `workspace-conditional-write`, and `workspace-file-ops` are listed.
- **Verification:**
  - Unit tests per endpoint: success, `404`, `403` for blocked paths, `409` for non-recursive directory delete and for move onto an existing path without `overwrite`, `400` for deleting the root, `412` on stale `If-Match`, `413` over 8 MiB; binary round trip is byte-identical.
  - Web unit tests: binary bodies pass through the bridge unchanged.
  - Manually: an artifact uploads an image with `residuum.fetch`, displays it from the raw endpoint, renames it, and deletes it.

## Phase 5 — Bulk reads

- **Modules:** workspace HTTP API (tree and batch read); glob matching dependency.
- **Preconditions:** Phase 3.
- **Shape when done:**
  - `GET /api/workspace/tree` and `POST /api/workspace/read` exist with the design's contracts (§4): flat sorted entries, glob and depth filters, per-file and response budgets with truncation markers, symlinks skipped, access policy applied.
  - Feature ids `workspace-tree` and `workspace-read-batch` are listed.
- **Verification:**
  - Unit tests: recursive listing; `*.md` matches at any depth while `notes/*.md` matches only relative to `path`; `depth` limit; blocked paths and symlinks absent; a binary file and an over-1 MiB file carry `skipped`; a tree larger than the budget returns a response under 8 MiB serialized with `content_truncated` and every entry's metadata; `listing_truncated` at the entry limit; batch read preserves order and reports per-file errors without failing the request.
  - Manually: through the relay, an artifact loads a 300-file folder with content in one request.

## Phase 6 — Change feed

- **Modules:** a workspace watcher (native notifications with polling fallback, debouncing, policy filtering); bus event and topic for workspace changes; WebSocket protocol (new client and server frames), per-connection watch sets, subscriber; workbench artifact change detection (moved onto the new stream; the polling watcher removed); web UI WebSocket coordinator, bridge, SDK; notice when live updates are off.
- **Preconditions:** Phases 2 and 3.
- **Shape when done:**
  - The watcher, subscriptions, frames, resync conditions, and artifact reload derivation behave as the design's §5 describes.
  - The web UI coordinator sends the open artifact's watch set and re-sends it after reconnect; the bridge filters per artifact, sends `workspace_resync { reason: "reconnected" }` after reconnects, and sends `connection` frames.
  - `residuum.watch(prefix, handler)` exists.
  - The dependency is pinned to the current stable release, and the PR runs the cross-compile workflow.
  - Feature id `workspace-watch` is listed.
- **Verification:**
  - Rust tests: a burst of writes produces one debounced batch; a rename produces `removed` + `created`; blocked paths never appear; a connection receives only changes under its prefixes (segment-matched: `wiki` never matches `wikipedia/`) and nothing when its set is empty; a batch over 500 matching changes becomes `workspace_resync { reason: "overflow" }`; an invalid prefix yields an `error` frame and leaves the set unchanged; artifact page and folder changes produce `artifact_updated`/`artifact_removed` while data-file changes do not.
  - Web unit tests: the coordinator sends the open artifact's watch set and re-sends it on reconnect; the bridge delivers only matching prefixes and sends the reconnect resync and `connection` frames.
  - Cross-compile succeeds for all four release targets.
  - Manually: an artifact watching `wiki` updates within a second of the agent editing a wiki page, including from a background session; an open artifact still reloads when its page is edited.

## Phase 7 — Model calls

- **Modules:** a model-call HTTP endpoint with provider access that follows config reloads; SDK; bridge (removing `send` and user-activation plumbing); web UI artifact view (in-flight call tracking); workbench skill.
- **Preconditions:** Phase 2.
- **Shape when done:**
  - `POST /api/model/complete` behaves as the design's §7 describes: small tier with its fallback and overrides, optional schema, the error contract, `info` logging with artifact identity and usage, and reload-following model resolution.
  - `residuum.ask` exists. `residuum.send`, the bridge's send handling, the user-activation check, and the "sent a message" notice are gone.
  - The bridge gives model calls their own limit of 4 (separate from the 8 for ordinary requests), tracks each frame's in-flight model calls by route, can abort them, and aborts them when the bridge is torn down (§10).
  - Feature id `model-complete` is listed.
- **Verification:**
  - Rust tests with a stub provider: shorthand and full requests produce the right messages; `schema` produces parsed `json`; unparsable structured output answers `502`; provider failure answers `502` with a plain message; timeout answers `504`; malformed requests answer `400`; the call uses the small tier's chain and falls back when small is unset; after a providers reload, calls use the new model.
  - Web unit tests: `send` is gone from the SDK and the bridge; aborting stops the relayed request and rejects the SDK promise.
  - Manually: an artifact summarizes a note via `residuum.ask`, and the log line names the artifact and the small model.

## Phase 8 — Artifact sessions

- **Modules:** sessions registry, runtime, and store (new category and trigger, idle timeout setting, relay and notify-router behavior); background configuration; sessions HTTP API (start, stop, message, artifact filter, artifact stop-all); web UI sessions store, sidebar, category formatting and badges; SDK sessions helper.
- **Preconditions:** Phase 2.
- **Shape when done:**
  - The `artifact` category, trigger, source label, defaults, idle timeout setting, and result delivery behave as the design's §8 describes. Every place that enumerates session categories (Rust and web) handles `artifact`.
  - The HTTP endpoints in §8 exist with their contracts, including the message endpoint's status-per-code mapping and artifact sender attribution.
  - An artifact session's `message_agent` to `main` fails with the design's tool error; its inbox tool still works.
  - The artifact idle timeout appears in the web UI settings beside the other per-category idle timeouts.
  - `residuum.sessions.start` returns a handle whose `on`, `send`, and `stop` work.
  - Feature id `artifact-sessions` is listed.
- **Verification:**
  - Rust tests: an artifact session's turn output is not relayed to main and its completion is not routed to the inbox; its `message_agent` to `main` is refused while its user-inbox tool succeeds; category and trigger round-trip through the store; the idle timeout setting applies; start without the identity header answers `400`; stop and message endpoints match the WebSocket commands' outcomes and error codes, with the design's HTTP status per code; a message sent with the identity header reaches the session attributed to the artifact, and one without it is attributed to the owner; the artifact filter returns only that artifact's sessions.
  - Web unit tests: artifact sessions group under their category; the SDK handle filters frames to its address.
  - Manually: an artifact starts a session that edits a file, streams its output into the page, and the result never appears in main chat or the inbox.

## Phase 9 — Visibility and stop controls

- **Modules:** web UI sessions list rows; artifact view bar; bridge (activity reporting and stop); artifact frame lifecycle.
- **Preconditions:** Phases 7 and 8.
- **Shape when done:**
  - Every stoppable session row has a stop button in every category; artifact sessions show and link to their artifact.
  - The artifact bar's activity indicator and panel list each live session the artifact started with its own stop button, and the in-flight model call count with Cancel calls. Stop page unloads the frame into a stopped state with Restart and aborts model calls without stopping sessions (design §9).
  - Closing an artifact leaves its sessions running.
- **Verification:**
  - Web unit tests: stop buttons appear for stoppable states only; the panel's session list follows session frames; stopping one session from the panel leaves the others running; Cancel calls aborts in-flight calls only; Stop page unloads the frame and aborts calls but issues no session stops; Restart reloads it.
  - Manually: an artifact looping on `residuum.ask` and `residuum.sessions.start` stops starting new work after Stop page; its already-started sessions keep running and can each be stopped from the panel or the sessions list.

## Phase 10 — Inbox, memory search, and artifact state

- **Modules:** inbox HTTP API; a memory search HTTP endpoint with access to the hybrid searcher; SDK state helper.
- **Preconditions:** Phase 2.
- **Shape when done:**
  - `POST /api/agent-inbox` and `GET /api/memory/search` behave as the design's §11 describes, with the source attributed to the artifact.
  - `residuum.state.get` and `residuum.state.set` exist.
  - Feature ids `inbox-add`, `memory-search`, and `artifact-state` are listed.
- **Verification:**
  - Rust tests: items land in the agent inbox with the artifact source and default title; blank bodies are refused; memory search validates its parameters, maps `source`, clamps `limit`, and reports `semantic`.
  - SDK tests or manual check: `state.get` returns `null` before the first `set` and the saved value after.
  - Manually: an artifact files an agent-inbox item and it appears where the `/inbox` command's items do; an artifact's search box returns memory results.

## Phase 11 — Consolidation and end-to-end verification

- **Modules:** bundled `workbench` skill and API reference; `residuum-system` skill; systems-usage docs; design docs.
- **Preconditions:** Phases 1–10.
- **Shape when done:**
  - The `workbench` skill teaches the full SDK surface (design, "SDK surface when done") with a short example of loading a subtree and staying current with `watch` + batch read, and of `ask` versus `sessions.start`.
  - `docs/systems-usage/workbench.md` describes the complete system, with no references to removed behavior.
  - This design and its phases document move to `docs/archive/` with their status updated.
- **Verification:**
  - End to end, locally and through the relay, an agent-built wiki-graph artifact: loads a 300-page wiki in one tree request, updates live as the agent and a background session edit pages, re-syncs after a network drop, summarizes a page with `ask`, starts a session that writes a new page (which then appears via the change feed), files an agent-inbox item, and after Stop page starts no new work while its running session can be stopped on its own.
  - Every capability in the design's feature list is present in `residuum.features`.
