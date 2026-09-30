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

`mock/route-parity.test.ts` calls every function `src/lib/api.ts` exports, with sample arguments, against a `fetch` that records the requests. It checks each recorded method and path against the mock: scoped the way the mock's handler scopes it, then matched against `apiRoutes` (the route table every mock route is in), plus the hub and agent socket paths and the routes artifacts reach through the bridge. A client function without a sample call, or a request the mock doesn't serve, fails `npm test` and names the function and route. When you add an API client function, add its sample call there, and the route to the mock in the same change.

## Project Structure

```
web/
├── src/
│   ├── main.ts               # App entry point
│   ├── App.svelte            # Layout — header, sessions sidebar, chat / session view, settings
│   ├── Chat.svelte           # Main chat view
│   ├── Setup.svelte          # Setup wizard
│   ├── Settings.svelte       # Settings panel
│   ├── styles/               # Global styles (tokens in variables.css)
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
│   └── lib/
│       ├── api.ts                # REST API client (typed fetch wrappers)
│       ├── ws.svelte.ts          # WebSocket coordinator: routes frames to the feed and sessions stores
│       ├── feed.svelte.ts        # Main chat feed state
│       ├── feed-items.ts         # History-to-feed conversion shared by chat and session views
│       ├── sessions.svelte.ts    # Agent sessions: listing, live frames, session view, commands
│       ├── routes.ts             # URL <-> location: session, workspace flag, settings section, workbench artifact
│       ├── router.svelte.ts      # Current location; push/replace history, back/forward
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

The URL is the source of truth for where the user is:

| URL | Shows |
|-----|-------|
| `/` | Redirects to the last-used agent (or the first) |
| `/agent/:name` | That agent's main chat |
| `/agent/:name/sessions/:runId` | A session's run in the main pane |
| `/agent/:name/workspace` (or `?workspace`) | Workspace panel open beside the main pane |
| `/agent/:name/scheduled` | Pulses and scheduled actions |
| `/agent/:name/settings/:section` | Agent settings: runtime, providers, channels, pulses, memory, skills, mcp, a2a, webhooks, history |
| `/team` | Team view: every agent with lifecycle controls and the create form |
| `/team/files` | Shared team files |
| `/team/workbench[/:artifact[?full]]` | The workbench's artifact list, one artifact, or one filling the window |
| `/team/settings/:section` | Hub settings: general, cloud, a2a, sessions, tracing, update, secrets, agent-keys, history |

The agent switcher under the header is on every page. Older unprefixed links (`/settings/...`, `/workbench/...`, `/scheduled`, `/sessions/:runId`) redirect to the agent or team page they belong to, and a settings section named under the wrong scope redirects to the scope that has it (`/settings/agent-keys` goes to `/team/settings/agent-keys`, `/settings/integrations` to the agent's `channels`).

`App.svelte` derives its layout state from `router` instead of mounting a component per route, so the chat, session view, and workspace stay mounted and every transition is the same CSS transition whether it came from a click or the back button. Navigate through `router` (or `sessions.openRun`), never by setting layout state directly.

History records places, not panel states. Opening a session, returning to the main chat, and opening or switching settings push an entry. Toggling the workspace replaces the current one, and so does a session view following its session into a new run. Back therefore moves between places the user visited. A settings URL says nothing about the chat side, so leaving settings returns to the session and workspace state that was showing before. Overlays (help, feedback, inbox) and the narrow-screen sessions drawer are not in the URL.

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

**Style lint.** `npm run lint` also runs Stylelint over `src/**/*.css` and the `<style>` blocks of `.svelte` files. Outside the token file (`src/styles/variables.css`) it forbids literal colors (hex, named, `rgb()` and the like), raw `font-size` and `font` values, raw `z-index` values, literal durations and easing curves in `transition` and `animation`, and `transition: all`. Reference a token with `var(--…)` instead. Stylesheets and components that still carry literal values are listed in `stylelint.config.js` and exempt from these rules; remove an entry when its file is rewritten or deleted, and never add new styles to the list.

**svelte-check.** It runs with `--fail-on-warnings`. The one accepted warning, a label without an associated control, is filtered in `svelte.config.js`.

**Generated types.** `src/lib/generated/` comes from the Rust types. After changing an exported Rust type, run `just types` and commit the result; `just types-check` (and CI) fails when the committed files are out of date.

**Mock modules.** Everything under `mock/` is formatted, linted and type-checked with the same rules as `src/`, and its tests (`mock/**/*.test.ts`) run in Node through the same `npm test`. Route handlers live in route tables (`Route` in `mock/routes.ts`), and response bodies are checked against the generated protocol types in `src/lib/generated/` wherever one exists. The Vite plugin entry is `mock/plugin.ts`, and nothing in the mock is left out of these checks. `mock/test-support.ts` has two harnesses: one serves route tables over HTTP against a stub hub, and `startMockServer` runs the whole mock (hub, agents, sockets, scoped routing) on a real HTTP server, with a WebSocket client that keeps the frames it receives. Both take a mock environment (`createMockEnv({ deterministic: true })`, or `startMockServer({ deterministic: true })`), so a test that asserts on times or ids runs on the fixed clock.

Component tests live next to the component as `src/components/**/*.test.ts` (or `*.component.test.ts` anywhere under `src/`). They run in jsdom, through the same `npm test` command as the Node unit tests under `src/lib/`. Mount with `render` and mock `fetch` using `src/test/component.ts`.

## Running Against the Real Backend

If you have the Rust backend running on port 7700:

```bash
npm run dev
```

This uses Vite's proxy to forward `/api` and `/ws` requests to `localhost:7700`.
