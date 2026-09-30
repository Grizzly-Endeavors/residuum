# Web UI Overhaul — Work Units

> **Status:** draft, pending owner sign-off. Design: [`design.md`](./design.md) (terms are defined there). Capability checklist: [`parity.md`](./parity.md). Visual reference: [`mockup.html`](./mockup.html).

> Module level only. No file or line references; each unit's implementer maps those first. A unit depends only on the units in its preconditions and is verifiable on its own.

## How this document is used

One main session, the orchestrator, runs the implementation.
- It dispatches one subagent per work unit.
- It owns branches, PRs, merges and anything that needs the owner.
- Work is split by amount of work, so that no agent runs too long. Phases group related units, but the unit is what gets dispatched.

**The implementing agent receives** this document (its unit), the design, the parity checklist, the mockup and the codebase.

**It does this:**
1. Map the file-level changes.
2. If the mapping exceeds the unit's size, stop and return a proposed split instead of starting.
3. Implement.
4. Add tests at the layers below.
5. Run the gates.
6. Commit on the unit branch the orchestrator gave it.
7. Report:
   - what changed
   - how it was verified
   - which parity items it checked
   - anything it could not do
   - anything the owner must check by hand

It does not merge, and does not contact the owner.

**Sizes.** Budgets count new and modified lines. Code moved verbatim between files counts once. Tests and snapshots are excluded.

| Size | Scope | Rough budget |
|---|---|---|
| S | One component family or small contract | ≈ 400 lines |
| M | One surface or one backend feature | ≈ 1,200 lines |
| L | The most one agent takes on | ≈ 2,000 lines |

**Targets.**
- **main**: branch from `main`, PR into `main`.
- **integration**: branch from `feat/web-overhaul`, PR into it.
- **relay**: a branch and PR in the relay project, which the owner deploys.

`feat/web-overhaul` is created from `main` after Phase 1 merges, and merges `main` after every backend unit lands. A precondition written as "W05 merged" means that merge has happened.

**Parallelism.** Units whose preconditions are met, and whose modules don't overlap, may run at once in separate worktrees. Each phase lists the groups that can overlap. Units that edit the same modules are ordered, not overlapped.

**Done, for every unit:**
- The pre-commit gates pass.
  - Web: format, lint, type check, unit and component tests.
  - Rust: fmt, clippy, tests, `cargo deny`.
- New or changed behavior has unit or component tests.
- Frontend units from W03 on also:
  - cover user-facing flows with end-to-end specs on both projects
  - scan every place or overlay they touch with axe
  - add visual baselines for new surfaces
- The mock server implements every endpoint and frame the unit adds or changes, and its route-parity test passes.
- Legacy components and styles the unit replaces are deleted, and removed from the style-lint ignore list.
- The unit's `parity.md` items are checked, in the same PR.
- Docs describing behavior the unit changed are updated in the same PR. Backend units update the matching `docs/systems-usage/` pages, and the bundled `residuum-system` reference where one exists for that page (`inbox`, `notifications`, `heartbeats`).

**Owner checks**, which no agent can perform (a real phone, the relay, a real push service), are listed in the unit's report and collected in W50.

**The relay project**, which some units read, is a separate repository checked out beside this one, at `../relay` relative to this repository's root.

---

## Phase 1 — Harness and guardrails (target: main)

W01, then W01b. Then W02 and W04 together. Then W03 and W02b after W02.

### W01 — Mock server as typed modules, part 1 (L)

- **Modules:** mock server; web type-check, lint and format configuration.
- **Preconditions:** none.
- **Shape when done:**
  - The mock server has a module structure, with shared state, routing and socket plumbing in their own modules.
  - The hub, agent chat, sessions and config areas are moved into modules, type-checked, linted and formatted with the app's rules, and use generated protocol types wherever one exists.
  - The remaining areas stay in the original file, excluded from the new checks until W01b.
  - Behavior is unchanged.
- **Verification:**
  - The type check and lint cover the moved modules.
  - Today's manual scenarios behave the same under `just web-mock`: the `spawn`, `drop` and `busy` chat triggers, the test-control endpoints, and setup mode.
  - Existing tests pass.

### W01b — Mock server as typed modules, part 2 (L)

- **Modules:** mock server.
- **Preconditions:** W01.
- **Shape when done:**
  - The files and checkpoints, workbench, inbox, scheduled and test-control areas are moved into modules under the same checks.
  - The original single file is gone.
  - Behavior is unchanged.
- **Verification:** the same as W01, now covering the whole mock.

### W02 — Mock determinism, preview mode and route parity (M)

