# Web UI Overhaul — Design

> **Status:** draft, ready for sign-off. Owner-approved decisions are listed at the end. Work units: [`phases.md`](./phases.md). Capability checklist: [`parity.md`](./parity.md). Visual reference: [`mockup.html`](./mockup.html) (open it in a browser).

> Systems level only. No file or line references. This document must stand on its own: it is implemented by subagents that have only this doc, `phases.md`, `parity.md`, the mockup and the codebase, not the conversation that produced it. Where this document and the mockup disagree, this document wins. The mockup's sample data, and the parts it labels "not in this mockup", are not requirements.

## Goal & context

The web UI has most of the capability Residuum needs, but reaching it is hard, and it looks flat and dated.

- **There is no single way to get around.** Eight destinations hide behind a hamburger menu, even on a wide screen. Beside it sit an agent chip row, a Team button, a sessions-sidebar toggle, header icons, a close button on every page, and slash commands. Pages for one agent and pages for the whole install are mixed together.
- **The defaults are backwards.** The sessions sidebar, open by default, shows the runtime's internal categories and ids. What an agent is doing in the conversation (its tool calls) is hidden unless the user types `/verbose`.
- **Failure states contradict themselves.** A failed agent's chat shows "disconnected", then "Reconnecting — messages will go out once back online", then two empty states and a toast. The real error and the Restart button live only on the Team page. A list that fails to load shows its empty state.
- **Settings mirror the config files.** Section names and field names are internal ("Observer Force Threshold (tokens)"). A Simple/Advanced/Raw toggle sits on top. MCP, A2A and tokens appear on primary surfaces.
- **The visual system does not scale.** Every element is an outlined box, and display type is used for small UI labels. There are about 5,700 lines of global CSS, with weak token use and breakpoints scattered between 400 and 1100px. There is no shared component layer.
- **It is not a real PWA.** A manifest exists, but there is no service worker, no offline shell, no push, and no iOS standalone support.
- **Testing covers logic, not the UI.** Store logic is well unit-tested. Most surfaces have no component tests, and there are no end-to-end, accessibility or visual tests. The mock server, the only integration harness, is not type-checked.

The owner reviewed three clickable directions and chose a combination, captured in `mockup.html` ("D · Combined"):
- Rail's agent-first sidebar and compact density.
- Control Room's team overview as the Home page.
- Conversation's large settings modal, with an agent picker.
- A phone bottom bar.

The palette is the only fixed element of the old visual language. The UI must be fully supported on phones and work as an installable PWA.

This design covers the new visual system and components, the shell and navigation, every surface, the backend contracts the new surfaces need, the PWA, and a rebuilt frontend test setup.

## Terms

- **Shell** — the persistent frame: the rail (or, on phones, the bottom bar and drawer), the main region, the context panel and the overlay layer.
- **Rail** — the left sidebar at medium and wide widths. On phones the same component opens as the **drawer**.
- **Place** — a destination in the main region, and a URL route: Home, Inbox, an agent's Chat, Activity, Schedule or Files, the Workbench, or Shared files.
- **Agent places** — Chat, Activity, Schedule and Files. They are listed under an agent in the rail's accordion.
- **Viewed agent** — the agent named in the current URL on an agent place. On Home, Inbox, Workbench and Shared files there is no viewed agent.
- **Bound agent** — the agent whose WebSocket the client keeps open. This is the viewed agent, or, when there is none, the most recently viewed one. The client holds at most one agent socket.
- **Context panel** — a right-side panel beside the main region that shows a session run, a file, or a conversation-size view. It is resizable at wide widths, floats over the main region at medium widths, and is a full-screen sheet on phones.
- **Settings modal** — the overlay that holds every setting, with a scope picker.
- **Scope** — either "All agents" (install-wide settings, which live in the hub's config) or one agent (settings that live in that agent's config files). Every setting belongs to exactly one scope.
- **Staged change** — a settings form edit held in memory until the user presses Save changes.
- **Immediate action** — a settings control with its own endpoint. It acts when used and reports its own result.
- **Overview** — the hub-level per-agent summary that feeds Home and the rail's badges (§9.3).
- **Team event** — an entry in the hub's in-memory event log (§9.4). The log feeds Home's "Across the team".
- **Boot id** — a random id the hub generates at startup, used to tell a restarted hub's event ids from old ones.
- **Chat unread** — the hub's existing per-agent count of main-conversation replies published while no client had that agent's socket open (any client counts, including the macOS app). It is reset when a client connects to that agent's socket. It is not persisted.
- **Inbox unread** — the number of unread items in an agent's user inbox (`inbox/user/`), counted from disk. The agent inbox (`inbox/agent/`), where background results are filed, is a separate folder, reachable through Files.
- **Needs-you item** — a Home entry for something the user should act on (§6).
- **Activity line** — the one-line summary of a turn's tool use that heads the agent's output for that turn. It expands to steps, and each step expands to its details.
- **Tool frames** — the agent socket's `tool_call` and `tool_result` frames. The server sends them only to connections that have set the verbose flag.
- **State card** — what an agent's Chat shows in place of the composer when the agent is not running.
- **Action registry** — the single list of named actions (navigate, chat commands, lifecycle, create agent, feedback…). The command palette and the composer's `/` menu both draw from it.
- **Overlay entry** — a history entry pushed when a modal overlay opens (§3), so that Back closes it.
- **Hub banner** — the notice at the top of the main region shown while the hub socket is disconnected.
- **Reload from disk** — re-reading a scope's config files and discarding staged changes for that scope.
- **Identity files** — SOUL.md, HEARTBEAT.yml, team/AGENTS.md and team/USER.md. The file tree tints them. The current UI also lists a CHANNELS.yml that no longer exists; it is dropped.
- **Turn hook** — a new method on the hub-owned activity tracker that the agent runtime calls exactly once at the end of every main turn (§9).
- **Artifact** — a page an agent builds in the team's `workbench/` folder: a single HTML file or a folder with an `index.html`. Artifacts are team-level: no agent owns one, and any agent can edit any of them.
- **Artifacts origin** — the separate origin that serves artifacts and forwards the API to them: a listener on its own port locally, or the relay's workbench host remotely. Artifacts open there in their own tab; the app never embeds them.
- **SDK** — the `window.residuum` script injected into every served artifact page. It is how an artifact reaches Residuum (§9.10).
- **Artifact session** — a session an artifact starts on a named agent. Its source label is `artifact:<name>`.
- **Watch registry** — the app's service that merges workspace watches from several owners onto one socket (§12).
- **Legacy view** — an existing component hosted inside the new shell until the unit that replaces it lands.
- **Integration branch** — `feat/web-overhaul`, where frontend work collects before cutover to `main`.
- **Work unit** — one subagent-sized piece of implementation, defined in `phases.md`.

## Shape

### 1. Visual system

**Palette (fixed).** The rest of the design derives tints and alphas from these values and adds no new hues.

| Token role | Value |
|---|---|
| Stone surfaces | `stone-0` `#0e0e10` (base), `stone-1` `#131315`, `stone-2` `#161618`, `stone-3` `#1c1c1f`, `stone-4` `#26262a` (hover and selected fills) |
| Input fill | `#141416` |
| Lines | `line` `#2a2a2e` (region separators), `line-soft` `#222225` |
| Control border | `#6a6a6f` (input and toggle boundaries; meets 3:1 against `stone-0` to `stone-3`) |
| Text | `text` `#e8e8ea`, `text-2` `#9a9a9f`, `text-3` `#85858b` (dimmest allowed) |
| Vein | `vein` `#3b8bdb` (accent, focus, links), `vein-bright` `#5aa3f0`, `vein-dim` `#2a6cb5` (primary button fill), `vein-hover` `#3175c2` (primary button hover); tints at 14%, 7% and 35% alpha |
| Moss | `moss` `#6b7a4a`, `moss-text` `#8a9e62`, `moss-tint` at 20% alpha (user message bubbles, positive states) |
| Error | `err` `#c0392b` (fills and marks), `err-text` `#e0675a`, `err-tint` at 13% alpha |
| On accent | `#ffffff` (text on `vein-dim` and `vein-hover` only) |
| Overlay | scrim `rgba(6,6,8,.62)`; floating shadow for menus, popovers, sheets, dialogs, palette and toasts only |

**Contrast rules.** The token set declares its allowed text/surface pairs, and a test checks each against WCAG AA (4.5:1 for text, 3:1 for large text, glyphs and control boundaries).

- `text` and `text-2` may sit on any stone surface.
- `text-3`, `vein` used as text, `moss-text` and `err-text` may sit only on `stone-0` to `stone-3`, never on `stone-4`. On `stone-4`, use `text` or `text-2`.
- White text sits only on `vein-dim` or `vein-hover`. Primary buttons therefore use `vein-dim` as their fill, not `vein`.
- Input, select and toggle boundaries use the control border.
- **Tint surfaces** (`vein-tint` over any stone surface, used for selected rows; `moss-tint`, used for user bubbles; `err-tint`) are declared surfaces too. On them, text uses only `text`, `text-2` or `vein-bright`. Selected rows therefore label in `vein-bright`, not `vein`.

**Type.**
- Onest (400, 500, 600) for all UI and message text.
- JetBrains Mono (400, 500) only for code, file paths and ids.
- Cinzel (500) only in the wordmark.
- Scale: 12, 13, 14 (UI), 15 (messages), 17 (section headings) and 20px (page titles).
- Fonts are bundled and self-hosted: no request goes to a font CDN.

**Shape and motion.**
- Radii are 6, 8 and 12px. Density is compact.
- Grouping comes from surface tone and spacing. Borders appear only on controls and on the hairlines that separate regions.
- Only floating layers carry the shadow.
- UI transitions run 150–200ms with an ease-out curve. All motion is disabled under `prefers-reduced-motion`.
- The grain overlay and the time-of-day vein intensity are dropped.

