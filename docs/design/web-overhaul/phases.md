# Web UI Overhaul — Work Units

> **Status:** draft, pending owner sign-off. Design: [`design.md`](./design.md) (terms are defined there). Capability checklist: [`parity.md`](./parity.md). Visual reference: [`mockup.html`](./mockup.html).

> Module level only. No file or line references — each unit's implementer maps those first. Units are ordered so each depends only on units listed in its preconditions, and each is verifiable on its own.

## How this document is used

Implementation is orchestrated from one main session. That session dispatches one subagent per work unit and handles branches, PRs and merges. The work is split by amount of work rather than by feature, so that no single agent runs too long. Phases group related units, but the unit is the unit of dispatch.

**What an implementing agent receives:**
- this document (its unit)
- the design, the parity checklist and the mockup
- the codebase

It does not receive the design conversation. The artifacts are meant to be enough.

**What it does:**
1. Map the file-level changes for its unit.
2. If the mapping shows the unit exceeds its size, stop and return a proposed split instead of starting.
3. Implement.
4. Add tests at the layers listed below.
5. Run the gates.
6. Commit on the unit branch.
7. Report what changed, how it was verified, and any parity items or open issues it touched.

**Sizes** estimate the work, not a promise:

| Size | Scope | Rough budget |
|---|---|---|
| S | One component family or one small contract | ≈ 400 changed lines, excluding tests and snapshots |
| M | One surface or one backend feature | ≈ 1,200 lines |
| L | The most one agent takes on | ≈ 2,000 lines. Anything larger is split before dispatch. |

**Targets:**
- **main:** branch from `main`, PR into `main`.
- **integration:** branch from `feat/web-overhaul`, PR into `feat/web-overhaul`.

`feat/web-overhaul` is created from `main` once Phase 1 has merged, and it merges `main` again whenever a backend unit lands.

**Parallelism.** Units whose preconditions are met and whose modules don't overlap may run at the same time in separate worktrees. Within a phase the notes call out which units can overlap.

**Done, for every unit:**
- The pre-commit gates pass.
  - Web: format, lint, type check, unit and component tests.
  - Rust: fmt, clippy, tests, `cargo deny`.
- Behavior the unit adds or changes is covered.
  - Unit or component tests always.
  - From W02 on, end-to-end specs for user-facing flows on both the desktop and phone projects.
  - An accessibility scan of every place or overlay the unit touches.
  - Visual baselines for new surfaces.
- The mock server implements every endpoint or frame the unit adds or changes.
- Legacy components and styles the unit replaces are deleted, and removed from the style-lint ignore list.
- The unit's parity items are checked, in the same PR.
- Docs that describe behavior the unit changed are updated in the same PR. Backend units update `docs/systems-usage/` and the matching bundled `residuum-system` references.

---

## Phase 1 — Test harness and guardrails (target: main)

The current UI benefits from these too, and landing them on `main` first means backend units and the integration branch share one mock server and one harness. W01 and W03 can run in parallel; W02 follows W01.

### W01 — Mock server as a harness (M)

