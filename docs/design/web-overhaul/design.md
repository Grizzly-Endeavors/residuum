# Web UI Overhaul — Design

> **Status:** draft, pending owner sign-off. Work units: [`phases.md`](./phases.md). Capability checklist: [`parity.md`](./parity.md). Visual reference: [`mockup.html`](./mockup.html) (open it in a browser).

> Systems level only. No file or line references. This document must stand on its own: it is implemented by subagents that have only this doc, `phases.md`, `parity.md`, the mockup, and the codebase — not the conversation that produced it.

## Goal & context

The web UI has most of the capability Residuum needs, but reaching it is hard, and it looks flat and dated. Concretely:

- **There is no single way to get around.** Eight main destinations are hidden behind a hamburger menu, even on a wide screen. Beside it there are an agent chip row, a Team button, a sessions-sidebar toggle, header icons, a close button on every page, and slash commands. Pages for one agent and pages for the whole install are mixed together.
- **The defaults are backwards.** The sessions sidebar, open by default, shows the runtime's internal categories and ids. Meanwhile what an agent is doing in the conversation (its tool calls) is hidden unless the user types `/verbose`.
- **Failure states contradict themselves.** A failed agent's chat shows "disconnected", "Reconnecting — messages will go out once back online", two different empty states, and a toast. The real error and the Restart button live only on the Team page. When a list fails to load it shows its empty state, so an error reads as "nothing here".
- **Settings mirror the config files.** Sections map one-to-one to TOML files, and field names are internal ("Observer Force Threshold (tokens)"). A Simple/Advanced/Raw toggle sits on top. Terms like MCP, A2A and tokens appear on primary surfaces.
- **The visual system does not scale.** Every element is an outlined box. Small-caps display type is used for UI labels. About 5,700 lines of global CSS with weak token use, breakpoints scattered between 400 and 1100px, and no shared component layer.
- **It is not a real PWA.** A manifest exists, but there is no service worker, no offline shell, no push, and no iOS standalone support.
- **Testing covers the logic but not the UI.** Store and helper logic is well unit-tested. Most surfaces have no component tests, and there are no end-to-end, accessibility or visual tests. The mock server, which is the only integration harness, is not type-checked.

The owner reviewed three clickable directions and chose a combination, captured in `mockup.html` ("D · Combined"):

- Rail's agent-first sidebar and compact density.
- Control Room's team overview as the Home page.
- Conversation's large settings panel, with an agent picker.
- A phone bottom bar: ☰, Inbox, Home, Search, Settings.

The palette is the one fixed brand element. Everything else in the old visual language is open. The UI must be fully supported on phones, and it must work as a proper installable PWA.

This design covers the whole overhaul: the new visual system and components, the shell and navigation, every surface, the backend contracts the new surfaces need, the PWA, and a rebuilt frontend test setup.

## Terms

- **Shell** — the persistent frame: rail (desktop) or bottom bar plus drawer (phone), the main region, the context panel, and the overlay layer.
- **Rail** — the left sidebar on desktop and medium widths. On phones the same component opens as the **drawer**.
- **Place** — a destination in the main region: Home, Inbox, an agent's Chat, Activity, Schedule or Files, the Workbench, or Shared files. Places are URL routes.
- **Agent places** — Chat, Activity, Schedule and Files, listed under an agent in the rail's accordion.
- **Context panel** — a right-side panel beside the main region that shows a session transcript, a file, or a conversation-size breakdown. It is resizable on wide screens, floats over the main region at medium widths, and is a full-screen sheet on phones.
- **Settings modal** — the large overlay holding all settings, with a **scope picker** at the top: "All agents" (install-wide settings) or one agent.
- **Scope** — either "All agents" or a single agent. Every setting belongs to exactly one scope.
- **Overview** — the new hub-level per-agent summary that feeds Home: run state, activity, last message, live sessions, next scheduled run, unread inbox count, and outbound problems.
- **Team event** — a timestamped entry in the new hub-level event log that feeds Home's "Across the team" column.
- **Needs-you item** — a Home entry for something the user should act on: an agent that can't start, an unread inbox item, or an outbound task that can't reach its agent.
- **Activity line** — the one-line summary of a turn's tool use that heads each agent reply. It expands to steps, and each step expands to its details.
- **State card** — the panel an agent's Chat shows in place of the conversation controls when the agent is not running (failed, stopped, starting, stopping).
- **Action registry** — the single list of named actions (navigate, run a command, lifecycle, create agent, feedback…). The command palette and the composer's `/` menu both draw from it.
- **Legacy view** — an existing component hosted inside the new shell until the work unit that replaces it lands.
- **Integration branch** — the long-lived branch where frontend work lands before cutover to `main`.
- **Work unit** — one subagent-sized piece of implementation, defined in `phases.md`.

## Shape

### 1. Visual system

**Palette (fixed).** The owner is attached to the colors. Every value below is a design constant; the rest of the design derives tints and alphas from them and adds no new hues.

| Role | Values |
|---|---|
| Stone surfaces | `#0e0e10` base, `#131315`, `#161618`, `#1c1c1f`, `#26262a`; input `#141416` |
| Lines | `#2a2a2e`, `#222225` |
| Text | primary `#e8e8ea`, secondary `#9a9a9f`, dimmest allowed `#85858b` |
| Vein (accent, focus, primary actions) | `#3b8bdb`, bright `#5aa3f0`, dim `#2a6cb5`, hover `#3175c2`, plus tints at 14%, 7% and 35% alpha |
| Moss (user messages, positive state) | `#6b7a4a`, text `#8a9e62`, tint 20% alpha |
| Error | fill `#c0392b`, text `#e0675a`, tint 13% alpha |
| Overlay | scrim `rgba(6,6,8,.62)`; floating shadow for menus, sheets, palette and dialogs only |