**Layout.**
- Rail width 248px.
- Context panel default 440px, resizable from 360px to 50% of the viewport.
- Reading column max 720px.
- Home container max 1200px, centered.
- Phone bottom bar 60px plus the bottom safe-area inset.
- Shell breakpoints:
  - phone ≤ 760px
  - medium 761–1180px (the context panel floats over the main region)
  - wide > 1180px (the panel sits beside it)

  Components that need their own responsive rules use container queries, not new viewport breakpoints. For example, the Home board hides its "Next up" column when the board is narrower than 720px.
- A z-index scale: base, sticky, panel, drawer, overlay, palette, toast.

All of the above live in one token set. Component styles use tokens only: no literal colors, font sizes, z-indexes, durations or easing curves. A style linter enforces this (§10).

**Components.** A primitive layer built on native elements where they fit (`<dialog>`, the Popover API, `inert`):
- Button (primary / secondary / quiet / danger) and IconButton.
- Fields:
  - text and number
  - select, toggle and segmented control
  - secret field, with the stored, from-environment-variable and replace states
- Badge, status dot, Disclosure, Tabs, EmptyState, Skeleton, Banner, Kbd.
- Menu, Popover, Tooltip, Dialog, Sheet (phone bottom sheet), Drawer.
- A toast region, and a Recent notifications dialog.

All floating layers share one model:
- Focus is trapped while modal and restored on close.
- Esc and a scrim click close.
- Scrolling is locked and the background made `inert` behind a modal.
- Nested overlays stack.
- Modal overlays (Dialog, Sheet, Drawer, palette) push an overlay entry (§3).

Every surface is built from these primitives.

### 2. Shell

**Medium and wide widths — the rail, top to bottom:**
- The wordmark, and "Search or jump to" (opens the palette; shows ⌘K or Ctrl+K).
- **Home**, with the count of needs-you items (one per item as listed in §6).
- **Inbox**, with the inbox unread total across agents.
- **Agents**, with a "+" that opens Create agent (§6).
  - Each agent row shows its state dot and name. It adds a working indicator while busy, a chat-unread badge capped at "99+", and a state word when not running.
  - Clicking a row only expands or collapses that agent's places; it never navigates.
  - Only one agent is expanded at a time, and clicking the expanded agent collapses it.
  - The viewed agent keeps its highlight when collapsed, and starts expanded on load.
  - Rows expose `aria-expanded`. Enter and Space toggle a row; Up and Down move between rows.
- **Team**: Workbench and Shared files.
- **Footer**:
  - A settings gear. It opens the Settings modal on the viewed agent's scope, or on "All agents" when there is no viewed agent.
  - A help menu: Keyboard shortcuts, Recent notifications, Send feedback, Report a bug, and Install app when available (§11).