- **Modules:** mock server; web type-check, lint and format configuration; pre-commit hook; justfile.
- **Preconditions:** none.
- **Shape when done:**
  - The mock server is split into modules by area (hub, agent chat, sessions, config, files and checkpoints, workbench, inbox, scheduled, test controls).
  - It is type-checked, linted and formatted with the same rules as the app.
  - It has a deterministic mode:
    - a fixed clock
    - artificial delays configurable, and zero in deterministic mode
    - a stable scenario matching today's (atlas and scout running, drifter stopped, brittle failed, and the existing sessions, outbound tasks, inbox and artifacts)
  - A test-control reset endpoint restores the initial scenario.
  - Every endpoint the API client calls is implemented. Any route the UI calls that answers 404 today (for example the Scheduled view's) is fixed.
  - A unit test enumerates the API client's routes and the mock's routes, and fails on any client route the mock lacks.
  - The mock starts headlessly on a given port.
  - The pre-commit web gate triggers on any change under the web app directory, not only its source folder.
- **Verification:**
  - Type check and lint cover the mock server.
  - The route-parity test passes and fails when a mock route is removed.
  - `just web-mock` behaves as before.
  - Deterministic mode plus a reset between two runs of the same interaction gives identical responses.

### W02 — End-to-end, accessibility and visual harness (M)

- **Modules:** Playwright configuration and helpers; justfile; CI quality workflow; web contributing guide (testing section).
- **Preconditions:** W01.
- **Shape when done:**
  - Playwright projects:
    - desktop 1440×900 and phone 390×844 (touch, mobile user agent), both Chromium
    - a WebKit phone project for local runs only
  - The mock server starts as the web server, in deterministic mode, and is reset before each test.
  - Helpers exist for:
    - an axe scan that fails on serious and critical violations
    - screenshots with frozen time and animation and masked dynamic regions
    - traces on failure
  - Smoke specs cover the current UI: load an agent's chat, send a message and see the reply, open settings, open the team page. They assert no accessibility results, because legacy screens are replaced later.
  - Recipes:
    - `just web-e2e` runs the suite
    - `just web-e2e-update` updates visual baselines
  - CI runs the suite in the web job after the unit tests, and uploads the report and traces as artifacts on failure.
  - The contributing guide documents the test layers and when to use each.
- **Verification:**
  - `just web-e2e` passes locally on both projects, and CI passes.
  - A deliberately broken selector fails with a trace artifact.
  - The WebKit project runs locally, or its absence of system dependencies is reported clearly.

### W03 — Lint and CI guardrails (M)

- **Modules:** style lint (replacing the grep-based CSS check); ESLint configuration; svelte-check invocation; CI workflows; package manifest; coverage configuration.
- **Preconditions:** none.
- **Shape when done:**
  - A style linter checks global stylesheets and component `<style>` blocks.
    - Outside the token definitions it forbids literal colors, raw font sizes, raw z-index values, and literal durations or easing curves.
    - Every existing stylesheet and component is on an ignore list, which later units shrink.
  - Rune store modules get the strict TypeScript rules other modules have. Existing violations in them are fixed.
  - `svelte-check` fails on warnings. The existing global accessibility suppression remains until cutover.
  - CI regenerates the TypeScript protocol types from Rust and fails on any difference.
  - Unit and component coverage is reported in the CI summary, with no threshold.
  - The package manifest declares the supported Node version, and the contributing guide matches it.
- **Verification:**
  - Lint passes on the current code.
  - Adding a literal color to a new component fails lint.
  - Changing a Rust type without regenerating fails the CI types step.
  - Coverage appears in the CI summary.

---

## Phase 2 — Backend contracts (target: main)

Additive changes only (design §9). Each unit updates the mock server, the generated types and the hub HTTP doc and its bundled mirror. W04, W05, W06 and W08 can run in parallel; W07 follows W04 and W05. These may start as soon as W01 has merged.

### W04 — Hub snapshot activity, typed hub frames, history for stopped agents (M)

- **Modules:** hub event types and WebSocket; hub agents listing; activity tracker; TypeScript export; agent routing for repair routes; chat history endpoint; web hub types.
- **Preconditions:** W01.
- **Shape when done:**
  - `agents_snapshot` and `GET /api/hub/agents` carry each agent's `busy`, `unread` and `busy_since`.
  - Hub WebSocket frames and hub list envelopes are exported to TypeScript, including `agent_stopping`. The web client uses the generated types instead of its hand-written ones.
  - `chat/history` answers for stopped and failed agents from persisted history, with the same shape. Episodes work too.
  - `agent_stopping` is documented.
- **Verification:**
  - Tests cover snapshot activity after a busy turn, and a snapshot taken during a turn.
  - History tests for a stopped agent (recent and episode) pass.
  - Type drift check is clean.
  - Manually: stop an agent in the mock and on a real hub, and its history still loads.

### W05 — Cross-agent inbox (M)

- **Modules:** inbox storage access; hub HTTP routes; inbox write path (the `user_inbox_add` tool); overview hooks for unread counts; team event emission hook (a no-op until W06, if W06 hasn't landed); mock server; hub HTTP and inbox docs.
- **Preconditions:** W01.
- **Shape when done:**
  - Hub endpoints:
    - list active items and archived items across all agents, including stopped and failed ones, each with its agent
    - mark read, archive and restore by agent and id
    - return the unread count per agent
  - Timestamps are RFC 3339 UTC. Errors are JSON `{error}`.
  - Adding an item (by the tool) signals the hub, so counts update live.
  - The previously unused unread counter is either used or removed.
  - The per-agent inbox endpoints are unchanged.
- **Verification:**
  - Tests cover listing across a running and a stopped agent, read/archive/restore round trips, UTC timestamps, and a JSON error for an unknown agent or item.
  - The mock serves the same contract.

### W06 — Team event log (M)

- **Modules:** hub event log (new); hub runtime and host (lifecycle, notices); agent-to-hub signals for replies, sessions, inbox items and scheduled runs; hub HTTP and WebSocket; mock server; docs.
- **Preconditions:** W01.
- **Shape when done:**
  - A bounded log (500 entries, in memory) records the event kinds in design §9.4. Each entry has `id`, `at`, `agent`, `kind`, `level`, `summary` and `target`.
  - `GET /api/hub/events?before&limit` pages backward.
  - The hub WebSocket sends `team_event` for each entry.
  - Existing hub notices are recorded as events.
  - Summaries are plain-language sentences following the brand voice ("atlas finished a research session", "brittle couldn't start: …").
- **Verification:**
  - Tests cover:
    - ordering and paging
    - eviction beyond 500 entries
    - one event per kind from the relevant trigger
    - notice capture
    - frames on the socket
  - After a hub restart the log starts fresh.

### W07 — Overview contract (L)

- **Modules:** hub overview (new); per-agent sources (last message, live sessions, scheduled next run, inbox unread, outbound tasks); scheduled next-run calculation; hub HTTP and WebSocket; mock server; docs.
- **Preconditions:** W04, W05.
- **Shape when done:**
  - `GET /api/hub/overview` and `agent_overview` frames carry the fields in design §9.3 for every agent, including stopped and failed agents (from disk).
  - Frames are coalesced to at most one per agent per second.
  - The next-run calculation respects each pulse's active hours. This fixes the current estimate, which ignores them.
  - The overview's `inbox_unread` stays consistent with W05's counts.
- **Verification:**
  - Tests cover each field's source for a running and a stopped agent.
  - They cover coalescing under a burst of changes.
  - They cover a next run falling outside active hours.
  - They cover a failing outbound task appearing and clearing.
  - Manually on the mock: start a turn and a session, add an inbox item, and see `agent_overview` frames.

### W08 — Asset caching and compression (S)

- **Modules:** embedded web asset handler; HTTP middleware.
- **Preconditions:** none.
- **Shape when done:**
  - Hashed build assets are served with long-lived immutable caching.
  - `index.html`, root-level worker and manifest files are served with `no-cache`.
  - Text assets are compressed when the client accepts it.
  - SPA fallback rules are unchanged.
- **Verification:**
  - Tests cover headers per asset class, compression negotiation, and SPA fallback for a client route.

---

## Phase 3 — Foundations (target: integration)

The integration branch is created from `main` after Phase 1. W09 and W12 can run in parallel. W10 and W11 follow W09 and can run in parallel with each other. W13 can run any time in this phase.

### W09 — Tokens, fonts and base styles (M)

- **Modules:** token set (new, non-colliding names); base styles and reset; bundled fonts; icon set; style-lint configuration; web aesthetic guide.
- **Preconditions:** Phase 1.
- **Shape when done:**
  - The token set in design §1 exists: colors, type, radii, spacing, layout constants, breakpoints, z-index scale, motion, floating shadow and scrim.
  - Onest, JetBrains Mono and Cinzel are bundled and self-hosted with the needed weights. The font CDN import is removed. Legacy styles fall back to the bundled faces or system faces until they are deleted.
  - Reduced-motion handling is global.
  - One icon component serves every icon the mockup uses.
  - The style linter treats the new token file as the only place literals may appear.
  - The web aesthetic guide is rewritten to describe this visual system.
- **Verification:**
  - A contrast test checks every text-on-surface token pair against AA.
  - No network request goes to a font CDN (checked in an end-to-end spec).
  - Lint passes.

### W10 — Primitives: controls (L)

- **Modules:** UI primitives (new): Button, IconButton, fields (text, number, select, toggle, segmented, secret), Badge, status dot, Disclosure, Tabs, EmptyState, Skeleton, Banner, Kbd; a development-only primitives gallery route.
- **Preconditions:** W09.
- **Shape when done:**
  - Each primitive exists with the variants the design and mockup use.
  - Each has keyboard and screen-reader behavior, disabled and error states, and 44px touch targets at phone width.
  - The gallery route renders every primitive and state, only in development and mock builds.
- **Verification:**
  - Component tests per primitive: states, keyboard, labels.
  - The gallery is scanned by axe.
  - Visual baselines of the gallery exist at both sizes.

### W11 — Primitives: overlays and toasts (L)

- **Modules:** UI primitives (new): Menu, Popover, Tooltip, Dialog, Sheet, Drawer, overlay stack and focus management, toast region (rendering the existing toast store).
- **Preconditions:** W09.
- **Shape when done:**
  - Overlays follow the shared model in design §1:
    - focus trap and restore
    - Esc and scrim close
    - scroll lock
    - `inert` background
    - stacking of nested overlays
  - A Dialog becomes full-screen at phone width when asked.
  - Sheets and Drawers support swipe to dismiss.
  - The toast region shows toasts with the existing timing rules and actions, and is announced to assistive technology.
  - The gallery gains overlay demos.
- **Verification:**
  - Component tests for focus, Esc, nested stacking and toast timing.
  - End-to-end specs open each overlay on both projects with axe scans.
  - Visual baselines exist.

### W12 — Route model and router (M)

- **Modules:** route parsing and formatting; router; sessions store (drop its router dependency); legacy redirects; settings section mapping.
- **Preconditions:** Phase 1.
- **Shape when done:**
  - The router implements design §3: places, the `panel` and `settings` parameters, and the push and replace rules.
  - Every redirect in the design's table works, including section mapping and `/notification/<id>`.
  - No store imports the router.
  - Until W15 lands, the existing app keeps working through an adapter that maps its view state onto the new locations.
- **Verification:**
  - Unit tests for every route, every redirect, and push-versus-replace for each navigation.
  - Existing smoke specs still pass.

### W13 — Config write coordinator (S)

- **Modules:** config write coordinator (new, replacing the config lock); composer model and thinking controls (switched to it); settings page save path (switched to it).
- **Preconditions:** Phase 1.
- **Shape when done:**
  - All config writes go through the coordinator:
    - serialized per file
    - re-read after another writer's change
    - subscribers notified after each write, reload or checkpoint restore
  - The composer controls and the footer model label refresh when settings change.
- **Verification:**
  - Unit tests:
    - concurrent writes to one file serialize
    - a write after an external change re-reads
    - subscribers fire on write, reload and restore
  - End-to-end: change the main model in settings and the composer control updates without a reload.

---

## Phase 4 — Shell (target: integration)

### W14 — App shell (L)

- **Modules:** application root and layout; rail (new); phone bottom bar and drawer (new); context panel host (new); overlay host; hub banner; legacy views hosted in places. The header, hamburger menu, agent chip row and sessions-sidebar toggle are deleted.
- **Preconditions:** W10, W11, W12.
- **Shape when done:**
  - The shell matches design §2 and the mockup at all three widths. Rail:
    - Home and Inbox rows with counts: from the hub store's activity until the overview store exists; Inbox shows the current agent's count until W21
    - agent accordion with the exact expand, collapse and highlight rules
    - Team group
    - footer with the settings gear and help menu
  - Phone:
    - the bottom bar in the specified order, visible on every page, safe-area aware
    - the drawer with scrim, swipe and Esc
  - The context panel is resizable when wide, floating at medium widths, and a full-screen sheet on phones. It is driven by the `panel` parameter.
  - The hub banner appears when the hub socket is down.
  - Every place routes. Places not yet rebuilt host their legacy view:
    - Chat is the current chat.
    - Activity is the current sessions list.
    - Schedule is the Scheduled page.
    - Files is the workspace.
    - Home is the Team page.
    - Inbox is the inbox drawer's content.
    - Settings opens the current settings page inside a Dialog.
- **Verification:**
  - End-to-end on both projects:
    - navigate to every place from the rail or drawer and the bottom bar
    - accordion behaviors
    - back and forward
    - deep links from the redirect table
    - panel open and close
    - the hub-down banner via the mock's test controls
  - axe scans of the shell.
  - Shell visual baselines.

### W15 — Action registry, command palette, help and feedback dialogs (M)

- **Modules:** action registry (new); command palette (new); keyboard shortcuts dialog; feedback dialog (onto the Dialog primitive); composer slash menu (switched to the registry).
- **Preconditions:** W14.
- **Shape when done:**
  - The registry holds:
    - navigation to every place and agent
    - live sessions
    - every settings section in both scopes
    - chat actions (the former slash commands, with plain labels and their old names as search terms)
    - lifecycle actions
    - create agent
    - feedback and bug report
    - keyboard shortcuts
    - the install app action (hidden until W31 supplies the capability)
  - The palette opens with ⌘K or Ctrl+K, from the rail search row, and from the phone Search tab (full-screen).
  - The composer's `/` shows chat-scoped actions from the same registry.
  - The shortcuts dialog lists Esc for stopping a reply, and describes `/` accurately.
- **Verification:**
  - Unit tests for registry filtering and matching on old command names.
  - End-to-end: palette navigation, running "Summarize older messages now", and the `/` menu, on both projects.
  - axe scans and visual baselines.

---

## Phase 5 — Surfaces (target: integration)

Each unit replaces a legacy view hosted by W14. Parallel groups, once W14 has landed:
- W16, W18, W22, W23 and W24 together
- W19 and W21 once Phase 2's W05–W07 are merged into the integration branch, then W20 after W19
- W17 after W16
- W25, then W26, W27 and W28 together, then W29
- W30 at any point after W10 and W11

### W16 — Chat feed and state cards (L)

- **Modules:** chat feed and message components; feed item rendering shared with sessions; chat place header; state cards (new); per-agent history loading for non-running agents.
- **Preconditions:** W14; W04 merged into the integration branch.
- **Shape when done:**
  - The feed renders per design §4: unboxed replies, moss bubbles, message cards, dividers, the plain-language compressed marker, attachments, and copy on code blocks.
  - All feed behaviors in parity are kept.
  - There is one empty state.
  - Failed, stopped, starting and stopping agents show their state card instead of the composer. The past conversation stays readable above it.
    - Restart and Start work, and their failures show in the card.
    - The settings link picks its section by the error's file, per design §4.
  - No "reconnecting" text appears for a non-running agent.
  - Open session appears only for session senders.
- **Verification:**
  - End-to-end on brittle (failed), drifter (stopped) and atlas (running):
    - state cards and restart failure
    - the fix path via the settings link once W26 exists (until then the link opens the legacy settings dialog at the mapped section)
    - lazy loading
    - jump to latest
  - Component tests for each message type.
  - axe and visual baselines.
  - Parity: Chat feed items.

### W17 — Activity line, live turn and composer (L)

- **Modules:** activity line (new: summary, steps, details); friendly tool labels; live turn state; composer (attach, actions menu, model and thinking popover, send and stop, per-agent drafts); post-turn status; conversation-size panel view. The verbose command and the legacy tool group, footer and thinking indicator are deleted.
- **Preconditions:** W16, W13, W15.
- **Shape when done:**
  - The client requests tool frames on every connect.
  - Every reply with tool calls has an activity line built from live frames or history, as in design §4.
    - Labels come from one table covering the built-in tools, with a "Used <tool>" fallback.
    - Durations appear for turns seen live.
  - The live turn behaves as specified: steps, timer, Stop, Esc, intermediate responses, collapse on end.
  - The composer meets parity, with drafts per agent.
  - The model and thinking popover goes through the coordinator, and is a sheet on phones.
  - The conversation-size view opens in the panel.
- **Verification:**
  - Unit tests for the label table and summary phrasing, including a failed step.
  - Component tests for the line's three levels.
  - End-to-end on both projects:
    - a live turn (steps appear, then collapse)
    - Stop mid-turn
    - image attach
    - a draft surviving navigation and reload
    - changing the model from the popover
  - axe and visual baselines.
  - Parity: Composer, Chat footer.

### W18 — Activity place and session panel (M)

- **Modules:** Activity place (new); session transcript panel content; outbound task rows; sessions store view state. The legacy sessions sidebar, session row and session page are deleted.
- **Preconditions:** W14.
- **Shape when done:**
  - Activity shows Running now and a filtered, paged Finished list with plain-language kinds.
  - Opening a run shows it in the context panel with every capability in parity: details, notes, transcript, message and resume, Stop, following a resumed run.
  - Outbound tasks keep Stop and the Stop watching fallback.
  - The chat header's running pill opens Activity.
- **Verification:**
  - End-to-end: open a live session from Activity and from a chat card, message it, stop it, resume a finished one, and stop an unreachable outbound task and fall back to Stop watching.
  - Phone project uses the full-screen sheet.
  - axe and visual baselines.
  - Parity: Sessions sidebar and session view.

### W19 — Overview store and Home (L)

- **Modules:** overview store (new); Home place (new: header counts, needs-you, agents board, Across the team, Coming up); rail Home count (switched to the overview). The legacy Team page's overview role is deleted.
- **Preconditions:** W14; W05, W06, W07 merged into the integration branch.
- **Shape when done:**
  - The overview store is fed by the snapshot, `agent_overview` and `team_event` frames and the overview and events endpoints. It is the source for Home and the rail counts.
  - Home matches design §6 and the mockup: centered container, needs-you items with inline fixes that clear live, the aligned board, and the right column stacking under 1180px. On phones, cards.
- **Verification:**
  - Unit tests for needs-you derivation and ordering.
  - End-to-end:
    - brittle's needs-you item clears after a fix via the mock
    - an unread item's Open lands in Inbox
    - Stop watching removes the outbound item
    - team events appear live after a mock turn
  - axe and visual baselines at 1440 and 1920 wide and on the phone.

### W20 — Agent lifecycle on Home: create, row menu, delete and restore (M)

- **Modules:** Create agent dialog (new); Home row "…" menu; recently deleted disclosure; delete-with-undo flow. The legacy Team page is deleted.
- **Preconditions:** W19.
- **Shape when done:**
  - The Create agent dialog opens from Home's Agents row, the rail "+" and the palette.
    - It validates names, including the reserved names in design §6.
    - It creates, and confirms per the design.
  - The row menu offers Open chat, Start, Stop, Restart, Start automatically, Settings and Delete, with pending and disabled reasons.
  - Delete keeps confirm-then-undo. Recently deleted restores.
- **Verification:**
  - End-to-end: create from each entry point, a rejected reserved name, start and stop from the menu, delete then undo, restore from recently deleted.
  - axe and visual baselines.
  - Parity: Team page.

### W21 — Inbox place (M)

- **Modules:** inbox store (cross-agent, through the API client, errors surfaced); Inbox place (new); badges on rail, bottom bar and app icon. The legacy inbox drawer is deleted.
- **Preconditions:** W14; W05 and W07 merged into the integration branch.
- **Shape when done:**
  - Inbox matches design §7: agent filter, Archived tab, Markdown bodies, attachments, read on open, archive and restore.
  - Counts come from the overview. Items refresh when counts change.
  - Load failures show an error with Try again.
  - The app icon badge is set where the Badging API exists.
- **Verification:**
  - End-to-end: items from a stopped agent appear, open marks read and the badge drops, archive and restore, filter by agent, and a mocked failure shows the error.
  - axe and visual baselines.
  - Parity: inbox items in Header section.

### W22 — Schedule place (S)

- **Modules:** Schedule place (new); scheduled store. The legacy Scheduled page is deleted.
- **Preconditions:** W14.
- **Shape when done:**
  - Schedule shows pulses and actions with every parity capability.
  - A failed load shows the error and Try again.
- **Verification:**
  - End-to-end: toggle a pulse, cancel an action, and a failed load via the mock.
  - axe and visual baselines.
  - Parity: Scheduled.

### W23 — Files, Shared files and the file panel (L)

- **Modules:** Files place and Shared files place (new); file tree; editor (shared between places and the context panel); file history dialog; unsaved-edit guard; team change subscription. The legacy workspace and file history modal are deleted.
- **Preconditions:** W14.
- **Shape when done:**
  - Both places meet parity:
    - tree
    - identity tint (team tree included)
    - editor with validation
    - rename and move
    - delete with Undo
    - history with diff and restore
    - conflict dialog
  - The team tree updates live.
  - A `panel=file:<path>` link opens the same editor in the panel.
  - Leaving with unsaved edits asks first: place change, panel close, agent change, reload.
- **Verification:**
  - End-to-end: edit and save, a validation diagnostic, rename, delete and undo, restore from history, a mocked conflict, the unsaved-guard prompt, and the phone full-screen editor.
  - axe and visual baselines.
  - Parity: Workspace.

### W24 — Workbench place and artifact host (M)

- **Modules:** Workbench list and artifact host views. The workbench bridge is unchanged.
- **Preconditions:** W14.
- **Shape when done:**
  - The list and the artifact view are restyled onto the primitives, with every parity capability: activity panel, stop page, full view with F and Esc, reload, notices.
  - Full view hides the whole shell, including the phone bottom bar.
- **Verification:**
  - End-to-end: open Tip Splitter, start a background session from it and see it in the activity panel, cancel calls, full view in and out, delete and undo from the list.
  - axe and visual baselines.
  - Parity: Workbench.

### W25 — Settings frame and model (L)

- **Modules:**
  - Settings modal (new): scope picker, section navigation, phone list-then-section, save bar, per-scope unsaved state, inline validation, reload from disk.
  - Settings model split per file and scope, including secret storing on save.
  - Section registry for the palette.
- **Preconditions:** W14, W13, W15.
- **Shape when done:**
  - The modal matches design §8:
    - It opens on the right scope and follows deep links.
    - Switching scope or section swaps only content, with no flash.
    - Section is kept across scope switches where it exists.
    - Unsaved state is kept per scope across closing and switching.
    - Save and Discard are explicit.
    - Validation errors appear inline where a diagnostic has a path.
    - Partial failures name the files.
    - Reload asks when changes are unsaved.
  - Agent and All-agents scopes never write each other's files.
  - Until W26–W29 land, sections not yet rebuilt host the legacy section content inside the new frame.
- **Verification:**
  - Unit tests for the per-scope unsaved state and scope isolation.
  - End-to-end:
    - deep link to a section
    - switch scope with unsaved changes and come back
    - Save and Discard
    - an inline validation error from the mock
    - phone list and section navigation
  - axe and visual baselines.

### W26 — Settings: Model and Connections (L)

- **Modules:** agent Model section (providers, main model, per-job roles, failover preservation); agent Connections section (Discord, Telegram, Teams, incoming webhooks).
- **Preconditions:** W25.
- **Shape when done:**
  - Both sections meet design §8 and parity, including:
    - Undo on removals
    - the subconscious role's model list loading on open
    - failover lists preserved through a save
    - `models.default` shown
  - A failed agent's settings link lands on Model with the bad model flagged. Saving then restarting from the state card brings it up.
- **Verification:**
  - Unit tests for failover preservation in the diff.
  - End-to-end: the brittle fix path end to end, add and remove a provider with Undo, connect Discord with a token (stored as a secret), add a webhook.
  - axe.
  - Parity: the Providers, model roles, channels and webhooks items.

### W27 — Settings: Tools & skills, Memory, Schedule, Runtime (M)

- **Modules:** agent Tools & skills, Memory, Schedule and Advanced → Runtime sections.
- **Preconditions:** W25.
- **Shape when done:**
  - Every field from the legacy Runtime, Pulses & sessions, Memory and Skills sections is present in its new home per design §8, with plain-language labels and the numbers shown.
- **Verification:**
  - End-to-end: change one field in each section, save, and reload from disk to confirm.
  - A unit test maps every legacy field to a new section, so none is lost.
  - axe.
  - Parity: the Runtime, Pulses, Memory and Skills items.

### W28 — Settings: agent Advanced (Tool servers, Agent-to-agent, Raw config, History) (L)

- **Modules:** Tool servers section (with catalog); agent Agent-to-agent section (visibility, remote agents, card); Raw config editors with diagnostics; History browser, shared by both scopes.
- **Preconditions:** W25.
- **Shape when done:**
  - The sections meet parity.
    - A catalog fetch failure shows an error.
    - Visibility is set only here.
  - The History browser restores and undoes through the coordinator, so open forms refresh.
- **Verification:**
  - End-to-end: add an MCP server from the catalog and remove it with Undo, edit raw config and see a diagnostic, restore a checkpoint and see the form update, change visibility.
  - axe.
  - Parity: the MCP, A2A client and History items.

### W29 — Settings: All agents (L)

- **Modules:** General, Residuum Cloud, Saved keys (agent keys and secrets), Updates, Session limits, Agent-to-agent listener and caller keys, Diagnostics, Raw config (install-wide) and History. The legacy settings page is deleted.
- **Preconditions:** W25, W28.
- **Shape when done:**
  - Every hub-scope section meets parity, with the Cloud fixes listed in parity.
  - The Notifications section's placeholder is replaced by W34.
  - No legacy settings content remains.
- **Verification:**
  - End-to-end: timezone change, cloud connect states via the mock, add and remove an agent key with Undo, add and remove a secret, the update check, create and revoke a caller key.
  - axe and visual baselines.
  - Parity: the All-agents items.

### W30 — Setup wizard restyle (M)

- **Modules:** setup wizard and its steps.
- **Preconditions:** W10, W11.
- **Shape when done:**
  - The six-step flow, validation, draft autosave and completion are unchanged, rendered with the new primitives and tokens.
  - It works at phone width.
- **Verification:**
  - End-to-end in the mock's setup mode on both projects: complete the wizard, and reload mid-way to restore the draft.
  - axe and visual baselines.
  - Parity: Setup wizard.

---

## Phase 6 — PWA

W31 and W33 can run in parallel; W32 follows W31; W34 follows W32 and W33.

### W31 — Installability and code splitting (M, target: integration)

- **Modules:** manifest; document head metas; safe-area handling audit; install action (feeds the registry); route-level code splitting; bundle size report and budget in CI.
- **Preconditions:** W14; W08 merged into the integration branch.
- **Shape when done:**
  - The manifest, metas and install entry match design §11.
  - Settings, the file editor, the artifact host and the palette load on demand.
  - CI reports the initial-route size and enforces a budget set from this unit's measurement.
  - Install and notification options are hidden when there is no secure context.
- **Verification:**
  - Lighthouse (or an equivalent check) confirms installability on the mock over localhost.
  - End-to-end: the settings chunk loads on first open.
  - The budget step fails when a large dependency is imported into the shell.

### W32 — Service worker (M, target: integration)

- **Modules:** service worker (new); build step emitting the precache list; registration and update-ready flow; offline shell state; relay compatibility check.
- **Preconditions:** W31.
- **Shape when done:**
  - The worker precaches the shell, falls back on navigation, bypasses `/api` and WebSocket traffic, versions and cleans its cache, and waits for the user to reload on update.
  - Launching offline shows the shell with the hub banner.
  - The relay project has been checked for passing the worker script and manifest unmodified. Any needed relay change is filed or made in that repository, and noted in the unit's report.
- **Verification:**
  - End-to-end: after first load, going offline and reloading shows the shell and the banner. A new build shows "Update ready", and Reload activates it. `/api` requests never hit the cache.
  - Manually through the tunnel: the worker registers on the relay origin.

### W33 — Web Push backend (L, target: main)

- **Modules:** VAPID key management (secret store); push subscription store (hub); hub HTTP endpoints; push sender with Web Push encryption; triggers (inbox item added, agent failed to start, outbound unreachable beyond 15 minutes, a reply while no web client is connected); notifications doc and hub HTTP doc with bundled mirrors; mock server.
- **Preconditions:** W05, W06.
- **Shape when done:**
  - Keys are created once and kept across restarts.
  - Endpoints (design §11): get the public key, register, list devices, update preferences, remove, send a test.
  - Triggers send to subscribed devices per their preferences.
  - 404 and 410 responses remove the subscription.
  - Failures are logged at warn level and never block the trigger.
  - Payloads carry a title, body and target URL.
  - Dependencies are checked for maintenance and current versions when pinned.
- **Verification:**
  - Tests: encryption against the standard's test vectors, subscription lifecycle, preference filtering per trigger, and pruning on 410 against a local fake push service.
  - Manually: a test push reaches a desktop browser.

### W34 — Web Push client and Notifications settings (M, target: integration)

- **Modules:** worker push and notification-click handling; the All agents → Notifications settings section; permission flow.
- **Preconditions:** W32, W33 merged into the integration branch, W29.
- **Shape when done:**
  - The Notifications section manages this device: permission, subscribe or unsubscribe, per-event toggles, test.
  - It lists other devices with Remove.
  - On iOS it explains that the installed app is needed.
  - Clicking a notification focuses or opens the app at its target.
- **Verification:**
  - End-to-end with a mocked push subscription: toggle preferences, send a test through the mock, and a simulated notification click navigates to the target.
  - Manually: on an installed phone app, a new inbox item arrives as a notification.

---

## Phase 7 — Cutover

### W35 — Legacy removal, guardrail tightening and documentation (L, target: integration)

- **Modules:** everything left from the legacy UI (components, stylesheets, variables, helpers used only by them); style-lint ignore list; svelte-check accessibility suppression; web contributing guide; systems-usage pages that describe the web UI or the new contracts, and their bundled mirrors; web instructions file.
- **Preconditions:** W16 to W30, W31 to W34.
- **Shape when done:**
  - No legacy component, stylesheet or variable remains.
  - The style-lint ignore list is empty.
  - The global accessibility suppression is removed, and any resulting warnings are fixed.
  - Every `parity.md` item is checked or marked Changed or Dropped.
  - The contributing guide describes the new structure, routes and test layers.
  - Systems-usage pages match the new UI and contracts: hub HTTP, inbox, notifications, heartbeats (Schedule place), workbench, hub.
- **Verification:**
  - All gates and the full end-to-end, accessibility and visual suites pass.
  - A search for legacy class names and variables finds nothing.
  - `parity.md` has no unchecked items.

### W36 — End-to-end verification against the design (M, target: integration, then cutover)

- **Modules:** none new; verification and fixes.
- **Preconditions:** W35.
- **Shape when done:**
  - The assembled app is checked against every section of `design.md`, not only against unit criteria:
    - shell behaviors at all three widths
    - every route and redirect
    - activity lines on live and historical turns
    - state cards for every agent state
    - Home's derived items clearing live
    - settings scope isolation
    - PWA install, offline and update
    - push
  - Gaps are fixed, or filed as issues with the owner's agreement.
  - The owner performs the phone checks that need a real device (install through the tunnel, a push arriving, safe areas in standalone mode).
  - The cutover PR merges `feat/web-overhaul` into `main` with `closes` references, and the design documents move to `docs/archive/`.
- **Verification:**
  - A written check against each design section, in the cutover PR description.
  - The owner's device checks.
  - CI green on `main` after the merge.