The two text adjustments (`#85858b` instead of `#6a6a6f`, and the lighter error text) exist to meet contrast. Every text/background pair must reach WCAG AA (4.5:1 for body text, 3:1 for large text and UI glyphs).

**Type.** The product face for all UI and message text is Onest (weights 400, 500, 600). JetBrains Mono (400, 500) is used only for code, file paths and ids. Cinzel appears only in the wordmark. The type scale is 12, 13, 14 (UI), 15 (messages), 17 (headings) and 20px (page titles). Fonts are self-hosted and bundled with the app: no request goes to a font CDN. This is needed for offline launch and removes a render-blocking third-party request.

**Shape, density and motion.**
- Radii are 6, 8 and 12px. Density is compact.
- Grouping comes from surface tone and spacing, not outlines. Borders are reserved for inputs and the few places where a hairline separates regions.
- Floating layers (menus, popovers, sheets, dialogs, palette, toasts) carry the floating shadow. Nothing else does.
- UI transitions run 150–200ms with an ease-out curve, and every animation is disabled under `prefers-reduced-motion`.
- The grain overlay and the time-of-day vein intensity are dropped.

**Layout constants.**
- Rail width 248px.
- Default context panel width 440px, resizable between 360px and 50% of the viewport.
- Reading column max 720px.
- Phone bottom bar 60px plus the bottom safe-area inset.
- Breakpoints:
  - phone ≤ 760px
  - medium 761–1180px, where the context panel floats over the main region
  - wide > 1180px, where the context panel sits beside it

  These are the only breakpoints.
- A z-index scale covers base, sticky, panel, drawer, overlay, palette and toast.

All of the above live in one token set. Components use tokens only: no literal colors, font sizes, z-indexes, durations or easing curves. A lint guardrail enforces this (see §10).

**Components.** A shared primitive layer, built on native elements where they fit (`<dialog>`, the Popover API, `inert`):
- Buttons: Button (primary / secondary / quiet / danger) and IconButton.
- Form fields: text, number, select, toggle, segmented control, and a secret field that shows the stored / from-environment / replace states.
- Status and structure: Badge, status dot, Disclosure, Tabs, EmptyState, Skeleton, Banner, Kbd.
- Floating layers: Menu, Popover, Tooltip, Dialog, Sheet (phone bottom sheet), Drawer.
- Toast region.

Floating layers share one focus-management model: they trap focus while modal, restore it on close, close on Esc and scrim click, lock scrolling behind a modal, and make the background `inert`. Every surface is built from these primitives.

### 2. Shell and navigation

**Desktop and medium widths.** The rail, top to bottom:
- The wordmark, and "Search or jump to" (opens the command palette; shows the shortcut).
- **Home**, with a count of needs-you items.
- **Inbox**, with the unread count across all agents.
- **Agents**:
  - The heading carries a "+" that opens Create agent.
  - Each agent row shows its state dot, name, unread badge, and a state word when it isn't running.
  - Clicking an agent row only expands or collapses its places (Chat, Activity, Schedule, Files). It never navigates. Only one agent is expanded at a time, and clicking the expanded agent collapses it.
  - The agent being viewed keeps its highlight even when collapsed, and starts expanded on load.
  - Rows expose `aria-expanded` and toggle with Enter and Space.
- **Team**: Workbench and Shared files.
- Footer:
  - a settings gear, which opens the Settings modal scoped to the agent being viewed, or to "All agents" when no agent is being viewed
  - a help menu with Keyboard shortcuts, Send feedback, Report a bug, and "Install app" when available

**Phones (≤ 760px).** A bottom bar with, left to right:
1. ☰ — opens the rail as a left drawer: scrim, closes on scrim tap, swipe left or Esc.
2. Inbox, with unread badge.
3. Home, centered, same weight as the others.
4. Search — opens the command palette full-screen.
5. Settings — opens the settings list for the current scope.

The bar stays visible on every page, including Chat, with the composer sitting above it. The drawer, the palette and bottom sheets cover the bar with a scrim. There is no separate agent switcher; the drawer lists agents. Every page has a compact title bar naming the place and, inside an agent, the agent.

**Main region per place.** Each place has a header with its title and place-specific actions. Agent Chat's header shows:
- the agent name and role
- a running-sessions pill that opens Activity
- a gear that opens Settings for that agent

**Connection state is only shown when degraded.**
- If the hub socket is down, a banner at the top of the main region says so and offers Retry.
- If an agent's socket is down while that agent is *running*, the composer shows "Reconnecting — N messages will send once back online".
- A non-running agent never shows reconnecting (see §4).

### 3. Routes

The URL records the place, the context-panel content and whether the Settings modal is open. Client routes contain no dots and never start with `/api` or `/ws`; the server's SPA fallback depends on this.

| URL | Shows |
|---|---|
| `/` | Redirects to `/home` |
| `/home` | Home |
| `/inbox` | Inbox (all agents); `?agent=<name>` filters |
| `/agent/:name` | That agent's Chat |
| `/agent/:name/activity` | Activity |
| `/agent/:name/schedule` | Schedule |
| `/agent/:name/files` | Files (agent workspace) |
| `/team/workbench` | Workbench list |
| `/team/workbench/:artifact` | An artifact; `?full` fills the window |
| `/team/files` | Shared files |

