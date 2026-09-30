# Web UI Overhaul — Work Units

> **Status:** draft, pending owner sign-off. Design: [`design.md`](./design.md) (terms are defined there). Capability checklist: [`parity.md`](./parity.md). Visual reference: [`mockup.html`](./mockup.html).

> Module level only. No file or line references; each unit's implementer maps those first. A unit depends only on the units in its preconditions and is verifiable on its own.

## How this document is used

One main session orchestrates the implementation. It dispatches one subagent per work unit and owns branches, PRs, merges and anything that needs the owner. The work is split by amount of work, so that no single agent runs too long. Phases group related units, but the unit is the unit of dispatch.

**What the implementing agent receives:**
- this document (its unit)
- the design, the parity checklist and the mockup
- the codebase

**What it does:**
1. Map the file-level changes.
2. If the mapping exceeds the unit's size, stop and return a proposed split instead of starting.
3. Implement.
4. Add tests at the layers below.
5. Run the gates.
6. Commit on the unit branch the orchestrator gave it.
7. Report back:
   - what changed
   - how it was verified
   - which parity items it checked
   - anything it could not do
   - anything the owner must check by hand

It does not merge, and it does not contact the owner.

**Sizes:**

| Size | Scope | Rough budget |
|---|---|---|
| S | One component family or small contract | ≈ 400 changed lines, excluding tests and snapshots |
| M | One surface or one backend feature | ≈ 1,200 lines |
| L | The most one agent takes on | ≈ 2,000 lines |

**Targets:**
- **main:** branch from `main`, PR into `main`.
- **integration:** branch from `feat/web-overhaul`, PR into it.

`feat/web-overhaul` is created from `main` after Phase 1 merges. It merges `main` after every backend unit lands. A precondition written as "W05 merged" on an integration unit means that merge has happened.

**Parallelism.** Units whose preconditions are met and whose modules don't overlap may run at once, in separate worktrees. Each phase lists the groups that can overlap.

**Done, for every unit:**
- The pre-commit gates pass.
  - Web: format, lint, type check, unit and component tests.
  - Rust: fmt, clippy, tests, `cargo deny`.
- Behavior the unit adds or changes has unit or component tests.
- Frontend units from W03 on also cover user-facing flows with end-to-end specs on both projects, scan every place or overlay they touch with axe, and add visual baselines for new surfaces.
- The mock server implements every endpoint and frame the unit adds or changes. Its route-parity test passes.
- Legacy components and styles the unit replaces are deleted, and removed from the style-lint ignore list.
- The unit's `parity.md` items are checked, in the same PR.
- Docs describing behavior the unit changed are updated in the same PR. Backend units update the matching `docs/systems-usage/` pages, and the bundled `residuum-system` reference where one exists for that page.

**Owner checks** that no agent can perform (a real phone, the relay, a real push service) are listed in the unit's report and collected in W45.

---

## Phase 1 — Harness and guardrails (target: main)

W01 and W04 can run together. Then W02 follows W01, and W03 follows W02.

### W01 — Mock server as typed modules (L)

- **Modules:** mock server; web type-check, lint and format configuration.
- **Preconditions:** none.
- **Shape when done:**
  - The mock server is split into modules by area: hub, agent chat, sessions, config, files and checkpoints, workbench, inbox, scheduled, test controls.
  - It is type-checked, linted and formatted with the app's rules.
  - Response shapes use the generated protocol types wherever a generated type exists.
  - Behavior is unchanged.
- **Verification:**
  - The type check and lint cover the mock.
  - Today's manual scenarios behave the same under `just web-mock`: the chat triggers `spawn`, `drop` and `busy`, the test-control endpoints, and setup mode.
  - Existing unit and component tests pass.

### W02 — Mock determinism, preview mode and route parity (M)