- **Modules:** mock server; justfile; pre-commit hook.
- **Preconditions:** W01b.
- **Shape when done:**
  - **Deterministic mode:**
    - a fixed clock
    - configurable delays, zero in this mode
    - a stable scenario matching today's: atlas and scout running, drifter stopped, brittle failed, with the existing sessions, outbound tasks, inbox and artifacts
  - **Reset:** a test-control endpoint restores the initial scenario.
  - **Preview mode:** serves a production build together with the mock. Both modes start headlessly on a given port.
  - **Coverage:** every endpoint the API client calls is implemented, including those that answer 404 today (the Scheduled view's).
  - **Route parity:** a unit test calls every API client function with sample arguments against a recording fetch, and checks each recorded method and path against the mock's exported route table.
  - **Pre-commit:** the web gate triggers on any change under the web app directory.
- **Verification:**
  - The parity test passes, and fails when a mock route is removed.
  - Two identical sequences separated by a reset give identical responses.
  - Preview mode serves the built app and the mock API.

### W02b — Workbench mock fidelity (S)

- **Modules:** the mock server's workbench area and artifacts listener.
- **Preconditions:** W02.
- **Shape when done:** the mock matches today's real listener closely enough to test against:
  - a fixed artifacts port in deterministic mode, and the listener included in preview mode
  - folder artifacts, the `/name` → `/name/` redirect, and `nosniff`
  - the sample artifact names its agent in `ask` and `sessions.start`
  - deletes return a checkpoint id
  - `artifact_updated`, `artifact_removed` and `workspace_changed` sent when mock files change, through a test-control endpoint

  API forwarding, the session relay and the new SDK behavior are added to the mock by the units that build them (W11b, W12b, W32b).
- **Verification:**
  - Route parity passes.
  - A manual run of the sample artifact in today's embedded view:
    - starting a session delivers its frames
    - Fire 3 calls shows three calls in flight, and Cancel calls clears them
    - delete then Undo restores it
    - a test-control edit live-reloads it

### W03 — End-to-end, accessibility and visual harness (M)

- **Modules:** Playwright configuration and helpers; justfile; CI quality workflow; web contributing guide (testing section).
- **Preconditions:** W02.
- **Shape when done:**
  - **Projects:** Chromium desktop 1440×900 and Chromium phone 390×844 (touch, mobile user agent), plus a local-only WebKit phone project.
  - **Servers:** the mock in deterministic dev mode, and in preview mode for specs tagged as needing a production build. The mock is reset before each test.
  - **Helpers:**
    - an axe scan failing on serious and critical violations
    - screenshots with frozen time and animation and masked dynamic regions
    - traces on failure
  - **Visual comparisons** run in the official Playwright container, pinned to the installed Playwright version.
  - **Runner check:** the unit determines whether the CI runner can run that container.
    - If it can, CI runs visual comparisons.
    - If it can't, CI skips them, the orchestrator runs them locally before integration merges, and the unit's report says so, so the orchestrator can file the follow-up issue.
  - **Smoke specs** cover the current UI: open a chat, send a message and see the reply, open settings, open the team page. They make no accessibility assertions.
  - **Recipes:** `just web-e2e` and `just web-e2e-update`, the latter refreshing baselines in the container.
  - **CI:** runs the suite in the web job, and uploads the report and traces on failure.
  - **Docs:** the contributing guide documents the layers.
- **Verification:**
  - The suite passes locally and in CI.
  - A deliberately broken selector fails with a trace.
  - A sample baseline made in the container matches when compared, in CI or locally per the runner result.

### W04 — Lint and CI guardrails (M)

- **Modules:** style lint (replacing the grep-based CSS check); ESLint configuration and the rune store modules; svelte-check invocation; CI workflows; package manifest; coverage configuration.
- **Preconditions:** W01b.
- **Shape when done:**
  - **Style lint.** Checks global stylesheets and component style blocks. Outside a designated token file it forbids literal colors, raw font sizes, raw z-index values, and literal durations and easing curves. Every existing stylesheet and component is on an ignore list.
  - **Stores.** Rune store modules get the strict TypeScript rules, and their violations are fixed.
  - **svelte-check** fails on warnings. The one existing global accessibility suppression remains.
  - **Types.** The Rust CI job regenerates the TypeScript types and fails on any difference.
  - **Coverage** is reported in the CI summary, with no threshold.
  - **Node version.** The package manifest declares the supported Node version, and the contributing guide matches.
- **Verification:**
  - Lint passes on the current code.
  - A literal color in a new component fails.
  - An unregenerated Rust type change fails CI.
  - Coverage appears in the summary.

---

## Phase 2 — Backend contracts (target: main)

Each unit implements its part of design §9, and updates the mock, the generated types and the matching docs.

**Order, after W02:**
- W05, W06, W07 and W12 together.
- W08 after W05, then W09, then W10 (which also needs W07), then W11, then W11b.
- W12b after W12, then W12c, then W12d in the relay project (it uses W12c's protocol types).
- These share hub runtime, WebSocket and host modules, so they run in sequence.

### W05 — Hub snapshot, error kind and hub frames (L)

- **Modules:**
  - hub events and WebSocket
  - agents listing
  - activity tracker (`busy_since`)
  - the failure recorder and its callers
  - hub config reload
  - TypeScript export
  - web hub types (switched to generated)
- **Preconditions:** W02.
- **Shape when done:** design §9.1 in full:
  - activity and stopping in the snapshot and listing
  - `busy_since`
  - `AgentLastError.kind` and `reason`, passed by each failure caller
  - the `hub_config_reloaded` (with `changed`) and `hub_boot` frames
  - every hub frame exported, including `HubClientMessage` and `agent_stopping`
- **Verification:** Rust tests:
  - snapshot activity during and after a turn
  - the stopping set during a stop
  - `kind` and `reason` for a config failure, a Teams port conflict, and a crash
  - `hub_config_reloaded` for a change, no change, and a failure
  - `hub_boot` sent first

  Type drift must be clean.

### W06 — File-only routes for non-running agents (S)

- **Modules:** agent route dispatch; repair and running routers.
- **Preconditions:** W02.
- **Shape when done:** the routes in design §9.2 answer for stopped and failed agents with unchanged shapes. `status` stays running-only.
- **Verification:** Rust tests for each route on a stopped agent; updated dispatch tests.

### W07 — Cross-agent inbox and unique item ids (M)

- **Modules:** hub inbox endpoints; inbox storage (unique ids); a shared local-to-UTC conversion.
- **Preconditions:** W02.
- **Shape when done:**
  - The endpoints and shapes in design §9.5.
  - Timestamps follow the conversion rule in §9.
  - Saved items get a unique id when their stem already exists, and existing ids keep working (closes #305).
  - The hub's own inbox notes are **not** removed here (W48).
- **Verification:** Rust tests:
  - listing across a running and a stopped agent
  - paging
  - read, archive and restore round trips
  - DST-edge conversions
  - JSON errors for an unknown agent or item
  - two same-title items on one day both surviving

### W08 — Per-agent watcher, turn hook and new agent events (M)

- **Modules:**
  - agent control handle (a bus subscription handle)
  - hub per-agent watcher (new)
  - activity tracker turn hook
  - the `UserInbox` bus topic and its event, published by the user-inbox tool
  - the outbound task tracker's threshold event
- **Preconditions:** W05.
- **Shape when done:**
  - The watcher, turn hook and events behave per design §9, including the drain, `Resync` and `Unavailable` rules.
  - The watcher feeds a typed internal hub stream of agent changes: session lifecycle, outbound task changes, `user_inbox_added`, watched path changes, resync. It also keeps every session event available to the session relay that W11b builds.
  - Nothing is exposed over HTTP yet.
- **Verification:** Rust tests drive a running test agent and assert that:
  - each change kind reaches the stream
  - the turn hook fires once per turn, with texts and visibility
  - one threshold event fires per unreachable streak
  - a lossy overflow gives a resync
  - `Unavailable` switches to the 60-second recompute
  - the watcher drains while unconsumed
  - nothing arrives after stop

### W09 — Team event log (M)

- **Modules:** hub event log (new); hub runtime (lifecycle, notices, hub start); consumers of the internal stream and turn hook; hub HTTP and WebSocket.
- **Preconditions:** W08.
- **Shape when done:** design §9.4 in full: kinds, levels, targets, protected eviction, boot id, the endpoint with `before` and `after`, and `team_event` frames.
- **Verification:** Rust tests:
  - one event per kind from its trigger, and none for per-connection warnings
  - scheduled runs producing only `scheduled_run_finished`
  - eviction keeping the newest 100 warn and error entries
  - paging both ways
  - frames on the socket

### W10 — Overview: last message, sessions, inbox counts (L)

- **Modules:** hub overview (new); stream and turn hook consumers; disk readers for stopped agents; hub HTTP and WebSocket.
- **Preconditions:** W07, W09.
- **Shape when done:**
  - `GET /api/hub/overview` and `agent_overview` per design §9.3, for `last_message`, `live_sessions` (with `source_label`) and `inbox_unread`.
  - `upcoming` and `outbound_problems` are present but empty until W11.
  - Coalescing is trailing-edge, at most one frame per agent per second.
  - The hub's inbox actions update counts.
- **Verification:** Rust tests:
  - `last_message` from the turn hook, from recent history at start, and from an episode with day precision
  - preview stripping and truncation
  - background turns ignored
  - live sessions appearing and clearing
  - counts after `user_inbox_added`, after a hub read, and after a hand-placed file
  - coalescing under a burst, with the final state delivered

### W11 — Overview: upcoming runs and outbound problems (M)

- **Modules:** pulse next-run calculation (shared with the per-agent scheduled endpoint); scheduled actions reader; outbound task reader; hub overview.
- **Preconditions:** W10.
- **Shape when done:**
  - `upcoming` and `outbound_problems` per design §9.3.
  - The per-agent pulses endpoint's `next_fire_at` uses the same calculation.
- **Verification:** Rust tests:
  - a run pushed past quiet hours
  - a never-run pulse
  - `pulse_enabled` off
  - an unloadable config giving no runs
  - actions ordering
  - an outbound problem appearing at the threshold and clearing
  - empty outbound problems for a stopped agent

### W11b — Hub artifact events and session relay (M)

- **Modules:** hub workbench watcher (new, on the team change feed); hub socket subscriptions, acknowledgements, lag notice and `session_frame` forwarding from the per-agent watcher, with its address-to-source-label map; mock server; hub HTTP and `workbench.md` docs.
- **Preconditions:** W11.
- **Shape when done:**
  - Design §9.8 in full.
  - Existing per-agent artifact frames remain.
- **Verification:** Rust tests:
  - artifact events with no agent running
  - a subscribed session's full event stream, tool frames included, arriving on the hub socket
  - an acknowledgement before any frame
  - artifact subscriptions matching sessions started later on two different agents
  - unsubscribe stopping delivery
  - `session_relay_lagged` on a lagged connection
  - a notice and no acknowledgement for an unknown agent
  - subscriptions ending with the connection

### W12 — Asset caching and compression (S)

- **Modules:** embedded asset handler; HTTP middleware.
- **Preconditions:** W02.
- **Shape when done:**
  - Headers and compression per design §9.6, with HTML never compressed.
  - The relay's HTTP forwarding code is read to confirm that `Cache-Control`, `ETag` and `Content-Encoding` pass through, and that its switcher insertion still works on uncompressed HTML. The result goes in the report.
- **Verification:** Rust tests for:
  - headers per asset class
  - 304 on a matching `If-None-Match`
  - compression negotiation
  - SPA fallback

### W12b — API forwarding on the artifacts origin (M)

- **Modules:** artifacts listener (`/api` and WebSocket dispatch to the hub router through a late-bound handle, the request-extension marker, the block list, the reserved name `api`); the activity tracker (forwarded agent sockets don't count as clients); mock artifacts listener; `workbench.md`, including its security model paragraph.
- **Preconditions:** W12.
- **Shape when done:**
  - Design §9.9's local half: the artifacts listener serves `/api/*` and socket upgrades through the hub router, marked as arriving through the artifacts origin.
  - Marked requests are refused on the block list.
  - The cross-site guard still refuses other sites.
  - The embedded UI keeps working unchanged. Forwarding is additive.
- **Verification:**
  - Rust tests:
    - an API call and a hub socket through the artifacts port
    - each block-list route refused with the marker and allowed without it
    - a forged marker header from a client having no effect
    - a cross-site request refused
  - A mock-backed end-to-end check opens the sample artifact's URL directly and calls an agent route.

### W12c — Tunnel socket surface (S)

- **Modules:** tunnel protocol types (the socket-open frame's optional `surface`); tunnel client socket forwarding; the `workbench-sockets` capability.
- **Preconditions:** W12b.
- **Shape when done:**
  - A socket open with the workbench surface connects to the artifacts listener.
  - One without a surface behaves as today.
  - An old relay that sends no surface still works.
- **Verification:** Rust tests for both surfaces, a missing artifacts listener answering a failed open, and backward compatibility.

### W12d — Relay workbench host (M, target: the relay project)

- **Modules:** in the relay project: the workbench host dispatch (all methods with request bodies, socket upgrades allowed, owner-session check kept, login returning to the artifact URL); socket forwarding with the workbench surface, gated on the `workbench-sockets` capability; its copy of the tunnel protocol types; its tests and docs.
- **Preconditions:** W12c.
- **Shape when done:** design §9.9's relay half.
- **Verification:**
  - Relay tests: writes and socket upgrades on the workbench host forwarded with the surface; unauthenticated requests still redirected or refused; the relay's own routes still unreachable there.
  - Owner check: deploy, then open an artifact through Residuum Cloud, start a session and see its frames. This deploy must happen before the cutover merge (W50).

---

## Phase 3 — Foundations (target: integration)

W13 and W16 together. W14 after W13. W17 and W17b after W16. W15 after W13 and W17. W18 after W17b, once W05 is merged.

### W13 — Tokens, fonts, base styles and icons (M)

- **Modules:** token set (new names, no collision with legacy variables); scoped base styles; bundled fonts; icon component; style-lint configuration; web aesthetic guide.
- **Preconditions:** Phase 1.
- **Shape when done:**
  - The token set in design §1 exists, including the allowed text pairs on stone and tint surfaces.
  - Fonts are bundled and the font CDN import is removed. Legacy styles fall back to the bundled or system faces.
  - Reduced motion is handled globally.
  - One icon component covers the mockup's icons.
  - The token file is the only lint exemption.
  - The aesthetic guide is rewritten to describe this visual system.
- **Verification:**
  - A unit test checks every declared pair against AA.
  - An end-to-end check confirms no request goes to a font CDN.
  - Lint passes.

### W14 — Primitives: controls (L)

- **Modules:** Button, IconButton, fields (text, number, select, toggle, segmented, secret), Badge, status dot, Disclosure, Tabs, EmptyState, Skeleton, Banner, Kbd; a development-only gallery route.
- **Preconditions:** W13.
- **Shape when done:**
  - Each primitive has the variants the design uses.
  - Each has keyboard behavior, labels, disabled and error states, and 44px touch targets at phone width.
  - The gallery shows every primitive and state, in development and mock builds only.
- **Verification:**
  - Component tests per primitive.
  - An axe scan of the gallery.
  - Gallery baselines.

### W15 — Primitives: overlays, toasts and notification history (L)

- **Modules:** Menu, Popover, Tooltip, Dialog, Sheet, Drawer; the overlay stack (focus, `inert`, scroll lock, stacking, overlay entries through the router); toast region; Recent notifications dialog.
- **Preconditions:** W13, W17.
- **Shape when done:**
  - Overlays follow design §1's model, and modal ones use overlay entries (§3).
  - A Dialog can go full-screen at phone width. Sheets and Drawers dismiss by swipe.
  - The toast region renders the existing toast store with today's timings and actions, and is announced to assistive technology.
  - The Recent notifications dialog shows the existing history, with details, clear and Undo.
  - The gallery gains overlay demos.
- **Verification:**
  - Component tests: focus trap and restore, Esc, nested stacking, toast timing.
  - End-to-end: each overlay opens, and Back closes it.
  - axe scans and baselines.

### W16 — API client agent scoping (M)

- **Modules:** API client; cache keys; every call site; the module-level current-agent state (reduced to routing's viewed agent and the socket coordinator's bound agent).
- **Preconditions:** Phase 1.
- **Shape when done:**
  - Every agent-scoped API function takes the agent explicitly.
  - Cache keys include it.
  - No request reads a module-level current agent.
  - Behavior is otherwise unchanged.
- **Verification:**
  - Unit tests of recorded requests for two agents.
  - The route-parity test passes.
  - Smoke specs pass.

### W17 — Route model, router and section registry (L)

- **Modules:**
  - route parsing and formatting
  - router: push and replace, close semantics, overlay entries, the unsaved-edit guard
  - settings section registry: ids, scopes, labels, groups, old-to-new and new-to-old mappings, and a coarse table from top-level config keys to sections (used by Fix settings until the settings model's full field map exists)
  - the sessions store's router dependency (removed)
  - an adapter for the current app
- **Preconditions:** W16.
- **Shape when done:**
  - Design §3 is implemented.
  - The registry covers every section in design §8.
  - Until W19, the current app runs on the new router through the adapter, which also maps new section ids back to legacy sections for hosting.
  - No store imports the router.
- **Verification:**
  - Unit tests for:
    - every route, redirect row and correction
    - push versus replace per navigation kind
    - closing via back versus replace
    - the guard on navigation and on `popstate`
  - Smoke specs pass.

### W17b — Watch registry (S)

- **Modules:** watch registry (new) for the agent socket and the hub socket's team watch; the existing workspace-watch sync, the Files view and the legacy artifact bridge, switched to it (the bridge until W32a deletes it).
- **Preconditions:** W16.
- **Shape when done:**
  - Design §12's registry: owners register and unregister prefixes.
  - The union is sent, and re-sent on reconnect and on a bound-agent switch.
  - Each owner gets only its matching changes.
  - No owner can replace another's watches.
- **Verification:**
  - Unit tests: two owners' prefixes merged and delivered separately; one owner unregistering leaves the other's watches; re-send on reconnect.
  - Smoke specs pass, and the sample artifact's watch still works.

### W18 — Config write coordinator (M)

- **Modules:** config write coordinator (new, replacing the lock); the composer's model and thinking controls and the legacy settings save path, switched to it.
- **Preconditions:** W17b; W05 merged.
- **Shape when done:**
  - Every config write goes through the coordinator per design §8:
    - serialized per file
    - a pre-save re-read with overlap detection
    - subscribers notified after write, reload and restore
    - external changes picked up from the bound agent's `config/` watch and `hub_config_reloaded`
  - The composer controls and footer model label refresh when settings change.
- **Verification:**
  - Unit tests:
    - concurrent writes serialize
    - a non-overlapping external change survives
    - an overlapping one prompts
    - subscribers fire on each trigger
  - End-to-end: changing the main model in settings updates the composer control without a reload.

---

## Phase 4 — Shell (target: integration)

W19, then W20 and W21 together.

### W19 — Shell frame (L)

- **Modules:** application root and layout; rail (new); phone bottom bar and drawer (new); overlay host; hub banner; place routing with legacy views hosted; legacy layout rules adjusted for hosting. The header, hamburger menu, agent chip row, sessions-sidebar toggle and notification corner are deleted.
- **Preconditions:** W14, W15, W17; W05 merged.
- **Shape when done:**
  - The shell matches design §2 and the mockup at all three widths, except the context panel (W20):
    - the rail with the accordion rules and the footer (gear, help menu with Recent notifications)
    - the bottom bar and the drawer
    - the hub banner
  - **Interim counts:**
    - The Home count is the number of failed agents from the snapshot, until W27.
    - The Inbox count is the bound agent's inbox unread from the legacy per-agent poll, until W29.
    - Agent chat-unread comes from the snapshot.
  - **Every place routes.** Places not yet rebuilt host their legacy view:

    | Place | Hosted legacy view |
    |---|---|
    | Chat | current chat |
    | Activity | sessions list |
    | Schedule | Scheduled page |
    | Files, Shared files | workspace |
    | Home | Team page |
    | Inbox | inbox drawer content |
    | Settings modal | current settings page, through the registry mapping |
    | Workbench | itself |
- **Verification:**
  - End-to-end on both projects:
    - every place from the rail, drawer and bottom bar
    - the accordion
    - back and forward
    - each redirect row
    - the hub banner via mock controls
  - axe scans and baselines.

### W20 — Context panel host (M)

- **Modules:** context panel (resizable, floating, sheet); `panel` parameter wiring; hosting the legacy session view and file editor in the panel until their units land.
- **Preconditions:** W19.
- **Shape when done:**
  - The panel behaves per design §2 at all three widths and follows the `panel` rules in §3.
  - `session:` and `file:` open their legacy views inside it.
- **Verification:**
  - End-to-end: open and close by link, Back, resize (wide), overlay (medium), full-screen sheet (phone), and an invalid `panel` corrected.
  - axe scans and baselines.

### W21 — Action registry, palette, shortcuts and feedback (M)

- **Modules:** action registry (new); command palette (new); Keyboard shortcuts dialog; feedback dialog on Dialog; the composer's `/` menu switched to the registry.
- **Preconditions:** W19.
- **Shape when done:**
  - **The registry holds:**
    - every place and agent
    - live sessions
    - every settings section
    - chat actions: the former slash commands, with plain labels and their old names as search terms
    - lifecycle actions and create agent
    - feedback and bug report
    - shortcuts and Recent notifications
    - Install app, hidden until W43
  - Actions needing a running agent are disabled, with the reason.
  - **The palette** opens by ⌘K or Ctrl+K, from the rail search row, and from the phone Search tab.
  - **The shortcuts dialog** lists Esc for stopping a reply, and describes `/` accurately.
- **Verification:**
  - Unit tests for matching old command names and for disabled reasons.
  - End-to-end: navigate by palette, run "Summarize older messages now", and use the `/` menu.
  - axe scans and baselines.

---

## Phase 5 — Surfaces (target: integration)

**Groups:**
- Once W20 and W21 are in: W22, W30 and W31 together.
- W32b once W11b, W12b and W12c are merged.
- W32a after W32b, W26 and W27. Between W32b and W32a, the legacy embedded view on the integration branch no longer works with the new SDK. That is acceptable, because the branch doesn't ship until cutover.
- W23 after W22, then W24, then W25. W26 after W22.
- Once W09–W11 are merged: W27, then W28.
- W29 once W06, W07 and W10 are merged.
- W33 after W18. W34 after W33.
- W35–W40 together after W34, where W35 also needs W23. W41 after W38, W39 and W40.
- W42 at any point after W14 and W15.

### W22 — Chat feed rendering (L)

- **Modules:** chat feed and message components (shared with session transcripts); path links; chat header; the legacy message and feed components (deleted).
- **Preconditions:** W20, W21.
- **Shape when done:**
  - The feed renders per design §4:
    - unboxed replies and moss bubbles
    - message cards, with Open session for session senders only
    - dividers and the plain-language compressed marker
    - attachments and copy on code blocks
    - path links into the panel
  - Every Chat feed parity item is kept, with one empty state.
  - Tool calls render as a plain placeholder until W24.
- **Verification:**
  - Component tests per message type.
  - End-to-end: lazy loading, jump to latest, reconnect reconciliation via the mock's `drop` trigger, a path link opening the panel, and a missing path.
  - axe scans and baselines.
  - Parity: Chat feed.

### W23 — State cards and turn grouping (M)

- **Modules:** display-state derivation; state cards (new); Fix settings resolution through the validate endpoints; turn grouping; history loading for non-running agents.
- **Preconditions:** W22; W05 and W06 merged.
- **Shape when done:**
  - State cards follow design §4 for failed, stopped, starting and stopping, including the kind-specific actions and Fix settings. Until W35, Fix settings opens the hosted legacy settings at the mapped section.
  - The past conversation stays readable. No reconnecting text appears for a non-running agent.
  - Turns group per design §4.
- **Verification:**
  - Unit tests: display state (including stopping), turn boundaries for live, recent and episode messages, and Fix settings choosing a section from a diagnostic.
  - End-to-end on brittle, drifter and atlas: the cards, a restart failure, and Fix settings landing.
  - axe scans and baselines.

### W24 — Activity line and live turn (L)

- **Modules:**
  - activity line (new)
  - friendly label table
  - feed store per-turn aggregation, timing and failure state
  - `set_verbose` on connect
  - removed: the `/verbose` command, and the legacy tool group and thinking indicator
- **Preconditions:** W23.
- **Shape when done:**
  - Activity lines per design §4 for live and historical turns, including the connected-mid-turn note.
  - Live turns show steps, the timer, Stop and Esc, and intermediate texts, and collapse on end.
- **Verification:**
  - Unit tests for labels and phrasing: repeats merged, a failed live step, the MCP fallback.
  - Component tests for the three levels.
  - End-to-end: a live turn; Stop mid-turn; a historical line.
  - axe scans and baselines.

### W25 — Composer and conversation size (M)

- **Modules:** composer (attach, `/` actions, model and thinking popover, send and stop, per-agent drafts); post-turn status line; conversation-size panel view. The legacy chat footer is deleted.
- **Preconditions:** W24, W18, W21.
- **Shape when done:**
  - The composer meets parity, with drafts per agent.
  - The popover writes through the coordinator, and is a sheet on phones.
  - The post-turn status and the conversation-size view follow design §4.
- **Verification:**
  - End-to-end:
    - attach images
    - a draft survives navigation and reload
    - change the model from the popover
    - the offline queue notice
    - conversation size for running and stopped agents
  - axe scans and baselines.
  - Parity: Composer, Chat footer.

### W26 — Activity place and session panel (M)

- **Modules:** Activity place (new); session run panel content; outbound task rows; sessions store view state. The legacy sessions sidebar, row and session page are deleted.
- **Preconditions:** W22.
- **Shape when done:**
  - Activity and the session panel meet design §5 and parity.
  - The chat header's running pill opens Activity.
- **Verification:**
  - End-to-end:
    - open a live session from Activity and from a chat card
    - message it, stop it, resume a finished one
    - stop an unreachable outbound task, then Stop watching
    - on phones, the full-screen sheet
  - axe scans and baselines.
  - Parity: Sessions.

### W27 — Overview store and Home (L)

- **Modules:** overview store (new); Home place (new: header, needs-you, board, Across the team, Coming up); rail Home count. The legacy Team page's overview role is deleted.
- **Preconditions:** W19; W09, W10 and W11 merged.
- **Shape when done:**
  - The overview store follows design §12, with the recovery rules in §9.
  - Home matches design §6 and the mockup:
    - centered container
    - needs-you with inline fixes clearing live
    - the board with its container-query column rule
    - the right column stacking under 1180px
    - phone cards
- **Verification:**
  - Unit tests: needs-you derivation and ordering, boot-id reset, and refetch after lag.
  - End-to-end:
    - brittle's item clears after a mock fix
    - an inbox item's Open reaches `/inbox?item=`
    - Stop watching removes an outbound item
    - a team event appears after a mock turn
  - axe scans and baselines at 1440 and 1920 wide and on the phone.

### W28 — Create agent and lifecycle on Home (M)

- **Modules:** Create agent dialog (new); board row menu; Recently deleted disclosure; delete with Undo. The legacy Team page is deleted.
- **Preconditions:** W27.
- **Shape when done:** design §6's New agent, row menu and Recently deleted, from Home, the rail "+" and the palette.
- **Verification:**
  - End-to-end:
    - create from each entry point
    - a name rejected by validation
    - start and stop from the menu
    - delete, then Undo
    - restore
  - axe scans and baselines.
  - Parity: Team page.

### W29 — Inbox place (M)

- **Modules:** inbox store (cross-agent, via the API client, errors surfaced); Inbox place (new); rail, bottom bar and app icon badges. The legacy inbox drawer is deleted.
- **Preconditions:** W19; W06, W07 and W10 merged.
- **Shape when done:**
  - Inbox matches design §7.
  - `?agent`, `?tab` and `?item` work.
  - Counts come from the overview.
- **Verification:**
  - End-to-end:
    - a stopped agent's item and attachment
    - opening marks it read and the badge drops
    - archive and restore
    - the agent filter
    - a deep link
    - a mocked failure
  - axe scans and baselines.

### W30 — Schedule place (S)

- **Modules:** Schedule place (new); scheduled store. The legacy Scheduled page is deleted.
- **Preconditions:** W20, W21.
- **Shape when done:** Schedule meets design §5 and parity.
- **Verification:**
  - End-to-end: toggle a pulse, cancel an action, and a failed load.
  - axe scans and baselines.
  - Parity: Scheduled.

### W31 — Files, Shared files and the file panel (L)

- **Modules:** Files and Shared files places (new); file tree; editor (shared with the panel); file history dialog; unsaved-edit guard wiring; team change subscription through the watch registry. The legacy workspace and file history modal are deleted.
- **Preconditions:** W20, W21, W17b.
- **Shape when done:**
  - Both places meet design §5 and parity.
  - `panel=file:` uses the same editor, including the missing-file state.
  - The team tree updates live.
  - The guard covers every exit.
- **Verification:**
  - End-to-end:
    - edit and save
    - a diagnostic
    - rename
    - delete and Undo
    - restore from history
    - a mocked conflict
    - the guard on place change, panel close and Back
    - the phone editor
  - axe scans and baselines.
  - Parity: Workspace.

### W32a — Workbench launcher (M)

- **Modules:** Workbench place (new): list, artifact detail row, Open and Copy link, sessions per artifact from the overview, and the session panel on Workbench routes. It refreshes on hub artifact events. The legacy in-app artifact view, its bar and activity panel, the bridge, and their styles are deleted.
- **Preconditions:** W32b, W26, W27; W11b merged.
- **Shape when done:**
  - Design §5's Workbench: the launcher list, detail, origin rule (#308), and banner.
  - The redirects for `/team/workbench/:artifact` and `?full`.
  - Nothing embeds an artifact anywhere in the app.
- **Verification:**
  - End-to-end on both projects:
    - the list and detail
    - Open producing the artifacts-origin URL in a new tab
    - Copy link
    - a mock edit making the row glow with no agent running
    - an artifact's running sessions listed and opening in the panel
    - delete then Undo
    - an unknown name redirecting
    - the banner for the HTTPS-without-relay case
  - axe scans and baselines.
  - Parity: Workbench.

### W32b — SDK direct access (M)

- **Modules:**
  - the SDK
  - the mock artifacts listener
  - the bundled workbench skill and its API reference
  - `workbench.md`
- **Preconditions:** W02b; W11b, W12b and W12c merged.
- **Shape when done:**
  - Design §9.10 in full: every SDK member's behavior, `agent(name)`, no implicit agent, live reload, removed host messages, and the rewritten docs.
- **Verification:**
  - End-to-end, opening artifact URLs directly on the mock artifacts origin:
    - `fetch` to hub, team and named-agent paths
    - an unscoped agent path answering 400 with the guidance error
    - `agent(name).on` receiving that agent's turn frames
    - `watch` on a team prefix
    - `agent(name).watch` on an agent prefix
    - a top-level `watch` of a non-team prefix throwing
    - `sessions.start` on an agent with no open socket in the page, receiving its frames from the first one (#292)
    - `sessions.start` rejecting after 10 seconds when the hub socket is refused
    - a top-level `on` of an agent frame type throwing
    - request lanes capping at 8 and 4, and the overloaded-503 retry
    - `resync` after a simulated lag
    - live reload
    - a page-registered `artifact_updated` handler suppressing the reload
    - a block-list route answering 403
  - Unit tests for unscoped-path mapping and the guidance errors.

### W33 — Settings model (M)

- **Modules:** the settings model per scope and file: baselines, staged changes, the field-to-key-path map, save order, secret storing, per-scope save-bar state, multi-file Undo. Logic only.
- **Preconditions:** W18.
- **Shape when done:**
  - Design §8's save rules:
    - staged versus immediate
    - per-scope files
    - save order
    - diagnostics mapped to fields
    - partial failures
    - raw versus form locking
    - scope isolation
    - Undo across every returned checkpoint
  - Every legacy form field is in the key-path map.
- **Verification:** unit tests:
  - a diff per file
  - staged state kept across scope switches
  - Discard
  - save order
  - a partial failure
  - a mapped diagnostic
  - no cross-scope writes
  - a multi-file Undo with one skipped path
  - every legacy field mapped

### W34 — Settings modal frame (M)

- **Modules:** Settings modal (new): scope picker, section list with the Advanced heading, phone list-then-section, save bar, content swap without flash, reload from disk, non-running notices.
- **Preconditions:** W33, W21.
- **Shape when done:**
  - The frame matches design §8 and the mockup.
  - Sections not yet rebuilt host their legacy content inside it.
- **Verification:**
  - End-to-end:
    - a deep link
    - a scope switch with staged changes, and back
    - Save and Discard
    - an inline validation error
    - the phone list and section with Back
    - the modal element not remounting on switches
  - axe scans and baselines.

### W35 — Settings: Model (L)

- **Modules:** agent Model section.
- **Preconditions:** W34, W23.
- **Shape when done:**
  - Model meets design §8 and parity:
    - providers with staged removal
    - main model and `models.default`
    - roles named by job
    - the subconscious role's list on open
    - failover lists preserved
  - Fix settings for a bad model lands here with the field flagged.
- **Verification:**
  - A unit test for failover preservation.
  - End-to-end: brittle's Fix settings, Save, then Restart from the state card, and it runs. Add and remove a provider.
  - axe.
  - Parity: Providers, model roles.

### W36 — Settings: Connections and Tools & skills (M)

- **Modules:** agent Connections and Tools & skills sections.
- **Preconditions:** W34.
- **Shape when done:** both sections meet design §8 and parity. Tokens are stored as secrets on Save.
- **Verification:**
  - End-to-end: connect Discord with a token, add a webhook, add a skill folder, set a web search backend.
  - axe.
  - Parity: channels, webhooks, skills and tools.

### W37 — Settings: Memory, Schedule and Runtime (M)

- **Modules:** agent Memory, Schedule and Advanced → Runtime sections.
- **Preconditions:** W34.
- **Shape when done:** every field from the legacy Runtime, Pulses & sessions and Memory sections is in its design §8 home, with plain labels and the numbers shown.
- **Verification:**
  - End-to-end: change one field per section, save, reload from disk and confirm.
  - axe.
  - Parity: Runtime, Pulses, Memory.

### W38 — Settings: Tool servers and Agent-to-agent (M)

- **Modules:** agent Tool servers (with catalog) and Agent-to-agent sections.
- **Preconditions:** W34; W06 merged.
- **Shape when done:**
  - Both sections meet parity.
  - A catalog failure shows an error with Try again.
  - Visibility is set only here.
  - A non-running agent shows the notice for status, card and reachability, and keeps the remote-agents editor.
- **Verification:**
  - End-to-end:
    - add a server from the catalog, then remove it and Discard
    - a catalog failure
    - change visibility
    - edit remote agents on a stopped agent
  - axe.
  - Parity: MCP, A2A client.

### W39 — Settings: Raw config and History (M)

- **Modules:** agent Raw config editors; the History browser (shared by both scopes).
- **Preconditions:** W34.
- **Shape when done:**
  - The raw editors behave per design §8, including read-only while the form has staged changes.
  - History meets parity. Its restore and undo go through the coordinator, so open forms refresh.
- **Verification:**
  - End-to-end:
    - edit raw config and see a diagnostic
    - raw is read-only during staged changes
    - restore a checkpoint and see the form update
    - undo a checkpoint
  - axe.
  - Parity: History.

### W40 — Settings: All agents, part 1 (M)

- **Modules:** General, Residuum Cloud, Updates, Session limits and Diagnostics sections.
- **Preconditions:** W34.
- **Shape when done:** the sections meet design §8 and parity, including the Cloud fixes.
- **Verification:**
  - End-to-end: timezone, the cloud states through the mock, an update check, and a diagnostics toggle.
  - axe.

### W41 — Settings: All agents, part 2 (M)

- **Modules:** Saved keys (agent keys and secrets), Agent-to-agent listener and caller keys, install-wide Raw config and History. The legacy settings page is deleted. The Notifications section is added as a placeholder, filled by W47.
- **Preconditions:** W38, W39, W40.
- **Shape when done:**
  - The sections meet parity.
  - No legacy settings content remains.
- **Verification:**
  - End-to-end:
    - an agent key added and removed, with Undo
    - a secret added and removed
    - a caller key created and revoked
    - the hub raw config edited
  - axe scans and baselines.
  - Parity: All-agents items.

### W42 — Setup wizard restyle (M)

- **Modules:** setup wizard and its steps.
- **Preconditions:** W14, W15.
- **Shape when done:** the six-step flow, validation, draft and completion are unchanged, rendered with the new primitives and tokens, and working at phone width.
- **Verification:**
  - End-to-end in setup mode on both projects: complete the wizard; reload mid-way and the draft restores.
  - axe scans and baselines.
  - Parity: Setup wizard.

---

## Phase 6 — PWA

W43 and W45 together. W44 after W43. W46 after W45. W47 after W44 and W46.

### W43 — Installability and code splitting (M, target: integration)

- **Modules:** manifest; document head; safe-area audit across the shell; the install capability for the registry; route-level code splitting.
- **Preconditions:** W21, W31, W32a, W34, W42; W12 merged.
- **Shape when done:**
  - The manifest, metas, install entry and secure-context hiding follow design §11.
  - The manifest link carries `crossorigin="use-credentials"`. A browser fetches the manifest without cookies otherwise, and the relay's login check redirects a request with no session cookie to the login page, so the manifest would never load through the relay.
  - The listed splits load on demand.
  - The initial-route size is reported in CI.
- **Verification:**
  - A preview-mode end-to-end spec checks the manifest fields and icons, and that the settings chunk loads on first open.
  - A spec hides install on a simulated non-secure origin.
  - Owner check: install from a phone through the tunnel.

### W44 — Service worker (M, target: integration)

- **Modules:** service worker (new); the build step for the precache list and version; registration and the update-ready flow; offline shell state.
- **Preconditions:** W43.
- **Shape when done:** the worker behaves per design §11, registering only in production builds.
- **Verification:**
  - Preview-mode end-to-end:
    - an offline reload after first load shows the shell and the hub banner
    - a rebuilt app shows "Update ready", and Reload activates it
    - `/api` requests never hit the cache
  - The relay code is read to confirm the worker and manifest pass through unmodified; the result goes in the report.
  - Owner check: the worker registers on the relay origin.

### W45 — Web Push core (L, target: main)

- **Modules:** VAPID key file; devices file; hub push endpoints; sender (encryption, retry, pruning, `last_failure`); the `[push] contact` hub setting; the agent write blocklist; notifications and hub HTTP docs and the bundled mirror; mock server.
- **Preconditions:** W02.
- **Shape when done:**
  - Design §9.7's storage, endpoints, payload encoding and delivery rules.
  - A test send works.
  - No triggers are wired yet.
  - Dependencies are checked for maintenance and current versions when pinned.
- **Verification:**
  - Rust tests, against a local fake push service:
    - encryption against the Web Push standard's test vectors
    - upsert by endpoint
    - pruning on 410
    - a single retry on 503
    - `last_failure` recorded
    - agents blocked from the key file
  - Owner check: a test push reaches a real browser.

### W46 — Web Push triggers (M, target: main)

- **Modules:** trigger wiring from `user_inbox_added`, agent failure, the outbound threshold event and the turn hook; payload text per event; preference filtering; device presence on the hub socket.
- **Preconditions:** W45, W08, W11.
- **Shape when done:** the four triggers, payloads and tags in design §9.7.
- **Verification:** Rust tests:
  - each trigger fires once per its rule
  - preferences filter per device
  - `reply_while_away` is suppressed while a client is connected
  - no push to a device with fresh active presence, and pushes resume after it goes stale or its socket closes
  - the badge count

### W47 — Web Push client and Notifications settings (M, target: integration)

- **Modules:** the worker's push and notification-click handling; presence reporting while the window is visible and focused; the All agents → Notifications section, including the push contact; permission flow.
- **Preconditions:** W44, W41; W46 merged.
- **Shape when done:** the Notifications section and push handling behave per design §11, including last-failure display and the iOS note.
- **Verification:**
  - Preview-mode end-to-end with a mocked subscription:
    - preference toggles
    - a test through the mock
    - presence sent while focused and cleared on blur
    - a simulated push always showing a notification
    - a click navigating to its target
  - Owner check: an installed phone app receiving a new-inbox-item push.

---

## Phase 7 — Cutover (target: integration)

### W48 — Stop the hub's user-inbox notes (S)

- **Modules:** the hub's failure note and acting-agent note writers (removed); inbox and hub docs.
- **Preconditions:** W27, W29.
- **Shape when done:**
  - The hub writes nothing into user inboxes (design §9.5).
  - Failures and lifecycle outcomes appear only through the state card, needs-you, team events and push.
- **Verification:** Rust tests confirm there is no user-inbox write on an agent failure or on create, delete or restore by an agent.

### W49 — Legacy removal, guardrails and documentation (L)

- **Modules:** anything left of the legacy UI (components, stylesheets, variables, helpers); the style-lint ignore list; the accessibility suppression; the bundle budget; the web contributing guide and instructions; systems-usage pages describing the web UI, with their bundled references where they exist.
- **Preconditions:** W22–W48.
- **Shape when done:**
  - No legacy code remains, and the ignore list is empty.
  - The accessibility suppression is removed, and its warnings are fixed.
  - CI enforces a bundle budget set from the measured initial-route size.
  - Every `parity.md` item is checked, or marked Changed or Dropped.
  - Docs describe the new UI.
- **Verification:**
  - All gates and the full end-to-end, accessibility and visual suites pass.
  - A search for legacy class names and variables finds nothing.
  - `parity.md` has no unchecked items.

### W50 — Verification against the design (M)

- **Modules:** none new; verification, and fixes within an M budget.
- **Preconditions:** W49.
- **Shape when done:**
  - A written check against every section of `design.md`, with gaps fixed up to the budget. Larger gaps are listed for the orchestrator to raise with the owner.
  - One list of every owner check from earlier units, confirming the relay deploy (W12d) happened before cutover.
  - The orchestrator then opens the cutover PR, merging `feat/web-overhaul` into `main`, and moves the design documents to `docs/archive/` in it.
- **Verification:**
  - The written check.
  - The owner's checks.
  - CI green on `main` after the merge.