**Query parameters on any route:**
- **Context panel:** `panel=session:<runId>`, `panel=file:<path>` or `panel=size`. Values are URL-encoded.
- **Settings modal:** `settings=<scope>/<section>`. `<scope>` is an agent name or `all`; section ids are listed in §8. Example: `/agent/brittle?settings=brittle/model`.

**History:**
- Opening a place, opening a session in the panel, and opening the Settings modal push an entry. Back therefore closes the modal and the panel, which matches the Android back gesture in the installed app.
- Switching settings sections, switching scope inside the modal, toggling the panel's file, and correcting a malformed URL replace the current entry.
- The drawer, palette, menus, sheets and Create agent are not in the URL.

**Redirects from existing URLs** (all via replace):

| Existing URL | Redirects to |
|---|---|
| `/team` | `/home` |
| `/agent/:name/sessions/:runId` | `/agent/:name?panel=session:<runId>` |
| `/agent/:name/workspace`, `?workspace` | `/agent/:name/files` |
| `/agent/:name/scheduled` | `/agent/:name/schedule` |
| `/agent/:name/settings/:old` | `/agent/:name?settings=<name>/<mapped>` |
| `/team/settings/:old` | `/home?settings=all/<mapped>` |
| `/settings/…`, `/workbench/…`, `/scheduled`, `/sessions/:runId` | Resolved under the last-used agent as today, then mapped as above |
| `/notification/<id>` (the macOS notification "Open" action) | The Inbox |

The old-to-new settings section mapping is in §8.

### 4. Agent Chat

**Feed.**
- Agent replies render as unboxed prose in the reading column, as sanitized Markdown with code blocks.
- User messages are moss-tinted bubbles, right-aligned. A sender line appears when the message came from another interface or an artifact.
- Messages from sessions and teammates render as compact cards: sender, kind, clamped body with "Show all", and "Open session" for sessions only.
- Day dividers, episode dividers and the compressed-history marker remain. The marker is explained in plain words: "Older messages are summarized. <agent> remembers what was said, not the exact wording."
- The chat has one empty state.
- All existing feed behaviors in `parity.md` are kept: lazy-loaded older episodes with a stable scroll anchor, follow-at-bottom with a Jump-to-latest pill, reconnect reconciliation, and turn undo on user messages. Each code block gains a copy button.

**Activity line.** Every agent reply that used tools starts with one line.
- **The summary line.** It combines friendly step labels, for example "Searched memory, read 2 files, started a research session".
  - It shows the turn's duration when the turn was observed live.
  - It flags failures ("1 step failed").
  - It is collapsed by default.
- **Expanded,** it lists the steps in order, each with a friendly label and its target: a file path, a query, or a session.
- **Expanded again,** a step shows its arguments and formatted result, using the existing per-tool argument summaries and result formatters.
- **Friendly labels** come from one table keyed by tool name. It covers every built-in tool the chat currently summarizes, and falls back to "Used <tool>" (with the server name for MCP tools).
- **History:** tool calls are present in chat history and session transcripts, so the line renders for past turns as well. Durations are not available for past turns, so none is shown.
- **The client always requests tool frames** (it sets the per-connection verbose flag on every connect). The `/verbose` toggle is removed. The native macOS client uses its own connection, so this does not affect it.

**Live turn.** While a turn runs:
- The activity line is expanded and appends steps as tool frames arrive.
- It shows an elapsed timer and a Stop control. Esc stops the turn while the composer has focus.
- Intermediate `broadcast_response` texts appear as they arrive, and the final response replaces the live state.
- There is no simulated typing. The backend delivers whole responses.
- When the turn ends, the line collapses to its summary.
- Post-turn memory work ("updating memory", "reviewing turn") appears as a quiet status line under the last reply until it finishes.

**Composer.**
- Auto-growing text area; Enter sends, Shift+Enter adds a new line.
- Images attach by button, paste or drop, with the existing type and size limits.
- A `/` button, and `/` typed as the first character, open the chat-scoped actions from the action registry.
- A compact model + thinking control opens a popover (a sheet on phones). It shows the main model and thinking level, writes through the config write coordinator (§8), and updates whenever settings change.
- Send becomes Stop while a turn runs and the composer is empty; typing brings Send back, so a steering message can go mid-turn.
- The draft is kept per agent across navigation and reloads, in local storage. It is cleared on send.

**State cards.** When the agent's `AgentSummary.state` is not `running`, the composer is replaced by a state card:
- **Failed:**
  - "<agent> couldn't start" and the error in plain words, with the raw message behind a disclosure.
  - Restart.
  - A settings link to the most relevant section: a `providers.toml` error opens Model, `mcp.json` opens Tool servers, `channels`-related errors open Connections, and anything else opens Raw config, where the diagnostics show.
- **Stopped:** "<agent> is stopped", Start, and a "Start automatically" toggle.
- **Starting / stopping:** a progress line with no actions.

Restart and Start failures surface in the card with the new error. The conversation above the card stays readable: the chat history endpoint serves non-running agents (§9).

**Conversation size.** The session token and context figures move out of the always-visible footer. The palette action "Show conversation size" and the chat header's overflow menu open them in the context panel as plain-language figures. Raw token counts appear in a details disclosure.

### 5. Activity, Schedule, Files

**Activity** (per agent) replaces the sessions sidebar and the session page.

