# Contributing to the Residuum Web UI

Welcome! This guide will get you up and running with the frontend without needing the Rust backend.

## Prerequisites

- **Node.js** `^22.13.0 || ^24.0.0 || >=26.0.0` (the `engines` range in `package.json`, set by the test tooling) — check with `node --version`
- **npm** — comes with Node.js

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
- The multi-agent hub contract: four agents (`scout` and `atlas` running, `drifter` stopped, `brittle` failed), each with its own chat, sessions, inbox and config under `/api/agents/{name}/...`. `/api/hub/agents` lists, creates, deletes, starts, stops and restarts them and toggles autostart, and `/api/hub/ws` sends the agent snapshot, state changes, busy/unread activity and notices. Unscoped `/api/...` paths answer 404. A stopped or failed agent serves the routes the backend serves for one: the repair routes (config, providers, MCP, workspace, checkpoints) and the file-only ones (chat history, usage, inbox, raw A2A settings). Every other agent route answers `409`, and an unknown agent `404`. An agent that has never run (`drifter`, `brittle`) has no conversation, inbox or A2A agents until it first runs. Team files and the workbench are shared under `/api/team/...`. `POST /api/mock/teammate-message?agent=atlas` sends atlas a teammate message and lights its unread indicator until you open it
- Agent sessions: live sessions (including a Discord conversation session) and a page-able list of finished ones. Messaging a session simulates a turn (include "busy" in the message to see a delivery failure), messaging a finished one resumes it, and a chat message starting with `spawn` starts a spawned session that relays its result to the main chat. Transcripts load after a short delay, so the loading state and anything racing it can be tried by hand
- The `POST /api/agents/{name}/sessions` / `.../stop` / `.../messages` HTTP endpoints an artifact's `residuum.sessions.start` uses: the bundled "Tip Splitter" artifact (`/workbench/tip-splitter`) has "Start a background session" and "Fire 3 calls at once" buttons for trying the artifact bar's activity panel, Cancel calls, and Stop page by hand. Like every artifact it names its agent, `atlas`, in `ask` and `sessions.start`, and a session's frames reach the artifact while atlas is the agent the web UI has open. Model calls are slowed down (`MODEL_CALL_DELAY_MS`) so they're visibly "in flight" long enough to cancel
- Tasks sent to other agents in the sessions sidebar's External group: stopping `research-buddy`'s task succeeds, while `laptop` is unreachable, so its Stop fails and the row offers "Stop watching"
- `POST /api/mock/missed-relay` records a session result in the main chat's history and drops the WebSocket, to exercise catching up after a reconnect
- Workspace files, for an agent (`/api/agents/{name}/workspace/...`) and for the shared team tree (`/api/team/workspace/...`): directory listings with size, modification time and version, reads with the version as the `ETag`, writes that answer `412` when the client's `If-Match` no longer matches, and delete, move, validate, `dir`, `raw` reads and writes, the recursive `tree` (with `glob`, `depth` and `content`) and the batch `read` with the backend's size budgets. Edits change the listings, and the team tree is the same one under an agent's `team/`
- The Scheduled view (`/api/agents/{name}/scheduled/...`): the pulses, with their next fire, last outcome and current run worked out from the agent's sessions the way the backend reads them, toggling a pulse, and the pending actions with cancel. One pulse is disabled and one failed to load
- Checkpoints, for an agent (`workspace` and `agent_config` repositories) and for the hub (`hub` and `team`): list with `path`, `turn_id` and paging, stats, a checkpoint's detail, diff and file, restore and undo. Each repository keeps the whole tree of each checkpoint, so a restore writes the files back (Settings, the workspace and the team tree show it) and an undo skips a path that changed again since. A route answers `400` for a repository of the other scope, like the backend. The sample histories end at the live files, and `status` reports their stats
- `POST /api/agents/{name}/agent-inbox` (what an artifact adds to the agent's own inbox, with the backend's ids, title default and `artifact:<name>` source), and the update routes (`/api/hub/update/status`, `check` and `apply`; the mock is always on the latest version) with `cloud/disconnect`
- The user inbox: a listing, an archive, mark read, archive, restore and attachments (`/api/agents/{name}/inbox/...`), with the backend's response shapes, including its `500` for an item that isn't there. No sample item carries an attachment; an attachment serves a stand-in file of its type
- The workbench (`/api/team/workbench/...`): the artifact list, where artifacts are served, and deleting an artifact along with its saved state. Artifacts are files in the team tree, `team/workbench/<name>.html` or a folder `team/workbench/<name>/index.html` with the files it loads, found the way the backend finds them (a folder wins over a page of the same name; `<name>.*` data files aren't part of an artifact). A delete checkpoints the team first and returns the checkpoint's id, so Undo (a restore of each removed path through the hub's checkpoint routes) brings back the page or folder and its data files. `POST /api/agents/{name}/model/complete` answers from a canned model, with parsed JSON when the call asks for a schema
- The artifacts listener, on its own port, serves those files as the real listener does: `/{name}/` is the page, `/{name}/{path}` a file in a folder artifact (a trailing `/` means that folder's `index.html`), `/{name}` redirects to `/{name}/` with a `308` that keeps the query, and a path that climbs out of the folder, a missing artifact or file, and anything but `GET` and `HEAD` are refused with the backend's pages and statuses. Files go out with `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`, and every HTML file gets the SDK at the same place the backend puts it (after `<head>`, else `<html>`, else the doctype, else first)
- Live updates for team files, through a test control: `POST /api/mock/team-file` simulates an agent editing or deleting a team file. The body is `{ "path": "team/workbench/tip-splitter.html", "content": "<title>…" }`: `path` is in the file API's namespace (under `team/`), and `content` is the file's new text, or `null` to remove it (a folder goes with everything in it). The mock's files change, then its sockets send what the real change feed does: `workspace_changed` (one change, `created`, `modified` or `removed`; a new folder is reported alone, standing for what it holds) to the hub socket and to every agent socket whose `watch_team` or `watch_workspace` prefixes match (by whole path segments; a prefix above or at the path matches, and so does one below a removed folder), and, when the change touches an artifact's page or folder, `artifact_updated` or `artifact_removed` to every agent socket, whatever it watches. The answer is `{ "changes": [...], "artifacts": { "updated": [...], "removed": [...] } }`, what was sent. An artifact's saved data doesn't count as the artifact, and a rewrite that leaves its files as they were sends no artifact frame. A path outside `team/`, or a folder to write to, answers `422`, and removing what isn't there `404`
- Main chat turns are recorded in history when they end. A chat message starting with `drop` loses the connection mid-turn: `drop finish …` ends the turn while disconnected, `drop compress …` also compresses history into a new episode (forcing a history reload), and any other `drop …` finishes the turn live after the page reconnects
- Config files are loaded from `../assets/*.example.*` and can be edited in the UI
- Secrets can be added and removed (stored in memory)
- Agent keys list with one user key and one agent-saved key, and can be added and removed (stored in memory)

### What's NOT mocked

- No real LLM calls happen — responses are canned
- Config saves don't persist across server restarts
- Some edge cases (rate limits, network errors) aren't simulated
- The workspace routes don't block paths the backend blocks, and a delete, move or raw write records no checkpoint (deleting a workbench artifact does). Only `POST /api/mock/team-file` sends `workspace_changed` and the artifact frames: a write through the workspace routes, or an Undo, changes the files without announcing it, and the mock has no batches, resyncs or lag. A raw write stores its body as text, so bytes that aren't valid UTF-8 don't round-trip
- The Scheduled view's pulses and actions are kept apart from `HEARTBEAT.yml` in the workspace: toggling a pulse doesn't edit that file
- `POST /api/secrets` doesn't validate the value like the real server does — it accepts anything, including a `secret:` or `${ENV_VAR}` reference the real server would reject with a 400. The frontend already avoids sending those (see `lib/secrets.ts`), so this only matters if you're testing the rejection path itself

### Setup Wizard Mode

To test the first-run setup wizard:

```bash
VITE_MOCK_SETUP=1 npm run dev:mock
```

This starts the app in "setup" mode so you can walk through the onboarding flow.

### Deterministic mode

```bash
MOCK_DETERMINISTIC=1 npm run dev:mock -- --port 5173 --strictPort   # or: just web-mock-serve 5173
```

With `MOCK_DETERMINISTIC=1` the mock gives the same responses to the same steps, so end-to-end and visual tests can rely on them:

- **One fixed clock.** Every timestamp the mock makes (sample inbox and sessions, workspace versions, workbench and chat times, `busy_since`, uptime) reads one clock that stands at 2026-03-14 12:00 UTC until a test moves it with `POST /api/mock/clock/advance` (`{ "ms": 3600000 }`). Dates are read in UTC, so a machine's time zone changes nothing. Ids that would otherwise come from the time or from chance (tool call ids, turn ids, A2A key tokens, delete checkpoint ids) come from a counter that starts over at reset.
- **No delays.** Model calls, transcript loads, agent start and stop windows, reloads and every step of the chat and session simulations take no time, and run in the order they were started. `MOCK_DELAY_SCALE` multiplies all of them (`1` is the natural pace, `0.2` a fifth of it), and `POST /api/mock/delays` (`{ "scale": 1 }`) changes it until the next reset. A chat message starting with `drop` relies on real time to lose and regain the connection, so run it with a scale above zero.
- **The scenario** is the one `dev:mock` starts with: scout and atlas running, drifter stopped, brittle failed, and the sample sessions, outbound tasks, inbox, checkpoints and artifacts. The hub announces the same boot id every time.
- **A fixed artifacts port**, 5180 (or `MOCK_ARTIFACTS_PORT`), where a live mock takes a free one. A port that can't be bound is logged, and the workbench reports artifacts as unavailable.

`POST /api/mock/reset` puts the mock back as it started: the clock, the delays, the counter, every pending timer (a request waiting on simulated time answers `503`), the hub's own state (secrets, hub config, team files, workbench), and the agents, which are recreated from the scenario with their sessions, inbox and files. Every open socket is closed, so a page reconnects to the initial scenario. The port and the artifacts listener stay.

### Preview mode

```bash
just web-mock-preview 4173   # builds, then: MOCK_DETERMINISTIC=1 npm run preview:mock -- --port 4173 --strictPort
```

`npm run preview:mock` (`VITE_MOCK=1 vite preview`) serves the production build in `dist/` together with the whole mock: the API, the hub and agent sockets, and the artifacts listener. It is what service worker and installability tests run against, since the dev server can't serve a built app. Both modes start headlessly, need no browser, and take the port with `--port`; `--strictPort` makes a taken port an error instead of moving on.

### Route parity

`mock/route-parity.test.ts` calls every function `src/lib/api.ts` exports, with sample arguments, against a `fetch` that records the requests. An agent-scoped call samples with an agent (`"atlas"`), and a call that also serves the team, the hub or onboarding samples with `null` for that scope as well as an agent for the agent scope. It checks each recorded method and path against the mock: scoped the way the mock's handler scopes it, then matched against `apiRoutes` (the route table every mock route is in), plus the hub and agent socket paths and the routes artifacts reach through the bridge. A client function without a sample call, or a request the mock doesn't serve, fails `npm test` and names the function and route. When you add an API client function, add its sample call there, and the route to the mock in the same change.

## Project Structure

```
web/
├── src/
│   ├── main.ts               # App entry point
│   ├── App.svelte            # Layout — header, sessions sidebar, chat / session view, settings
│   ├── Chat.svelte           # Main chat view
│   ├── Setup.svelte          # Setup wizard
│   ├── Settings.svelte       # Settings panel
│   ├── styles/               # Design tokens, bundled fonts, base styles, legacy global styles
│   ├── components/
│   │   ├── ChatFeed.svelte         # Main chat message list (lazy-loads older episodes)
│   │   ├── ChatInput.svelte        # Input box with slash commands
│   │   ├── ChatFooter.svelte       # Quiet status line: model, session tokens, context size
│   │   ├── ThinkingIndicator.svelte # Running-turn indicator: elapsed time, tokens, stop hint
│   │   ├── FeedItemView.svelte     # Renders one feed item; shared by chat and session views
│   │   ├── Message*.svelte         # Message components (user, assistant, agent message, status, …)
│   │   ├── ToolGroup.svelte        # Groups related tool calls together
│   │   ├── ToolItem.svelte         # Individual tool call display
│   │   ├── SessionsSidebar.svelte  # Live and finished agent sessions
│   │   ├── SessionView.svelte      # One session's transcript, live activity, message box, stop
│   │   ├── Header.svelte           # Top bar with navigation
│   │   ├── AgentSwitcher.svelte    # Persistent agent switcher: state, working and unread per agent
│   │   ├── TeamView.svelte         # Team page: lifecycle controls, autostart, delete, create agent
│   │   ├── Workbench.svelte        # Workbench artifact list; hosts the open artifact
│   │   ├── WorkbenchArtifact.svelte # One artifact in its sandboxed frame; full view
│   │   ├── settings/               # Settings sub-panels
│   │   └── setup/                  # Setup wizard steps
│   ├── test/                 # Component-test helpers and harnesses
│   └── lib/
│       ├── ui/                   # Primitive controls (buttons, fields, badges, tabs, banners…); gallery at /dev/gallery (see AESTHETIC.md)
│       ├── icons/                # The Icon component and icon set
│       ├── api.ts                # REST API client (typed fetch wrappers); every agent-scoped call takes the agent name first
│       ├── paths.ts              # API and WebSocket URL builders for the agent, hub and team scopes
│       ├── viewed-agent.ts       # The bound agent: the router publishes it, the WebSocket coordinator binds to it
│       ├── ws.svelte.ts          # WebSocket coordinator: routes frames to the feed and sessions stores
│       ├── feed.svelte.ts        # Main chat feed state
│       ├── feed-items.ts         # History-to-feed conversion shared by chat and session views
│       ├── sessions.svelte.ts    # Agent sessions: listing, live frames, session view, commands
│       ├── routes.ts             # URL <-> location: places, the panel and settings parameters, redirects from old URLs, corrections
│       ├── router.svelte.ts      # Current location; push/replace, closing by going back, overlay entries, the unsaved-edit guard
│       ├── history-entry.ts      # The marks the router keeps in history.state
│       ├── navigation-guard.ts   # Checks views register for unsaved work, and how the user is asked
│       ├── settings-sections.ts  # Settings section registry: ids, scopes, labels, groups, old names, config keys
│       ├── legacy-router.svelte.ts # The current views' navigation, on the router
│       ├── legacy-settings-sections.ts # The current Settings page's sections
│       ├── session-address.ts    # Opens a session from where it is mentioned
│       ├── relay.ts              # Recognizes agent-message headers in transcripts
│       ├── workbench-bridge.ts   # What workbench artifacts may call, relayed from their frames on the artifacts origin
│       ├── workbench.ts          # Where artifacts are served: relay origin or this host on the artifacts port
│       ├── time.ts               # Relative times ("5m ago")
│       ├── generated/            # Protocol types generated from Rust (cargo test --test ts_export)
│       ├── types.ts              # TypeScript types for API and messages
│       ├── commands.ts           # Slash command parser (/help, /reload, etc.)
│       ├── models.ts             # Model fetching and caching
│       ├── markdown.ts           # Markdown rendering
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
│   ├── hub-config-reload.ts  # The hub's config reload and its frames
│   ├── lifecycle.ts          # Agent lifecycle routes and hub status
│   ├── hub-socket.ts         # The hub WebSocket: hub frames, team watches
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
│   ├── scheduled.ts          # The Scheduled view: pulses and actions
│   ├── checkpoints.ts        # Checkpoint histories: list, stats, detail, diff, file, restore, undo
│   ├── update.ts             # The update routes
│   ├── workbench.ts          # Workbench artifact list, info and delete (with its checkpoint)
│   ├── workbench-files.ts    # Artifacts in the team tree: discovery and file lookup
│   ├── artifact-name.ts      # The artifact name rule, shared by the workbench and the identity header
│   ├── team-changes.ts       # Changing a team file and sending its live-update frames
│   ├── model.ts              # The artifact model call
│   ├── controls.ts           # Test controls: reset, clock, delays, missed-relay, teammate-message and team-file
│   ├── artifacts-listener.ts # The second origin that serves artifact pages and files, with the SDK injected
│   ├── mock.ts               # Starts the mock on a Vite server, from the environment's options
│   ├── plugin.ts             # The Vite plugin: starts the mock on the dev server and the preview server
│   ├── data/                 # Sample data: chat, sessions, workspace files, inbox, the workbench artifact
│   ├── test-support.ts       # Test harnesses: route tables over HTTP, the whole mock with its sockets
│   ├── route-parity.test.ts  # Every API client request lands on a mock route
│   └── *.test.ts             # Unit tests, run by `npm test`
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

**Navigation.** `openPlace`, `openPanel`, `openSettings` and `openSettingsSection` (a section opened from the phone's section list) push. `replacePlace`, `replacePanel`, `switchSettingsSection` and `switchSettingsScope` replace, and so does every correction and redirect. Each returns whether the navigation happened.

**Closing.** `closePanel` and `closeSettings` go back in the history when this page pushed the entry that opened what is closing, and replace the URL with one that omits the parameter otherwise (a deep link, a reload into the modal). Back therefore closes the panel or modal before it leaves a place.

**Overlays.** A modal overlay calls `router.openOverlay(onDismiss)` when it opens. That pushes an entry with the same URL, so Back closes the overlay. The overlay closes itself through the returned handle (`handle.close()`), which pops that entry. `onDismiss` runs when the entry is left any other way: Back, or a navigation that takes the overlay's entry.

**Unsaved work.** A view that holds work the user would lose registers a check with `router.guard.register(check)`. The check returns a line saying what navigating to the given location would lose, or null. In-app navigation asks first, through the function the app gives `router.guard.setConfirm`, and does not navigate until the user confirms; with no such function it refuses. On Back or Forward the router puts the location back on top of the history, asks, and goes where the user was headed only if they confirm. On reload or tab close the browser's own prompt appears.

**The bound agent** is the viewed agent on an agent place, and on the other places the agent most recently viewed. The last-used agent is remembered in local storage, and `router.setKnownAgents` settles on agents that exist once the agent list is known.

The current views (the header menu, the Settings page, the workbench, the scheduled view, the team pages, the inbox drawer) navigate through `legacyRouter` in `lib/legacy-router.svelte.ts`. It turns their commands into router navigations, and reads off the router's location which old view fills the window and which old Settings section hosts the new one. The workbench's full view is a mode of that page and isn't in the URL. Overlays other than the inbox (help, feedback) and the narrow-screen sessions drawer aren't in the URL.

### Agents in API calls

No request reads the viewed agent. Every agent-scoped function in `lib/api.ts` takes the agent name as its first argument, and its cache key includes that agent. Components take the agent from their props (`Settings`, `Workspace`), stores hold the agent they were bound to (`ws.sessions`, `scheduled`, `userInbox`), and the chat's controls use `ws.agent`, the agent the WebSocket is bound to. The workbench bridge maps an artifact's unscoped paths onto `ws.agent` per request.

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
| `undo()` | Restores the last save's checkpoints in reverse order and reports each file's reverted and skipped paths, naming any file it couldn't restore. |
| `fieldDiagnostics(ref)` and `sectionDiagnostics(section)` | The problems from a save. A diagnostic whose key path names a form field is on that field; the rest are for the top of a section. |

`dirty`, `saving`, `lastResult` and `undoable` drive the save bar. `file(name)` gives a file's `lockedBy` (`"form"` keeps its raw editor read-only, `"raw"` keeps its form read-only while the raw editor holds a draft set with `setRawDraft`), `changedOnDisk`, `unreadable` and `loadError`. A change to a file that has staged changes leaves them alone, so the coordinator's re-read before Save finds the clash. A typed credential is stored under its name (`discord`, `webhook_<name>`, a provider's name) and the reference goes in the file. Immediate actions have no state here.

`lib/settings-fields.ts` is the one map from a form field to its key (`keyPathOf`), which diffing and diagnostic placement (`locateField`) share, so a field that saves to a key is the field that shows that key's error. A form field the map doesn't cover fails `settings-fields.test.ts`. Failover lists ride along with a role (`models.fallbacks`) so a save never shortens one.

The model's tests are `settings-model.component.test.ts`, so they run with live runes in jsdom.

## Code Quality

Before submitting changes, run:

```bash
npm run lint          # ESLint, then the style lint
npm run format        # Prettier auto-format
npm run check         # TypeScript / Svelte type check; warnings fail it
npm test              # Vitest: lib unit tests and Svelte component tests
npm run test:coverage # The same tests with a coverage summary (HTML report in coverage/)
```

**TypeScript lint.** Every `.ts` module under `src/`, including the rune store modules (`*.svelte.ts`), gets the strict type-aware ESLint rules. Only `.svelte` files get the relaxed set that fits runes. When a rule is wrong for one line, use a scoped `// eslint-disable-next-line <rule> -- <reason>`, never a blanket disable.

**Style lint.** `npm run lint` also runs Stylelint over `src/**/*.css` and the `<style>` blocks of `.svelte` files. Outside the token files (`src/styles/tokens.css`, the design token set described in [AESTHETIC.md](./AESTHETIC.md), and `src/styles/variables.css`, the legacy variables) it forbids literal colors (hex, named, `rgb()` and the like), raw `font-size` and `font` values, raw `z-index` values, literal durations and easing curves in `transition` and `animation`, and `transition: all`. Reference a token with `var(--…)` instead. Viewport media queries may use only the shell breakpoints, written as `min-width`/`max-width`; a component that needs its own responsive rule uses a container query. Stylesheets and components that still carry literal values are listed in `stylelint.config.js` and exempt from these rules; remove an entry when its file is rewritten or deleted, and never add new styles to the list.

**svelte-check.** It runs with `--fail-on-warnings`. The one accepted warning, a label without an associated control, is filtered in `svelte.config.js`.

**Generated types.** `src/lib/generated/` comes from the Rust types. After changing an exported Rust type, run `just types` and commit the result; `just types-check` (and CI) fails when the committed files are out of date.

**Mock modules.** Everything under `mock/` is formatted, linted and type-checked with the same rules as `src/`, and its tests (`mock/**/*.test.ts`) run in Node through the same `npm test`. Route handlers live in route tables (`Route` in `mock/routes.ts`), and response bodies are checked against the generated protocol types in `src/lib/generated/` wherever one exists. The Vite plugin entry is `mock/plugin.ts`, and nothing in the mock is left out of these checks. `mock/test-support.ts` has two harnesses: one serves route tables over HTTP against a stub hub, and `startMockServer` runs the whole mock (hub, agents, sockets, scoped routing) on a real HTTP server, with a WebSocket client that keeps the frames it receives. Both take a mock environment (`createMockEnv({ deterministic: true })`, or `startMockServer({ deterministic: true })`), so a test that asserts on times or ids runs on the fixed clock.

Component tests live next to the component as `src/components/**/*.test.ts` (or `*.component.test.ts` anywhere under `src/`). They run in jsdom, through the same `npm test` command as the Node unit tests under `src/lib/`. Mount with `render` and mock `fetch` using `src/test/component.ts`. Drive keyboard and pointer input with `@testing-library/user-event`, which presses keys the way a browser does (Space and Enter activate buttons). `src/test/snippets.ts` turns markup into a snippet for a component's `children`, and `src/test/ui/` holds small harnesses for components that need a parent, such as a context provider or a live snippet.

## Running Against the Real Backend

If you have the Rust backend running on port 7700:

```bash
npm run dev
```

This uses Vite's proxy to forward `/api` and `/ws` requests to `localhost:7700`.