- **Modules:** mock server; justfile; pre-commit hook.
- **Preconditions:** W01.
- **Shape when done:**
  - **Deterministic mode:**
    - a fixed clock
    - artificial delays configurable, and zero in this mode
    - a stable scenario matching today's (atlas and scout running, drifter stopped, brittle failed, with the existing sessions, outbound tasks, inbox and artifacts)
  - A test-control reset endpoint restores the initial scenario.
  - **Preview mode** serves a production build together with the mock.
  - Both modes start headlessly on a given port.
  - Every endpoint the API client calls is implemented, including those that answer 404 today (the Scheduled view's).
  - **Route-parity test.** A unit test calls every API client function with sample arguments against a recording fetch, and checks each recorded method and path against the mock's exported route table.
  - The pre-commit web gate triggers on any change under the web app directory.
- **Verification:**
  - The route-parity test passes, and fails when a mock route is removed.
  - Two identical interaction sequences separated by a reset return identical responses.
  - Preview mode serves the built app and the mock API together.

### W03 — End-to-end, accessibility and visual harness (M)

- **Modules:** Playwright configuration and helpers; justfile; CI quality workflow; web contributing guide (testing section).
- **Preconditions:** W02.
- **Shape when done:**
  - **Projects:** Chromium desktop 1440×900 and Chromium phone 390×844 (touch, mobile user agent), plus a local-only WebKit phone project.
  - **Servers:** the mock in deterministic dev mode by default, and in preview mode for specs tagged as needing a production build. It is reset before each test.
  - **Helpers:**
    - an axe scan that fails on serious and critical violations
    - screenshots with frozen time and animation and masked dynamic regions
    - traces on failure
  - **Visual comparisons** run inside the official Playwright container image, pinned to the installed Playwright version, both locally (through the `just` recipes) and in CI.
  - **Smoke specs** cover the current UI: open an agent's chat, send a message and see the reply, open settings, open the team page. They make no accessibility assertions, because those screens are replaced later.
  - **Recipes:** `just web-e2e` runs the suite; `just web-e2e-update` refreshes baselines in the container.
  - **CI** runs the suite in the web job after unit tests, and uploads the report and traces on failure.
  - The contributing guide documents the layers and when to use each.
- **Verification:**
  - The suite passes locally and in CI on both projects.
  - A deliberately broken selector fails with a trace artifact.
  - A sample baseline created in the container matches when compared in CI.

### W04 — Lint and CI guardrails (M)

- **Modules:** style lint (replacing the grep-based CSS check); ESLint configuration and the rune store modules; svelte-check invocation; CI workflows; package manifest; coverage configuration.
- **Preconditions:** none.
- **Shape when done:**
  - **Style lint.** A style linter checks global stylesheets and component style blocks. Outside a designated token file it forbids:
    - literal colors
    - raw font sizes
    - raw z-index values
    - literal durations and easing curves

    Every existing stylesheet and component is on an ignore list.
  - Rune store modules get the strict TypeScript rules other modules have, and their existing violations are fixed.
  - `svelte-check` fails on warnings. The one existing global accessibility suppression remains.
  - The Rust CI job regenerates the TypeScript types and fails on any difference.
  - Coverage is reported in the CI summary, with no threshold.
  - The package manifest declares the supported Node version, matching the contributing guide.
- **Verification:**
  - Lint passes on the current code.
  - A literal color in a new component fails.
  - Changing an exported Rust type without regenerating fails the CI types step.
  - Coverage appears in the summary.

---

## Phase 2 — Backend contracts (target: main)

Each unit implements its part of design §9. Each also updates the mock server, the generated types, and the hub HTTP and related systems-usage docs.

Once W02 has merged, W05, W06, W07, W08 and W12 can run together. Then:
- W09 follows W05 and W08.
- W10 follows W07 and W08.
- W11 follows W10.

### W05 — Hub snapshot, last-error detail and hub frames (M)

- **Modules:**
  - hub events and WebSocket
  - agents listing
  - activity tracker
  - agent start path and last-error recording
  - hub config reload
  - TypeScript export
  - web hub types (switched to generated)
- **Preconditions:** W02.
- **Shape when done:** design §9.1 in full:
  - activity and stopping in the snapshot and listing
  - `busy_since`
  - `AgentLastError.reason` and `file`, carried structurally from the start path
  - `hub_config_reloaded` and `hub_boot` frames
  - exported hub frame types, including `HubClientMessage` and `agent_stopping`
- **Verification:**
  - Rust tests:
    - snapshot activity while a turn runs and after
    - the stopping set during a stop
    - `reason` and `file` for a bad `providers.toml` model, a malformed `config.toml`, and a non-file failure
    - `hub_config_reloaded` after an edit
    - `hub_boot` first on connect
  - Type drift is clean.

### W06 — File-only routes for non-running agents (S)

- **Modules:** agent route dispatch; repair and running routers.
- **Preconditions:** W02.
- **Shape when done:** the routes in design §9.2 answer for stopped and failed agents with unchanged shapes; `status` stays running-only.
- **Verification:**
  - Rust tests for each route on a stopped agent.
  - The dispatch tests are updated.

### W07 — Cross-agent inbox (M)

- **Modules:**
  - hub inbox endpoints
  - inbox storage, including unique item ids
  - the hub's failure and lifecycle note writers (removed)
  - a shared local-to-UTC timestamp conversion
- **Preconditions:** W02.
- **Shape when done:**
  - The endpoints and shapes in design §9.5.
  - Timestamps follow the conversion rule in §9.
  - Saved items get unique ids when their stem already exists (closes #305); existing ids keep working.
  - The hub no longer writes "<agent> failed" or the lifecycle notes into user inboxes.
- **Verification:** Rust tests:
  - listing across a running and a stopped agent
  - paging
  - read, archive and restore round trips
  - DST-edge conversions
  - JSON errors for an unknown agent or item
  - two same-title items on one day both surviving
  - no inbox write on an agent failure

### W08 — Per-agent watcher and reply hook (M)

- **Modules:** agent control handle (adds a bus subscription handle); hub per-agent watcher (new); activity tracker reply hook.
- **Preconditions:** W02.
- **Shape when done:**
  - When an agent starts, the hub attaches a watcher that subscribes to the topics listed in design §9. It drains them continuously and emits a typed internal hub stream of agent changes:
    - session started, changed and completed
    - outbound task changed
    - watched paths changed
    - resync
  - The watcher stops with the agent.
  - The reply hook carries each reply's text and time.
  - Nothing is exposed over HTTP yet.
- **Verification:**
  - Rust tests drive a running test agent and assert the internal stream receives each change kind, a resync after a lossy overflow, and no events after stop.
  - A test checks that the watcher drains while nothing consumes its output.

### W09 — Team event log (M)

- **Modules:** hub event log (new); hub runtime (lifecycle, notices, hub start); consumers of the watcher stream and reply hook; hub HTTP and WebSocket.
- **Preconditions:** W05, W08.
- **Shape when done:** design §9.4 in full:
  - kinds, levels, targets
  - level-aware eviction
  - boot id
  - endpoint with `before` and `after`
  - `team_event` frames
- **Verification:** Rust tests:
  - one event per kind from its trigger
  - eviction order, keeping warn and error entries
  - paging both ways
  - boot id stable within a run
  - frames on the socket

### W10 — Overview: last message, sessions, inbox counts (L)

- **Modules:** hub overview (new); watcher stream and reply hook consumers; disk readers for stopped agents; hub HTTP and WebSocket.
- **Preconditions:** W07, W08.
- **Shape when done:**
  - `GET /api/hub/overview` and `agent_overview` frames per design §9.3 for `last_message`, `live_sessions` and `inbox_unread`.
  - `upcoming` and `outbound_problems` are present but empty until W11.
  - Coalescing is trailing-edge, at most one frame per agent per second.
  - Changes through the hub inbox endpoints update counts.
- **Verification:** Rust tests:
  - `last_message` for a running agent after a turn, for a stopped agent from recent history, and from an episode with day precision
  - preview stripping and truncation
  - live sessions appearing and clearing
  - an inbox count after a tool write and after a hub read
  - coalescing under a burst, with the final state delivered

### W11 — Overview: upcoming runs and outbound problems (M)

- **Modules:** pulse next-run calculation (shared with the per-agent scheduled endpoint); scheduled actions reader; outbound task reader; hub overview.
- **Preconditions:** W10.
- **Shape when done:**
  - `upcoming` and `outbound_problems` per design §9.3.
  - The per-agent pulses endpoint's `next_fire_at` uses the same calculation, respecting active hours and `pulse_enabled`.
- **Verification:** Rust tests:
  - a next run pushed past a quiet-hours window
  - a never-run pulse
  - `pulse_enabled` off
  - actions ordering
  - an unreachable task appearing and clearing live
  - a stopped agent's frozen values

### W12 — Asset caching and compression (S)

- **Modules:** embedded asset handler; HTTP middleware.
- **Preconditions:** W02.
- **Shape when done:**
  - The headers and compression in design §9.6.
  - The relay project's HTTP forwarding is read to confirm it passes `Cache-Control`, `ETag` and `Content-Encoding` through. The result goes in the unit report.
- **Verification:** Rust tests:
  - headers per asset class
  - 304 on a matching `If-None-Match`
  - compression negotiation
  - SPA fallback for a client route

---

## Phase 3 — Foundations (target: integration)

W13, W16 and W17 can run together. Then:
- W14 follows W13.
- W15 follows W13 and W17.
- W18 follows W16, once W05 is merged.

### W13 — Tokens, fonts, base styles and icons (M)

- **Modules:** token set (new names, no collision with legacy variables); scoped base styles; bundled fonts; icon component; style-lint configuration; web aesthetic guide.
- **Preconditions:** Phase 1.
- **Shape when done:**
  - The token set in design §1, including the declared allowed text and surface pairs.
  - Fonts are bundled and the font CDN import is removed. Legacy styles fall back to the bundled or system faces until they're deleted.
  - Global reduced-motion handling.
  - One icon component covers every icon the mockup uses.
  - The token file is the linter's only exemption.
  - The aesthetic guide is rewritten to describe this visual system.
- **Verification:**
  - A unit test checks every allowed pair against AA.
  - An end-to-end check that no request goes to a font CDN.
  - Lint passes.

### W14 — Primitives: controls (L)

- **Modules:** Button, IconButton, fields (text, number, select, toggle, segmented, secret), Badge, status dot, Disclosure, Tabs, EmptyState, Skeleton, Banner, Kbd; a development-only gallery route.
- **Preconditions:** W13.
- **Shape when done:**
  - Each primitive has the variants the design and mockup use.
  - Each has keyboard behavior, labels, disabled and error states, and 44px touch targets at phone width.
  - The gallery renders every primitive and state, in development and mock builds only.
- **Verification:**
  - Component tests per primitive.
  - An axe scan of the gallery.
  - Gallery baselines at both sizes.

### W15 — Primitives: overlays, toasts and notification history (L)

- **Modules:** Menu, Popover, Tooltip, Dialog, Sheet, Drawer; the overlay stack (focus, `inert`, scroll lock, stacking, overlay entries through the router); toast region; Recent notifications dialog.
- **Preconditions:** W13, W17.
- **Shape when done:**
  - Overlays follow design §1's model. Modal overlays push and pop overlay entries per design §3.
  - A Dialog can go full-screen at phone width.
  - Sheets and Drawers dismiss by swipe.
  - The toast region renders the existing toast store with today's timings and actions, and is announced to assistive technology.
  - The Recent notifications dialog shows the existing notification history, including details, clear and Undo.
  - The gallery gains overlay demos.
- **Verification:**
  - Component tests: focus trap and restore, Esc, nested stacking, toast timing.
  - End-to-end on both projects: open each overlay, and Back closes it.
  - axe scans and baselines.

### W16 — API client agent scoping (M)

- **Modules:** API client; cache keys; every call site; the module-level current-agent state (reduced to routing's viewed agent and the socket coordinator's bound agent).
- **Preconditions:** Phase 1.
- **Shape when done:**
  - Every agent-scoped API function takes the agent explicitly.
  - Cache keys include the agent.
  - No request reads a module-level current agent.
  - Behavior is otherwise unchanged.
- **Verification:**
  - Unit tests of the recorded requests for two different agents.
  - The route-parity test passes.
  - Smoke specs pass.

### W17 — Route model, router and section registry (L)

- **Modules:**
  - route parsing and formatting
  - router: push and replace, close semantics, overlay entries, the unsaved-edit guard
  - settings section registry: ids, scopes, labels, groups, and the old-to-new and new-to-old mappings
  - the sessions store's router dependency (removed)
  - an adapter for the current app
- **Preconditions:** Phase 1.
- **Shape when done:**
  - Design §3 is implemented: places, the `panel`, `settings` and inbox-item parameters, corrections, every redirect, history rules, overlay entries and the guard.
  - The section registry covers every section in design §8, both scopes.
  - Until W19 lands, the existing app runs on the new router through the adapter, which also maps new section ids back to legacy sections so they can be hosted.
  - No store imports the router.
- **Verification:**
  - Unit tests: every route, every redirect row, every correction, push versus replace for each navigation kind, close via back versus replace, and the guard on navigation and on `popstate`.
  - Smoke specs pass.

### W18 — Config write coordinator (M)

- **Modules:** config write coordinator (new, replacing the lock); the composer's model and thinking controls, and the legacy settings save path, switched to it.
- **Preconditions:** W16; W05 merged.
- **Shape when done:**
  - Every config write goes through the coordinator, per design §8:
    - serialized per file
    - pre-save re-read with overlap detection and the keep-or-use choice
    - subscribers notified after write, reload and restore
    - external changes picked up from config-file `workspace_changed` frames and `hub_config_reloaded`
  - The composer controls and footer model label refresh when settings change.
- **Verification:**
  - Unit tests: concurrent writes serialize; a non-overlapping external change is preserved; an overlapping one prompts; subscribers fire on each trigger.
  - End-to-end: change the main model in settings, and the composer control updates without a reload.

---

## Phase 4 — Shell (target: integration)

### W19 — App shell (L)

- **Modules:** application root and layout; rail (new); phone bottom bar and drawer (new); context panel host (new); overlay host; hub banner; legacy views hosted in places; legacy layout rules adjusted for hosting. The header, hamburger menu, agent chip row, sessions-sidebar toggle and notification corner are deleted.
- **Preconditions:** W14, W15, W16, W17; W05 merged.
- **Shape when done:**
  - The shell matches design §2 and the mockup at all three widths:
    - rail rows and counts: Home and Inbox counts come from the hub store until W25 and W27; agent chat-unread comes from the snapshot
    - the accordion rules
    - the footer's gear and help menu, including Recent notifications
    - the phone bottom bar and drawer
    - the context panel's three behaviors
    - the hub banner
  - Every place routes. Places not yet rebuilt host their legacy view:
    - Chat hosts the current chat.
    - Activity hosts the sessions list.
    - Schedule hosts the Scheduled page.
    - Files and Shared files host the workspace.
    - Home hosts the Team page.
    - Inbox hosts the inbox drawer's content.
    - The Settings modal hosts the current settings page, through the registry's mapping.
    - The Workbench hosts itself.
- **Verification:**
  - End-to-end on both projects:
    - every place from the rail, drawer and bottom bar
    - the accordion
    - back and forward
    - each redirect row
    - the panel opening and closing
    - the hub banner via mock test controls
  - axe and baselines for the shell.

### W20 — Action registry, palette, shortcuts and feedback (M)

- **Modules:** action registry (new); command palette (new); Keyboard shortcuts dialog; feedback dialog on Dialog; the composer's `/` menu switched to the registry.
- **Preconditions:** W19.
- **Shape when done:**
  - The registry holds:
    - every place and agent
    - live sessions
    - every settings section from the section registry
    - chat actions: the former slash commands with plain labels and their old names as search terms
    - lifecycle actions
    - create agent
    - feedback and bug report
    - shortcuts
    - Recent notifications
    - Install app (hidden until W40 provides the capability)
  - Actions needing a running agent are disabled with a reason.
  - The palette opens by ⌘K or Ctrl+K, from the rail search row, and from the phone Search tab, full-screen.
  - The shortcuts dialog lists Esc for stopping a reply, and describes `/` accurately.
- **Verification:**
  - Unit tests for matching on old command names and for disabled reasons.
  - End-to-end: navigate by palette, run "Summarize older messages now", and use the `/` menu.
  - axe and baselines.

---

## Phase 5 — Surfaces (target: integration)

**Groups:**
- Once W19 is in: W21, W28, W29 and W30 together.
- W22 after W21, then W23. W24 after W21.
- Once W09–W11 are merged: W25, then W26. W27 once W07 and W10 are merged.
- W31 after W18; W32 after W31 and W20; then W33–W37 together; W38 after W36 and W37.
- W39 after W14 and W15.

### W21 — Chat feed and state cards (L)

- **Modules:** chat feed and message components (shared with session transcripts); turn grouping; path links; chat header; state cards (new); history loading for non-running agents.
- **Preconditions:** W19; W05 and W06 merged.
- **Shape when done:**
  - The feed renders per design §4: unboxed replies, moss bubbles, message cards, dividers, the plain-language compressed marker, attachments, copy on code blocks, and path links.
  - Turns are grouped per design §4. Tool steps appear in a plain placeholder until W22.
  - Every Chat feed parity item is kept, with one empty state.
  - Display state follows design §4, including stopping.
  - Failed, stopped, starting and stopping agents show their state card. The past conversation stays readable.
    - Restart and Start work, and their failures show in the card.
    - The settings link follows `last_error.file`. Until W33 it opens the hosted legacy settings at the mapped section.
  - No reconnecting text appears for a non-running agent.
  - Open session appears for session senders only.
- **Verification:**
  - End-to-end on brittle, drifter and atlas: state cards, restart failure, the settings link target, lazy loading, jump to latest, and path links opening the panel.
  - Component tests per message type.
  - axe and baselines.
  - Parity: Chat feed.

### W22 — Activity line and live turn (L)

- **Modules:**
  - activity line (new: summary, steps, details)
  - friendly label table
  - feed store per-turn aggregation, timing and failure state
  - `set_verbose` on connect
  - the `/verbose` command and the legacy tool group and thinking indicator (deleted)
- **Preconditions:** W21.
- **Shape when done:**
  - Each turn with tool calls has an activity line from live frames or history, per design §4.
  - Live turns show steps, the timer, Stop and Esc, intermediate texts, and collapse on end.
- **Verification:**
  - Unit tests for labels and summary phrasing: repeats merged, a failed live step, an MCP fallback.
  - Component tests for the three levels.
  - End-to-end on both projects: a live turn appearing, then collapsing; Stop mid-turn; a historical turn's line.
  - axe and baselines.

### W23 — Composer and conversation size (M)

- **Modules:** composer (attach, `/` actions, model and thinking popover, send and stop, per-agent drafts); post-turn status line; conversation-size panel view. The legacy chat footer is deleted.
- **Preconditions:** W22, W18, W20.
- **Shape when done:**
  - The composer meets parity, with drafts kept per agent.
  - The model and thinking popover writes through the coordinator, and is a sheet on phones.
  - The post-turn status and conversation-size view follow design §4.
- **Verification:**
  - End-to-end: attach images, a draft surviving navigation and reload, changing the model from the popover, the offline queue notice, and conversation size in the panel.
  - axe and baselines.
  - Parity: Composer, Chat footer.

### W24 — Activity place and session panel (M)

- **Modules:** Activity place (new); session run panel content; outbound task rows; sessions store view state. The legacy sessions sidebar, row and session page are deleted.
- **Preconditions:** W21.
- **Shape when done:**
  - Activity and the session panel meet design §5 and parity.
  - The chat header's running pill opens Activity.
- **Verification:**
  - End-to-end:
    - open a live session from Activity and from a chat card
    - message it, stop it, resume a finished one
    - stop an unreachable outbound task, then Stop watching
    - on phones, check the full-screen sheet
  - axe and baselines.
  - Parity: Sessions.

### W25 — Overview store and Home (L)

- **Modules:** overview store (new); Home place (new: header, needs-you, board, Across the team, Coming up); rail Home count. The legacy Team page's overview role is deleted.
- **Preconditions:** W19; W09, W10 and W11 merged.
- **Shape when done:**
  - The overview store follows design §12, with the recovery rules in §9.
  - Home matches design §6 and the mockup: centered container, needs-you with inline fixes clearing live, the board with its container-query column rule, the right column stacking under 1180px, and phone cards.
- **Verification:**
  - Unit tests: needs-you derivation and ordering; boot-id reset; refetch after lag.
  - End-to-end:
    - brittle's item clears after a mock fix
    - an inbox item's Open reaches `/inbox?item=`
    - Stop watching removes an outbound item
    - a team event appears after a mock turn
  - axe and baselines at 1440 and 1920 wide and on the phone.

### W26 — Create agent and lifecycle on Home (M)

- **Modules:** Create agent dialog (new); board row menu; Recently deleted disclosure; delete with Undo. The legacy Team page is deleted.
- **Preconditions:** W25.
- **Shape when done:** design §6's New agent, row menu and Recently deleted, opened from Home, the rail "+" and the palette.
- **Verification:**
  - End-to-end:
    - create from each entry point
    - a name rejected by validation
    - start and stop from the menu
    - delete then Undo
    - restore
  - axe and baselines.
  - Parity: Team page.

### W27 — Inbox place (M)

- **Modules:** inbox store (cross-agent, via the API client, errors surfaced); Inbox place (new); rail, bottom bar and app icon badges. The legacy inbox drawer is deleted.
- **Preconditions:** W19; W07 and W10 merged.
- **Shape when done:**
  - Inbox matches design §7.
  - `?agent`, `?tab` and `?item` work.
  - Counts come from the overview.
- **Verification:**
  - End-to-end:
    - a stopped agent's item appears
    - opening marks it read and the badge drops
    - archive and restore
    - the agent filter
    - a deep-linked item
    - a mocked failure showing the error
  - axe and baselines.

### W28 — Schedule place (S)

- **Modules:** Schedule place (new); scheduled store. The legacy Scheduled page is deleted.
- **Preconditions:** W19.
- **Shape when done:** Schedule meets design §5 and parity.
- **Verification:**
  - End-to-end: toggle a pulse, cancel an action, and a failed load.
  - axe and baselines.
  - Parity: Scheduled.

### W29 — Files, Shared files and the file panel (L)

- **Modules:** Files and Shared files places (new); file tree; editor (shared with the panel); file history dialog; unsaved-edit guard wiring; team change subscription. The legacy workspace and file history modal are deleted.
- **Preconditions:** W19.
- **Shape when done:**
  - Both places meet design §5 and parity.
  - `panel=file:` opens the same editor.
  - The team tree updates live.
  - The guard covers every exit listed in §5.
- **Verification:**
  - End-to-end:
    - edit and save
    - a validation diagnostic
    - rename
    - delete and Undo
    - restore from history
    - a mocked conflict
    - the guard on place change, panel close and Back
    - the phone editor
  - axe and baselines.
  - Parity: Workspace.

### W30 — Workbench place and artifact host (M)

- **Modules:** Workbench list and artifact host views. The bridge is unchanged.
- **Preconditions:** W19.
- **Shape when done:** the list and artifact view meet parity on the new primitives. Full view hides the whole shell, including the bottom bar.
- **Verification:**
  - End-to-end:
    - open Tip Splitter and start a background session from it
    - cancel calls
    - full view in and out
    - delete and Undo from the list
  - axe and baselines.
  - Parity: Workbench.

### W31 — Settings model (M)

- **Modules:** the settings model split per scope and file: baselines, staged changes, the field-to-key-path map, save order, secret storing, and per-scope save-bar state. It is logic only.
- **Preconditions:** W18.
- **Shape when done:**
  - The model implements design §8's save rules:
    - staged versus immediate
    - per-scope files
    - save order
    - inline diagnostics mapping
    - partial-failure reporting
    - raw-versus-form locking
    - scope isolation
  - Every legacy form field has an entry in the key-path map.
- **Verification:** unit tests:
  - a diff per file
  - staged state kept across scope switches
  - Discard
  - the save order
  - a partial failure
  - a diagnostic mapped to its field
  - no cross-scope writes
  - every legacy field mapped

### W32 — Settings modal frame (M)

- **Modules:** Settings modal (new): scope picker, section list with the Advanced heading, phone list-then-section, save bar, content swap without flash, reload from disk, non-running notices.
- **Preconditions:** W31, W20.
- **Shape when done:**
  - The frame matches design §8 and the mockup.
  - Sections not yet rebuilt host their legacy content inside the frame.
- **Verification:**
  - End-to-end:
    - a deep link to a section
    - scope switch with staged changes, and back
    - Save and Discard
    - an inline validation error from the mock
    - the phone list and section with Back
    - no remount on switch (the modal element is unchanged)
  - axe and baselines.

### W33 — Settings: Model (L)

- **Modules:** agent Model section.
- **Preconditions:** W32; W21 (for the fix path).
- **Shape when done:**
  - Model meets design §8 and parity:
    - providers with staged removal
    - main model
    - `models.default`
    - roles named by job
    - the subconscious role's list loading on open
    - failover lists preserved through a save
  - A failed agent's link lands here with the bad model flagged.
- **Verification:**
  - A unit test for failover preservation.
  - End-to-end: the brittle fix path, where Save, then Restart from the state card, brings it up; add and remove a provider.
  - axe.
  - Parity: Providers, model roles.

### W34 — Settings: Connections and Tools & skills (M)

- **Modules:** agent Connections (Discord, Telegram, Teams, webhooks) and Tools & skills sections.
- **Preconditions:** W32.
- **Shape when done:** both sections meet design §8 and parity. Tokens are stored as secrets on Save.
- **Verification:**
  - End-to-end: connect Discord with a token, add a webhook, add a skill folder, set a web search backend.
  - axe.
  - Parity: channels, webhooks, skills and tools.

### W35 — Settings: Memory, Schedule and Runtime (M)

- **Modules:** agent Memory, Schedule and Advanced → Runtime sections.
- **Preconditions:** W32.
- **Shape when done:** every field from the legacy Runtime, Pulses & sessions and Memory sections is in its design §8 home, with plain labels and the numbers shown.
- **Verification:**
  - End-to-end: change one field per section, save, reload from disk and confirm.
  - axe.
  - Parity: Runtime, Pulses, Memory.

### W36 — Settings: agent Advanced (L)

- **Modules:** Tool servers (with catalog), agent Agent-to-agent, Raw config editors, and the History browser (shared by both scopes).
- **Preconditions:** W32; W06 merged.
- **Shape when done:**
  - The sections meet parity.
  - A catalog fetch failure shows an error with Try again.
  - Visibility is set only here.
  - A non-running agent shows the notice for status, card and reachability, and keeps the remote-agents editor.
  - History restore and undo refresh open forms through the coordinator.
- **Verification:**
  - End-to-end:
    - add an MCP server from the catalog, then remove and Discard
    - edit raw config and see a diagnostic
    - raw is read-only while the form has staged changes
    - restore a checkpoint and see the form update
    - change visibility
  - axe.
  - Parity: MCP, A2A client, History.

### W37 — Settings: All agents, part 1 (M)

- **Modules:** General, Residuum Cloud, Updates, Session limits and Diagnostics sections.
- **Preconditions:** W32.
- **Shape when done:** the sections meet design §8 and parity, including the Cloud fixes.
- **Verification:**
  - End-to-end: change the timezone; the cloud states through the mock; an update check; a diagnostics toggle.
  - axe.

### W38 — Settings: All agents, part 2 (M)

- **Modules:** Saved keys (agent keys and secrets), Agent-to-agent listener and caller keys, install-wide Raw config and History. The legacy settings page is deleted, and the Notifications section added as a placeholder until W43.
- **Preconditions:** W36, W37.
- **Shape when done:**
  - The sections meet parity.
  - No legacy settings content remains.
- **Verification:**
  - End-to-end:
    - add and remove an agent key, with Undo
    - add and remove a secret
    - create and revoke a caller key
    - edit the hub raw config
  - axe and baselines.
  - Parity: All-agents items.

### W39 — Setup wizard restyle (M)

- **Modules:** setup wizard and its steps.
- **Preconditions:** W14, W15.
- **Shape when done:** the six-step flow, validation, draft and completion are unchanged, rendered with the new primitives and tokens, working at phone width.
- **Verification:**
  - End-to-end in the mock's setup mode on both projects: complete it; reload mid-way and the draft restores.
  - axe and baselines.
  - Parity: Setup wizard.

---

## Phase 6 — PWA

W40 and W42 can run together. W41 follows W40, and W43 follows W41 and W42.

### W40 — Installability and code splitting (M, target: integration)

- **Modules:** manifest; document head; safe-area audit across the shell; install capability for the registry; route-level code splitting.
- **Preconditions:** W20, W29, W30, W32, W39; W12 merged.
- **Shape when done:**
  - The manifest, metas, install entry and secure-context hiding follow design §11.
  - The splits listed there load on demand.
  - The initial-route size is reported in CI.
- **Verification:**
  - A preview-mode end-to-end spec checks the manifest's fields and icons, and that the settings chunk loads on first open.
  - A secure-context spec hides install on a non-secure origin (simulated).
  - Owner check: installing from a phone through the tunnel.

### W41 — Service worker (M, target: integration)

- **Modules:** service worker (new); build step for the precache list and version; registration and the update-ready flow; offline shell state.
- **Preconditions:** W40.
- **Shape when done:** the worker behaves per design §11, and registers only in production builds.
- **Verification:** preview-mode end-to-end:
  - after first load, offline reload shows the shell and the hub banner
  - a rebuilt app shows "Update ready", and Reload activates it
  - `/api` requests never hit the cache

  Owner check: the worker registers on the relay origin.

### W42 — Web Push backend (L, target: main)

- **Modules:**
  - VAPID key file
  - devices file
  - hub push endpoints
  - sender with Web Push encryption, retries and pruning
  - triggers from the inbox save path, agent failure, the overview's outbound data and the reply hook
  - `[push] contact` hub setting
  - agent write blocklist
  - notifications and hub HTTP docs with the bundled mirror
  - mock server
- **Preconditions:** W07, W08, W11.
- **Shape when done:**
  - Design §9.7 in full.
  - Dependencies are checked for maintenance and current versions when pinned.
- **Verification:** Rust tests:
  - encryption against the Web Push standard's test vectors
  - device upsert by endpoint
  - preferences filtering per trigger
  - the outbound trigger firing once per task
  - pruning on 410
  - a single retry on 503
  - `last_failure` recorded

  All against a local fake push service. Owner check: a test push reaching a real browser.

### W43 — Web Push client and Notifications settings (M, target: integration)

- **Modules:** the worker's push and notification-click handling; the All agents → Notifications section; permission flow.
- **Preconditions:** W41, W38; W42 merged.
- **Shape when done:** the Notifications section and click behavior per design §11, including last-failure display and the iOS note.
- **Verification:**
  - Preview-mode end-to-end with a mocked subscription: toggle preferences; send a test through the mock; a simulated notification click navigates to its target.
  - Owner check: an installed phone app receiving a new-inbox-item push.

---

## Phase 7 — Cutover (target: integration)

### W44 — Legacy removal, guardrails and documentation (L)

- **Modules:** anything left of the legacy UI (components, stylesheets, variables, helpers); style-lint ignore list; the accessibility suppression; bundle budget; web contributing guide and instructions; systems-usage pages describing the web UI, with their bundled references where they exist.
- **Preconditions:** W21–W43.
- **Shape when done:**
  - No legacy component, stylesheet or variable remains, and the ignore list is empty.
  - The accessibility suppression is removed, with resulting warnings fixed.
  - CI enforces a bundle budget set from the measured initial-route size.
  - Every `parity.md` item is checked, or marked Changed or Dropped.
  - Docs describe the new UI.
- **Verification:**
  - All gates and the full end-to-end, accessibility and visual suites pass.
  - A search for legacy class names and variables finds nothing.
  - `parity.md` has no unchecked items.

### W45 — Verification against the design (M)

- **Modules:** none new; verification, and fixes within scope.
- **Preconditions:** W44.
- **Shape when done:**
  - A written check against every section of `design.md`, in the report, with gaps fixed. Gaps that can't be fixed within scope are listed for the orchestrator to raise with the owner.
  - The report collects every owner check from earlier units into one list for the owner:
    - install through the tunnel
    - a push on a real phone
    - safe areas in standalone mode
    - the worker on the relay origin
  - The orchestrator then opens the cutover PR, merging `feat/web-overhaul` into `main`, and moves the design documents to `docs/archive/` in that PR.
- **Verification:**
  - The written check.
  - The owner's checks.
  - CI green on `main` after the merge.