- **Running now** lists every live run and every open outbound task, with plain-language kinds:
  - "From another app" for external
  - "Scheduled" for scheduled
  - "Started by <spawner>" for spawned
  - "From a workbench page" for artifact
  - "Sent to <agent>" for outbound tasks
- Each row shows purpose, how long it has run, state, and Stop. Outbound tasks that can't be reached keep the "Stop watching" fallback.
- **Finished** is one paged list with a kind filter, showing outcomes.
- Opening a run shows it in the context panel:
  - details (kind, started by, depth, remembered-as episode, run id)
  - notes (interrupted, failed with details, overlap)
  - the transcript, rendered with the same feed components as Chat
  - a message box that sends to the session, or resumes it if finished
  - Stop
- The panel follows a resumed session into its new run.

**Schedule** (per agent) replaces the Scheduled page, with the same content:
- Pulses: enable toggle, schedule, active hours, next run, last result, problems, running and overlap badges.
- Scheduled actions: due time and Cancel.

When loading fails, it shows the error and "Try again"; it never shows the empty state.

**Files** (per agent) and **Shared files** (team) keep the workspace capabilities:
- lazy tree
- identity-file tint (also in the team tree)
- editor with live validation, diagnostics, Save and Discard
- rename and move
- delete with Undo
- file history with diff and restore
- the save-conflict dialog

Two things are added:
- Leaving the place, closing the panel, or reloading with unsaved edits asks first.
- Files open in the context panel from links elsewhere (for example a path in a chat reply), using the same editor.

On phones the editor is a full-screen view with a back control.

### 6. Home

Home is the landing page (`/`). Its content sits in a centered container, max 1200px, beside the rail.