**Phones (≤ 760px):**
- A bottom bar with, left to right:
  1. ☰ (opens the rail as a left drawer)
  2. Inbox, with its badge
  3. Home, centered, at the same weight as the others
  4. Search (opens the palette full-screen)
  5. Settings (opens the settings list for the viewed agent's scope, or for All agents)
- The bar stays visible on every place, including Chat, with the composer above it.
- The drawer, palette, sheets and dialogs cover the bar with a scrim.
- The drawer closes on scrim tap, swipe left, Esc or Back.
- There is no separate agent switcher.
- Every place has a compact title bar naming the place and, on agent places, the agent.

**Main region.** Each place has a header with its title and place-specific actions. The agent Chat header shows:
- the agent's name and role line
- a running-sessions pill that opens Activity
- a gear that opens Settings on that agent's scope
- an overflow menu with Show conversation size, Restart and Stop

**Connection state is shown only when degraded.**
- While the hub socket is down, the hub banner says so and offers Retry.
- While the bound agent's socket is down and that agent is running, the composer says "Reconnecting — N messages will send once back online".
- A non-running agent never shows reconnecting (§4).

### 3. Routes and history

Client routes contain no dots and never start with `/api` or `/ws`, because the server's SPA fallback depends on this. Query values may contain dots.

| URL | Place |
|---|---|
| `/` | Redirects to `/home` |
| `/home` | Home |
| `/inbox` | Inbox. `?agent=<name>` filters; `?tab=archived` shows the archive; `?item=<agent>:<id>` opens an item |
| `/agent/:name` | Chat |
| `/agent/:name/activity` | Activity |
| `/agent/:name/schedule` | Schedule |
| `/agent/:name/files` | Files |
| `/team/workbench` | Workbench list |
| `/team/workbench/:artifact` | The Workbench list with that artifact's row selected and expanded. The old `?full` parameter is removed by a replace. |
| `/team/files` | Shared files |

**Context panel parameter** — `panel=<kind>:<value>`, URL-encoded:

| Kind | Valid on | Value |
|---|---|---|
| `session:<agent>:<runId>` | agent places and Workbench routes | A run of that agent. On an agent place the agent must match the viewed agent, otherwise the parameter is removed. |
| `file:<path>` | agent places and `/team/files` | On agent places, a path in the agent's workspace namespace (`team/…` paths reach team files). On `/team/files`, a path relative to the team folder. |
| `size` | agent places | The conversation-size view |

On any other place, the parameter is removed by a replace.

**Settings parameter** — `settings=<scope>[/<section>]`.
- `<scope>` is an agent name or `_all`. Agent names can never contain an underscore, so `_all` cannot collide with one.
- Without a section:
  - at medium and wide widths, the scope's default section opens: `model` for an agent, `general` for `_all`;
  - on phones, the scope's section list opens.
- Section ids are listed in §8.
- An unknown agent in the scope becomes `_all/general`, with a toast naming the missing agent.
- An unknown section becomes the scope's default.

**Corrections** (by replace, with a toast where noted):
- An unknown agent on an agent place goes to `/home`, with a toast.
- An unknown artifact goes to `/team/workbench`, with a toast, once the artifact list has loaded.
- A `panel` with an unknown kind or a malformed value is removed.

**History rules.**
- **Push:** opening a place; opening a session or file in the panel from elsewhere; opening the Settings modal; opening a settings section from the phone list; opening a modal overlay (an overlay entry: same URL, marked in the history state).
- **Replace:** switching sections at medium and wide widths; switching scope inside the modal; switching which file the panel shows from inside the panel; every correction and redirect.
- **Closing** the Settings modal, the panel or a modal overlay through the UI (close button, Esc, scrim):
  - If this page pushed the entry that opened it, closing is `history.back()`.
  - Otherwise, for example when the page was deep-linked, closing replaces the URL with one that omits the parameter.
  - Back therefore always closes the topmost modal, the modal, or the panel before it leaves a place. This matches the Android back gesture in the installed app.
- **Unsaved-edit guard.** When unsaved file edits (§5) or staged settings changes (§8) would be lost:
  - On in-app navigation, the router asks first and does not navigate until the user confirms.
  - On Back or Forward (a `popstate` the router cannot cancel), the router immediately re-pushes the location it was showing, asks, and navigates only if the user confirms.
  - On reload or tab close, the browser's `beforeunload` prompt is used.

**Redirects from existing URLs** (all by replace):

| From | To |
|---|---|
| `/team` | `/home` |
| `/agent/:name/sessions/:runId`, with or without `?workspace` | `/agent/:name?panel=session:<name>:<runId>`. The workspace flag is dropped. |
| `/agent/:name/workspace`, `/agent/:name?workspace` | `/agent/:name/files` |
| `/agent/:name/scheduled` | `/agent/:name/schedule` |
| `/agent/:name/settings[/:old]` | `/agent/:name?settings=<name>/<new>`, or `settings=_all/<new>` when the old section belongs to the install-wide scope. Mapping in §8. |
| `/team/settings[/:old]` | `/home?settings=_all/<new>` |
| `/settings[/:old]`, `/scheduled`, `/sessions/:runId` | Resolved under the last-used agent, then as above |
| `/workbench[/…]` | `/team/workbench[/…]` |
| `/notification/<id>` (the macOS notification "Open" action) | `/agent/<last-used agent>/files`, as today. The notified result is in the agent inbox of whichever agent sent it, which may be a different agent. |

The last-used agent is the most recently viewed agent, remembered in local storage. When there is none, or it no longer exists, it is the first agent by name. With no agents at all, the setup wizard shows.

### 4. Agent Chat

**Feed.**
- Agent replies render as unboxed prose in the reading column: sanitized Markdown with GFM, line breaks and code blocks, each with a copy button.
- User messages are moss-tinted, right-aligned bubbles. A sender line appears for messages from another interface or a workbench artifact.
- Messages from sessions and teammates render as compact cards with sender, kind, and a clamped body with "Show all". Sender addresses of sessions (not teammates) also get "Open session".
- Day dividers, episode dividers and the compressed-history marker remain. The marker reads: "Older messages are summarized. <agent> remembers what was said, not the exact wording."
- One empty state.
- Every existing feed behavior in `parity.md` is kept.
- **Path links.** Inline code whose whole text is a workspace path becomes a link that opens that file in the context panel. A workspace path means at least two `/`-separated segments of letters, digits, `.`, `_` or `-`, with the last containing a `.`; `team/` reaches team files. Activity-line step targets that are paths link the same way. When the file doesn't exist, the panel says so and offers nothing else.

**Turn grouping.** A turn's output is:
- one activity line built from every tool call in the turn, placed first;
- any intermediate agent texts, in order;
- the final reply.

Turns are bounded as follows:
- **Live turns:** by `turn_started` and `turn_ended`.
- **Recent history:** by a change of `turn_id` where messages carry one, and otherwise by the next user message or agent-message card.
- **Episodes,** which carry no turn ids: by user messages and agent-message cards.

Main history shows background turns only when they begin with an agent message, so every displayed turn has a boundary.

**Activity line.**
- **Summary.** Friendly step labels joined in order, with repeats merged and counted, for example "Searched memory, read 2 files, started a research session".
  - For turns observed live, it adds the duration ("· 14s") and flags failures ("1 step failed").
  - Persisted history records neither timing nor tool errors, so past turns show neither.
- **Expanded.** The steps in order, each with its label, its target (path, query or session, linked where §4 allows), and, for live turns, its status.
- **Expanded again.** A step shows its arguments and formatted result, using the existing per-tool argument summaries and result formatters.
- **Labels** come from one table keyed by tool name. It covers every built-in tool the current UI summarizes. Other tools fall back to "Used <tool>", and MCP tools to "Used <server>: <tool>".
- **Tool frames.** The client sends `set_verbose {enabled: true}` as its first frame on every agent socket connect. When the page connects to a turn already in progress, its line shows the steps seen, with "Earlier steps happened before this page connected". The next history load renders the full line. There is no fetch on `turn_ended`, because history is written after that frame.
- The `/verbose` command is removed.
- The feed store gains per-turn aggregation of steps, live timings and failure state. Its existing frame handling is otherwise unchanged.

**Live turn.** While a turn runs:
- The line is expanded and appends steps as tool frames arrive.
- It shows an elapsed timer and a Stop control. Esc stops the turn while the composer has focus.
- Intermediate texts (`broadcast_response`) appear as they arrive.
- There is no simulated typing, because the backend sends whole responses.
- When the turn ends, the line collapses to its summary.
- Post-turn memory work appears as a quiet status line under the last reply until it finishes: "Noting what matters from this conversation" (memory) and "Reviewing the last reply" (subconscious).

**Composer.**
- Auto-growing text area: Enter sends, Shift+Enter adds a new line.
- Images attach by button, paste or drop: JPEG, PNG, GIF or WebP up to 5 MB each.
- A `/` button, and `/` typed as the first character, open the chat-scoped actions from the registry.
- A model and thinking control opens a popover (a sheet on phones). It shows the main model and thinking level and writes through the config write coordinator (§8), so it updates whenever settings change.
- Send becomes Stop while a turn runs and the composer is empty. Typing brings Send back, for a mid-turn steering message.
- The draft is kept per agent in local storage and cleared on send.

**State cards.** Display state comes from the hub:
- **Stopping:** the agent is running but in the hub's stopping set, or an `agent_stopping` frame has arrived and no `agent_state` has followed yet.
- **Starting, Running, Stopped, Failed:** from `AgentSummary.state`.

While not running, the composer is replaced:

| State | Card |
|---|---|
| **Failed** | "<agent> couldn't start", a plain-language line chosen by `last_error.kind` (§9.1), and `last_error.reason` behind a Details disclosure. Actions: **Restart**, plus by kind: `config` gets **Fix settings** (below); `port_conflict` gets **Open Connections**; `crash` and `other` get **Report a bug**. |
| **Stopped** | "<agent> is stopped", **Start**, and a Start automatically toggle. |
| **Starting / Stopping** | A progress line with no actions. |

- **Fix settings.** The client asks the agent's repair validate endpoints for diagnostics on `providers.toml` and `config.toml`. It opens the Settings section of the first diagnostic whose key path maps to a settings field, with that field flagged (for example, a bad main model opens Model). When no diagnostic maps, it opens Raw config, where the diagnostics show.
- If Restart or Start fails, the new error shows in the card.
- The conversation above the card stays readable: chat history is served for non-running agents (§9.2).
- Actions that need a running agent are disabled, with the reason "Start <agent> first": the composer, and session and schedule controls. Show conversation size still works, showing the last recorded figures.

**Conversation size.** The session token and context figures leave the always-visible footer. "Show conversation size" (overflow menu and palette) opens them in the context panel as plain-language figures, with raw token counts in a Details disclosure.

### 5. Activity, Schedule, Files, Workbench

**Activity** (agent place) replaces the sessions sidebar and the session page.
- **Running now** lists every live run and every open outbound task. Kinds are shown in plain language:

  | Kind | Label |
  |---|---|
  | external | From another app |
  | scheduled | Scheduled |
  | spawned | Started by <spawner> |
  | artifact | From a workbench page |
  | outbound tasks | Sent to <remote agent> |

  Each row shows purpose, running time, state and Stop.
  - For an outbound task, **Stop task** asks the remote agent to cancel.
  - **Stop watching** stops tracking the task locally; it is offered when Stop task can't reach the agent.
- **Finished** is one paged list with a kind filter, showing outcomes.
- **Opening a run** shows it in the context panel:
  - details: kind, started by, depth, remembered-as episode, run id
  - notes: interrupted, failed with details, overlap
  - the transcript, rendered with the Chat feed components
  - a message box, which messages the session or resumes it if finished
  - Stop

  The panel follows a resumed session into its new run.

  **Data path.** The session panel works for a run on any agent:
  - The transcript loads over HTTP.
  - **Live frames:** from the bound agent's socket when the run is on the bound agent, and otherwise from a hub `subscribe_session` (§9.8).
  - **Commands:** message and stop use the per-agent HTTP session routes when the run isn't on the bound agent. Resuming a finished run is a message.

**Schedule** (agent place) replaces the Scheduled page with the same content:
- Pulses: toggle, schedule, active hours, next run, last result, problems, and the running and overlap badges.
- Scheduled actions: due time and Cancel.
- Load failures show the error with Try again, never the empty state.

**Files** (agent place) and **Shared files** (team place) keep every workspace capability:
- the lazy tree, with identity-file tint (in the team tree too)
- the editor, with live validation, diagnostics, Save and Discard
- rename and move
- delete with Undo
- file history, with diff and restore
- the save-conflict dialog

In addition:
- The unsaved-edit guard (§3) covers leaving the place, closing the panel, changing agent and reloading.
- The team tree subscribes to team change frames and updates live.
- On phones, the editor is full-screen with Back.
- The same editor serves `panel=file:` links.

**Workbench** (team place). Artifacts are team-level. The Workbench is a launcher: artifacts open in their own browser tab or window on the artifacts origin, never inside the app.

- **The list** shows each artifact's title, its path as `/team/workbench/<name>`, and when it was edited.
  - An artifact an agent is editing glows with "updating now" for a moment. The list refreshes on the hub's artifact events (§9.8), so it works when no agent is running.
  - **Open** opens the artifact's URL on the artifacts origin in a new tab (`target="_blank"`, `rel="noopener"`). In the installed app, the platform decides whether that is the browser or an in-app browser.
  - **Copy link** copies that URL.
  - Delete removes the artifact at once, and the toast offers Undo (the team checkpoint).
  - Each row shows how many sessions the artifact has running, on any agent, from the overview's live sessions matched by source label `artifact:<name>`.
  - States: loading, error with Try again, and empty ("Nothing on the bench yet", with a line on what artifacts are).
  - When artifacts can't be opened, a banner with the reason shows whether or not the list is empty.
- **Artifact detail.** `/team/workbench/<name>` selects that artifact's row and expands it, showing:
  - Open and Copy link
  - its running sessions: purpose, agent, state, and Stop; opening one shows it in the context panel (`panel=session:<agent>:<runId>`)
  - a note that finished sessions are in each agent's Activity
- **Choosing the artifacts origin** for Open and Copy link:
  - If the UI is on the relay's UI origin and the relay reports an artifacts origin, use it.
  - Otherwise, if the UI page is plain HTTP and the listener's port is known, use the UI's host on that port.
  - Otherwise artifacts can't be opened, and the banner says why: the listener isn't available, or Residuum Cloud hasn't reported a workbench address yet (#308). A UI served over HTTPS by the user's own reverse proxy is in this case, and the banner says so.
- **The page runs on its own.** An artifact talks to Residuum directly, through the API its own origin forwards (§9.9), using the SDK (§9.10). The app shows no bar, full view, Stop page or activity panel for it: the page owns its window, and closing the tab unloads it. Its sessions keep running and stay visible in the Workbench row and in their agent's Activity.
- **Service worker.** The artifacts origin is separate, so the UI's service worker never controls artifact pages.

### 6. Home

**Header:** "Home" and counts: N running, N stopped, N can't start.

**Needs you.** Ordered by severity (error, then warn, then info), newest first within a severity. Each item carries its fix and disappears as soon as its condition clears.

| Item | Severity | Condition | Actions |
|---|---|---|---|
| Agent couldn't start | error | the agent's state is failed | Restart; the kind-specific action from §4 |
| Can't reach a remote agent | warn | an overview `outbound_problems` entry (running agents only, §9.3) | Stop task; Stop watching |
| Inbox item | info | an unread user-inbox item; the five newest are shown, then "N more in Inbox" | Open → `/inbox?item=<agent>:<id>` |

The rail's Home count is the number of needs-you items. Inbox items beyond the five shown don't count toward it.

**Agents board.** An aligned table at medium and wide widths, cards on phones. One row per agent:

| Column | Content |
|---|---|
| Agent | Name, role line, chat-unread badge |
| State | Dot and word (Running, Stopped, Can't start, Starting, Stopping) |
| Now, and its last message | While busy: "Working on a reply" and how long, from `busy_since`. Otherwise: the purpose of the newest live session, or "Idle". Under it: the last message's time and preview. |
| Running | Live session count |
| Next up | The next upcoming run and its time. "Won't run while stopped" for a non-running agent that has one. "Nothing scheduled" otherwise. |
| "…" menu | Open chat; Start, Stop or Restart; Start automatically; Settings; Delete (with the existing confirm, then Undo) |

**New agent.**
- A button on the Agents heading row opens the Create agent dialog (a sheet on phones). The rail "+" and the palette open the same dialog.
- Fields:
  - name, validated live against the backend's rules (1–24 characters, lowercase letters, digits and hyphens, no leading or trailing hyphen, not `hub`, `team` or `agents`, not taken)
  - "What should it help with?" (the description)
  - under More options: "Copy model settings from" and "Who can find it" (visibility, default private)
- On create:
  - the dialog closes
  - the agent appears in the rail and on the board
  - a toast confirms it
  - the user stays where they were, with focus on the new agent's rail row
- **Recently deleted** is a collapsed disclosure under the board, with Restore.

**Right column** (below the board under 1180px):
- **Across the team:** the newest team events, with time, agent and a link to their target.
- **Coming up:** the soonest upcoming runs across agents, up to eight.

### 7. Inbox

- One list across all agents' user inboxes.
- Controls: an agent filter and an Archived tab.
- Each item shows title, agent, source, time and an unread marker.
- Opening an item:
  - marks it read
  - shows its body as sanitized Markdown
  - lists its attachments as downloads
- Items can be archived and restored.
- Load failures show the error with Try again.
- The inbox unread total drives the rail and bottom-bar badges, the Home needs-you items and, when installed, the app icon badge (the Badging API, where supported).
- "Add a note to <agent>'s inbox" (the former `/inbox` command, which writes the agent inbox) is a registry action.

### 8. Settings

**Frame.**
- A large modal: full-screen on phones.
- At the top, the scope picker. "All agents" comes first, then each agent with its state dot, and under it "Applies to every agent" or "Only affects <agent>".
- Beside it (on phones, as its own screen) the section list, with an Advanced group labeled as a non-interactive heading.
- **Default scope when opened:**
  - the viewed agent
  - "All agents" when there is no viewed agent
  - whatever a deep link names
- Switching sections or scope swaps only the content pane: no remount or re-animation of the modal, and at most a 120ms fade on the content.
- Switching scope keeps the current section when the new scope has it.

**What saves how.**
- **Staged changes.** Every form edit to a scope's config files is staged and saved together by Save changes.
  - An agent scope's files are its `config.toml`, `providers.toml` and `mcp.json`. The All-agents scope's file is the hub's `config.toml`.
  - Removing a provider, MCP server, webhook, skill folder or tool folder is a staged change; Discard brings it back.
- **Immediate actions.** Controls with their own endpoints act at once and report their own result:
  - secrets and agent keys (add, remove)
  - agent-to-agent caller keys (create, revoke)
  - Residuum Cloud connect, cancel, reconnect and disconnect
  - update check and install
  - agent visibility and autostart
  - the remote-agents editor
  - raw config saves
  - History restore and undo
- **Save bar.** A scope with staged changes shows a save bar with Save changes and Discard.
  - Staged changes are kept per scope while the modal is closed or another scope is selected, and the bar reappears on return.
  - They are lost on reload, which asks first (§3).
- **Save.** Save sends, for each file in the scope with staged changes, the diff through the config write coordinator:
  - providers first, then config, then MCP servers, because config validation reads providers from disk
  - each PATCH validates before writing
  - diagnostics whose location is a key path show inline on the field mapped to that key (the settings model holds one field-to-key-path map, used for both diffing and error placement); other diagnostics show at the top of the section
  - a partial failure names which files saved and which didn't
  - a successful save offers Undo when any file returned a checkpoint. Undo restores every checkpoint that save returned, in reverse save order, and reports reverted and skipped paths per file, as History's undo does. A partial Undo failure names the files it couldn't restore.
- **Secret fields** exchange a typed value for a stored secret during Save, as today.
- **Raw config** editors have their own Save, and always write, as today. While the form has staged changes to a file, that file's raw editor is read-only with "Save or discard your form changes first", and the reverse.
- **Scope isolation.** An agent scope never writes install-wide files, and the All-agents scope never writes an agent's files. Install-wide values that matter to an agent page (for example, whether the agent-to-agent listener is on) show read-only there, with a link to the All-agents section.
- **Non-running agents.** Everything backed by a config file stays editable. Parts that need a running agent (agent-to-agent status, card and remote-agent reachability) show "Start <agent> to see this" with Start.

**Config write coordinator.** A client-side service; every config write goes through it: settings saves, raw saves, the composer's model and thinking control, and History restore and undo.
- It serializes writes per file.
- **Before a save,** it re-reads the file's raw text:
  - If the text differs from the baseline the form loaded, and the changed keys overlap the staged diff, the user chooses "Keep my changes" or "Use what's on disk".
  - If they don't overlap, the save proceeds: PATCH diffs apply to the current file, so external changes to other keys survive.
- **After a write, reload or restore,** it notifies subscribers, so every view that shows a config value refreshes.
- **External changes** are picked up from:
  - `workspace_changed` frames for the bound agent's config files; the coordinator keeps a watch on that agent's `config/` folder registered on the socket
  - the hub's `hub_config_reloaded` frame (§9.1)

  Other agents' files have no change feed, so for those the pre-save re-read is the only protection.
- It replaces the current client-side lock, which only the composer used.

**Sections.** Ids are in brackets.

*Agent scope:*

| Section | Holds |
|---|---|
| **Model** `[model]` | Provider connections: add or remove, type, API key, base URL, Ollama keep-alive. Main provider and model, thinking level, temperature. The default model (`models.default`). "Use different models for specific jobs", naming each role by what it does: Summarizing older messages (observer), Condensing memories (reflector), Regular checks (pulse), Reviewing replies (subconscious), Background sessions small / medium / large, Search index (embedding). Failover model lists are kept; the form never collapses a list to one entry. |
| **Connections** `[connections]` | Discord, Telegram, Teams (connected state, token or IDs, "Let others talk to this agent", context messages, Teams listener port). Incoming webhooks (name, secret, routing, format, content fields, route preview). |
| **Tools & skills** `[tools]` | Skill folders, tool PATH folders, web search backend and key, provider-native search options. |
| **Memory** `[memory]` | When to summarize and condense (observer and reflector thresholds, cooldown, force threshold), in plain words with the numbers shown. Learning from conversations and reviewing replies (the subconscious and learning settings). Search tuning under More options. |
| **Schedule** `[schedule]` | Regular checks on or off (pulse enabled). How long idle background sessions stay open, per kind. Episode skip floor. Nesting depth cap. |
| **Advanced → Runtime** `[runtime]` | Reply time limit, reply length, retries, agent abilities (allowed changes, tool-call caps, repeat-call guard, steer and stop thresholds), idle timeout and idle channel. |
| **Advanced → Tool servers** `[servers]` | MCP servers: list, remove, add (stdio or http), catalog. |
| **Advanced → Agent-to-agent** `[a2a]` | "Who can find this agent" (the only place visibility is set), status, public or local URL, remote agents with their raw editor, card preview. |
| **Advanced → Raw config** `[raw]` | Editors for `config.toml`, `providers.toml` and `mcp.json`, with live diagnostics. |
| **Advanced → History** `[history]` | Checkpoint browser for the workspace and agent-config repos. |

*All agents scope:*

| Section | Holds |
|---|---|
| **General** `[general]` | Timezone. Gateway bind address and port under More options. |
| **Notifications** `[notifications]` | Push on this device, per-event toggles, other devices (§11). The push contact (`[push] contact`) under More options. |
| **Residuum Cloud** `[cloud]` | Every existing connection state and action. Relay URL and local port under More options. |
| **Saved keys** `[keys]` | Two lists. Keys agents use as environment variables (agent keys). Stored secrets referenced by settings. |
| **Updates** `[updates]` | Status, check, update and restart. |
| **Session limits** `[limits]` | Concurrent background turns, hop soft and hard limits. |
| **Advanced → Agent-to-agent** `[listener]` | Listener toggle, port, own address, caller keys. |
| **Advanced → Diagnostics** `[diagnostics]` | Log detail, redaction, automatic error reports. |
| **Advanced → Raw config** `[raw]` | The hub's `config.toml`. |
| **Advanced → History** `[history]` | Team and hub repos. |

**Old section mapping** (for redirects):

| Old section | New scope and section |
|---|---|
| agent `runtime` | agent `runtime` |
| `providers` | agent `model` |
| `channels`, `integrations`, `webhooks` | agent `connections` |
| `pulses` | agent `schedule` |
| `memory` | agent `memory` |
| `skills` | agent `tools` |
| `mcp` | agent `servers` |
| agent `a2a` | agent `a2a` |
| agent `history` | agent `history` |
| hub `general` | `_all` `general` |
| `cloud` | `_all` `cloud` |
| hub `a2a` | `_all` `listener` |
| `sessions` | `_all` `limits` |
| `tracing` | `_all` `diagnostics` |
| `update` | `_all` `updates` |
| `secrets`, `agent-keys` | `_all` `keys` |
| hub `history` | `_all` `history` |

A section named under the wrong scope moves to the scope that has it.

- The Simple/Advanced/Raw toggle is removed.
- Destructive immediate actions keep today's model: a single click, then Undo where the backend returns a checkpoint. Deleting an agent and removing a secret keep their confirmations. No new confirmation gates are added.

### 9. Backend contracts

Existing endpoints and frames keep their shapes, because the macOS client and older web builds use them. Every change is additive except two:
- **Removing the hub's own user-inbox notes (§9.5).** This lands on the integration branch just before cutover, so the current UI on `main` keeps those notes until the new UI replaces it.
- **The SDK's move to direct access (§9.10).** Embedding ends, and the bridge's host messages go away. This lands on the integration branch, because the current UI still embeds artifacts. The API forwarding (§9.9) is additive and lands on `main`.

- **Timestamps.** New fields are RFC 3339 with an offset. Data stored as naive local minute times is converted using the hub's configured timezone at read time:
  - an ambiguous time during a DST fall-back takes the earlier offset
  - a nonexistent time in a spring-forward gap moves forward by the gap
- **Errors.** New endpoints return JSON `{error}` on failure, with 400 (bad request), 404 (unknown agent or item), 409 (conflict), 500 and 503 (hub shutting down).
- **Types.** New types are exported to TypeScript by the existing generator. The hub's client-message type is exported as `HubClientMessage` so it doesn't collide with the agent protocol's `ClientMessage`.

**How the hub learns about agents.** The hub already owns each agent's activity tracker and passes it into the agent runtime. The runtime calls it directly for busy, unread and client connections. The hub also needs to watch each running agent:

- **Per-agent watcher.** When an agent starts, the hub attaches a watcher that subscribes to that agent's event bus, through a subscription handle added to the agent's control handle. It listens to:
  - the Sessions topic. Started, state-changed and completed events feed the overview and team events. Every session event also feeds the session relay (§9.8).
  - the system Notification topic, for outbound A2A task changes.
  - a new `UserInbox` topic, carrying `user_inbox_added {item_id}`. It is published by the only in-agent code that creates user-inbox items, the user-inbox tool, after the item is saved.
  - the Workspace topic, filtered to:
    - top-level `*.json` files in `inbox/user/`
    - `scheduled_actions.json`
    - `HEARTBEAT.yml`
    - `pulse_state.json`
    - the agent's config files

  Rules:
  - Lossless topics use unbounded channels, so the watcher drains every subscription continuously, and stops when the agent stops.
  - A Workspace `Resync` makes it recompute everything for that agent.
  - A Workspace `Unavailable` makes it recompute every 60 seconds until the next `Changed` or `Resync`, logging one warning.
- **Outbound threshold event.** The outbound task tracker already sends a notice once per unreachable streak when a task has been unreachable for its notice threshold (10 minutes). It also publishes a task-change event at that moment. The tracker checks the threshold on each failed poll, so that event can come up to a minute late; the overview waits for the threshold itself (§9.3).
- **Turn hook.** The activity tracker gains a method the runtime calls exactly once when a main turn ends. It carries:
  - the turn's user message text, if any
  - its last reply text, if any
  - the time
  - the turn's visibility (`user` or `background`)
  - whether any client had the agent's socket open

  The texts are captured before the turn's outcome is published. The existing per-reply unread counting is unchanged.
- **Stopped and failed agents.** The hub computes the same data from disk: on request, and when the hub itself changes something (hub inbox actions).
- **Inbox counts** are recomputed from disk whenever:
  - the watcher sees a `user_inbox_added` event, or a Workspace change to a top-level inbox file
  - the hub's own inbox actions change something
  - an overview is requested

  Hand-placed files and changes made through the per-agent inbox routes are counted this way, but only the user-inbox tool produces an "added" event.

#### 9.1 Hub snapshot and frames

- **`agents_snapshot`** gains:
  - `activity: {<name>: AgentActivity}`
  - `stopping: string[]` (the names in the hub's stopping set)
- **`GET /api/hub/agents`** gains the same two fields beside `agents`.
- **`AgentActivity`** gains `busy_since: string | null`, the start of the current main turn. The `agent_activity` frame carries it too.
- **`AgentSummary` is unchanged.** Busy changes never produce `agent_state` frames.
- **`AgentLastError`** gains two fields:
  - `kind: "config" | "port_conflict" | "crash" | "other"`
  - `reason: string`: the underlying error text, without the "<agent> couldn't start: … Fix its settings…" wrapper that `message` keeps

  Each caller of the failure recorder passes its kind:
  - `config`: the start path's configuration errors, which today are the config-error variant.
  - `port_conflict`: the Teams port conflict.
  - `crash`: a panicked or exited run.
  - `other`: anything else.

  No file provenance is carried. §4 finds the failing setting through the validate endpoints instead.
- **New frames:**
  - `hub_config_reloaded {ok: boolean, changed: boolean, message: string | null}`, sent after each hub config reload attempt, alongside the existing notice. A reload that found nothing changed sends `ok: true, changed: false`.
  - `hub_boot {boot_id}`, sent first on every hub socket connection
- **`agent_stopping`** is exported and documented.
- **On a lagged hub socket,** the hub sends `agents_snapshot` as today. The client then refetches the overview and the events since its newest id (§9.4).

#### 9.2 Routes for non-running agents

These per-agent routes are file-only, and move to the router that serves stopped and failed agents:
- `chat/history` (recent and episodes)
- `usage`
- the user-inbox routes: list, archive list, read, archive, restore, attachments
- `a2a/agents/raw` (GET and PUT)

Their shapes are unchanged. The per-agent `status` route stays running-only.

#### 9.3 Overview

**`GET /api/hub/overview`** → `{boot_id, agents: AgentOverview[]}`, sorted by name.

```
AgentOverview {
  name,
  last_message: { role: "user" | "assistant", preview, at, at_precision: "minute" | "day" } | null,
  live_sessions: [{ address, run_id, category, source_label, purpose, state, started_at }],
  upcoming: [{ kind: "pulse" | "action", name, at }],     // soonest first, at most 3
  inbox_unread: number,
  outbound_problems: [{ task_id, remote_agent, status_text, unreachable_since }]
}
```

- **Run state, activity and summary are not repeated here.** They come from the snapshot and the `agent_state` and `agent_activity` frames.
- **`last_message`.** The newest main-conversation message with user visibility and non-empty text content, from the user or the agent. Assistant messages with only tool calls are skipped.
  - `preview` is the text as plain text: Markdown syntax removed, whitespace collapsed, cut to 200 characters with "…".
  - For a running agent it comes from the turn hook for turns with `user` visibility: the reply if there is one, otherwise the user message. At hub startup and agent start it is read from disk, as for a stopped agent.
  - For a stopped agent it comes from recent history on disk. When recent history is empty, it comes from the newest episode containing a main-conversation message, with `at` set to that episode's date and `at_precision: "day"`.
  - It is `null` when there is none.
- **`live_sessions`.** Empty for a non-running agent.
- **`upcoming`.**
  - Pulses: from HEARTBEAT.yml, pulse state and the agent's `pulse_enabled`.
  - Actions: from the scheduled actions file.
  - A pulse's time is the first moment at or after `max(now, last_run + interval)` that falls inside its active hours. A never-run pulse counts from now.
  - Disabled pulses, or all pulses when `pulse_enabled` is off, are excluded.
  - The same calculation replaces the existing `next_fire_at` on the per-agent scheduled pulses endpoint.
  - Computed for non-running agents too, so Home can say "won't run while stopped". It is empty when the agent's config can't be loaded.
- **`outbound_problems`.** Open tracked tasks whose current unreachable streak has passed the tracker's notice threshold. It updates on the task-change events at streak start, at the threshold and at clearing, and at the threshold itself, which the hub waits for rather than relying on the event. It is empty for non-running agents: their tasks aren't being watched, and the stop controls need a running agent.

**Frames.**
- `agent_overview {overview: AgentOverview}` replaces the client's copy for that agent whenever any field changes.
- Coalescing: trailing edge, at most one frame per agent per second. The last state is always sent.
- Agent creation, deletion and restore send one immediately. A deleted agent gets no further frames; `agent_deleted` removes it.

**Recovery.** On hub socket connect or reconnect, or after an `agents_snapshot` caused by lag, the client refetches the overview.

#### 9.4 Team events

**The log.** In memory, holding up to 500 entries, reset on hub restart.
- The newest 100 `warn` and `error` entries are protected.
- When the log is full, the oldest unprotected entry is evicted, whatever its level.
- Ids are monotonic within a boot.

```
TeamEvent {
  id: number,
  at,
  agent: string | null,
  kind,
  level: "info" | "warn" | "error",
  summary,        // plain language, brand voice: "atlas finished a research session"
  target: TeamEventTarget | null
}

TeamEventTarget =
  | { kind: "agent_place", agent, place: "chat" | "activity" | "schedule" | "files" }
  | { kind: "session", agent, run_id }
  | { kind: "inbox_item", agent, item_id }
```

The client turns targets into URLs:

| Target | URL |
|---|---|
| `agent_place` | `/agent/<agent>`, with `/activity`, `/schedule` or `/files` appended for those places |
| `session` | `/agent/<agent>?panel=session:<agent>:<run_id>` |
| `inbox_item` | `/inbox?item=<agent>:<item_id>` |

**Kinds and levels:**

| Kind | Level | Target |
|---|---|---|
| `hub_started` | info | none |
| `agent_started` | info | agent_place chat |
| `agent_stopped` | info | agent_place chat |
| `agent_failed` | error | agent_place chat (the state card holds the fix) |
| `agent_created` | info | agent_place chat |
| `agent_deleted` | info | none |
| `agent_restored` | info | agent_place chat |
| `agent_replied` | info | agent_place chat. Once per turn, from the turn hook, when the turn has a reply and `user` visibility. |
| `session_started` | info | session; not for scheduled sessions |
| `session_finished` | info if completed, warn if cancelled, error if failed | session; not for scheduled sessions |
| `inbox_item_added` | info | inbox_item; from `user_inbox_added` |
| `scheduled_run_finished` | info, or error if it failed | session; the only event for a scheduled session's run |
| `hub_notice` | the notice's level | none; every hub-wide `notice` broadcast. Warnings sent to a single socket connection are excluded. |

**Endpoints and frames.**
- `GET /api/hub/events?before=<id>&after=<id>&limit=<n>` → `{boot_id, events: TeamEvent[], next_before: number | null}`:
  - events newest first
  - `before` pages older and `after` returns newer
  - `limit` defaults to 50, maximum 200
- `team_event {boot_id, event}` is sent for each new entry.
- A client that sees a different `boot_id` discards its events and refetches.

**Toasts.** Team events never produce toasts. The client keeps its existing toasts for the user's own actions and for failures.

#### 9.5 Cross-agent inbox

| Method and path | Result |
|---|---|
| `GET /api/hub/inbox?status=active\|archived&agent=<name>&before=<cursor>&limit=<n>` | `{items: HubInboxItem[], next_cursor: string \| null}`, newest first by time then id. `limit` defaults to 50, maximum 200. `agent` is optional. |
| `GET /api/hub/inbox/unread` | `{total, by_agent: {<name>: number}}` |
| `PUT /api/hub/inbox/{agent}/{id}/read` | `{item: HubInboxItem}` |
| `POST /api/hub/inbox/{agent}/{id}/archive` | `{item: HubInboxItem}` |
| `POST /api/hub/inbox/{agent}/{id}/restore` | `{item: HubInboxItem}` |

```
HubInboxItem { agent, id, title, body, source, at, read, attachments: [{ filename, mime_type, size, url }] }
```

- `url` points at the per-agent attachment route, which serves non-running agents (§9.2).
- All agents are included, whatever their state.
- A change made through these endpoints updates that agent's overview.

**Failure notes.**
- The hub no longer writes its own notes into user inboxes:
  - "<agent> failed" when an agent fails to start
  - "Created the agent X" (and deleted, restored) into the acting agent's inbox
- Failures and lifecycle outcomes are shown by the state card, the needs-you item, team events and push. The user inbox holds only what agents choose to send.

#### 9.6 Serving

**Cache headers:**
- **Hashed assets** (files under `/assets/`, which the build names by content hash): `Cache-Control: public, max-age=31536000, immutable`.
- **Every other embedded file** (`index.html`, `/sw.js`, `/manifest.webmanifest`, icons, `favicon.svg`, `mcp-catalog.json`): `Cache-Control: no-cache` and a strong `ETag` from a content hash, answering `If-None-Match` with 304.

**Compression.**
- JS, CSS, JSON, SVG and webmanifest responses are compressed with brotli or gzip when the request accepts it, with `Vary: Accept-Encoding`.
- HTML documents are never compressed. Residuum Cloud's relay inserts its instance switcher into top-level HTML, and it can't find the insertion point in a compressed body.

The SPA fallback rules are unchanged.

#### 9.7 Web Push

**Storage.** Two hub-owned files beside the existing untracked state files in the hub directory.
- **Key file:** the VAPID key pair. Mode 0600. Created on first use and never regenerated automatically.
- **Devices file:** subscriptions and preferences.

Neither is in the hub checkpoint allowlist, so restores never roll them back. Both are added to the paths agents are always blocked from writing.

The VAPID `sub` claim is the `[push] contact` value in the hub config (a `mailto:` or `https:` URL) when set, otherwise `https://github.com/Grizzly-Endeavors/residuum`.

**Endpoints:**

| Method and path | Result |
|---|---|
| `GET /api/hub/push/key` | `{public_key}` (base64url) |
| `GET /api/hub/push/devices` | `{devices: PushDevice[]}` |
| `PUT /api/hub/push/devices` | Body `{subscription, label, preferences}`, where `subscription` is the browser's subscription JSON. Upserts by the subscription endpoint URL (idempotent). Returns `{device: PushDevice}`. |
| `PATCH /api/hub/push/devices/{id}` | Body `{label?, preferences?}`. Returns `{device}`. |
| `DELETE /api/hub/push/devices/{id}` | 204 |
| `POST /api/hub/push/devices/{id}/test` | `{delivered: boolean, error: string \| null}` |

```
PushDevice {
  id, label, created_at, last_success_at: string | null,
  last_failure: { at, status: number | null, message } | null,
  preferences: { inbox_item: bool, agent_failed: bool, outbound_unreachable: bool, reply_while_away: bool }
}
```

Defaults for a new device: `inbox_item` and `agent_failed` on, the other two off.

**Triggers:**

| Preference | Fires when | Target | Urgency | TTL |
|---|---|---|---|---|
| `inbox_item` | A `user_inbox_added` event (§9) | `/inbox?item=<agent>:<id>` | normal | 24h |
| `agent_failed` | An agent enters the failed state | That agent's Chat | high | 24h |
| `outbound_unreachable` | Once per unreachable streak, when the tracker's 10-minute threshold event arrives (§9) | That agent's Activity | normal | 6h |
| `reply_while_away` | Once per turn, from the turn hook, when the turn has a reply, has `user` visibility, and no client had that agent's socket open | That agent's Chat | normal | 1h |

**Payload.** Encrypted JSON:

```
{ v: 1, event: "inbox_item" | "agent_failed" | "outbound_unreachable" | "reply_while_away" | "test",
  agent, title, body, target, tag, badge }
```

- `target` is a URL path from the triggers table.
- `badge` is the total inbox unread.
- `body` is plain text, at most 120 characters.
- `test` is the event of the notification that Send test in the Notifications section asks the hub to send to one device. It has no preference and no agent (`agent` is empty), carries the real inbox unread count in `badge`, and is shown like every other push.

| Event | Title | Body | Tag |
|---|---|---|---|
| `inbox_item` | The item's title | "From <agent>: " and the start of its body | `inbox:<agent>:<id>` |
| `agent_failed` | "<agent> couldn't start" | A line chosen by the error's kind | `failed:<agent>` |
| `outbound_unreachable` | "<agent> can't reach <remote>" | "A task has been waiting since <time>." | `outbound:<agent>:<task_id>` |
| `reply_while_away` | "<agent> replied" | The reply's preview | `reply:<agent>` (later replies replace earlier ones) |

**Presence.**
- While an app window is visible and focused on a device with push enabled, the app sends `presence {device_id, active: true}` on the hub socket. It sends `active: false` when the window hides or loses focus.
- The hub skips pushes to a device that reported active within the last 60 seconds and whose hub socket is still connected. The app re-sends `active: true` every 30 seconds while it stays active.

**Delivery.**
- Standard Web Push encryption and VAPID authentication, sent from the host to the subscription's endpoint.
- Responses:
  - **404 or 410:** removes the device.
  - **429, 5xx or a network error:** retried once after 30 seconds, then recorded.
  - **Any other failure:** recorded without retry.
- Recording sets the device's `last_failure` and logs at warn level with the device label. The Notifications section shows the last failure, so failures are visible to the user.
- Delivery never blocks or fails the operation that triggered it.

#### 9.8 Workbench events and the session relay

**Artifact events on the hub.**
- One hub-owned workbench watcher reads the team change feed.
- It publishes `artifact_updated {name}` and `artifact_removed {name}` on the hub socket, using the same rescan-and-compare rule as today's per-agent watchers.
- Artifact events therefore reach clients with no agent running.
- The per-agent `artifact_*` frames stay while the current UI on `main` uses them, and are removed at cutover (W49).

**Session relay on the hub socket.** New client frames:

| Frame | Effect |
|---|---|
| `subscribe_session {agent, address}` / `unsubscribe_session {agent, address}` | Every session event for that session on that agent |
| `subscribe_artifact_sessions {artifact}` / `unsubscribe_artifact_sessions {artifact}` | Every session event for every session with source label `artifact:<artifact>`, on any agent, including ones started after the subscription |

- **Acknowledgement.** Each subscribe is acknowledged with `subscribed {kind, agent?, address?, artifact?}` once it is active. A client that needs a session's first frames (the SDK starting a session) waits for the acknowledgement before starting it.
- **Delivery.** The per-agent watcher (§9) subscribes to the full Sessions topic. It keeps an address-to-source-label map from each session's start event, and forwards matching events as `session_frame {agent, frame}`. `frame` is exactly the `session_*` frame the agent socket would send. Tool frames are included whatever the connection's verbose flag, because the hub socket has no verbose flag.
- **Lag.** When the hub socket lags and drops relayed frames, the hub sends `session_relay_lagged` on that connection. Clients then re-read the state of the sessions they follow over HTTP. The SDK passes this on to each session handle as a `resync` event.
- **Scope.** Subscriptions belong to one hub socket connection and end when it closes. A client re-sends them after reconnecting.
- **Errors.** A subscription naming an unknown agent gets a `notice` and no acknowledgement. A stopped agent produces no events until it runs again. Nothing is buffered for late subscribers.

#### 9.9 The artifacts origin forwards the API

**Local.** The artifacts listener serves artifact pages as today, and also serves `/api/*`, WebSocket upgrades included.
- It dispatches only `/api/*` in-process to the same hub router the gateway uses. It holds a late-bound handle to that router, because the listener starts before the router is built.
- It marks each request as arriving through the artifacts origin, with an internal request extension that no client can set.
- `api` is reserved and is not a valid artifact name.
- Agent sockets opened through the artifacts origin don't count as clients. They neither reset chat unread nor suppress `reply_while_away` pushes.
- Requests from the artifacts origin to itself are same-origin, so the cross-site guard passes them. It still refuses other sites.

**Through Residuum Cloud.**
- **The relay's workbench host** allows every method and WebSocket upgrades. It keeps its owner-session check and its refusal of the relay's own routes. It forwards everything with the workbench surface: HTTP requests as today, and socket opens.
- **The tunnel protocol.** The tunnel's socket-open frame gains an optional `surface`. The tunnel client opens a workbench-surface socket against the artifacts listener, as it already does for workbench HTTP.
- **Capability.** The hub advertises a new `workbench-sockets` capability alongside `workbench-surface`. The relay sends workbench socket opens only to hubs that advertise it. For other hubs it refuses the upgrade on the workbench host with a 502 whose body says the Residuum version is too old. An older hub, which ignores the unknown `surface` field, can therefore never receive one against its main listener.
- **Login.** An unauthenticated request to the workbench host returns to the requested artifact URL after login, not to the UI root.
- This is a change in the relay project and in the tunnel client. Until the relay change is deployed, remote artifacts can load pages and call HTTP routes but not open sockets. The SDK reports its connection as `disconnected`.

**Block list.** Requests carrying the artifacts-origin marker are refused with 403 `{error}` on:
- shutdown
- stop-all
- update check, apply and restart
- setup completion

Everything else the UI can call, an artifact can call.

**Identity.**
- The SDK sends `X-Residuum-Artifact` with its artifact name, as the bridge does today. Session starts and inbox attribution use it.
- The header is informative, not a security boundary.

**Limits.** Through Residuum Cloud, responses over 10 MB and HTTP calls over 25 seconds fail at the tunnel (#310). `workbench.md` documents both.

#### 9.10 The SDK

Artifacts have never shipped in a release, so the SDK is shaped for standalone pages with no compatibility constraints. It talks to Residuum directly.

**No implicit agent.** Artifacts are team-level and belong to no agent. Every agent-specific call names its agent, as `ask` and `sessions.start` already do. Nothing defaults to an agent.

**Members:**

| Member | Behavior |
|---|---|
| `fetch` | Calls `/api` on the page's own origin with `X-Residuum-Artifact`. Unscoped hub prefixes map to `/api/hub` and `/api/workbench/` to `/api/team/workbench/`, as today. An agent path that names no agent is not sent: it resolves to a 400 response whose error says to use `/api/agents/<name>/…`. Concurrency is capped as today: at most 8 ordinary requests and 4 model calls per page, queued in order. The relay's "agent overloaded" 503 is retried up to three times with backoff. |
| `ask` | Unchanged: requires `agent`, and calls `/api/agents/<agent>/model/complete`. |
| `on(type, handler)` | Accepts only `artifact_updated`, `artifact_removed`, `connection` and `"*"`, which covers those three. Any other type throws a `TypeError` that says to use `agent(name).on`. |
| `watch(prefix)` | Team paths only, `team` or `team/…`, over the hub socket's team watch. Any other prefix, `""` included, throws a `TypeError` that says to use `agent(name).watch`. |
| `agent(name)` (new) | A handle for one named agent. Its `on` receives that agent's frames over its socket, opened on first use and reopened on reconnect. The socket sets the verbose flag on connect, so tool frames arrive. Its `watch` follows that agent's workspace (`""` for all of it). Its `connection` events follow that socket. |
| `sessions.start` | Unchanged signature. It subscribes to the artifact's sessions on the hub socket (§9.8), waits for the acknowledgement, then starts the session over HTTP. With no acknowledgement within 10 seconds, it rejects with an error saying Residuum's live connection isn't available, and the session is not started. Handles receive frames whichever agent the session runs on (fixes #292), plus `resync` after relay lag. |
| `state` | Unchanged. |
| Hub socket | Opened when the SDK loads, for live reload, artifact events, team watches and sessions. It reconnects with backoff. Top-level `connection` follows it, and a reconnect triggers `workspace_resync {reason: "reconnected"}` for active watches on it. |

**Live reload.** On `artifact_updated` for its own name, the SDK reloads the page. A page that registers its own `artifact_updated` handler takes over and the SDK doesn't reload. `artifact_removed` for its own name is delivered to handlers only.

**Removed:** `embedded`, the host messages `ready` and `escape`, and the "not open inside Residuum" rejections. No host page exists.

**Docs.** The bundled workbench skill (`SKILL.md` and its API reference) and `workbench.md` are rewritten to match:
- opening artifacts
- naming the agent for every agent-specific call, with `agent(name)`
- the block list
- the relay limits

### 10. Frontend test and quality setup

| Layer | Environment | Covers |
|---|---|---|
| Unit | Node | Stores, routing, formatters, the settings model, the action registry, activity-line labeling. |
| Component | jsdom and Testing Library | Every primitive, and every surface's empty, loading, error, populated and live states. Shared fixtures build hub, agent and feed state without a socket. |
| End-to-end | Playwright against the mock server | Navigation and flows in two Chromium projects, desktop 1440×900 and phone 390×844 with touch. A WebKit phone project is also available. |
| Accessibility | axe-core inside end-to-end | Every place and overlay. Serious and critical violations fail. |
| Visual | Playwright screenshots | A small set of baselines per surface at both sizes, with frozen time and animation and masked dynamic regions. They run inside the official Playwright container image, pinned to the Playwright version, so rendering matches wherever they run. `just web-e2e-update` regenerates baselines in that container. |

**Mock server as a harness.**
- Split into modules; type-checked, linted and formatted like the app.
- Response shapes typed with the generated protocol types, so mock and backend drift shows up as a type error.
- **Deterministic mode:**
  - fixed clock
  - configurable delays, zero by default in tests
  - a stable scenario
- A reset endpoint.
- Two ways to run:
  - with the Vite dev server
  - a preview mode that serves a production build alongside the mock, for service worker and installability tests
- It implements every endpoint the app calls, including §9. A test records every request the API client can make and checks each against the mock's route table.
- **Workbench fidelity.**
  - In deterministic mode the mock's artifacts listener runs on a fixed port, and preview mode includes it.
  - The listener forwards `/api` and sockets to the mock, with the block list.
  - Its sample artifact names its agent in `ask` and `sessions.start`.
  - Artifact deletes return a checkpoint id.
  - It sends artifact events and workspace changes when files change, and implements the session relay with acknowledgements.

**Guardrails.**
- A style linter checks global and component styles for token use. Legacy styles are on an ignore list that must be empty at cutover.
- Rune store modules get the same strict TypeScript lint rules as other modules.
- `svelte-check` fails on warnings. The one existing suppressed accessibility warning (labels without an associated control) stays suppressed until cutover removes the suppression.
**Where checks run.** The repository has a single maintainer, so pull requests don't run CI. The pre-commit hook is the per-change gate, and the quality-checks workflow runs everything at release.
- **Pre-commit:** format, lint, type check, unit and component tests, on any change under the web app, including the mock server and config. For commits that touch Rust, it also runs the generated-types check, which regenerates the TypeScript types and fails on any difference.
- **End-to-end, accessibility and visual tests** run locally through `just web-e2e`: by each frontend unit before it reports, and by the orchestrator before each merge. The release workflow runs them too.
- **Coverage** is reported by `just web-coverage` and at release, with no threshold.
- **The initial-route bundle size** is reported by a `just` recipe and at release. At cutover a budget is set from the measured size, and enforced from then on by the same recipe, which is part of the pre-merge checks.

### 11. PWA

**Installability.**
- **Manifest:**
  - `id` and `start_url` of `/home`
  - `display: standalone`
  - background and theme color `#0e0e10`, so the status bar matches the base surface (today's theme color is the vein blue)
  - the existing icons, with the maskable icon's content inside the 80% safe zone (replaced if not)
  - shortcuts to Home and Inbox
- **iOS:** the standalone and status-bar metas (black-translucent) and `viewport-fit=cover`. The shell honors all four safe-area insets.
- **Install app:** appears in the help menu and palette when the browser offers an install prompt. On iOS it opens an explanation of Add to Home Screen.
- **Without a secure context** (plain-HTTP LAN access), install and notification options are hidden.

**Service worker (`/sw.js`).**
- **Build:** a step writes the precache list (`index.html`, hashed assets, fonts, icons) and a version derived from that list into the worker. Every build with changed assets produces a byte-different worker.
- **Caching:**
  - It precaches the shell into a versioned cache, the lazy chunks and fonts included. On activate it keeps that cache and the one it replaced and deletes the rest, since a page opened before an update still asks for the hashed files it was built with.
  - Navigations are network-first, falling back to the cached `index.html` when the network fails or the hub answers 502, 503 or 504 (the relay answers 503 for an instance that is offline).
  - `/api` and WebSocket traffic are never intercepted.
- **Offline:** the app launches to the shell, and the hub banner explains that Residuum can't be reached. No data is cached for offline reading.
- **Updates:** a new worker waits. The app shows "Update ready" with Reload, and Reload activates it.
- **Push:** the same worker handles push and notification-click events.
  - The worker always shows the notification, with the payload's title, body and tag, and updates the app badge from `badge`. Browsers require every push to show a notification: Safari can revoke a subscription after silent pushes, and Chrome substitutes a generic one.
  - Suppression while the app is in use happens on the hub instead (§9.7).
  - A click focuses an open app window and navigates it to `target`, or opens a window there.
- **Registration** happens only in production builds, including the mock's preview mode, never in the dev server.

**Code splitting.** Settings, the file editor, the command palette and the setup wizard load on demand. The shell, Home and Chat are in the initial bundle.

**Notifications section (All agents).**
- This device:
  - permission state
  - enable or disable, which subscribes or unsubscribes
  - label
  - per-event toggles
  - Send test
  - last delivery result
- Other devices, with their labels, last results and Remove.
- On iOS, a note that notifications need the installed app.
- Together with the cross-agent inbox count, this resolves issue #105.

### 12. Stores and services

The data layer stays. Changes:

- **API client.** Every agent-scoped function takes the agent name explicitly. No request depends on a module-level current agent. Cache keys include the agent.
- **Routing.** Implements §3. Settings, panel and inbox-item state live in the URL. Stores never import the router: they expose data and commands, and views navigate.
- **Agent socket coordinator.**
  - Binds to the bound agent.
  - No longer resets unrelated stores on an agent switch; each store subscribes to the agent-change signal itself.
  - Sends `set_verbose` first on connect.
- **Overview store (new).**
  - Holds the hub snapshot's activity and stopping set, the overview, and team events.
  - Is fed by the frames in §9 and the overview and events endpoints, with the recovery rules in §9.1, §9.3 and §9.4.
  - Is the only source for Home, the rail's badges and needs-you items.
- **Inbox store.** Becomes cross-agent on the hub endpoints. It uses the API client, surfaces errors, takes counts from the overview, and fetches items when Inbox opens or a count changes.
- **Settings model.** Split per scope and file. It holds baselines, staged changes, the field-to-key-path map, and save-bar state per scope. The config write coordinator (§8) replaces the current lock.
- **Action registry (new).**
- **Watch registry (new).** One per socket: the bound agent's socket and the hub socket's team watch.
  - Owners register and unregister prefixes: the Files and Shared files places, and the config write coordinator's `config/` watch.
  - The registry sends the union of the prefixes, re-sends it on reconnect, and delivers each change to the owners whose prefixes match.
  - No owner can replace another's watches. On a bound-agent switch it re-sends the registered agent-socket prefixes that still apply.
- **Two consolidations:**
  - turn counters, today duplicated between the main feed and session views, become one shared helper
  - the near-duplicate restore helpers in undo become one
- **Notification history.** Kept as today, and shown in the Recent notifications dialog. Team events do not feed it.

## Reasoning & alternatives

**Direction.** The owner compared three clickable mockups:
- Rail: agent-first sidebar.
- Conversation: chat-first, with a settings modal.
- Control Room: overview-first.

They chose a combination:
- Multi-agent work is what Residuum does that single-assistant apps don't, so agents belong in the primary navigation.
- A team overview answers "what needs me" at a glance.
- A modal keeps settings out of the navigation while making every section reachable.

**View-layer rewrite on the existing data layer.**
- The stores, socket handling, undo helpers and API client are sound and tested.
- The problems are layout, components and styling. Rewriting those while keeping the data layer is less risky than a full rewrite.
- Restyling in place was rejected: the outline-box look, CSS sprawl and navigation model are structural.

**In-house primitives on native elements.**
- `<dialog>`, the Popover API and `inert` cover the hard accessibility parts.
- The component set is small. A library would add a dependency to track, and styling overrides against the fixed palette.

**Home data from the hub, not browser fan-out.**
- Per-agent requests from the browser fail for stopped agents, and cost a request per agent per field.
- They also need a socket per agent for live updates.
- The hub already owns agent state and the activity tracker. A per-agent watcher plus one overview contract gives Home, the badges and push a single source.

**Bus subscription for the watcher, and the activity tracker for replies.**
- Sessions, outbound tasks and workspace changes are already published on each agent's bus, so subscribing needs no new publishers.
- The last message and per-turn events come through a new turn hook on the activity tracker, which the runtime already calls directly.
  - The recent-messages file is written only after a turn, and loses messages to episode rotation.
  - The existing per-reply call fires once per reply text, not once per turn.

**Team event log in memory.**
- "Across the team" needs recent timestamped history, and nothing records one today.
- A bounded in-memory log costs no storage format or retention policy.
- It restarts with the hub, and `hub_started` marks the break.
- Level-aware eviction keeps failures from being pushed out by routine replies.

**Hub-level cross-agent inbox.**
- The inbox is per agent on disk.
- One hub list gives one request, consistent UTC timestamps, and counts for push and the app badge.
- The per-agent routes remain for compatibility.

**The hub stops writing notes into user inboxes.**
- Those notes duplicated what the state card, Home, team events and push now show. A failed agent would appear twice in needs-you and send two pushes.
- They made the user inbox hold system chatter instead of what agents chose to send.
- Repeated same-day failures also overwrote each other (#305).
- The removal waits for the integration branch, so users of the current UI keep the notes until the new surfaces that replace them ship.

**Explicit save in settings.**
- Autosave validated half-typed values, raced the composer's model control, and reported failures only after the fact.
- The owner approved the mockup's save bar.
- It makes a fix-then-restart flow read naturally, and puts validation errors inline before anything is written.

**`_all` as the install-wide scope token.** Agent names can't contain underscores, so no agent name needs reserving and existing agents are unaffected.

**Overlay entries in history.** In the installed app, Android's Back gesture should close what's on top before leaving the place. Pushing an entry per modal overlay is the only way the browser offers.

**A hand-written service worker.**
- It has three small jobs: precache the shell, fall back on navigation, and handle push.
- Owning it avoids a build-plugin dependency and keeps push handling in the same file.

**Always-on tool frames with a collapsed summary.**
- Tool activity is what a user most wants to see about a turn.
- The summary keeps it quiet, whereas the toggle hid it behind a command few would find.

**Visual tests in a pinned container.** Screenshots differ between machines. Running both baseline creation and comparison in the same pinned image makes them reproducible.

**Chromium-only end-to-end tests in CI.** It is unverified whether the CI runners can host WebKit's system dependencies. A local WebKit project covers iOS-like checks. Adding it to CI is a follow-up once the runner is confirmed.

**Integration branch for frontend work, `main` for backend contracts.**
- A half-converted UI must never ship, so frontend work collects on one branch and cuts over in one merge.
- The backend changes are additive and harmless to the current UI, so they land on `main` as built. That avoids a long-lived backend divergence.
- The integration branch merges `main` after each backend unit lands.

## External touchpoints

- **Hub HTTP and WebSocket.** Existing contracts are unchanged; additions are in §9. Unknown frame types are ignored by the current web client, which switches on the frame type. The macOS client uses only `GET /api/hub/agents` and the agent socket, and both keep their shapes.
- **Agent HTTP and WebSocket.** No shape changes. The web client sets the verbose flag on every connect. Some file-only routes gain service for non-running agents (§9.2).
- **Agent event bus.** The hub subscribes per running agent (§9). Lossless topics use unbounded channels, so the watcher must drain continuously.
- **Generated TypeScript types.** Produced from Rust by the existing export test. CI regenerates and diffs them.
- **Embedded asset serving.** The SPA fallback serves `index.html` for paths without a dot that don't start with `/api` or `/ws`. `/sw.js` and `/manifest.webmanifest` are dotted root files served directly with correct MIME types. Cache headers and compression are in §9.6.
- **Residuum Cloud relay and tunnel.**
  - Remote use goes through an HTTPS origin, which gives service workers and push the secure context they need.
  - The tunnel forwards HTTP with a 25-second timeout and a 10MB response cap.
  - The relay (a separate project) must pass through, unmodified and without an auth redirect:
    - the worker script
    - the manifest
    - `Cache-Control`, `ETag` and `Content-Encoding`

    This is verified against the relay's code. Any needed relay change is made in that project.
  - Plain-HTTP LAN access has no secure context, so install and push are hidden (§11).
- **Workbench artifacts origin and SDK.**
  - The artifacts listener gains API forwarding with the block list (§9.9).
  - The SDK moves to direct access with no implicit agent (§9.10). Artifacts have never shipped in a release, so nothing needs migrating.
  - **Relay.** The workbench host must allow every method and sockets, and the tunnel's socket-open frame gains a surface. Both are relay-project changes, deployed by the owner.
- **Web Push services.**
  - Outbound HTTPS from the host to each subscription's endpoint, with VAPID authentication and Web Push encryption.
  - Failure handling is in §9.7.
  - Apple's service requires a valid VAPID `sub` claim.
- **macOS notification bridge.** Its "Open" action opens `/notification/<id>`, which redirects to the last-used agent's Files, as today (§3).
- **Fonts.** Onest, JetBrains Mono and Cinzel are bundled under their OFL licenses.

## Integration with existing system

**Branches.**
- Frontend work lands on `feat/web-overhaul`, one PR per work unit.
- `main` keeps shipping the current UI until cutover.
- Backend units branch from `main`, merge to `main`, and reach the integration branch through its merges from `main`. The mock server changes that accompany a backend unit land with it.
- The test harness and guardrail units land on `main` first, so both branches share them.

**Coexistence on the integration branch.**
- The new shell lands early and hosts legacy views for places not yet rebuilt.
- New tokens use names that don't collide with the legacy variables.
- New base styles are scoped to new components; the legacy global reset stays until cutover.
- The shell unit adjusts legacy layout rules that assumed the old header and sidebar, so hosted legacy views fill the main region.
- Each surface unit deletes the legacy components and styles it replaces.

**Unchanged:**
- the socket transport
- the feed and session stores' frame handling, apart from the activity-line aggregation (§4)
- the undo, checkpoint and pending-save helpers
- the setup wizard's flow, which is restyled only

**Replaced:**
- the header and hamburger menu
- the agent chip row
- the sessions sidebar and session page
- the Team page
- the Scheduled page
- the inbox drawer
- the notification corner: its toasts move to the toast region, and its history to the Recent notifications dialog
- the Settings page and its mode toggle
- the in-app artifact view and the workbench bridge: artifacts open in their own tab (§5), and the bridge is deleted
- the help overlay (now the Keyboard shortcuts dialog)
- the feedback modal, rebuilt on Dialog
- the global stylesheets

**Documentation.**
- Backend units update the matching systems-usage pages (hub HTTP, hub, inbox, notifications, heartbeats), and the bundled `residuum-system` reference where one exists for that page (`inbox`, `notifications`, `heartbeats`).
- At cutover:
  - the web contributing guide (structure, routes, testing)
  - the web aesthetic guide, rewritten for this visual system
  - the systems-usage pages that describe the web UI

**Cutover.**
- One PR merges `feat/web-overhaul` into `main` once:
  - no legacy views remain
  - the style ignore list is empty
  - every `parity.md` item is checked or marked Changed or Dropped
  - all test layers pass
  - the relay change (W12d) is deployed, so remote artifacts have sockets
- The design documents then move to `docs/archive/`.

## Decisions

Choices the owner has approved, recorded here with where each applies:

1. **Home's data** comes from a hub overview contract fed by a per-agent bus watcher (§9, §9.3), not browser fan-out.
2. **"Across the team"** is an in-memory event log of 500 entries with level-aware eviction, reset on restart (§9.4).
3. **The Inbox** is one cross-agent list backed by hub endpoints (§9.5).
4. **The hub stops writing** its failure and lifecycle notes into user inboxes (§9.5), at cutover.
5. **Settings** use explicit Save and Discard per scope, with the staged/immediate split in §8.
6. **The service worker** is hand-written (§11).
7. **Web Push** is in scope. It covers:
   - the four events and defaults in §9.7; the outbound event reuses the tracker's existing 10-minute threshold
   - keys and devices in untracked hub files
   - a `[push] contact` hub setting for the VAPID contact
   - no push to a device whose app is open and focused, reported through presence on the hub socket
8. **No CI on pull requests.** The pre-commit hook gates each change, the end-to-end, accessibility and visual suites run locally before each merge, and the full workflow runs at release. Visual tests run inside a pinned Playwright container (§10).
9. **Backend units** merge to `main` directly; frontend units go to the integration branch.
10. **The grain overlay and time-of-day vein intensity** are dropped (§1).
11. **Modal overlays push history entries** so that Back closes them (§3).
12. **Input borders** use `#6a6a6f`, the old dim text tone, to meet the 3:1 control-boundary contrast. They will read slightly more visible than the mockup's. Selected rows label in `vein-bright` instead of `vein` for contrast on the tint (§1).
13. **A failed agent's Fix settings** finds the failing field through the validate endpoints and falls back to Raw config, instead of the backend reporting which file failed (§4, §9.1).
14. **Artifact sessions and artifact events move to the hub socket,** fixing #292 (§9.8).
15. **Artifacts reach Residuum through API forwarding on the artifacts origin** (§9.9). Everything is allowed except shutdown, stop-all, updates and setup, so users can build their own interfaces to the whole program.
16. **Artifacts are never embedded.** They open in their own tab, and the Workbench place is a launcher (§5). The SDK talks to Residuum directly, with no implicit agent: agent-specific calls name their agent (§9.10).
