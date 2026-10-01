# Contributing to the Residuum Web UI

Welcome! This guide will get you up and running with the frontend without needing the Rust backend.

## Prerequisites

- **Node.js** `^22.13.0 || ^24.0.0 || >=26.0.0` (the `engines` range in `package.json`, set by the test tooling) — check with `node --version`
- **npm** — comes with Node.js
- **Docker** — only for the visual comparisons and the WebKit project (see [Testing](#testing)); everything else runs without it

## Getting Started

```bash
cd web
npm install
npm run dev:mock
```

Open [http://localhost:5173](http://localhost:5173) in your browser. That's it — no backend required.

With [`just`](https://github.com/casey/just), `just web-mock` from the repo root does the same (installing dependencies first if they are missing or out of date), and `just web-mock-setup` starts in setup wizard mode. Extra arguments go to Vite: `just web-mock --port 5199`. `just web-mock-serve [port]` and `just web-mock-preview [port]` start the deterministic mock headlessly (see [Deterministic mode](#deterministic-mode) and [Preview mode](#preview-mode)).

[http://localhost:5173/dev/gallery](http://localhost:5173/dev/gallery) shows every primitive control in every state. The dev server serves it, and so does a mock build; a production build leaves it out.

## Mock Mode

`npm run dev:mock` starts the Vite dev server with a built-in mock server that fakes all API endpoints and WebSocket connections. You'll see this in the terminal:

```
[mock] API mock server active
[mock] Mode: running (set VITE_MOCK_SETUP=1 for setup wizard)
[mock] Hub WebSocket on /api/hub/ws, agent WebSockets on /api/agents/{name}/ws
```

### What's mocked

- All REST endpoints return realistic fake data
- WebSocket simulates chat responses with tool calls and delays. Like the backend, it sends tool calls and results only to pages that turned verbose mode on, reads client frames strictly (a frame it can't read gets an `error` frame), and `/stop` ends a running turn
- Scoped routing, as the backend has it: an agent's path (`/api/agents/{name}/...`) reaches only what an agent owns, the hub's (`/api/hub/...`) only what the hub owns, and the team's (`/api/team/...`) only the workbench and team files. `/api/agents/scout/workbench/artifacts` and `/api/hub/workspace/files` answer `404`, like the real routes
- The multi-agent hub contract: four agents (`scout` and `atlas` running, `drifter` stopped, `brittle` failed), each with its own chat, sessions, inbox and config under `/api/agents/{name}/...`. `/api/hub/agents` lists, creates, deletes, starts, stops and restarts them and toggles autostart, and `/api/hub/ws` sends the agent snapshot, state changes, busy/unread activity and notices. Unscoped `/api/...` paths answer 404. A stopped or failed agent serves the routes the backend serves for one: the repair routes (config, providers, MCP, workspace, checkpoints) and the file-only ones (chat history, usage, inbox, raw A2A settings). Every other agent route answers `409`, and an unknown agent `404`. An agent that has never run (`drifter`, `brittle`) has no conversation, inbox or A2A agents until it first runs. Team files and the workbench are shared under `/api/team/...`. `POST /api/mock/teammate-message?agent=atlas` sends atlas a teammate message (with the backend's teammate header and sender) and lights its unread indicator until you open it. brittle fails every start on a broken model setting until `POST /api/mock/fix-agent?agent=brittle` fixes it, the way a user would in Settings; its next start succeeds
- Agent sessions: live sessions (including a Discord conversation session) and a page-able list of finished ones. Messaging a session simulates a turn (include "busy" in the message to see a delivery failure), messaging a finished one resumes it, and a chat message starting with `spawn` starts a spawned session that relays its result to the main chat. Transcripts load after a short delay, so the loading state and anything racing it can be tried by hand
- The `POST /api/agents/{name}/sessions` / `.../stop` / `.../messages` HTTP endpoints an artifact's `residuum.sessions.start` uses: the bundled "Tip Splitter" artifact (`/team/workbench/tip-splitter`) has "Start a background session" and "Fire 3 calls at once" buttons for trying the artifact bar's activity panel, Cancel calls, and Stop page by hand. Like every artifact it names its agent, `atlas`, in `ask` and `sessions.start`, and its sessions' frames reach it through the hub socket's session relay, whichever agent the web UI has open. Model calls are slowed down (`MODEL_CALL_DELAY_MS`) so they're visibly "in flight" long enough to cancel
- The hub socket's session relay. A page subscribes with `subscribe_session` (`{ agent, address }`) to one session, or with `subscribe_artifact_sessions` (`{ artifact }`) to every session whose source label is `artifact:<artifact>` on any agent, including ones that start later, and stops with the matching `unsubscribe_` message. Each subscribe is acknowledged with `subscribed` before any frame, and one that names an unknown agent gets a warning `notice` and no acknowledgement (a stopped agent is known). Every `session_*` frame an agent's state broadcasts then reaches the pages that follow it as `session_frame` `{ agent, frame }`, where `frame` is exactly what the agent's socket sent, tool frames included, because the hub socket has no verbose flag. Subscriptions end with the connection. `POST /api/mock/session-relay-lag` sends `session_relay_lagged` to every page that follows something, as a connection that fell behind the relay is told, and answers `{ "notified": n }`; the page then reads its sessions again over HTTP
- Tasks sent to other agents in the sessions sidebar's External group: stopping `research-buddy`'s task succeeds, while `laptop` is unreachable, so its Stop fails and the row offers "Stop watching"
- `POST /api/mock/hub-socket` with `{ "online": false }` takes the hub WebSocket down: every page is dropped and new connections are refused (the HTTP API stays up), so the hub banner shows. `{ "online": true }` lets pages connect again, and a reset does too
- `POST /api/mock/missed-relay` records a session result in the main chat's history and drops the WebSocket, to exercise catching up after a reconnect
- The team event log: `GET /api/hub/events` (with `before`, `after` and `limit`) and a `team_event` frame on the hub socket for each new entry. It records what the mock's own lifecycle, chat turns, sessions and notices do, worded as the backend words it, with ids counting from 1 and times from the mock's clock. It starts with `hub_started` and what starting the scenario's agents did, and starts over on reset. `POST /api/mock/user-inbox-add?agent=atlas` (`{ title?, body?, attachments? }`, each attachment `{ filename, mime_type? }`) saves an item in an agent's user inbox the way its `user_inbox_add` tool does, which adds an `inbox_item_added` entry
- The team overview: `GET /api/hub/overview` and an `agent_overview` frame on the hub socket whenever an agent's overview changes. Each agent's last message, live sessions and unread inbox count are read from the data the mock's other routes serve (its conversation, sessions and inbox), `upcoming` lists its three soonest runs (its pulses by their next fire time, and its pending scheduled actions) and `outbound_problems` its open tasks to other agents that have been unreachable for ten minutes, and changes are gathered so an agent gets at most one frame per simulated second (the next tick when delays are off). A created agent is sent at once. It starts over on reset
- Push presence: the hub socket accepts `{ "type": "presence", "device_id", "active" }` and keeps what each connected page reported on the mock clock, as the hub does. A device is present for 60 seconds after an `active: true` report from a page that is still connected, and `active: false` or the page disconnecting ends it; a frame without a `device_id` string and an `active` boolean is refused with the warning `notice` any unreadable frame gets, and a valid one gets no answer. `GET /api/mock/push/presence` answers `{ "devices": [...] }`, the devices the real hub would send no push right now. The mock sends no pushes.
- Workspace files, for an agent (`/api/agents/{name}/workspace/...`) and for the shared team tree (`/api/team/workspace/...`): directory listings with size, modification time and version, reads with the version as the `ETag`, writes that answer `412` when the client's `If-Match` no longer matches, and delete, move, validate, `dir`, `raw` reads and writes, the recursive `tree` (with `glob`, `depth` and `content`) and the batch `read` with the backend's size budgets. Edits change the listings, and the team tree is the same one under an agent's `team/`
- The Schedule place's routes (`/api/agents/{name}/scheduled/...`): the pulses, with their next fire, last outcome and current run worked out from the agent's sessions the way the backend reads them, toggling a pulse, and the pending actions with cancel. One pulse is disabled and one failed to load. Like the backend, they answer `409` for an agent that isn't running
- Checkpoints, for an agent (`workspace` and `agent_config` repositories) and for the hub (`hub` and `team`): list with `path`, `turn_id` and paging, stats, a checkpoint's detail, diff and file, restore and undo. Each repository keeps the whole tree of each checkpoint, so a restore writes the files back (Settings, the workspace and the team tree show it) and an undo skips a path that changed again since. A route answers `400` for a repository of the other scope, like the backend. The sample histories end at the live files, and `status` reports their stats
- `POST /api/agents/{name}/agent-inbox` (what an artifact adds to the agent's own inbox, with the backend's ids, title default and `artifact:<name>` source), and the update routes (`/api/hub/update/status`, `check` and `apply`; the mock is always on the latest version) with `cloud/disconnect`
- The user inbox: a listing, an archive, mark read, archive, restore and attachments (`/api/agents/{name}/inbox/...`), with the backend's response shapes, including its `500` for an item that isn't there. No sample item carries an attachment, but `POST /api/mock/user-inbox-add` can give a new one some; an attachment serves a stand-in file of its type
- The workbench (`/api/team/workbench/...`): the artifact list, where artifacts are served, and deleting an artifact along with its saved state. Artifacts are files in the team tree, `team/workbench/<name>.html` or a folder `team/workbench/<name>/index.html` with the files it loads, found the way the backend finds them (a folder wins over a page of the same name; `<name>.*` data files aren't part of an artifact). A delete checkpoints the team first and returns the checkpoint's id, so Undo (a restore of each removed path through the hub's checkpoint routes) brings back the page or folder and its data files. `POST /api/agents/{name}/model/complete` answers from a canned model, with parsed JSON when the call asks for a schema
- The artifacts listener, on its own port, serves those files as the real listener does: `/{name}/` is the page, `/{name}/{path}` a file in a folder artifact (a trailing `/` means that folder's `index.html`), `/{name}` redirects to `/{name}/` with a `308` that keeps the query, and a path that climbs out of the folder, a missing artifact or file, and anything but `GET` and `HEAD` are refused with the backend's pages and statuses. Files go out with `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`, and every HTML file gets the SDK at the same place the backend puts it (after `<head>`, else `<html>`, else the doctype, else first)
- `/api` and everything under it on the artifacts port, WebSocket upgrades included, goes to the same API and the same hub and agent sockets as the app's own origin, so a page opened there reaches the mock directly. The listener refuses the backend's block list there with `403` and `{ "error" }`: shutdown, stop-all, update check, apply and restart, and setup completion (`/api/hub/shutdown`, `/api/hub/stop-all`, `/api/hub/update/{check,apply,restart}`, `/api/hub/config/complete-setup`), whatever the method, while the app's origin still serves them. A socket opened there doesn't count as a client: it neither resets an agent's unread count nor shows in the agent's connected clients. An upgrade no socket route takes, such as an unknown agent, answers `404`, and no other path is forwarded. `api` is not an artifact name, so an `api` page or folder is neither listed nor served
- Live updates for team files, through a test control: `POST /api/mock/team-file` simulates an agent editing or deleting a team file. The body is `{ "path": "team/workbench/tip-splitter.html", "content": "<title>…" }`: `path` is in the file API's namespace (under `team/`), and `content` is the file's new text, or `null` to remove it (a folder goes with everything in it). The mock's files change, then its sockets send what the real change feed does: `workspace_changed` (one change, `created`, `modified` or `removed`; a new folder is reported alone, standing for what it holds) to the hub socket and to every agent socket whose `watch_team` or `watch_workspace` prefixes match (by whole path segments; a prefix above or at the path matches, and so does one below a removed folder), and, when the change touches an artifact's page or folder, `artifact_updated` or `artifact_removed` to every hub socket and every agent socket, whatever they watch, and with no agent running. The answer is `{ "changes": [...], "artifacts": { "updated": [...], "removed": [...] } }`, what was sent. An artifact's saved data doesn't count as the artifact, and a rewrite that leaves its files as they were sends no artifact frame. A path outside `team/`, or a folder to write to, answers `422`, and removing what isn't there `404`
- Live updates for an agent's own files, the same way: `POST /api/mock/agent-file?agent=atlas` with `{ "path": "notes/plan.md", "content": "…" }` (a path in the agent's own workspace, `content: null` to remove it) changes the file and sends `workspace_changed` to that agent's sockets whose `watch_workspace` prefixes match, and answers `{ "changes": [...] }`. A `team/` path or `""` answers `422`, an unknown agent and removing what isn't there `404`
- Main chat turns are recorded in history when they end, each message tagged with the turn's id (`turn_id`, the `reply_to` of its frames) as the backend does. A chat message starting with `drop` loses the connection mid-turn: `drop finish …` ends the turn while disconnected, `drop compress …` also compresses history into a new episode (forcing a history reload), and any other `drop …` finishes the turn live after the page reconnects
- Config files are loaded from `../assets/*.example.*` and can be edited in the UI. A `config.toml` or `providers.toml` patch checkpoints the file first and answers with the checkpoint, as the backend does, and an agent's `config.toml` patch that sets `agent.max_tool_iterations` to 0 is refused with the backend's message
- Secrets can be added and removed (stored in memory)
- Agent keys list with one user key and one agent-saved key, and can be added and removed (stored in memory)

### What's NOT mocked

- No real LLM calls happen — responses are canned
- Config saves don't persist across server restarts
- Some edge cases (rate limits, network errors) aren't simulated
- The workspace routes don't block paths the backend blocks, and a delete, move or raw write records no checkpoint (deleting a workbench artifact does). Only `POST /api/mock/team-file` and `POST /api/mock/agent-file` send `workspace_changed`, and only the first the artifact frames: a write through the workspace routes, or an Undo, changes the files without announcing it, and the mock has no batches, resyncs or lag. A raw write stores its body as text, so bytes that aren't valid UTF-8 don't round-trip
- The Schedule place's pulses and actions are kept apart from `HEARTBEAT.yml` in the workspace: toggling a pulse doesn't edit that file or send `workspace_changed`
- `POST /api/secrets` doesn't validate the value like the real server does — it accepts anything, including a `secret:` or `${ENV_VAR}` reference the real server would reject with a 400. The frontend already avoids sending those (see `lib/secrets.ts`), so this only matters if you're testing the rejection path itself

### Setup Wizard Mode

To test the first-run setup wizard:

```bash
VITE_MOCK_SETUP=1 npm run dev:mock
```

This starts the app in "setup" mode so you can walk through the onboarding flow. A running mock enters it too with `POST /api/mock/reset` and the body `{ "setup": true }`, which is how the end-to-end specs reach the wizard.

### Deterministic mode

```bash
MOCK_DETERMINISTIC=1 npm run dev:mock -- --port 5173 --strictPort   # or: just web-mock-serve 5173
```

With `MOCK_DETERMINISTIC=1` the mock gives the same responses to the same steps, so end-to-end and visual tests can rely on them:

- **One fixed clock.** Every timestamp the mock makes (sample inbox and sessions, workspace versions, workbench and chat times, `busy_since`, uptime) reads one clock that stands at 2026-03-14 12:00 UTC until a test moves it with `POST /api/mock/clock/advance` (`{ "ms": 3600000 }`). Dates are read in UTC, so a machine's time zone changes nothing. Ids that would otherwise come from the time or from chance (tool call ids, turn ids, A2A key tokens, delete checkpoint ids) come from a counter that starts over at reset.
- **No delays.** Model calls, transcript loads, agent start and stop windows, reloads and every step of the chat and session simulations take no time, and run in the order they were started. `MOCK_DELAY_SCALE` multiplies all of them (`1` is the natural pace, `0.2` a fifth of it), and `POST /api/mock/delays` (`{ "scale": 1 }`) changes it until the next reset. A chat message starting with `drop` relies on real time to lose and regain the connection, so run it with a scale above zero.
- **The scenario** is the one `dev:mock` starts with: scout and atlas running, drifter stopped, brittle failed, and the sample sessions, outbound tasks, inbox, checkpoints and artifacts. The hub announces the same boot id every time.
- **A fixed artifacts port**, 5180 (or `MOCK_ARTIFACTS_PORT`), where a live mock takes a free one. A port that can't be bound is logged, and the workbench reports artifacts as unavailable.

`POST /api/mock/reset` puts the mock back as it started: the clock, the delays, the counter, every pending timer (a request waiting on simulated time answers `503`), the hub's own state (secrets, hub config, team files, workbench), and the agents, which are recreated from the scenario with their sessions, inbox and files. Every open socket is closed, so a page reconnects to the initial scenario. The port and the artifacts listener stay. With the body `{ "setup": true }` it starts over with no agents instead, as a hub that hasn't been set up, so the page opens the setup wizard; the next plain reset brings the scenario's agents back.

### Preview mode

```bash
just web-mock-preview 4173   # builds, then: MOCK_DETERMINISTIC=1 npm run preview:mock -- --port 4173 --strictPort
```

`npm run preview:mock` (`VITE_MOCK=1 vite preview`) serves the production build in `dist/` together with the whole mock: the API, the hub and agent sockets, and the artifacts listener. It is what service worker and installability tests run against, since the dev server can't serve a built app. Both modes start headlessly, need no browser, and take the port with `--port`; `--strictPort` makes a taken port an error instead of moving on.

### Route parity

`mock/route-parity.test.ts` calls every function `src/lib/api.ts` exports, with sample arguments, against a `fetch` that records the requests. An agent-scoped call samples with an agent (`"atlas"`), and a call that also serves the team, the hub or onboarding samples with `null` for that scope as well as an agent for the agent scope. It checks each recorded method and path against the mock: scoped the way the mock's handler scopes it, then matched against `apiRoutes` (the route table every mock route is in), plus the hub and agent socket paths and the scoped routes the workbench SDK calls on an artifact page. A client function without a sample call, or a request the mock doesn't serve, fails `npm test` and names the function and route. When you add an API client function, add its sample call there, and the route to the mock in the same change.

## Project Structure

```
web/
├── src/
│   ├── main.ts               # App entry point
│   ├── App.svelte            # The root: the setup wizard, or the shell; draws toasts and tooltips in both
│   ├── shell/                # The shell: the rail, the phone's bottom bar and drawer, the hub banner, place routing, the Settings modal, the command palette, the app's actions, the shortcuts, feedback and Create agent dialogs
│   │   ├── panel/                # The context panel: its frame and header, its width, and what each kind shows
│   │   └── settings/             # The Settings modal's parts: scope picker and section list, save bar, the section API, hosted legacy sections, Raw config
│   ├── places/               # Rebuilt places, one folder each
│   │   ├── home/             # Home: needs-you, the agents board and its row menus, Recently deleted, Across the team, Coming up, and the words and times they show
│   │   ├── inbox/            # Inbox: the list, the filter and tabs, an item opened in place, and the words for sources
│   │   ├── chat/             # An agent's Chat: its header, and the feed with the legacy composer under it
│   │   └── schedule/         # An agent's Schedule: its pulses and scheduled actions, and the words they show
│   ├── feed/                 # A conversation: the feed and each kind of message in it, shared by Chat and session transcripts; path links
│   ├── Setup.svelte          # Setup wizard
│   ├── styles/               # Design tokens, bundled fonts, base styles, legacy global styles
│   ├── components/
│   │   ├── ChatInput.svelte        # Input box with the `/` menu of chat actions (SlashMenu.svelte)
│   │   ├── ChatFooter.svelte       # Quiet status line: model, session tokens, context size
│   │   ├── ThinkingIndicator.svelte # Running-turn indicator: elapsed time, tokens, stop hint
│   │   ├── ToolGroup.svelte        # Groups related tool calls together
│   │   ├── ToolItem.svelte         # Individual tool call display
│   │   ├── SessionsSidebar.svelte  # Live and finished agent sessions, hosted as the Activity place
│   │   ├── SessionView.svelte      # One session's transcript, live activity, message box, stop
│   │   ├── Workspace.svelte        # A workspace: the file tree beside the editor, rename, delete, history
│   │   ├── WorkspaceEditor.svelte  # One workspace file's editor: validation, Save, Discard, the save conflict
│   │   ├── Workbench.svelte        # Workbench artifact list; hosts the open artifact
│   │   ├── WorkbenchArtifact.svelte # One artifact in its sandboxed frame; full view
│   │   ├── settings/               # Settings sub-panels
│   │   └── setup/                  # Setup wizard steps
│   ├── test/                 # Component-test helpers and harnesses
│   └── lib/
│       ├── ui/                   # Primitive controls and overlays (buttons, fields, badges, dialogs, sheets, menus, popovers, tooltips, toasts…); the overlay stack and float placement in ui/overlay/; gallery at /dev/gallery (see AESTHETIC.md)
│       ├── icons/                # The Icon component and icon set
│       ├── api.ts                # REST API client (typed fetch wrappers); every agent-scoped call takes the agent name first
│       ├── paths.ts              # API and WebSocket URL builders for the agent, hub and team scopes
│       ├── viewed-agent.ts       # The bound agent: the router publishes it, the WebSocket coordinator binds to it
│       ├── ws.svelte.ts          # WebSocket coordinator: routes frames to the feed and sessions stores
│       ├── feed.svelte.ts        # Main chat feed state
│       ├── feed-items.ts         # History-to-feed conversion shared by chat and session views
│       ├── sessions.svelte.ts    # Agent sessions: listing, live frames, session view, commands
│       ├── scheduled.svelte.ts   # The bound agent's pulses and scheduled actions, for the Schedule place
│       ├── overview.svelte.ts    # The team overview: each agent's overview, team events, the newest unread inbox items, what needs the user
│       ├── inbox.svelte.ts       # The Inbox's list across agents: filter and tab, paging, read, archive, restore
│       ├── app-badge.ts          # The app icon's badge (the Badging API): the inbox unread total
│       ├── needs-you.ts          # What needs the user and in what order, and the rail's Home count
│       ├── agent-failure.ts      # Plain words for why an agent couldn't start, and the Settings section that fixes it
│       ├── routes.ts             # URL <-> location: places, the panel and settings parameters, redirects from old URLs, corrections
│       ├── router.svelte.ts      # Current location; push/replace, closing by going back, overlay entries, the unsaved-edit guard
│       ├── history-entry.ts      # The marks the router keeps in history.state
│       ├── navigation-guard.ts   # Checks views register for unsaved work, and how the user is asked
│       ├── settings-sections.ts  # Settings section registry: ids, scopes, labels, groups, old names, config keys
│       ├── session-address.ts    # Opens a session, on any agent, from where it is mentioned
│       ├── relay.ts              # Recognizes agent-message headers in transcripts
│       ├── workbench-bridge.ts   # What workbench artifacts may call, relayed from their frames on the artifacts origin
│       ├── workbench.ts          # Where artifacts are served: relay origin or this host on the artifacts port
│       ├── time.ts               # Relative times ("5m ago")
│       ├── toast.svelte.ts       # Toasts: kinds, timings, actions (ui/ToastRegion draws them)
│       ├── notifications.svelte.ts # What is surfaced to the user: a toast, kept in the Recent notifications history
│       ├── generated/            # Protocol types generated from Rust (cargo test --test ts_export)
│       ├── types.ts              # TypeScript types for API and messages
│       ├── action-registry.svelte.ts # The action registry: sources of named actions, matching, `/name` lines, running
│       ├── chat-actions.ts       # The chat actions (the former slash commands) and why each can't run
│       ├── models.ts             # Model fetching and caching
│       ├── markdown.ts           # Message Markdown to sanitized nodes: code blocks with Copy, workspace paths as links
│       ├── format-usage.ts       # Elapsed time / token count formatting for the indicator and footer
│       ├── format-tool-result.ts # Tool result display: JSON, file dumps, lists, errors, long-output collapse
│       ├── settings-toml.ts      # Config parsing (for display) and diffing (for the patch endpoints)
│       ├── settings-fields.ts    # The field-to-key-path map: every form field's key, and which field a diagnostic's key path names
│       ├── settings-model.svelte.ts # The settings model: per scope and file baselines, staged changes, Save, Undo, locks (no UI)
│       ├── config-coordinator.ts # Config write coordinator: serialized writes, re-read before a save, change notifications, checkpoint restore and undo
│       ├── config-sync.ts        # Passes config changes made elsewhere (the agent's config/ watch, hub_config_reloaded) to the coordinator
│       └── secrets.ts            # secret:/${ENV_VAR} reference detection for settings fields
├── mock/                     # Typed mock modules, checked like src/ (only used in dev:mock and preview:mock)
│   ├── routes.ts             # Route tables: each endpoint is a method, a path pattern and a handler
│   ├── api-routes.ts         # Every route table the mock serves
│   ├── env.ts                # The clock, simulated delays and timers, the id counter; what reset restores
│   ├── scope.ts              # Scoped routing: agent, hub and team paths to state and unscoped path; what each scope owns; stopped-agent rules
│   ├── middleware.ts         # The /api request handler and its Connect middleware
│   ├── state.ts              # Per-agent and hub state, and the agent and hub types
│   ├── http.ts               # Request and response helpers, typed body parsing
│   ├── hub.ts                # The hub: agents, activity and unread, run state changes, reset
│   ├── hub-inbox.ts          # The cross-agent inbox: every agent's items in one listing
│   ├── push.ts               # Web Push: the public key and the registered devices, with a test send that always succeeds
│   ├── hub-config-reload.ts  # The hub's config reload and its frames
│   ├── lifecycle.ts          # Agent lifecycle routes and hub status
│   ├── hub-socket.ts         # The hub WebSocket: hub frames, team watches, session subscriptions
│   ├── session-relay.ts      # The hub socket's session relay: who follows which session, `session_frame` delivery and the lag notice
│   ├── agent-socket.ts       # An agent's WebSocket: client frames, commands, verbose mode, workspace watches
│   ├── sockets.ts            # Upgrade routing, frame helpers and watch prefix matching shared by the sockets
│   ├── chat.ts               # Chat history and usage routes, the chat turn simulation
│   ├── scenario.ts           # The agents the mock starts with
│   ├── sessions.ts           # Sessions endpoints, session socket commands, the session lifecycle
│   ├── config.ts             # Status, config, providers, MCP, secrets, agent keys, A2A, setup, tracing
│   ├── workspace.ts          # Workspace file routes, for an agent and for the team tree
│   ├── workspace-tree.ts     # The workspace tree in state: listings, versions, writes, moves
│   ├── workspace-bulk.ts     # The recursive tree listing and the batch read, with their budgets
│   ├── inbox.ts              # The user inbox: listing, archive, read, restore, attachments
│   ├── agent-inbox.ts        # The agent's own inbox: what an artifact adds to it
│   ├── scheduled.ts          # The Schedule place's routes: pulses and actions
│   ├── checkpoints.ts        # Checkpoint histories: list, stats, detail, diff, file, restore, undo
│   ├── update.ts             # The update routes
│   ├── workbench.ts          # Workbench artifact list, info and delete (with its checkpoint)
│   ├── workbench-files.ts    # Artifacts in the team tree: discovery and file lookup
│   ├── artifact-name.ts      # The artifact name rule (`api` is reserved), shared by the workbench and the identity header
│   ├── workspace-changes.ts  # Changing a team file or an agent's own file and sending its live-update frames
│   ├── model.ts              # The artifact model call
│   ├── controls.ts           # Test controls: reset, clock, delays, hub-socket, missed-relay, teammate-message, fix-agent, team-file, agent-file and session-relay-lag
│   ├── team-events.ts        # The team event log: entries, paging, the events route, the user-inbox test control
│   ├── overview.ts           # The team overview: each agent's last message, live sessions and unread count, its route and frames, and message previews
│   ├── artifacts-listener.ts # The second origin that serves artifact pages and files, with the SDK injected
│   ├── artifacts-origin.ts   # What that origin forwards to the API and sockets, its block list, and the marker for requests that came through it
│   ├── mock.ts               # Starts the mock on a Vite server, from the environment's options
│   ├── plugin.ts             # The Vite plugin: starts the mock on the dev server and the preview server
│   ├── data/                 # Sample data: chat, sessions, workspace files, inbox, the workbench artifact
│   ├── test-support.ts       # Test harnesses: route tables over HTTP, the whole mock with its sockets
│   ├── route-parity.test.ts  # Every API client request lands on a mock route
│   └── *.test.ts             # Unit tests, run by `npm test`
├── e2e/                      # Playwright specs, checked like src/ (see Testing)
│   ├── support/              # Fixtures, the axe scan, the screenshot helper, server ports
│   ├── smoke/                # Flows on the current UI; `@preview` specs run on the production build
│   ├── visual/               # `@visual` specs; their baselines are in __screenshots__/
│   └── harness/              # Specs for the harness itself
├── playwright.config.ts      # Projects, servers and reporters
├── vite.config.ts
└── package.json
```

## Routing

The URL is the source of truth for where the user is. A location is a place, an optional context panel, and an optional Settings modal:

| URL | Place |
|-----|-------|
| `/` | Redirects to `/home` |
| `/home` | Home |
| `/inbox` | Inbox. `?agent=<name>` filters, `?tab=archived` shows the archive, `?item=<agent>:<id>` opens an item |
| `/agent/:name` | That agent's Chat |
| `/agent/:name/activity` | Its Activity |
| `/agent/:name/schedule` | Its Schedule |
| `/agent/:name/files` | Its Files |
| `/team/workbench[/:artifact]` | The Workbench list, with an artifact's row selected |
| `/team/files` | Shared files |

Any place takes two more parameters:

- `panel=session:<agent>:<runId>`, `panel=file:<path>` or `panel=size` is the context panel. A session shows on the viewed agent's places (for that agent) and on the Workbench, a file on agent places and Shared files, and the conversation size on agent places. Anywhere else the router removes it.
- `settings=<agent | _all>[/<section>]` is the Settings modal. `_all` is the install-wide scope. Without a section, the frame opens the scope's default section, or on phones its section list. Section ids are in `lib/settings-sections.ts`.

Old URLs redirect by replace: `/team`, `/agent/:name/sessions/:runId`, `/agent/:name/workspace`, `/agent/:name/scheduled`, `/agent/:name/settings[/:section]`, `/team/settings[/:section]`, `/workbench[/…]`, and the unprefixed `/settings`, `/scheduled`, `/sessions/:runId` and `/notification/<id>`, which resolve under the last-used agent once the agent list is known. Old settings section names map to the new scope and section (`lib/settings-sections.ts`). A URL the router can't read, an agent or artifact that doesn't exist, and a panel a place can't show are corrected by replace, with a toast where the user should know.

`routes.ts` reads and formats URLs and knows nothing of the browser. `router.svelte.ts` holds the location. Stores never import it: they expose data and commands, and views navigate (ESLint enforces this under `src/lib/`).

**Navigation.** `openPlace`, `openPanel`, `openSettings` and `openSettingsSection` (a section opened from the phone's section list) push. `replacePlace`, `replacePanel`, `switchSettingsSection` and `switchSettingsScope` replace, and so does every correction and redirect. `closeSettingsSection` is a phone section's Back: it goes back to the list when this page pushed the section from it, and replaces the section away otherwise. Each returns whether the navigation happened.

**The Inbox's parameters.** Design §3 leaves them open, so the Inbox decides: opening an item from the list pushes (`openPlace` with `item`), and opening another while one is open replaces it, the way the panel switches files. Changing the agent filter or the tab replaces and closes the open item, so Back leaves the Inbox instead of stepping through filters. A link to an item that's gone is corrected by replace, with a toast.

**Closing.** `closePanel`, `closeSettings` and `closeInboxItem` go back in the history when this page pushed the entry that opened what is closing, and replace the URL with one that omits the parameter otherwise (a deep link, a reload into the modal). Back therefore closes the panel, the modal or the open inbox item before it leaves a place.

**Overlays.** A modal overlay calls `router.openOverlay(onDismiss)` when it opens. That pushes an entry with the same URL, so Back closes the overlay. The overlay closes itself through the returned handle (`handle.close()`), which pops that entry. `onDismiss` runs when the entry is left any other way: Back, or a navigation that takes the overlay's entry. `ModalLayer`, under Dialog, Sheet and Drawer, does all of this; `historyEntry={false}` is for a layer whose URL parameter is already its entry, like the Settings modal. The gallery, outside the app's routes, calls `router.startForOverlays()`, which follows overlay entries and leaves the address alone.

**Unsaved work.** A view that holds work the user would lose registers a check with `router.guard.register(check)`. The check returns a line saying what navigating to the given location would lose, or null. In-app navigation asks first, through the function the app gives `router.guard.setConfirm`, and does not navigate until the user confirms; with no such function it refuses. `confirmLeave` from `lib/ui` is that function: it asks in a confirm dialog, shown by `ConfirmHost`. A confirm dialog has its own overlay entry, so the router goes on only once that entry has left the history. On Back or Forward the router puts the location back on top of the history, asks, and goes where the user was headed only if they confirm. On reload or tab close the browser's own prompt appears.

**The bound agent** is the viewed agent on an agent place, and on the other places the agent most recently viewed. The last-used agent is remembered in local storage, and `router.setKnownAgents` settles on agents that exist once the agent list is known.

### The shell

`shell/Shell.svelte` is the frame around every place: the rail (`Rail.svelte`) beside the main region at medium and wide widths, and on phones the bottom bar (`BottomBar.svelte`) with the rail in a `Drawer`. The main region starts with the hub banner (`HubBanner.svelte`, shown while the hub socket is down) and then the place, which `PlaceHost.svelte` picks from the router's location. The shell root carries `data-ui`, and mounts `ConfirmHost` (and gives the router's guard `confirmLeave`), the Settings modal, the command palette, `RecentNotifications`, the Keyboard shortcuts dialog, the feedback dialog, the inbox-note prompt and the Create agent dialog once each; `App.svelte` draws the toast region and tooltips, in setup too.

The context panel (`panel/PanelHost.svelte`) is open while the URL has a `panel` its place can show. Its frame (`ContextPanel.svelte`) is a column beside the main region at wide widths, resized from its left edge by pointer or by the arrow keys, Home and End, between `--layout-panel-min-width` and half the viewport; the width the viewer chose is kept in local storage. At medium widths it floats over the main region's right edge at the default width, and on phones it is a full-screen sheet over the bottom bar, a `ModalLayer` whose history entry is the `panel` parameter. Beside or over the main region it takes focus when it opens, Esc inside it closes it, and focus goes back to where it was; on phones the sheet's layer does the same. Closing goes through `router.closePanel`, so Back closes it before it leaves the place. What the panel shows is chosen by kind in `PanelHost`, and each kind's content starts with `PanelHeader`, which names the panel and holds its actions and the way out (Close, or Back on a phone). Until their units rebuild them, a session shows the legacy session view, a file the workspace editor (from the agent's workspace on agent places, the team's folder on Shared files; a file that can't be opened shows only why), and the conversation size the chat footer's figures, each inside a `data-legacy-view` element. The session view reads the bound agent's sessions, so on the Workbench a run on another agent offers to open it beside that agent's chat.

The rail's agents are an accordion (`accordion.svelte.ts`): one agent's places are open at a time, a press on the open agent closes it, a row press never navigates, and arriving on an agent opens it. `rail-model.ts` works out each agent row's mark, word and unread badge from the hub's snapshot. The Home count is the number of needs-you items and the Inbox count the unread items across every agent, both from the overview store, and an agent's Activity count the bound agent's running sessions. The bottom bar's Inbox count is the same, and so is the installed app's icon badge (`lib/app-badge.ts`, where the browser has the Badging API). The rail starts with a search row that opens the command palette, as the phone bar's Search tab and ⌘K or Ctrl+K do. Its footer has a Help menu, which lists the registry's help actions (Keyboard shortcuts, Recent notifications, Send feedback, Report a bug, and Install app while the browser offers it), and the Settings gear. The search row, the gear and the rail's "+" go through `ShellActions`, which the shell answers.

`ShellActions.createAgent` is the one way to create an agent: Home's New agent, the rail's "+" and the palette's Create an agent call it, and it opens the Create agent dialog (`CreateAgentDialog.svelte`, a sheet on phones) over the current place. The name is checked against the backend's rules and the agent list as it is typed (`newAgentNameProblem` in `lib/agent-name.ts`), and a name a deleted agent had points at Recently deleted. Under More options are the agent to copy model settings from (the first by name until another is chosen) and who can find it (private by default). What was typed stays when the dialog closes without creating. Once the agent exists the dialog closes, the hub's `agent_created` frame raises the "You created …" toast, and beside the main region the new agent's rail row takes focus.

Home, Inbox, Chat and the Schedule are rebuilt (see [Home and the overview](#home-and-the-overview), [The Inbox](#the-inbox), [The chat feed](#the-chat-feed) and [The Schedule](#the-schedule)). Places not rebuilt yet host their legacy view inside a `data-legacy-view` element, so the legacy global styles apply there and the new base styles don't: Activity the sessions list, Files and Shared files the workspace, and the Workbench itself. The Workbench's full view is a mode of the Workbench page and isn't in the URL. The Settings modal is described in [The Settings modal](#the-settings-modal). The palette and the help dialogs aren't in the URL either; each holds an overlay entry, so Back closes it.

### Actions

`lib/action-registry.svelte.ts` holds the one list of named actions that the command palette (`shell/CommandPalette.svelte`), the composer's `/` menu and the rail's help menu draw from. A source is a function that builds actions from current state; `actionRegistry.register(key, source)` adds one (replacing the source under that key) and returns a function that removes it, and `actionRegistry.all` is every source's actions in registration order. The shell registers the app's sources (`shell/app-actions.svelte.ts`): the team places, every agent, the bound agent's places and live sessions, the settings sections, the chat actions, Start, Stop and Restart where they apply, Create an agent, and help. Another agent's places, settings and lifecycle actions are `searchOnly`: the palette lists them once something is typed. A unit adds actions by registering a source of its own, or by adding them to the source they belong with.

An action has a heading (`group`), a plain-language `label`, an optional `hint` and `terms` it is also found by, and `run(text?)`. A chat action has a `command`, its old slash name: `/observe` in the composer, or typing `/observe` in the palette, finds "Summarize older messages now". `takesText` marks one that acts on what follows `/name`, and asks for text otherwise. `disabled` holds the reason it can't run now ("Start atlas first"); the palette and the `/` menu show the reason in place of the hint, and running it does nothing. `matchActions` finds the actions holding every typed word; `readCommandLine` reads a `/name text` line the composer sends.

Run actions with `actionRegistry.run(action, text?)`. It tells the registry's run listeners first, then runs the action once the page has settled: the shell closes the phone drawer there, so a dialog an action opens, or a place it goes to, never sits under the drawer. `installOffer.install`, in `app-actions.svelte.ts`, lists Install app among the help actions while it is set.

### Home and the overview

`lib/overview.svelte.ts` (`overview`) holds what Home and the rail's Home count show beyond the hub store's agent list, activity and stopping set, which it reads from there: each agent's overview (`overviews`, `overviewOf(name)`), the newest 50 team events (`events`), the newest five unread user-inbox items across agents (`unreadItems`), and `needsYou`, worked out by `lib/needs-you.ts`: its `items` worst first (an agent that couldn't start, a running agent's task to a remote agent it can't reach, an unread inbox item), `moreInInbox`, and `count`, the rail's number. `App.svelte` starts it before the hub socket connects, and it follows the socket through `hub.onFrame`:

| Frame | The store |
|---|---|
| `hub_boot` | Fetches the overview and the team events. When the boot id differs from the last one, it first drops everything it holds, since that came from another hub process |
| `agents_snapshot` | The first after `hub_boot` is the connection's own; any later one replaces frames the connection lost, so it fetches the overview and the events newer than its newest again |
| `agent_overview` | Replaces that agent's overview. One that arrives while the overview request is out wins over the request's answer |
| `agent_deleted` | Forgets that agent's overview |
| `team_event` | Adds the event once; one from another boot id starts the events over |

An overview answered by a different hub process than `hub_boot` announced is dropped. The inbox items are fetched again whenever an agent's unread count changes. A failed fetch lands in `loadError`, `eventsError` or `unreadItemsError` for the section that shows it, with Try again. `stopTask` and `stopWatching` act on a running agent's outbound task and drop its problem at once, ahead of the hub's frame; when the remote agent can't be reached, `taskNotes` says so on the item.

Home's sections (`places/home/`) read the store and the hub store directly; `home-model.ts` holds their words and times. Restart and the kind-specific fixes on a failed agent go through the hub store and `lib/agent-failure.ts`, which finds the Settings section for Fix settings from the agent's validate endpoints, or Raw config.

Each board row ends in a "…" menu (`AgentMenu.svelte`), headed by the agent's state: Open chat; Start, Stop and Restart, each offered where `lifecycleApplies` (`lib/agent-lifecycle.ts`, which the palette's lifecycle actions use too) says it applies, and none while the agent stops; Start automatically; Settings; and Delete. `agent-actions.svelte.ts` runs them through the hub store, which surfaces any failure, one action per agent at a time. Start automatically shows the value being saved until the hub answers, then the hub's. Delete asks first (`confirmations.ask`), then drops the agent's staged settings (`settingsModel.drop`, which `App.svelte` also does for a deletion made elsewhere), and the hub's `agent_deleted` frame raises the "You deleted …" toast with Undo; a deletion that took no checkpoint says so in an error. Recently deleted (`RecentlyDeleted.svelte`), collapsed under the board, lists `hub.deleted` with Restore once there is something to list, or a failed load with Try again; restoring uses the checkpoint the deletion took. An agent's A2A card visibility is set in its Settings.

### The Inbox

`places/inbox/` shows every agent's user inbox in one list, from `lib/inbox.svelte.ts` (`inbox`), which reads the hub's inbox routes through the API client. The place's URL is the list it shows: `inbox.show({ agent, tab })` fetches it each time the Inbox opens or the filter or tab changes, keeping what is on screen while the same list is fetched again, and `loadMore` pages older items in. Counts aren't kept here: the rail, the bottom bar, the tab and the filter read them from the overview store, and the place hands `overview.unreadCounts()` to `inbox.followCounts`, which fetches the list again whenever a count changes, so a new item shows up without polling.

The item in the URL (`?item=<agent>:<id>`) is open in place. `inbox.open(ref)` marks it read when it is unread; when the loaded pages don't hold it (a link to an older or archived item), it is fetched on its own through the read route and shown above the list (`openedApart`), and an item that's gone (`missing`) is corrected away. `markRead`, `archive` and `restore` each answer whether they worked, keep one action per item at a time (`pending`), and leave a plain-language reason in `problems` when they don't, which the item shows with Try again. A failed list or older page lands in `loadError` or `moreError`, and a link that couldn't be fetched in `openError`, each with Try again. Archiving offers Undo on its toast.

### The chat feed

An agent's Chat (`places/chat/ChatPlace.svelte`) is its header (`ChatHeader.svelte`: the agent and its role, a pill counting its running sessions that opens Activity, its settings, and a menu with Show conversation size, Restart and Stop, taken from the action registry with their reasons when they can't run), the conversation, and under it the legacy composer and footer in a `data-legacy-view` element. While a turn runs, the legacy running-turn line follows the conversation.

The conversation is `feed/Feed.svelte`, made for any agent's conversation, the main chat or a session's transcript:

| Prop | Is |
|---|---|
| `agent` | The agent the conversation belongs to. Its links, Undo this turn and Open session act on this agent, never on the bound one by assumption |
| `items` | The `FeedItem`s to show |
| `verbose` | Tool calls show (the legacy tool rows, while "Show tool calls" is on) |
| `label` | The scrolling region's name, such as "Conversation with atlas" |
| `history` | Optional `FeedHistory` (`feed/feed-history.ts`): older parts to load as the reader nears the top, and a `generation` bumped when the whole feed is replaced |
| `loading` | The items haven't arrived, so the empty state waits |
| `live` | Something at the tail is growing, such as a turn in progress |
| `empty`, `tail` | Snippets: the one empty state, and live content after the items |

The feed follows new content while the reader is within 120px of the bottom; further up, a Jump to latest pill names the divider at the top of the view (its button's description) and takes them back, and their own message brings them down. Older parts load one at a time near the top, keeping the item the reader was at in place, until the view is filled or there are none; a reload under a reader who scrolled up finds the item they were at by its kind and text and puts it back, once the part holding it has loaded.

`feed/FeedItemView.svelte` draws one item, and takes the same `agent`. A user message is a moss bubble with its sender line, images and Undo this turn; an agent's reply is unboxed prose; a message from a session or teammate is a card with its kind and sender (`feed/feed-words.ts`), a clamped body with Show all, and Open session for session senders; then day and episode dividers, the compressed-history note naming the agent, file attachments, and a session transcript's status lines. Message text goes through `feed/Prose.svelte`: sanitized Markdown, a Copy button on each code block, and inline code that is a whole workspace path (`isWorkspacePath` in `lib/markdown.ts`) as a link that opens the file in the context panel beside that agent's places, or beside its chat from anywhere else (`feed/feed-links.ts`). Open session finds the run on its agent (`lib/session-address.ts`).

### The Schedule

`places/schedule/Schedule.svelte` shows the bound agent's pulses and scheduled actions from `lib/scheduled.svelte.ts` (`scheduled`), and `schedule-model.ts` holds its words, taking run names and next-run times from Home's `home-model.ts` so both read the same. The store is reset by the socket coordinator on an agent switch, so it can't import `ws`: the place passes it the socket's frames and watch registry with `startWatching({ onFrame, watches })`. While a place is open the store owns a watch on `HEARTBEAT.yml` and `scheduled_actions.json`, tied to the bound agent, and refetches when either changes, on a resync, and on a scheduled run's session frames. A failed load lands in `loadError`, which the place shows with Try again. The schedule routes answer only while the agent runs, so the place loads when the agent is running and otherwise shows that it is stopped, with Start.

### Agents in API calls

No request reads the viewed agent. Every agent-scoped function in `lib/api.ts` takes the agent name as its first argument, and its cache key includes that agent. Components take the agent from their props (`Settings`, `Workspace`), stores hold the agent they were bound to (`ws.sessions`, `scheduled`), and the chat's controls use `ws.agent`, the agent the WebSocket is bound to. The workbench bridge maps an artifact's unscoped paths onto `ws.agent` per request.

Calls that serve more than one scope take `agent: string | null`: the workspace and checkpoint functions accept `null` for the team's files and the hub and team repositories, and `fetchProviderModels(null, …)` asks the hub before any agent exists. Asking for an agent's own resource with `null` throws `NoAgentSelectedError` before any request is made.

### Config writes

Every write to a config file goes through `configCoordinator` in `lib/config-coordinator.ts`: an agent's `config.toml`, `providers.toml` and `mcp.json`, and the hub's `config.toml`. Name a file with `agentConfigFile(agent, "providers")` or `HUB_CONFIG_FILE`. Never call a `patch…`, `put…` or checkpoint restore function from `lib/api.ts` for one of these files directly.

| Call | Does |
|------|------|
| `save(file, { baseline, edit, choose, source? })` | Writes `edit`, either `{ patch }` (merged into the file) or `{ text }` (the whole file). `baseline` is the file's text as the caller's view last loaded or saved it. Resolves to `{ kind: "saved", result, written, raw }`, where `raw` is the file's text now and becomes the caller's next baseline, or `{ kind: "used-disk", raw }`. |
| `edit(file, build, source?)` | Reads the file, builds a patch from its text and writes it with no other write to that file in between. For a control that changes one key from the current text, like the composer's model and thinking controls. |
| `reload(file, source?)` | Reads the file from disk and tells subscribers to do the same. |
| `restore(agent, id, repo, path)` and `undo(agent, id, repo)` | Restore or undo a checkpoint and tell subscribers about any config file it wrote. Every caller that restores from a checkpoint uses these, whatever it restores. |
| `subscribe(file, listener)` | Hears every change to `file`: `{ file, cause, source }`, with `cause` one of `write`, `reload`, `restore` or `external`. A view that shows a config value subscribes and reads the file again. Pass `source` to a write and skip notifications that carry it to ignore your own. |

Writes to one file are serialized. Before a save the coordinator reads the file again. When it differs from `baseline` and the keys that changed overlap the keys the edit sets (a raw `{ text }` save overlaps every change), it calls `choose` with `{ file, keys, disk }`. `choose` answers `"keep-mine"` ("Keep my changes"), which goes on with the write, or `"use-disk"` ("Use what's on disk"), which writes nothing and resolves `used-disk`. No lock is held while `choose` waits, so it can ask the user. When the changes don't overlap, a patch goes ahead and the other keys' changes survive.

Changes made outside the coordinator reach its subscribers through `lib/config-sync.ts`, started in `main.ts`: `workspace_changed` frames under the bound agent's `config/` folder (a watch owner on the agent's socket, tied to that agent), and the hub's `hub_config_reloaded` frame. Another agent's files have no change feed, so the re-read before a save is the only protection for them.

### The settings model

`lib/settings-model.svelte.ts` holds what the Settings modal edits, with no UI. `settingsModel.agent(name)` and `settingsModel.all()` return a scope: an agent's `config.toml`, `providers.toml` and `mcp.json`, or the hub's `config.toml` for All agents. A scope never writes another scope's files, and an agent's form holds only agent keys. The hub's values an agent page shows are `scope.install`, read-only. Get a scope in a script, not in a template expression, since creating one changes the registry.

Each file has a baseline (its text as the model last loaded or saved it) and a form (a copy parsed into the shapes the sections bind to). What the user changed is the difference between them, so every edit is staged, removals included, and the scope keeps it while the modal is closed or another scope is open. Read the form through the scope each time (`scope.config.timeout_secs`, `scope.providers`, `scope.models`, `scope.mcpServers`): a reload gives it new objects.

| Call | Does |
|------|------|
| `load()` | Reads each file, keeping the staged changes of a file that has some, and follows changes made elsewhere. |
| `reload()` | Discards the staged changes and reads every file again. |
| `discard()` | Drops the staged changes. |
| `save(choose)` | Stores typed secrets, then writes each changed file's diff through the coordinator, providers then config then MCP servers. A file that fails keeps its changes, and `config.toml` waits on `providers.toml`. Resolves to a `SaveResult`: what each file did, every checkpoint taken, and a plain-language message. |
| `undo()` | Puts each file the last save wrote back from the checkpoint taken just before its write, in reverse order. A file that changed again since is skipped, and one that can't be restored is named. |
| `fieldDiagnostics(ref)` and `sectionDiagnostics(section)` | The problems from a save. A diagnostic whose key path names a form field is on that field; the rest are for the top of a section. |

`dirty`, `saving`, `lastResult` and `undoable` drive the save bar. `file(name)` gives a file's `lockedBy` (`"form"` keeps its raw editor read-only, `"raw"` keeps its form read-only while the raw editor holds a draft set with `setRawDraft`), `changedOnDisk`, `unreadable` and `loadError`. A change to a file that has staged changes leaves them alone, so the coordinator's re-read before Save finds the clash. A typed credential is stored under its name (`discord`, `webhook_<name>`, a provider's name) and the reference goes in the file. Immediate actions have no state here.

`lib/settings-fields.ts` is the one map from a form field to its key (`keyPathOf`), which diffing and diagnostic placement (`locateField`) share, so a field that saves to a key is the field that shows that key's error. A form field the map doesn't cover fails `settings-fields.test.ts`. Failover lists ride along with a role (`models.fallbacks`) so a save never shortens one.

The model's tests are `settings-model.component.test.ts`, so they run with live runes in jsdom.

### The Settings modal

`shell/SettingsModal.svelte` is open while the URL has a `settings` parameter. Its side (`settings/SettingsNav.svelte`) is the scope picker, "All agents" first and then each agent with its state, the line saying what the scope affects, and the scope's sections with the Advanced group under a heading; on a phone it is the screen a URL without a section shows, and a section opens over it with Back. The modal is never remounted while the scope or section changes: the content pane swaps, with a `--duration-swap` fade, once the new scope has loaded, and keeps the last section on screen until then. On a phone it stops above the bottom bar, which stays pressable. The frame's top bar has Reload from disk (which asks first when the scope has staged changes) and Close.

The save bar (`settings/SaveBar.svelte`) shows while the scope has staged changes. Save changes writes them; a save that wrote everything says so in a toast with Undo, and one that left files unsaved names them on the bar until those changes are saved or discarded. When a file changed on disk under the keys being saved, `ChangedOnDiskDialog` asks "Keep my changes" or "Use what's on disk"; closing it leaves that file unsaved with its changes staged. While the modal is open the palette lists Save changes, Discard changes and Reload settings from disk for its scope. Reloading the page with staged changes in any scope asks first.

**Writing a section.** A section is a component taking `{ scope, section }` (`AgentSectionProps` or `AllSectionProps` in `settings/sections.ts`), listed in `AGENT_SECTION_VIEWS` or `ALL_SECTION_VIEWS`; a section not listed shows its legacy panels (`LegacySection.svelte`), bound to the same forms.

- Start with `SettingsSection` (title, a line on what it holds, and the save's problems no field shows), then the fields.
- Bind fields to the scope's forms (`scope.config.timeout_secs`, `scope.providers`, …); the frame loads the scope before the section renders, and the save bar follows the staged changes by itself.
- Give a field its problems with `error={fieldError(scope, { kind: "config", field: "timeout_secs" })}`.
- A part that needs the agent running goes inside `RunningOnly` (`<RunningOnly agent={scope.agent} subject="its status">…</RunningOnly>`), which says "Start atlas to see its status." with Start until it runs.
- Actions of the section's own register with `actionRegistry.register(key, source)` inside `untrack` in an `$effect` that returns the remover.
- Immediate actions (secrets, keys, Cloud, updates) call their endpoints and report their own result; they have no part in the save bar.

## Code Quality

Before submitting changes, run:

```bash
npm run lint          # ESLint, then the style lint
npm run format        # Prettier auto-format
npm run check         # TypeScript / Svelte type check; warnings fail it
npm test              # Vitest: lib unit tests and Svelte component tests
npm run test:coverage # The same tests with a coverage summary (HTML report in coverage/)
npm run e2e:fast      # Playwright specs in Chromium, visual comparisons left out (see Testing)
```

**TypeScript lint.** Every `.ts` module under `src/` and `e2e/`, including the rune store modules (`*.svelte.ts`), gets the strict type-aware ESLint rules. Only `.svelte` files get the relaxed set that fits runes. When a rule is wrong for one line, use a scoped `// eslint-disable-next-line <rule> -- <reason>`, never a blanket disable.

**Style lint.** `npm run lint` also runs Stylelint over `src/**/*.css` and the `<style>` blocks of `.svelte` files. Outside the token files (`src/styles/tokens.css`, the design token set described in [AESTHETIC.md](./AESTHETIC.md), and `src/styles/variables.css`, the legacy variables) it forbids literal colors (hex, named, `rgb()` and the like), raw `font-size` and `font` values, raw `z-index` values, literal durations and easing curves in `transition` and `animation`, and `transition: all`. Reference a token with `var(--…)` instead. Viewport media queries may use only the shell breakpoints, written as `min-width`/`max-width`; a component that needs its own responsive rule uses a container query. Stylesheets and components that still carry literal values are listed in `stylelint.config.js` and exempt from these rules; remove an entry when its file is rewritten or deleted, and never add new styles to the list.

**svelte-check.** It runs with `--fail-on-warnings`. The one accepted warning, a label without an associated control, is filtered in `svelte.config.js`.

**Generated types.** `src/lib/generated/` comes from the Rust types. After changing an exported Rust type, run `just types` and commit the result; `just types-check` (and CI) fails when the committed files are out of date.

**Mock modules.** Everything under `mock/` is formatted, linted and type-checked with the same rules as `src/`, and its tests (`mock/**/*.test.ts`) run in Node through the same `npm test`. Route handlers live in route tables (`Route` in `mock/routes.ts`), and response bodies are checked against the generated protocol types in `src/lib/generated/` wherever one exists. The Vite plugin entry is `mock/plugin.ts`, and nothing in the mock is left out of these checks. `mock/test-support.ts` has two harnesses: one serves route tables over HTTP against a stub hub, and `startMockServer` runs the whole mock (hub, agents, sockets, scoped routing) on a real HTTP server, with a WebSocket client that keeps the frames it receives. Both take a mock environment (`createMockEnv({ deterministic: true })`, or `startMockServer({ deterministic: true })`), so a test that asserts on times or ids runs on the fixed clock.

Component tests live next to the component as `src/components/**/*.test.ts` (or `*.component.test.ts` anywhere under `src/`). They run in jsdom, through the same `npm test` command as the Node unit tests under `src/lib/`. Mount with `render` and mock `fetch` using `src/test/component.ts`. Drive keyboard and pointer input with `@testing-library/user-event`, which presses keys the way a browser does (Space and Enter activate buttons). `src/test/snippets.ts` turns markup into a snippet for a component's `children`, and `src/test/ui/` holds small harnesses for components that need a parent, such as a context provider or a live snippet.

## Testing

Tests sit in five layers. Use the lowest one that can show the behavior: a lower layer is faster and breaks for fewer unrelated reasons.

| Layer | Runs in | Use it for | Lives in |
|---|---|---|---|
| Unit | Node, Vitest | Stores, routing, formatters, parsers, the mock's own logic | `src/**/*.test.ts`, `mock/**/*.test.ts` |
| Component | jsdom and Testing Library, in Vitest | One component's empty, loading, error, populated and live states, with fixtures in place of a socket | `src/components/**/*.test.ts`, `src/**/*.component.test.ts` |
| End-to-end | Playwright in real Chromium, against the mock | A user flow across components: navigation, sockets, focus, touch, anything that needs a real browser and real layout | `e2e/**/*.spec.ts` |
| Accessibility | axe-core, inside an end-to-end spec | Every place and overlay a change touches | `expectNoAxeViolations` in `e2e/support/axe.ts` |
| Visual | Playwright screenshots, in the Playwright container | How a surface looks: a few baselines per surface, at desktop and phone size | `e2e/visual/` |

The unit and component layers run in `npm test`, and the pre-commit hook runs them. The other three run through Playwright and are not part of the hook. Pull requests don't run CI, so a frontend change runs `just web-e2e` before it is reported, and the release workflow runs the end-to-end suite too.

### Running the end-to-end suite

| Recipe | Runs | Needs |
|---|---|---|
| `just web-e2e` | Everything: the specs on this machine, the visual comparisons in the Playwright container | Docker |
| `just web-e2e-fast` | Everything but the visual comparisons | Chromium, installed on first use |
| `just web-e2e-update` | Regenerates the visual baselines in the container | Docker |
| `just web-e2e-webkit` | The same specs in a WebKit phone, in the container | Docker |

Extra arguments go to Playwright's test command, so `just web-e2e-fast e2e/smoke/chat.spec.ts` runs one file and `--grep`, `--headed` and `--debug` work. The recipes name their projects, so `--project` adds to them instead of narrowing; to run one project, call Playwright directly: `cd web && npx playwright test --project=phone e2e/smoke`. `npm run e2e:report` opens the last HTML report. The first Docker run pulls the Playwright image, about a gigabyte. A fresh Linux machine also needs Chromium's system libraries: `cd web && npx playwright install --with-deps chromium`.

The projects:

| Project | Browser and size | Runs specs |
|---|---|---|
| `desktop` | Chromium, 1440×900 | without a tag, against the Vite dev server |
| `phone` | Chromium, 390×844, touch, mobile user agent | without a tag, against the Vite dev server |
| `preview-desktop`, `preview-phone` | The same two | tagged `@preview`, against the production build |
| `visual-desktop`, `visual-phone` | The same two, rendered in the Playwright container, with the page's clock frozen | tagged `@visual` |
| `webkit-phone` | WebKit as an iPhone 13, 390×844 | without a tag, against the Vite dev server; local only, the release workflow leaves it out |

A spec runs in every project that matches its tag, so one spec covers both sizes. Branch on the size only when behavior differs, with Playwright's `isMobile` fixture. Tags go on a test or a describe block: `test("installs", { tag: "@preview" }, async ({ page }) => { ... })`. Use `@preview` for what the dev server can't show (the service worker, installability, the bundle as shipped) and `@visual` for screenshot comparisons. Give a spec one of them: a spec tagged with both matches no project and never runs.

### Servers and state

Playwright starts two mock servers and stops them when the run ends. One is the deterministic mock on the Vite dev server (port 5273, artifacts on 5280). The other is the production build, rebuilt first, served with the mock (port 4273, artifacts on 4280). They sit apart from `just web-mock` (5173) and `just web-mock-preview` (4173), so a mock you started yourself can stay up. Two suites at once on one machine, from two worktrees for instance, each set their own `E2E_DEV_PORT` and `E2E_PREVIEW_PORT`; a port that is already taken is an error, never a server to reuse.

Every test shares one mock, whose state is global to its server, so the suite runs on one worker and each test starts with `POST /api/mock/reset`. Don't pass `--workers`, and don't rely on what another test did. Requests to anything but localhost are aborted, so the internet can't change a run (a web font fails fast and falls back the same way everywhere).

### Writing a spec

Import `test` and `expect` from `e2e/support/fixtures`, not from `@playwright/test`. Those are Playwright's with the harness added: the mock reset before each test, the loopback-only page, and the skips described below. A spec takes Playwright's own fixtures (`page`, `request`, `isMobile`), and `mock`:

```ts
import { expect, test } from "../support/fixtures";

test("a teammate's message shows in the chat", async ({ page, mock }) => {
  await mock.post("/api/mock/teammate-message", { params: { agent: "atlas" } });
  await page.goto("/agent/atlas");
  await expect(page.getByText("scout asked me to check the wiki index")).toBeVisible();
});
```

`mock.post(path, { params, data })` calls one of the mock's test controls (see [Deterministic mode](#deterministic-mode)) and fails the test on any answer but 2xx. Locate elements by role and accessible name, and wait with web-first assertions (`expect(locator).toBeVisible()`), never with fixed sleeps. The mock runs with zero delays and a fixed clock, so what a spec waits for arrives at once and looks the same on every run; `POST /api/mock/delays` slows it down for a spec about waiting.

**Accessibility.** `expectNoAxeViolations(page)` from `e2e/support/axe.ts` scans the page as it stands and fails on serious and critical violations, naming the rule and the elements. Put the page in the state under test first (open the menu, then scan), and scan each place and overlay a change touches. The scan waits for animations that end, such as an overlay fading in, since text at part opacity reads as low contrast. `{ within: "[role=dialog]" }` limits the scan to a region. The full result is attached to the test. A screen that is known to fail and that a later change replaces lists its violations in the spec, each with the reason and the change that removes it: `{ allow: [{ rule: "color-contrast", reason: "legacy header, removed with the new shell" }] }`. The scan fails when an allowed rule no longer fires, so an entry goes away with the screen it excuses. Minor and moderate findings don't fail; they are in the attached result.

**Visual.** `expectScreenshot(page, "name")` from `e2e/support/screenshot.ts` compares the page with the baseline `name` in `e2e/__screenshots__/`, one per project. Wait for the page to reach the state under test first; the helper waits only for fonts. Put `@visual` on the test, so it runs in the `visual-*` projects, where the page's clock is frozen at the mock's clock (so "2h ago" reads the same on every run) and reduced motion is on. The helper turns off animations and hides the caret, and it paints over anything carrying a `data-visual-mask` attribute; pass `mask: [locator]` to cover a region for one screenshot. Compare the viewport unless a spec needs `fullPage: true`.

Outside the container a `@visual` spec skips itself and says why, so `just web-e2e-fast` never compares screenshots. The WebKit project skips itself, naming the reason, when this machine can't start WebKit (run it through `just web-e2e-webkit` instead).

### Baselines

Text and image rendering differ between machines, so baselines are only ever made and compared inside the official Playwright container (`mcr.microsoft.com/playwright`, the `-noble` image for the installed Playwright version, pulled for `linux/amd64` on any host). `scripts/with-playwright-container.sh` starts it with a browser server, the recipes run the suite against that browser, and the mock stays on your machine. The image tag follows the exact version pinned in `package.json`, so bumping `@playwright/test` means running `just web-e2e-update` and reviewing every changed image.

`just web-e2e-update` rewrites the baselines. Look at each changed image under `e2e/__screenshots__/` before committing it: a baseline records whatever the page showed, including a bug.

### When a spec fails

The run keeps a trace and a screenshot of each failed test under `web/test-results/`, and writes the HTML report to `web/playwright-report/`. Open a trace with `npx playwright show-trace test-results/<test>/trace.zip` to step through the actions, the page at each one, the console and the network. A failed visual comparison leaves the actual, expected and diff images beside it. The release workflow uploads the report and traces as the `playwright-report` artifact when the suite fails.

### Release CI

The CI runners have no Chromium and can't download one, so the quality-checks workflow's web job runs every project in the Playwright container: it sets `E2E_ALL_IN_CONTAINER=1` and runs `npm run e2e` through `scripts/with-playwright-container.sh`, which starts the container beside the runner and stops it afterwards. That is every spec, the visual comparisons included. Turning the workflow's `visual` input off runs `npm run e2e:fast` instead, and the job summary says the comparisons were skipped. The workflow can also be started by hand from the Actions tab, to try a change on the runners without cutting a release.

`E2E_ALL_IN_CONTAINER=1` is the same switch locally. Under the wrapper it sends `desktop`, `phone` and the `preview-*` projects to the container's browser as well, so the run needs no Chromium on your machine: `E2E_ALL_IN_CONTAINER=1 scripts/with-playwright-container.sh npm --prefix web run e2e:fast`. Without a container browser the variable is an error, never a quiet fall-back to this machine's Chromium. The recipes leave it off, so `just web-e2e` and `just web-e2e-fast` keep using this machine's Chromium for those projects.

## Running Against the Real Backend

If you have the Rust backend running on port 7700:

```bash
npm run dev
```

This uses Vite's proxy to forward `/api` and `/ws` requests to `localhost:7700`.