**Header.** "Home", counts (N running, N stopped, N can't start).

**Needs you.** Sorted by severity then time. Each item carries its fix inline, and disappears as soon as its condition clears.

| Condition | Shows | Actions |
|---|---|---|
| Agent failed | Plain-language error | Restart; settings link chosen as in §4 |
| Unread inbox item | The newest unread items, up to five, then "N more in Inbox" | Open (opens the item in Inbox) |
| Outbound task can't reach its agent | The task and how long it has been unreachable | Stop task; Stop watching |

**Agents board.** An aligned table (cards on phones).

**Columns:**
- **Agent:** name, role and unread badge.
- **State:** a dot and a word.
- **Now, and its last message:**
  - While busy: "Working on a reply" plus how long.
  - Otherwise: the purpose of the most recent live session, or "Idle".
  - Under it: the last message's time and preview.
- **Running:** the live session count.
- **Next up:** the next pulse or action with its time, or "Won't run while stopped".
- A "…" menu:
  - Open chat
  - Start / Stop / Restart
  - Start automatically
  - Settings
  - Delete, with the existing confirm-then-undo flow

**Create and restore.**
- **New agent:** a button on the Agents heading row opens the Create agent dialog (a sheet on phones). The rail "+" and the palette open the same dialog.
- **Dialog fields:**
  - name, with live validation
  - "What should it help with?", which becomes the description
  - under "More options": "Copy model settings from" and who can find it
- **Reserved names:** `home`, `inbox`, `all`, `team`, `settings`, `workbench` and `shared-files` are rejected alongside the backend's own rules.
- **On create:** the dialog closes, the agent appears in the rail and on the board, a toast confirms it, and the user stays where they were.
- **Recently deleted** is a collapsed disclosure under the board, with Restore.

**Right column** (below the board under 1180px):
- **Across the team:** the newest team events, with time and agent.
- **Coming up:** the next scheduled pulses and actions across agents, soonest first.

### 7. Inbox

- One Inbox across all agents, with an agent filter and an Archived tab.
- Items show title, agent, source and time, plus an unread marker.
- Opening an item marks it read and shows the body as sanitized Markdown, with attachment downloads.
- Items can be archived and restored.
- The unread total drives the rail badge, the bottom-bar badge, the Home needs-you items and, when installed, the app icon badge (Badging API where supported).
- "Add a note to <agent>'s inbox" (the old `/inbox` command) is an action in the registry.

### 8. Settings

**Frame.**
- A large modal (full-screen on phones).
- At the top, a scope picker: "All agents" first, then each agent with its state dot. A line under it says what the scope affects: "Applies to every agent" or "Only affects atlas".
- **Section list.** Left on desktop. On phones the list is its own screen and a section opens full-screen with Back; the scope picker appears only on the list screen.
- **What it opens on:**
  - The scope of the agent being viewed.
  - "All agents" when opened from Home, Inbox, Workbench or Shared files.
  - Whatever a deep link names.
- Switching sections or scope swaps only the content pane: no re-mount or re-animation of the modal, and at most a 120ms fade on the content.
- When the new scope has the current section, switching scope keeps it.

**Saving.**
- Each scope has explicit **Save changes** and **Discard** in a save bar that appears only when that scope has unsaved changes.
- Unsaved changes are kept per scope while the modal is closed or another scope is selected, and the save bar reappears on return. They are lost on reload, which asks first when there are any.
- A save sends only the diff for each file in that scope, through the config write coordinator:
  - A PATCH validates before writing.
  - Field-level errors show inline next to the field when a diagnostic carries a path location. Otherwise they show at the top of the section.
  - A partial failure says which files saved and which didn't.
- A successful save that returns a checkpoint offers Undo.
- Secret fields exchange a typed value for a stored secret on save, as today.
- An agent scope never edits install-wide files, and an All-agents scope never edits an agent's files. Install-wide values that affect an agent are shown on the agent page read-only, with a link to change them for all agents.

**Config write coordinator.** A single client-side service through which every config write goes: settings saves, and the composer's model and thinking control.
- It serializes writes per file.
- It re-reads the file when another writer changed it.
- It notifies subscribers after every successful write or reload, so every view showing a config value refreshes.
- It replaces the current lock, which only the composer used.
- A reload from disk, or a checkpoint restore from History, goes through it too, so open forms refresh.

**Sections.** Section ids are shown in brackets.

*Agent scope:*

| Section | What it holds |
|---|---|
| **Model** (`model`) | Provider connections: add or remove a provider, type, API key, base URL, Ollama keep-alive. Main provider and model, thinking level and temperature. "Use different models for specific jobs" disclosure with each role named for what it does: Summarizing older messages (observer), Condensing memories (reflector), Regular checks (pulse), Reviewing turns (subconscious), Background sessions small/medium/large, Search index (embedding). Failover model lists are preserved; the form never collapses a list to one entry. |
| **Connections** (`connections`) | Discord, Telegram, Teams: connected state, token or IDs, "Let others talk to this agent", context messages; Teams listener port. Incoming webhooks: name, secret, routing, format, content fields, route preview. |
| **Tools & skills** (`tools`) | Skill folders, tool PATH folders, web search backend and key, provider-native search options. |
| **Memory** (`memory`) | When to summarize and condense: the observer and reflector thresholds, expressed in plain words with the numbers shown. Cooldown and force threshold. Learning from conversations and reviewing turns (the subconscious settings). Search tuning under "More options". |
| **Schedule** (`schedule`) | Regular checks on or off (pulse enabled). How long idle background sessions stay open, per kind. Episode skip floor. Nesting depth cap. |
| **Advanced → Runtime** (`runtime`) | Reply time limit, reply length, retries, agent abilities (allowed changes, tool-call caps, repeat-call guard, steer and stop thresholds), idle timeout and idle channel. |
| **Advanced → Tool servers** (`servers`) | MCP servers: list, remove with Undo, add (stdio or http), catalog. |
| **Advanced → Agent-to-agent** (`a2a`) | Who can find this agent (the only place this is set), status, public or local URL, remote agents with the raw editor, card preview. |
| **Advanced → Raw config** (`raw`) | Editors for `config.toml`, `providers.toml` and `mcp.json` with live diagnostics. A raw save always writes, as today. |
| **Advanced → History** (`history`) | Checkpoint browser for the workspace and agent-config repos. |

*All agents scope:*

| Section | What it holds |
|---|---|
| **General** (`general`) | Timezone. Gateway bind address and port under "More options". |
| **Notifications** (`notifications`) | This device's push subscription and per-event toggles, and the other devices list (§11). |
| **Residuum Cloud** (`cloud`) | Every existing connection state and action. Relay URL and local port under "More options". |
| **Saved keys** (`keys`) | Two lists: keys agents can use as environment variables (agent keys) and stored secrets referenced by settings. Add, remove, and Undo where the backend returns a checkpoint. |
| **Updates** (`updates`) | Status, check, update and restart, as today. |
| **Session limits** (`limits`) | Concurrent background turns, hop soft and hard limits. |
| **Advanced → Agent-to-agent** (`listener`) | The listener toggle, port, own address, and caller keys. |
| **Advanced → Diagnostics** (`diagnostics`) | Log detail, redaction, automatic error reports. |
| **Advanced → Raw config** (`raw`) | Install-wide `config.toml`. |
| **Advanced → History** (`history`) | Team and hub repos. |

**Old section mapping** (for redirects):

| Old section | New section |
|---|---|
| agent `runtime` | `runtime` |
| `providers` | `model` |
| `channels`, `webhooks` | `connections` |
| `pulses` | `schedule` |
| `memory` | `memory` |
| `skills` | `tools` |
| `mcp` | `servers` |
| agent `a2a` | `a2a` |
| agent `history` | `history` |
| hub `general` | `general` |
| `cloud` | `cloud` |
| hub `a2a` | `listener` |
| `sessions` | `limits` |
| `tracing` | `diagnostics` |
| `update` | `updates` |
| `secrets`, `agent-keys` | `keys` |
| hub `history` | `history` |

The Simple/Advanced/Raw mode toggle is removed. Advanced sections and "More options" disclosures replace it.

**Destructive actions keep the existing model.** A single click, then Undo where the backend returns a checkpoint. Deleting an agent and removing a secret keep their confirmations. No new confirmation gates are added.

### 9. Backend contracts (additive)

The new surfaces need data the hub doesn't expose today. All changes are additive. Existing endpoints and frames keep their shape, because the macOS client and older web builds consume them. New timestamps are RFC 3339 UTC. New hub frame and envelope types are exported to TypeScript by the existing generator, so the web client stops hand-writing hub types.

1. **Activity in the snapshot.** `agents_snapshot` and `GET /api/hub/agents` include each agent's activity (`busy`, `unread`, and a new `busy_since`), so a fresh page shows correct badges. The `agent_stopping` frame is added to the exported types and to the hub HTTP doc.
2. **Chat history for non-running agents.** `GET /api/agents/{name}/chat/history` is served for stopped and failed agents from persisted history, like the existing repair routes. It returns the same shape.
3. **Overview.**
   - `GET /api/hub/overview` returns, per agent:
     - the summary and activity
     - `last_message {role, preview (≤ 200 chars, plain text), at}`
     - `live_sessions [{address, run_id, category, purpose, state, started_at}]`
     - `next_scheduled {kind: pulse|action, name, at} | null`
     - `inbox_unread`
     - `outbound_problems [{task_id, agent, status_text, unreachable_since}]`
   - Stopped and failed agents are included: their last message and inbox count come from disk, and they have no live sessions or next run.
   - The hub WebSocket sends `agent_overview {name, overview}` whenever any of those fields change for an agent, coalesced to at most one frame per agent per second.
   - The `next_scheduled` time respects each pulse's active hours.
4. **Team events.**
   - The hub keeps a bounded in-memory log of the most recent team events: 500 entries, reset on hub restart. Each event has:
     - `id` (monotonic)
     - `at`
     - `agent?`
     - `kind`
     - `level: info|warn|error`
     - `summary`, a plain-language sentence
     - `target?`, a place to open: an agent place, a session run, or an inbox item
   - **Kinds:**
     - agent started, stopped, failed (with reason), created, deleted, restored
     - agent replied (a main turn ended with a reply)
     - session started and finished (with purpose and outcome)
     - inbox item added
     - scheduled run finished
     - hub notice (the existing notices, which today are never shown after their toast)
   - `GET /api/hub/events?before=<id>&limit=<n>` pages backwards.
   - The hub WebSocket sends `team_event {event}` for each new entry.
5. **Cross-agent inbox.**
   - Hub endpoints list inbox and archive items across all agents, including stopped and failed ones. Each item carries its agent name.
   - Hub endpoints mark an item read, archive it and restore it, addressed by agent and item id.
   - Adding, reading, archiving or restoring an item updates the overview's `inbox_unread` and emits a team event when an item is added.
   - Errors are JSON `{error}`.
   - Inbox timestamps in these responses are RFC 3339 UTC.
   - The existing per-agent inbox endpoints remain.
6. **Serving.** The embedded SPA is served with:
   - `Cache-Control: public, max-age=31536000, immutable` for hashed build assets
   - `no-cache` for `index.html`, the service worker and the manifest
   - gzip or brotli compression for text assets
7. **Web Push** (§11): VAPID keys, device subscriptions, and delivery.

### 10. Frontend test and quality setup

The test layers:

| Layer | Environment | What it covers |
|---|---|---|
| Unit | Node | Stores, routing, formatters, settings model, action registry, activity-line labeling. Unchanged tooling. |
| Component | jsdom + Testing Library | Every primitive and every surface's key states (empty, loading, error, populated, live). Shared fixtures build hub, agent and feed state without a socket. |
| End-to-end | Playwright against the mock server | Real navigation and flows on two projects: desktop 1440×900 and phone 390×844 with touch. Both run in Chromium in CI. A WebKit phone project is available locally for iOS-like checks. |
| Accessibility | axe-core inside the end-to-end suite | Every place and overlay is scanned. Serious and critical violations fail the run. `svelte-check` fails on accessibility warnings. |
| Visual | Playwright screenshots | A small set of baseline screenshots per surface at both sizes. Times and animations are frozen, and dynamic regions are masked. Baselines are updated deliberately with a dedicated command. |

**Mock server as a first-class harness.**
- It is split into modules and type-checked, linted and formatted like the app.
- It has a deterministic mode (fixed clock, configurable delays with zero as the test default) and a reset endpoint, so each test starts from the same scenario.
- It implements every endpoint the app calls, including the new contracts in §9.
- A test compares the routes the API client calls against the routes the mock implements, so they cannot drift.

**Guardrails.**
- **Token lint:** a style linter checks global and component styles for token use (colors, font sizes, z-index, durations, easing). Legacy styles sit on an ignore list that must be empty at cutover.
- **Strict typing:** rune store modules get the same strict TypeScript lint rules as other modules.
- **Generated types:** CI regenerates the TypeScript types from Rust and fails on any difference.
- **Pre-commit:** runs on any change under the web app, including the mock server and config. It runs format, lint, type check, and unit and component tests. End-to-end, accessibility and visual tests run in CI and on demand through a `just` recipe.
- **Coverage** is reported in CI without a threshold.
- **Bundle size:** the initial-route size is reported in CI. A budget is set from the measured size after code splitting (§11) and enforced from then on.

### 11. PWA

**Installability.**
- **Manifest:**
  - `id` and `start_url` of `/home`
  - `display: standalone`
  - background and theme color `#0e0e10`
  - maskable icons, verified
  - shortcuts to Home and Inbox
- **iOS:** the standalone metas, a black-translucent status bar, and `viewport-fit=cover`. The shell honors the safe-area insets everywhere, including the bottom bar and full-screen sheets.
- **Install prompt:** where the browser supports one, "Install app" appears in the help menu and the palette. On iOS, the same entry explains Add to Home Screen.

**Service worker.** A small hand-written worker at the site root:
- **Caching:**
  - It precaches the app shell: `index.html`, hashed assets, fonts, icons.
  - It uses a build-generated asset list and a versioned cache name, and deletes old caches on activate.
  - Navigations are network-first, falling back to the cached shell.
  - `/api` and WebSocket traffic are never cached or intercepted.
- **Offline:** the app launches to the shell. The hub banner explains that Residuum can't be reached, and offers Retry. No data is cached for offline reading.
- **Updates:** a new worker waits. The app shows "Update ready" with a Reload action, and reload activates it.
- **Push:** the same worker handles push events and notification clicks (below).

**Code splitting.** Settings, the file editor, the workbench artifact host and the command palette load on demand. Home, Chat and the shell are in the initial bundle.

**Web Push.**
- **Keys:** the hub generates and stores a VAPID key pair once, in its existing secret store.
- **Endpoints:**
  - return the public key
  - register a subscription for this device, with a device label and per-event preferences
  - list the registered devices
  - update a device's preferences
  - remove a device
  - send a test notification
- **Events a device can opt into:**
  - a new inbox item (default on)
  - an agent that can't start (default on)
  - an outbound task unreachable for more than 15 minutes (default off)
  - a reply that arrived while no web client was open (default off)
- **Delivery:**
  - The hub sends pushes directly to the browser's push service over HTTPS, with standard Web Push encryption.
  - A subscription the push service reports as gone (404 or 410) is removed.
  - Delivery failures are logged at warn level with the device label, and never block the triggering operation.
- **Notification content:** plain-language title and body, plus a target URL. Clicking opens or focuses the app at that URL.
- **Settings → All agents → Notifications** manages everything:
  - this device's permission and subscription
  - its per-event toggles
  - a test button
  - the list of other devices, with Remove
  - on iOS, a note that notifications need the installed app
- This, together with the cross-agent unread count, resolves issue #105.

### 12. Stores and services

The data layer stays. Each existing module keeps its responsibility, with these changes:

- **Routing** implements the route model in §3.
  - Session and settings state move into the URL.
  - Stores never import the router. They expose data and commands, and views navigate.
- **The agent socket coordinator** no longer resets unrelated stores on an agent switch. Each store subscribes to the agent-change signal itself.
- **A new overview store** is fed by the hub socket's snapshot, `agent_overview` and `team_event` frames plus the overview and events endpoints. It is the only source for Home, the rail's badges and the needs-you items.
- **The inbox store** becomes cross-agent.
  - It uses the API client and surfaces errors instead of swallowing them.
  - It relies on the overview for counts, and fetches items when the Inbox opens or a count changes.
- **The settings model** is split per file and scope in place of one flat field object, and gains the save-bar state per scope. The config write coordinator replaces the current lock.
- **The action registry** is new (§2, §4).
- **Two duplications are removed:**
  - Turn counters duplicated between the main feed and session views become one shared helper.
  - The near-duplicate restore helpers in undo become one.

## Reasoning & alternatives

**Direction.** Three directions were built as clickable mockups and compared by the owner:
- Rail: agent-first sidebar, compact.
- Conversation: chat-first, roomy, serif replies, settings modal.
- Control Room: overview-first, three panes.

The combination was chosen because:
- Multi-agent work is what Residuum does that single-assistant apps don't, so agents belong in the primary navigation (Rail).
- A team overview answers "what needs me" at a glance (Control Room).
- A modal keeps settings out of the navigation while making every section reachable (Conversation).

**View-layer rewrite on the existing data layer.**
- The stores, socket handling, routing, undo and API client are well-tested and sound.
- The problems are in layout, components and styling. Rewriting those and keeping the data layer is less risky than a full rewrite, and faster than restyling in place.
- Restyling in place was rejected: the outline-box look, CSS sprawl and navigation model are structural, not cosmetic.

**Primitives built in-house on native elements** rather than adopting a component library.
- `<dialog>`, the Popover API and `inert` now cover the hard accessibility parts.
- The component count is small, and a library would add a dependency to track, plus styling overrides against the fixed palette.

**Home data from the hub rather than client fan-out.**
- Fanning out per-agent requests from the browser fails for stopped agents: their non-repair routes return 409.
- It also costs one request per agent per field, and gets no live updates without a socket per agent.
- The hub already owns agent state. An overview contract with change frames is one request plus one socket, and it also gives push and the app badge a single source.

**Team event log kept in memory.**
- "Across the team" needs timestamped history, and nothing records one today.
- A bounded in-memory log is enough for "what happened recently", and costs no storage format.
- It is reset on hub restart, and the restart itself is the first event after it.
- Persisting it would add a file format and retention rules for little gain.

**Cross-agent inbox at the hub.** The inbox is per agent on disk, and the per-agent routes need a running agent. A hub-level view is the only way to show one inbox that includes stopped agents.

**Explicit save in settings, not autosave.**
- Autosave validated half-typed values mid-edit, and raced the composer's model chip.
- Its failure feedback was a toast after the fact.
- An explicit save with per-scope unsaved state matches the mockup the owner approved. It lets a fix-then-restart flow (a failed agent's model) read naturally, and lets validation errors appear inline before anything is written.

**A hand-written service worker** rather than a PWA plugin.
- The worker has three jobs: precache the shell, fall back on navigation, and handle push. It stays small.
- Owning it avoids a build-plugin dependency and keeps push handling in the same file.
- The build step emits the asset list.

**Always-on tool frames with a collapsed summary** rather than a verbose toggle. Tool activity is the main thing a user wants to see about a turn. The summary line keeps it quiet, and the toggle only hid information behind a command most users would never find.

**Chromium-only end-to-end tests in CI.** The CI runners' ability to host WebKit's system dependencies is unverified. A WebKit phone project exists for local runs. Adding it to CI is a follow-up issue once the runner is confirmed.

**Integration branch for frontend work, `main` for backend contracts.**
- A half-converted UI must never ship to users, so frontend work collects on one branch and cuts over in one merge.
- The backend changes in §9 are additive, and harmless to the current UI, so they land on `main` as they are built. That avoids a long-lived backend divergence.
- The integration branch merges `main` regularly.

## External touchpoints

- **Hub HTTP and WebSocket.** The existing contracts are unchanged. The additions are in §9.
  - New frames are ignored by current clients: both the web client and the macOS client switch on the frame type.
  - New endpoints return JSON `{error}` on failure, with the status codes the hub lifecycle routes already use (400, 404, 409, 500, 503).
- **Agent HTTP and WebSocket.** No shape changes.
  - The web client sets the verbose flag on every connect.
  - Chat history gains non-running service (§9.2).
  - The native macOS client connects to the same socket, so the protocol must stay backward compatible.
- **Generated TypeScript types.** Produced from Rust by the existing export test into the web app's generated-types directory. CI regenerates them and fails on any difference.
- **Embedded asset serving.** The binary embeds the built web app.
  - The SPA fallback serves `index.html` for paths without a dot that don't start with `/api` or `/ws`.
  - The service worker and manifest are root-level files with dotted names, so they are served directly with correct MIME types.
  - Cache headers and compression are added (§9.6).
- **Residuum Cloud relay and tunnel.** Remote use goes through an HTTPS origin. That gives the secure context that service workers and push require.
  - The tunnel forwards HTTP with a 25-second timeout and a 10MB response cap. App-shell assets are well under that.
  - Whether the relay passes the service worker and the manifest through unmodified, with their headers and without an auth redirect on the worker script, must be verified against the relay project before the PWA work ships. If it doesn't, the relay needs a change in its own repository.
  - Plain-HTTP LAN access to the gateway has no secure context. The PWA and push features are unavailable there, and the app must degrade by hiding install and notification options.
- **Workbench artifacts origin and bridge.** Unchanged. The artifact host is restyled; the bridge's allow and deny rules and the artifacts origin stay as they are.
- **Web Push services.**
  - Outbound HTTPS from the host to the browser vendor's push endpoint, authenticated with VAPID and encrypted per the Web Push standards.
  - The push endpoint URL comes from the browser's subscription.
  - Failure modes: 404 and 410 remove the subscription, 429 and 5xx are logged, and a push is never retried more than once.
- **macOS notification bridge.** Its "Open" action opens `/notification/<id>`, which redirects to the Inbox (§3).
- **Fonts.** Onest, JetBrains Mono and Cinzel are bundled under their OFL licenses. No font CDN is contacted.

## Integration with existing system

- **Branches.**
  - Frontend work lands on the integration branch `feat/web-overhaul`, one PR per work unit.
  - `main` continues to ship the current UI until cutover.
  - Backend units (§9, Web Push) branch from `main`, merge to `main`, and reach the integration branch through its regular merges from `main`. The mock server changes that accompany a backend unit land with it.
- **Coexistence on the integration branch.** The new shell lands early and hosts legacy views for places not yet rebuilt. Examples:
  - The existing sessions sidebar content serves as the Activity place.
  - The existing settings page opens inside the Settings modal.

  Each surface unit replaces its legacy view and deletes the legacy components and styles it no longer needs. New tokens use names that don't collide with the legacy variables, so both style sets work side by side until cutover removes the legacy ones.
- **Unchanged:**
  - the data layer's socket transport
  - the feed and session stores' frame handling
  - the workbench bridge
  - the undo, checkpoint and pending-save helpers
  - the API client, extended with the new endpoints
  - the setup wizard's flow, which is restyled onto the new primitives; its steps and what it collects don't change
- **Replaced:**
  - the header and hamburger menu
  - the agent chip row
  - the sessions sidebar and session page
  - the Team page
  - the Scheduled page
  - the inbox drawer
  - the notification corner: toasts move to the toast region, and notice history to "Across the team"
  - the Settings page and its mode toggle
  - the help overlay, restyled as a dialog
  - the feedback modal, restyled on the Dialog primitive
  - the global stylesheet set
- **Documentation updated at cutover:**
  - the web app's contributing guide (routes, structure, testing)
  - its aesthetic guide, rewritten to describe this visual system
  - the systems-usage pages that describe the web UI or the new hub contracts (hub HTTP, inbox, notifications, heartbeats' Scheduled view reference, workbench), and their mirrors in the bundled `residuum-system` skill references
- **Cutover.**
  - One PR merges `feat/web-overhaul` into `main` once:
    - no legacy views remain
    - the style ignore list is empty
    - every item in `parity.md` is checked or listed as intentionally changed
    - all test layers pass
  - The design documents then move to `docs/archive/`.

## Open questions

Decisions made in this draft that the owner has not yet confirmed. Each is written into the design above as the recommended option.

1. Home's data comes from a new hub overview contract (§9.3), not browser fan-out.
2. "Across the team" is backed by an in-memory event log of 500 entries, reset on restart (§9.4), and it absorbs notice history.
3. The Inbox becomes one cross-agent list backed by new hub endpoints (§9.5).
4. Settings uses explicit Save and Discard per scope instead of autosave (§8).
5. The service worker is hand-written, not generated by a plugin (§11).
6. Web Push is in scope, with the four opt-in events and the defaults listed in §11.
7. Visual screenshot tests are in CI for a small set of screens (§10). CI end-to-end tests run on Chromium only.
8. Backend contract units merge to `main` directly; frontend units go to the integration branch (Integration).
9. The grain overlay and time-of-day vein intensity are dropped (§1).
10. The macOS notification "Open" link lands on the Inbox instead of the agent's files (§3).
