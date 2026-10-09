# Contributing to the Residuum Web UI

Welcome! This guide covers the Residuum web app (a Svelte 5 single-page app that the Rust binary embeds): how to run it without the Rust backend, how it is organized and how its parts work, the checks every change passes, and how it is tested. [AESTHETIC.md](./AESTHETIC.md) covers the visual system.

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
- WebSocket simulates chat responses with tool calls and delays: a message reaches every page as a `user_message` frame, then the turn starts. A chat turn thinks, says it is looking through its notes, searches memory, reads two team wiki files at once, then writes its reply, and a message starting with `fail` can't read the second file. Text and reasoning stream in as `text_delta` and `thinking_delta` pieces, each followed by the complete `thinking` and `broadcast_response` (or `response`) of its model call, as the backend sends them. Like the backend, it sends tool calls and results only to pages that turned verbose mode on, reads client frames strictly (a frame it can't read gets an `error` frame), and `/stop` ends a running turn
- Scoped routing, as the backend has it: an agent's path (`/api/agents/{name}/...`) reaches only what an agent owns, the hub's (`/api/hub/...`) only what the hub owns, and the team's (`/api/team/...`) only the workbench and team files. `/api/agents/scout/workbench/artifacts` and `/api/hub/workspace/files` answer `404`, like the real routes
- The multi-agent hub contract: four agents (`scout` and `atlas` running, `drifter` stopped, `brittle` failed), each with its own chat, sessions, inbox and config under `/api/agents/{name}/...`. `/api/hub/agents` lists, creates, deletes, starts, stops and restarts them and toggles autostart, and `/api/hub/ws` sends the agent snapshot, state changes, busy/unread activity and notices. Unscoped `/api/...` paths answer 404. A stopped or failed agent serves the routes the backend serves for one: the repair routes (config, providers, MCP, workspace, checkpoints) and the file-only ones (chat history, usage, inbox, raw A2A settings). Every other agent route answers `409`, and an unknown agent `404`. An agent that has never run (`drifter`, `brittle`) has no conversation, inbox or A2A agents until it first runs. Team files and the workbench are shared under `/api/team/...`. `POST /api/mock/teammate-message?agent=atlas` sends atlas a teammate message (with the backend's teammate header and sender) and lights its unread indicator until you open it. An agent's start fails while its `providers.toml` names a model its provider doesn't offer, judged by the lists the mock's model listing answers with; the providers validate route and a raw save report such a model on the role's key path (`models.main`). brittle's main model is `openai/gpt-9`, so it fails every start until Settings, or `POST /api/mock/fix-agent?agent=brittle` (which swaps each such model for its provider's first), names one the provider offers; its next start succeeds
- Agent sessions: live sessions (including a Discord conversation session) and a page-able list of finished ones. Messaging a session simulates a turn (include "busy" in the message to see a delivery failure), messaging a finished one resumes it, and a chat message starting with `spawn` starts a spawned session that relays its result to the main chat. Transcripts load after a short delay, so the loading state and anything racing it can be tried by hand
- The `POST /api/agents/{name}/sessions` / `.../stop` / `.../messages` HTTP endpoints an artifact's `residuum.sessions.start` uses: the bundled "Tip Splitter" artifact (`/team/workbench/tip-splitter`) has "Start a background session" and "Fire 3 calls at once" buttons for trying a page's sessions and model calls by hand: open it from the Workbench, and the sessions it starts show on its row. Like every artifact it names its agent, `atlas`, in `ask` and `sessions.start`, and its sessions' frames reach it through the hub socket's session relay, whichever agent the web UI has open. Model calls are slowed down (`MODEL_CALL_DELAY_MS`) so they're visibly "in flight" long enough to cancel
- The hub socket's session relay. A page subscribes with `subscribe_session` (`{ agent, address }`) to one session, or with `subscribe_artifact_sessions` (`{ artifact }`) to every session whose source label is `artifact:<artifact>` on any agent, including ones that start later, and stops with the matching `unsubscribe_` message. Each subscribe is acknowledged with `subscribed` before any frame, and one that names an unknown agent gets a warning `notice` and no acknowledgement (a stopped agent is known). Every `session_*` frame an agent's state broadcasts then reaches the pages that follow it as `session_frame` `{ agent, frame }`, where `frame` is exactly what the agent's socket sent, tool frames included, because the hub socket has no verbose flag. Subscriptions end with the connection. `POST /api/mock/session-relay-lag` sends `session_relay_lagged` to every page that follows something, as a connection that fell behind the relay is told, and answers `{ "notified": n }`; the page then reads its sessions again over HTTP
- Tasks sent to other agents, in Activity's Running now: stopping `research-buddy`'s task succeeds, while `laptop` is unreachable, so its Stop fails and the row says why beside "Stop watching"
- `POST /api/mock/hub-socket` with `{ "online": false }` takes the hub WebSocket down: every page is dropped and new connections are refused (the HTTP API stays up), so the hub banner shows. `{ "online": true }` lets pages connect again, and a reset does too
- `POST /api/mock/rebuild` stands in for rebuilding the app, in preview mode only: from then on the preview server serves `/sw.js` as another version of the same worker (its version string changed everywhere it appears), so a page that already runs the first one finds an update. It answers `{ "rebuilds": n }`, and a reset puts the worker back as built
- `POST /api/mock/missed-relay` records a session result in the main chat's history and drops the WebSocket, to exercise catching up after a reconnect
- The team event log: `GET /api/hub/events` (with `before`, `after` and `limit`) and a `team_event` frame on the hub socket for each new entry. It records what the mock's own lifecycle, chat turns, sessions and notices do, worded as the backend words it, with ids counting from 1 and times from the mock's clock. It starts with `hub_started` and what starting the scenario's agents did, and starts over on reset. `POST /api/mock/user-inbox-add?agent=atlas` (`{ title?, body?, attachments? }`, each attachment `{ filename, mime_type? }`) saves an item in an agent's user inbox the way its `user_inbox_add` tool does, which adds an `inbox_item_added` entry
- The team overview: `GET /api/hub/overview` and an `agent_overview` frame on the hub socket whenever an agent's overview changes. Each agent's last message, live sessions and unread inbox count are read from the data the mock's other routes serve (its conversation, sessions and inbox), `upcoming` lists its three soonest runs (its pulses by their next fire time, and its pending scheduled actions) and `outbound_problems` its open tasks to other agents that have been unreachable for ten minutes, and changes are gathered so an agent gets at most one frame per simulated second (the next tick when delays are off). A created agent is sent at once. It starts over on reset
- Push presence: the hub socket accepts `{ "type": "presence", "device_id", "active" }` and keeps what each connected page reported on the mock clock, as the hub does. A device is present for 60 seconds after an `active: true` report from a page that is still connected, and `active: false` or the page disconnecting ends it; a frame without a `device_id` string and an `active` boolean is refused with the warning `notice` any unreadable frame gets, and a valid one gets no answer. `GET /api/mock/push/presence` answers `{ "devices": [...] }`, the devices the real hub would send no push right now. The mock sends no pushes.
- Workspace files, for an agent (`/api/agents/{name}/workspace/...`) and for the shared team tree (`/api/team/workspace/...`): directory listings with size, modification time and version, reads with the version as the `ETag`, writes that answer `412` when the client's `If-Match` no longer matches, and delete (which checkpoints the workspace or team repository first and names it, so Undo restores it), move, validate (an agent's `config/channels.toml` is checked as TOML, with the line and column of a mistake; the backend's other checks aren't mocked), `dir`, `raw` reads and writes, the recursive `tree` (with `glob`, `depth` and `content`) and the batch `read` with the backend's size budgets. Edits change the listings, and the team tree is the same one under an agent's `team/`
- The Schedule place's routes (`/api/agents/{name}/scheduled/...`): the pulses, with their next fire, last outcome and current run worked out from the agent's sessions the way the backend reads them, toggling a pulse, and the pending actions with cancel. One pulse is disabled and one failed to load. Like the backend, they answer `409` for an agent that isn't running
- Checkpoints, for an agent (`workspace` and `agent_config` repositories) and for the hub (`hub` and `team`): list with `path`, `turn_id` and paging, stats, a checkpoint's detail, diff and file, restore and undo. Each repository keeps the whole tree of each checkpoint, so a restore writes the files back (Settings, Files and Shared files show it) and an undo skips a path that changed again since. A route answers `400` for a repository of the other scope, like the backend. The sample histories end at the live files, and `status` reports their stats. The `hub` repository tracks `config.toml` and the two key stores (`agent-keys.toml.enc`, held as opaque text, and `a2a-keys.toml`), so removing an agent key or revoking a caller key returns the id of a checkpoint that a restore of its store puts the key back from
- `POST /api/agents/{name}/agent-inbox` (what an artifact adds to the agent's own inbox, with the backend's ids, title default and `artifact:<name>` source), and the update routes (`/api/hub/update/status`, `check` and `apply`; the mock is always on the latest version)
- Residuum Cloud: `GET /api/hub/cloud/status` reads the hub's `[cloud]` table the way the backend does (a token and `enabled` make the tunnel connected, as `mock-user`), and `POST /api/hub/cloud/disconnect` turns it off, keeps the token and reloads the hub, or answers the remote-control guard's `403` while the status is held as read through the tunnel. Two test controls stage the rest: `POST /api/mock/cloud` (`{ tunnel?, via_tunnel? }`) holds the tunnel at `connecting`, `connected` or `disconnected` (`null` hands it back to the config) and makes the status read as served through the tunnel, and `POST /api/mock/cloud-callback` (`{ token? }`) is the relay handing a signed-in browser back to the gateway's `/cloud/callback`, which keeps the token as the `cloud_token` secret and switches `[cloud]` on
- The user inbox: a listing, an archive, mark read, archive, restore and attachments (`/api/agents/{name}/inbox/...`), with the backend's response shapes, including its `500` for an item that isn't there. No sample item carries an attachment, but `POST /api/mock/user-inbox-add` can give a new one some; an attachment serves a stand-in file of its type
- The workbench (`/api/team/workbench/...`): the artifact list, where artifacts are served, and deleting an artifact along with its saved state. Artifacts are files in the team tree, `team/workbench/<name>.html` or a folder `team/workbench/<name>/index.html` with the files it loads, found the way the backend finds them (a folder wins over a page of the same name; `<name>.*` data files aren't part of an artifact). A delete checkpoints the team first and returns the checkpoint's id, so Undo (a restore of each removed path through the hub's checkpoint routes) brings back the page or folder and its data files. `POST /api/agents/{name}/model/complete` answers from a canned model, with parsed JSON when the call asks for a schema
- The artifacts listener, on its own port, serves those files as the real listener does: `/{name}/` is the page, `/{name}/{path}` a file in a folder artifact (a trailing `/` means that folder's `index.html`), `/{name}` redirects to `/{name}/` with a `308` that keeps the query, and a path that climbs out of the folder, a missing artifact or file, and anything but `GET` and `HEAD` are refused with the backend's pages and statuses. Files go out with `Cache-Control: no-store` and `X-Content-Type-Options: nosniff`, and every HTML file gets the SDK at the same place the backend puts it (after `<head>`, else `<html>`, else the doctype, else first)
- `/api` and everything under it on the artifacts port, WebSocket upgrades included, goes to the same API and the same hub and agent sockets as the app's own origin, so a page opened there reaches the mock directly. The listener refuses the backend's block list there with `403` and `{ "error" }`: shutdown, stop-all, update check, apply and restart, and setup completion (`/api/hub/shutdown`, `/api/hub/stop-all`, `/api/hub/update/{check,apply,restart}`, `/api/hub/config/complete-setup`), whatever the method, while the app's origin still serves them. A socket opened there doesn't count as a client: it neither resets an agent's unread count nor shows in the agent's connected clients. An upgrade no socket route takes, such as an unknown agent, answers `404`, and no other path is forwarded. `api` is not an artifact name, so an `api` page or folder is neither listed nor served
- Live updates for team files, through a test control: `POST /api/mock/team-file` simulates an agent editing or deleting a team file. The body is `{ "path": "team/workbench/tip-splitter.html", "content": "<title>…" }`: `path` is in the file API's namespace (under `team/`), and `content` is the file's new text, or `null` to remove it (a folder goes with everything in it). The mock's files change, then its sockets send what the real change feed does: `workspace_changed` (one change, `created`, `modified` or `removed`; a new folder is reported alone, standing for what it holds) to the hub socket and to every agent socket whose `watch_team` or `watch_workspace` prefixes match (by whole path segments; a prefix above or at the path matches, and so does one below a removed folder), and, when the change touches an artifact's page or folder, `artifact_updated` or `artifact_removed` to every hub socket and every agent socket, whatever they watch, and with no agent running. The answer is `{ "changes": [...], "artifacts": { "updated": [...], "removed": [...] } }`, what was sent. An artifact's saved data doesn't count as the artifact, and a rewrite that leaves its files as they were sends no artifact frame. A path outside `team/`, or a folder to write to, answers `422`, and removing what isn't there `404`
- Live updates for an agent's own files, the same way: `POST /api/mock/agent-file?agent=atlas` with `{ "path": "notes/plan.md", "content": "…" }` (a path in the agent's own workspace, `content: null` to remove it) changes the file and sends `workspace_changed` to that agent's sockets whose `watch_workspace` prefixes match, and answers `{ "changes": [...] }`. A `team/` path or `""` answers `422`, an unknown agent and removing what isn't there `404`
- Main chat turns are recorded in history when they end, each message tagged with the turn's id (`turn_id`, the `reply_to` of its frames) as the backend does. A chat message starting with `drop` loses the connection mid-turn: `drop finish …` ends the turn while disconnected, `drop compress …` also compresses history into a new episode (forcing a history reload), and any other `drop …` finishes the turn live after the page reconnects. Each turn sends `turn_usage` and adds to the conversation's totals, which `GET /api/agents/{name}/usage` reports (an agent that has run starts with sample totals), and a message starting with `remember` is followed by two seconds of memory work (`post_turn_activity`). More scripted turns (`mock/chat-scenarios.ts`), by how a message starts: `segments` has the agent think and read three files, say what it will check and run two commands, say what it found and edit a file, then think and answer, each round recorded in history; `error` fails after a search with an `error` frame (a plain message and its cause) and `turn_ended`, recording only the user's message; `retry` streams part of a reply, sends `stream_restart`, and streams it again; `think` thinks at length before a short answer. `POST /api/mock/telegram-message?agent=atlas` (`{ content?, name? }`, a question about the routing doc from Alex by default) brings a message from Telegram: every page is told of it with its sender, the agent answers on Telegram (`response` with `endpoint: "telegram"`), and history records it with the sender
- Config files are loaded from `../assets/*.example.*` and can be edited in the UI. A `config.toml` or `providers.toml` patch checkpoints the file first and answers with the checkpoint, as the backend does, and an agent's `config.toml` patch that sets `agent.max_tool_iterations` to 0 is refused with the backend's message
- Secrets can be added and removed (stored in memory)
- Agent keys list with one user key and one agent-saved key, and can be added and removed (stored in memory). Any value is stored, and a short one (under 8 characters) comes back with the backend's kind of warning. Removing one returns a checkpoint id for Undo
- A2A caller keys list with one key, and can be created (the token is in the response to creating it and nowhere else) and revoked, which returns a checkpoint id for Undo

### What's NOT mocked

- No real LLM calls happen — responses are canned
- Config saves don't persist across server restarts
- Some edge cases (rate limits, network errors) aren't simulated
- The workspace routes don't block paths the backend blocks, and a move or raw write records no checkpoint (a file delete and a workbench artifact delete do). An agent's `config/` folder in its workspace is separate from the config the config routes serve: `config.toml` and `providers.toml` aren't in it, and its `mcp.json` isn't the one Settings edits. Only `POST /api/mock/team-file` and `POST /api/mock/agent-file` send `workspace_changed`, and only the first the artifact frames: a write through the workspace routes, or an Undo, changes the files without announcing it, and the mock has no batches, resyncs or lag. A raw write stores its body as text, so bytes that aren't valid UTF-8 don't round-trip
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
- **No delays.** Model calls, transcript loads, agent start and stop windows, reloads and every step of the chat and session simulations, the pieces text and reasoning stream in as included, take no time, and run in the order they were started. `MOCK_DELAY_SCALE` multiplies all of them (`1` is the natural pace, `0.2` a fifth of it), and `POST /api/mock/delays` (`{ "scale": 1 }`) changes it until the next reset. A chat message starting with `drop` relies on real time to lose and regain the connection, so run it with a scale above zero.
- **Held turn ends.** `POST /api/mock/turn-hold` (`{ "held": true }`) keeps every simulated chat turn running at its last step until `{ "held": false }` or a reset; a turn's last model call, which streams its reply, starts only then. A spec that checks a turn while it runs holds it, rather than relying on a delay to outlast a slow browser.
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

The shell frames every place, and each place is a folder under `places/`. `feed/` draws a conversation for Chat and for session transcripts. `lib/` holds the data layer (API client, stores, routing, the settings model) and the primitive controls in `lib/ui/`. A store is a rune module (`*.svelte.ts`) that holds data and commands and never imports the router; views read stores, draw, and navigate.

```
web/
├── src/
│   ├── main.ts               # App entry point
│   ├── App.svelte            # The root: the setup wizard (loaded when it is needed), or the shell; draws toasts and tooltips in both
│   ├── shell/                # The shell: the rail, the phone's bottom bar and drawer, the hub banner and the Update ready banner, place routing, the Settings modal, the command palette, the app's actions, the install offer and its Add to Home Screen steps, the shortcuts, feedback and Create agent dialogs
│   │   ├── panel/                # The context panel: its frame and header, its width, and what each kind shows
│   │   └── settings/             # The Settings modal's parts: scope picker and section list, save bar, the section API and its shared group card and field components, focus on arrival, Raw config, the History browser, the All agents sections (General, Notifications, Residuum Cloud, Saved keys with its key lists, Updates, Session limits, the install's Agent-to-agent listener with its caller keys, Diagnostics) and the agent's Model (its roles, the settings for every model and the providers), Connections, Tools & skills (with the credential field and folder list they share), Tool servers, Agent-to-agent, Memory, Schedule and Runtime
│   ├── places/               # The places, one folder each
│   │   ├── home/             # Home: needs-you, the agents board and its row menus, Recently deleted, Across the team, Coming up, and the words and times they show; the agent actions and failure fixes the Chat's state card shares
│   │   ├── files/            # Files and Shared files: the tree, the file editor the context panel shows (Raw config shares it), a file's history
│   │   ├── inbox/            # Inbox: the list, the filter and tabs, an item opened in place, and the words for sources
│   │   ├── activity/         # An agent's Activity: what it is running and has finished, and the session panel a run opens in
│   │   ├── workbench/        # The Workbench: the artifact list, a row's detail with its running sessions, Open and Copy link
│   │   ├── chat/             # An agent's Chat: its header, the feed, the post-turn status line, and under it the composer with its `/` menu and model control, or the state card while the agent isn't running; the conversation size the context panel shows
│   │   └── schedule/         # An agent's Schedule: its pulses and scheduled actions, and the words they show
│   ├── feed/                 # A conversation: the feed, its turns and their activity lines, and each kind of message in it, shared by Chat and session transcripts; path links
│   ├── Setup.svelte          # Setup wizard, built into a chunk of its own
│   ├── pairing/              # The pairing page an unpaired browser lands on through Residuum Cloud (`/pair`): a page of its own, mounted by `main.ts` in place of the app, in a chunk of its own; its state is `lib/pairing-flow.svelte.ts`, its API calls `lib/pairing-api.ts`
│   ├── styles/               # Design tokens, bundled fonts, the base and reset styles, the shell breakpoints for scripts, and the contrast pairs the tokens are tested against
│   ├── components/
│   │   └── setup/                  # Setup wizard steps
│   ├── sw/                   # The service worker, a TypeScript program of its own (the worker's globals): the worker, its caching rules and what it shows for a push as pure functions, and the messages the page and the worker share
│   ├── test/                 # Component-test helpers and harnesses
│   └── lib/
│       ├── ui/                   # Primitive controls and overlays (buttons, fields, badges, dialogs, sheets, menus, popovers, tooltips, toasts…); the overlay stack and float placement in ui/overlay/; gallery at /dev/gallery (see AESTHETIC.md)
│       ├── icons/                # The Icon component and icon set
│       ├── actions/              # Svelte actions
│       ├── api.ts                # REST API client (typed fetch wrappers); every agent-scoped call takes the agent name first
│       ├── paths.ts              # API and WebSocket URL builders for the agent, hub and team scopes
│       ├── errors.ts             # Plain-language messages for failures, with the raw error logged
│       ├── cache.ts              # The fetch cache behind the API client's GET wrappers: in-flight deduplication, kept in local storage
│       ├── transport.svelte.ts   # The WebSocket transport: connecting, reconnecting, and holding what is sent while the connection is down
│       ├── hub.svelte.ts         # The hub's state from `/api/hub/ws`: each agent's state, activity and stopping set, hub notices, deleted agents; up across agent switches
│       ├── hub-types.ts          # Shapes for the hub API and frames: the generated ones, and the bodies the backend builds inline
│       ├── viewed-agent.ts       # The bound agent: the router publishes it, the WebSocket coordinator binds to it
│       ├── ws.svelte.ts          # WebSocket coordinator: routes frames to the feed and sessions stores; open while the bound agent runs
│       ├── feed.svelte.ts        # Main chat feed state
│       ├── feed-items.ts         # History-to-feed conversion shared by chat and session transcripts
│       ├── feed-id.ts            # Feed item ids
│       ├── feed-stream.ts        # Text and reasoning streaming into the feed: drafts per model call, the authoritative message replacing them, pieces added once per frame
│       ├── announce.ts           # What a screen reader is told about the agent's work
│       ├── message-id.ts         # Ids for the messages this page sends: web-<uuid>, unique across page loads, since the agent keeps one as its turn's id
│       ├── feed-scroll.svelte.ts # Following new content while the reader is at the end of a feed, the Jump to latest pill and its new-reply mark, and keeping a pressed expand or collapse control in place
│       ├── observed-turns.svelte.ts # What the page saw of each turn while it ran: timing, how it ended, where it may have missed steps
│       ├── sessions.svelte.ts    # The bound agent's sessions for Activity: live and finished runs, outbound tasks, Stop
│       ├── session-run.svelte.ts # One run in the session panel, on any agent: transcript, the hub's relay, message and stop
│       ├── session-format.ts     # Plain words for runs and outbound tasks: kinds, states, outcomes, durations
│       ├── scheduled.svelte.ts   # The bound agent's pulses and scheduled actions, for the Schedule place
│       ├── overview.svelte.ts    # The team overview: each agent's overview, team events, the newest unread inbox items, what needs the user
│       ├── inbox.svelte.ts       # The Inbox's list across agents: filter and tab, paging, read, archive, restore
│       ├── app-badge.ts          # The app icon's badge (the Badging API): the inbox unread total
│       ├── app-update.svelte.ts  # The service worker as the page sees it: registering it, noticing a rebuilt app (Update ready), and Reload
│       ├── push.svelte.ts        # Web Push on this device: availability, permission, subscribing, its registration with the hub, and the hub's device list
│       ├── push-presence.ts      # Telling the hub over its socket while this window is in front of the user
│       ├── push-devices.ts       # How delivery to a device is going, a new device's name, the subscription as the hub takes it
│       ├── install.ts            # Installing the app: what this browser can do (secure context, installed, iOS), and keeping the Install app offer current
│       ├── lazy-component.svelte.ts # A component whose code loads the first time it is needed, with the failure shown and a retry
│       ├── needs-you.ts          # What needs the user and in what order, and the rail's Home count
│       ├── agent-failure.ts      # Plain words for why an agent couldn't start, and the Settings section and field that fix it
│       ├── agent-display-state.ts # The state an agent is shown in: the hub's, or stopping while its stop is under way
│       ├── agent-lifecycle.ts    # Which of Start, Stop and Restart apply to an agent in a state
│       ├── routes.ts             # URL <-> location: places, the panel and settings parameters, redirects from URLs the app no longer uses, corrections
│       ├── router.svelte.ts      # Current location; push/replace, closing by going back, overlay entries, the unsaved-edit guard
│       ├── history-entry.ts      # The marks the router keeps in history.state
│       ├── navigation-guard.ts   # Checks views register for unsaved work, and how the user is asked
│       ├── settings-sections.ts  # Settings section registry: ids, scopes, labels, groups, the section names of earlier settings URLs, config keys
│       ├── session-address.ts    # Opens a session, on any agent, from where it is mentioned
│       ├── relay.ts              # Recognizes agent-message headers in transcripts
│       ├── watch-registry.ts     # One watch set per socket, merged from every owner's path prefixes
│       ├── workspace-watch.ts    # How a watched path prefix is spelled and matched
│       ├── tree-changes.ts       # Which listed folders of a file tree a batch of changes affects
│       ├── checkpoints.ts        # Words and shapes for the checkpoint history
│       ├── undo.ts               # Single-click destructive actions, and the Undo on their toast from the checkpoint the server took
│       ├── turn-undo.ts          # Undo this turn
│       ├── form-undo.ts          # Undo for an entry removed from a settings form list
│       ├── diagnostics.ts        # Rendering a diagnostic: message, position, key path
│       ├── file-size.ts          # A size in bytes as people read it
│       ├── agent-name.ts         # The backend's agent-name rules, checked as a name is typed
│       ├── mcp-form.ts           # The text forms a tool server's arguments, environment and headers are edited in
│       ├── toml.ts               # The config files the setup wizard writes, generated from its state
│       ├── workbench.ts          # Where artifacts open: the relay's origin, or this host on the artifacts port over plain HTTP, else why they can't
│       ├── time.ts               # Relative times ("5m ago")
│       ├── day-label.ts          # A calendar day as dividers name it: Today, Yesterday, Oct 6
│       ├── current-day.svelte.ts # The current time, reactive to its passing, for what is named from today
│       ├── toast.svelte.ts       # Toasts: kinds, timings, actions (ui/ToastRegion draws them)
│       ├── composer-clearance.svelte.ts # How much of the screen's foot a composer covers, which the toasts sit above
│       ├── file-drop.ts          # Telling a drag of files from other drags, and the window's refusal of file drops no place took
│       ├── press-again.svelte.ts # A key that takes two presses within a window (Esc to stop a reply)
│       ├── notifications.svelte.ts # What is surfaced to the user: a toast, kept in the Recent notifications history
│       ├── reload-notices.ts     # What the page says when the agent reloads its settings: plain words, and nothing for a reload a control asked for quietly
│       ├── running-count.ts      # How much an agent has running, the one number the rail's Activity row and the Chat header's pill show
│       ├── tab-title.ts          # The browser tab's title: the agent or place, and the markers a hidden tab carries
│       ├── generated/            # Protocol types generated from Rust (cargo test --test ts_export)
│       ├── types.ts              # TypeScript types for API and messages
│       ├── action-registry.svelte.ts # The action registry: sources of named actions, matching, `/name` lines, running
│       ├── chat-actions.ts       # The chat actions (typed as `/name` in the composer) and why each can't run
│       ├── models.ts             # Model fetching and caching
│       ├── model-roles.ts        # Model roles named by their job, the providers a role can name, and how a role's value splits
│       ├── markdown.ts           # Message Markdown to sanitized nodes: code blocks with Copy, workspace paths as links, tables in a scroller, task items as words, links in new tabs, headings two levels down
│       ├── format-usage.ts       # Elapsed time, token counts and token counts as words, for the activity line, the session panel and the conversation size
│       ├── composer-drafts.ts    # What is typed to each agent and not sent: text in local storage, images for the page's life
│       ├── image-attachments.ts  # Which images a message can carry, and reading them
│       ├── format-tool-result.ts # Tool result display: JSON, file dumps, lists, errors, long-output collapse
│       ├── settings-toml.ts      # Config parsing (for display) and diffing (for the patch endpoints)
│       ├── settings-fields.ts    # The field-to-key-path map: every form field's key, and which field a diagnostic's key path names
│       ├── settings-model.svelte.ts # The settings model: per scope and file baselines, staged changes, Save, Undo, locks (no UI)
│       ├── settings-bind.ts      # Binds a number box to a settings form field, which keeps numbers as text
│       ├── cloud.svelte.ts       # The Residuum Cloud connection: status polling, Connect, Disconnect, Reconnect, token, and the sign-in page for a relay
│       ├── config-coordinator.ts # Config write coordinator: serialized writes, re-read before a save, change notifications, checkpoint restore and undo
│       ├── config-sync.ts        # Passes config changes made elsewhere (the agent's config/ watch, hub_config_reloaded) to the coordinator
│       └── secrets.ts            # secret:/${ENV_VAR} reference detection for settings fields
├── build/                    # The Vite plugin that builds the service worker into `dist/sw.js`, and the choice of which files it precaches; checked like src/
├── mock/                     # Typed mock modules, checked like src/ (only used in dev:mock and preview:mock)
│   ├── routes.ts             # Route tables: each endpoint is a method, a path pattern and a handler
│   ├── api-routes.ts         # Every route table the mock serves
│   ├── env.ts                # The clock, simulated delays and timers, the id counter; what reset restores
│   ├── scope.ts              # Scoped routing: agent, hub and team paths to state and unscoped path; what each scope owns; stopped-agent rules
│   ├── middleware.ts         # The /api request handler and its Connect middleware
│   ├── state.ts              # Per-agent and hub state, and the agent and hub types
│   ├── http.ts               # Request and response helpers, typed body parsing
│   ├── assets.ts             # Reading the example config files and the web root
│   ├── constants.ts          # Values the mock shares: its version and features, the deterministic boot id, and why `brittle` can't start
│   ├── util.ts               # Ordering helpers
│   ├── zone.ts               # The mock hub's timezone, and the conversions between instants and the local times its files keep
│   ├── agent-name.ts         # The backend's agent-name rule
│   ├── diagnostics.ts        # The problems the backend's validators find in a config file's text
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
│   ├── chat-scenarios.ts     # The scripted chat turns: rounds of work, a failure, a restarted stream, long reasoning, a turn from another channel
│   ├── scenario.ts           # The agents the mock starts with
│   ├── sessions.ts           # Sessions endpoints, session socket commands, the session lifecycle
│   ├── config.ts             # Status, config, providers, MCP, secrets, agent keys, A2A, setup, tracing
│   ├── provider-models.ts    # The models each provider type lists, and the check for a role naming one its provider doesn't offer
│   ├── workspace.ts          # Workspace file routes, for an agent and for the team tree
│   ├── workspace-tree.ts     # The workspace tree in state: listings, versions, writes, moves
│   ├── workspace-bulk.ts     # The recursive tree listing and the batch read, with their budgets
│   ├── inbox.ts              # The user inbox: listing, archive, read, restore, attachments
│   ├── agent-inbox.ts        # The agent's own inbox: what an artifact adds to it
│   ├── scheduled.ts          # The Schedule place's routes: pulses and actions
│   ├── checkpoints.ts        # Checkpoint histories: list, stats, detail, diff, file, restore, undo
│   ├── update.ts             # The update routes
│   ├── cloud.ts              # The Residuum Cloud routes, read from the hub config, and the controls that stage its states
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
│   ├── rebuilt-worker.ts     # Serves the service worker as another version after `POST /api/mock/rebuild`, which stands in for rebuilding the app
│   ├── data/                 # Sample data: chat, sessions, workspace files, inbox, the workbench artifact
│   ├── test-support.ts       # Test harnesses: route tables over HTTP, the whole mock with its sockets
│   ├── route-parity.test.ts  # Every API client request lands on a mock route
│   └── *.test.ts             # Unit tests, run by `npm test`
├── e2e/                      # Playwright specs, checked like src/ (see Testing)
│   ├── support/              # Fixtures, "the app is ready" and lazy-chunk waits, the axe scan, the screenshot helper, the dev server warm-up, server ports
│   ├── smoke/                # User flows; `@preview` specs run on the production build
│   ├── visual/               # `@visual` specs; their baselines are in __screenshots__/
│   └── harness/              # Specs for the harness itself
├── public/                   # Served as is: the manifest, the icons, the favicon, the MCP catalog
├── playwright.config.ts      # Projects, servers and reporters
├── vite.config.ts
└── package.json
```

## How the App Works

This part describes each system in the order you meet it: how a URL becomes a place (Routing), the frame around every place (The shell), installing the app and its service worker, the action registry the command palette and the composer's `/` menu share, then each place (Home, Files, the Inbox, the chat feed and the composer, Activity, the Schedule, the Workbench), and the data layer and Settings modal under them.

### Routing

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

URLs the app no longer uses redirect by replace: `/team`, `/agent/:name/sessions/:runId`, `/agent/:name/workspace`, `/agent/:name/scheduled`, `/agent/:name/settings[/:section]`, `/team/settings[/:section]`, `/workbench[/…]`, and the unprefixed `/settings`, `/scheduled`, `/sessions/:runId` and `/notification/<id>`, which resolve under the last-used agent once the agent list is known. The section names of those settings URLs map to a scope and section (`lib/settings-sections.ts`). A URL the router can't read, an agent or artifact that doesn't exist, and a panel a place can't show are corrected by replace, with a toast where the user should know.

`routes.ts` reads and formats URLs and knows nothing of the browser. `router.svelte.ts` holds the location. Stores never import it: they expose data and commands, and views navigate (ESLint enforces this under `src/lib/`).

**Navigation.** `openPlace`, `openPanel`, `openSettings` and `openSettingsSection` (a section opened from the phone's section list) push. `replacePlace`, `replacePanel`, `switchSettingsSection` and `switchSettingsScope` replace, and so does every correction and redirect. `closeSettingsSection` is a phone section's Back: it goes back to the list when this page pushed the section from it, and replaces the section away otherwise. Each returns whether the navigation happened.

**The Inbox's parameters.** Opening an item from the list pushes (`openPlace` with `item`), and opening another while one is open replaces it, the way the panel switches files. Changing the agent filter or the tab replaces and closes the open item, so Back leaves the Inbox instead of stepping through filters. A link to an item that's gone is corrected by replace, with a toast.

**The Workbench's artifact.** Selecting a row pushes (`openPlace` with `artifact`), and selecting another while one is selected replaces it, as the Inbox does with its items.

**Closing.** `closePanel`, `closeSettings` and `closeItem` (the open inbox item, or the selected artifact) go back in the history when this page pushed the entry that opened what is closing, and replace the URL with one that omits the parameter otherwise (a deep link, a reload into the modal). Back therefore closes the panel, the modal, the open inbox item or the selected artifact before it leaves a place.

**Overlays.** A modal overlay calls `router.openOverlay(onDismiss)` when it opens. That pushes an entry with the same URL, so Back closes the overlay. The overlay closes itself through the returned handle (`handle.close()`), which pops that entry. `onDismiss` runs when the entry is left any other way: Back, or a navigation that takes the overlay's entry. `ModalLayer`, under Dialog, Sheet and Drawer, does all of this; `historyEntry={false}` is for a layer whose URL parameter is already its entry, like the Settings modal. The gallery, outside the app's routes, calls `router.startForOverlays()`, which follows overlay entries and leaves the address alone.

**Unsaved work.** A view that holds work the user would lose registers a check with `router.guard.register(check)`. The check returns a line saying what navigating to the given location would lose, or null. In-app navigation asks first, through the function the app gives `router.guard.setConfirm`, and does not navigate until the user confirms; with no such function it refuses. `confirmLeave` from `lib/ui` is that function: it asks in a confirm dialog, shown by `ConfirmHost`. A confirm dialog has its own overlay entry, so the router goes on only once that entry has left the history. On Back or Forward the router puts the location back on top of the history, asks, and goes where the user was headed only if they confirm. On reload or tab close the browser's own prompt appears.

**The bound agent** is the viewed agent on an agent place, and on the other places the agent most recently viewed. The last-used agent is remembered in local storage, and `router.setKnownAgents` settles on agents that exist once the agent list is known.

### The shell

`shell/Shell.svelte` is the frame around every place: the rail (`Rail.svelte`) beside the main region at medium and wide widths, and on phones the bottom bar (`BottomBar.svelte`) with the rail in a `Drawer`. The main region starts with the hub banner (`HubBanner.svelte`, shown while the hub socket is down) and then the place, which `PlaceHost.svelte` picks from the router's location. The shell root mounts `ConfirmHost` (and gives the router's guard `confirmLeave`), the Settings modal and the command palette (each once its code has loaded, see [Installing the app and code splitting](#installing-the-app-and-code-splitting)), `RecentNotifications`, the Keyboard shortcuts dialog, the Add to Home Screen steps, the feedback dialog, the inbox-note prompt and the Create agent dialog once each; `App.svelte` draws the toast region and tooltips, in setup too. `TabTitle.svelte` draws nothing and titles the browser tab: `<agent> · Residuum` on an agent's places and the place's own name elsewhere (`lib/tab-title.ts`), and while the tab is hidden a leading unread total (the chats' unread counts and the inbox's) and "is working" or "finished" after the viewed agent's name, from the hub's per-agent activity.

The context panel (`panel/PanelHost.svelte`) is open while the URL has a `panel` its place can show. Its frame (`ContextPanel.svelte`) is a column beside the main region at wide widths, resized from its left edge by pointer or by the arrow keys, Home and End, between `--layout-panel-min-width` and half the viewport; the width the viewer chose is kept in local storage. At medium widths it floats over the main region's right edge at the default width, and on phones it is a full-screen sheet over the bottom bar, a `ModalLayer` whose history entry is the `panel` parameter. Beside or over the main region it takes focus when it opens, Esc inside it closes it, and focus goes back to where it was; on phones the sheet's layer does the same. Closing goes through `router.closePanel`, so Back closes it before it leaves the place. What the panel shows is chosen by kind in `PanelHost`, and each kind's content starts with `PanelHeader`, which names the panel and holds its actions and the way out (Close, or Back on a phone). A file shows the file editor (see [Files](#files)), a session run the session panel (see [Activity and the session panel](#activity-and-the-session-panel)), and `size` the conversation size (see [The composer](#the-composer)).

The rail's agents are an accordion (`accordion.svelte.ts`): one agent's places are open at a time, a press on the open agent closes it, a row press never navigates, and arriving on an agent opens it. `rail-model.ts` works out each agent row's mark, word and unread badge from the hub's snapshot. The Home count is the number of needs-you items and the Inbox count the unread items across every agent, both from the overview store, and an agent's Activity count what the bound agent has running (`runningCount` in `lib/running-count.ts`: its live sessions and its open tasks on remote agents, and nothing once it is down), which is also the number on the Chat header's pill and under Running now. The bottom bar's Inbox count is the same, and so is the installed app's icon badge (`lib/app-badge.ts`, where the browser has the Badging API). The rail starts with a search row that opens the command palette, as the phone bar's Search tab and ⌘K or Ctrl+K do. Its footer has a Help menu, which lists the registry's help actions (Keyboard shortcuts, Recent notifications, Send feedback, Report a bug, and Install app while the browser offers it), and the Settings gear. The search row, the gear and the rail's "+" go through `ShellActions`, which the shell answers.

`ShellActions.createAgent` is the one way to create an agent: Home's New agent, the rail's "+" and the palette's Create an agent call it, and it opens the Create agent dialog (`CreateAgentDialog.svelte`, a sheet on phones) over the current place. The name is checked against the backend's rules and the agent list as it is typed (`newAgentNameProblem` in `lib/agent-name.ts`), and a name a deleted agent had points at Recently deleted. Under More options are the agent to copy model settings from (the first by name until another is chosen) and who can find it (private by default). What was typed stays when the dialog closes without creating. Once the agent exists the dialog closes, the hub's `agent_created` frame raises the "You created …" toast, and beside the main region the new agent's rail row takes focus.

Every place has its own section (see [Home and the overview](#home-and-the-overview), [The Inbox](#the-inbox), [The chat feed](#the-chat-feed), [Activity and the session panel](#activity-and-the-session-panel), [The Schedule](#the-schedule), [Files](#files) and [The Workbench](#the-workbench)). The Settings modal is described in [The Settings modal](#the-settings-modal). The palette and the help dialogs aren't in the URL either; each holds an overlay entry, so Back closes it.

### Installing the app and code splitting

`index.html` and `public/manifest.webmanifest` make the app installable. The manifest's `id` and `start_url` are `/home`, with the base surface as both its colors, the three icons (the maskable one's content stays inside the central 80%) and shortcuts to Home and the Inbox. The `<link rel="manifest">` carries `crossorigin="use-credentials"`: a browser fetches a manifest without cookies otherwise, and Residuum Cloud's relay answers a request with no session cookie with its login page. The viewport is `viewport-fit=cover` and the iOS metas ask for a standalone app with a translucent status bar, so every edge of the shell honours the safe-area insets (see [AESTHETIC.md](./AESTHETIC.md), "Layout").

`lib/install.ts` decides whether Install app is offered, and `shell/install-offer.ts` starts it before the app mounts, since a Chromium browser fires its `beforeinstallprompt` once, early. Install app is listed (`installOffer.install` in `shell/app-actions.svelte.ts`) only in a secure context and only outside an installed app. In Chromium it runs the stored prompt, which a browser shows once, so the entry goes with it and returns if the browser fires a new prompt. On an iPhone or iPad, which has no prompt, it opens `InstallHelpDialog.svelte`, the Add to Home Screen steps. Without a secure context (plain HTTP on a LAN) nothing is offered. Everything that needs a secure context, push included, asks `installContext().secure`.

Settings (`SettingsModal.svelte` and its sections), the command palette, the file view with its editor (`places/files/FilePanel.svelte`) and the setup wizard are built into chunks of their own, and the shell, Home and Chat are in the initial bundle. `LazyComponent` (`lib/lazy-component.svelte.ts`) starts fetching a chunk the first time something asks for it, keeps the component for the life of the page, and on a failed fetch shows an error toast, runs the caller's `onFailure` (the shell puts the modal, the palette or the panel away so the next press asks again) and tries again on the next ask. A chunk is named after the module it is imported from, which is how the `@preview` specs in `e2e/smoke/installable.spec.ts` find it. Importing a lazy module from the initial bundle statically pulls its chunk back in, so import only its types, or go through the `LazyComponent`.

### The service worker and app updates

The service worker (`src/sw/worker.ts`, served as `/sw.js`) holds the app shell so the app opens with no network, and nothing else: no data is cached for offline reading. It is a TypeScript program of its own (`src/sw/tsconfig.json`, with the worker's globals instead of the window's, checked by `npm run check`), and the page and the worker share only `src/sw/protocol.ts`. Which requests it answers is decided by pure functions in `src/sw/rules.ts`, tested in Node.

**Building it.** `build/service-worker.ts` is a Vite plugin that runs once the bundle and the public folder are in `dist/`. It lists the files the shell needs (`build/precache.ts`: `index.html`, everything under `assets/`, so the lazy chunks and the fonts too, `favicon.svg` and `icons/`), derives a version from those paths and their contents, bundles the worker with the list and the version filled in, and writes `dist/sw.js`, whose first line is `/* residuum-sw <version> */`. A build whose shell differs in any byte has a different worker, which is how a browser sees an update. The manifest, the MCP catalog and the font licenses are not precached.

**What the worker does.**

- It installs by fetching the listed files into the cache `residuum-shell:<version>`, six at a time (Residuum Cloud's relay lets 50 requests through a tunnel at once). A file that is missing or redirects (the relay's sign-in page, say) fails the install, and the active worker keeps serving.
- A page load (a client route, or `/index.html`) goes to the network first, so a rebuilt app arrives with its new document. When the network fails, or the hub answers 502, 503 or 504 (the relay's answers for an instance that is offline), the cached `index.html` stands in, and the app's hub banner says it can't reach Residuum. The live document is never cached: the relay inserts its instance switcher into it.
- A file of the app (anything under `/assets/`, the icons, the favicon) comes from the cache first: this version's cache, then another version's, then the network.
- It never answers anything else, so the browser sends it as it would with no worker: any request that isn't a GET, any other origin (the artifacts origin included), and `/api`, `/ws`, `/webhook` and `/cloud/callback`. Nothing at runtime goes into a cache.
- A new worker waits. It takes over only when the page posts `skip-waiting`, so the files under an open page never change on their own. On activation the worker keeps this version's cache and the one it replaced, and deletes the rest: a page opened before an update keeps asking for the hashed files it was built with, and they stay available until the update after next.

**What the page does.** `lib/app-update.svelte.ts` registers `/sw.js` after the page loads, in builds only (`vite.config.ts` defines `__SERVICE_WORKER__` as true for a build and false for the dev server, the mock's dev mode and the tests; a preview serves a build, so it registers). A secure context is needed, which the browser enforces. It asks the server whether the app was rebuilt each time the page becomes visible and every ten minutes. When a new worker is installed and waiting, or another window of the app took over and left this page on the old files, `ready` is set and `shell/UpdateBanner.svelte` shows "Update ready" with Reload. Reload asks `router.guard.confirmReload()` first, so unsaved work is asked about (the browser's own prompt is skipped for a reload the person just confirmed), then posts `skip-waiting`, waits for the new worker to take over (five seconds at most) and reloads. A worker that fails to register or update is logged to the console once, since nothing the person does would help.

**Push.** The worker shows every push as a notification, with the payload's title, body and tag (a repeated tag replaces the earlier notification and alerts again), falling back to a generic one for a payload it can't read, and sets the app badge to `badge` where the browser has the Badging API (`src/sw/push.ts`). A click brings the focused window, else a visible one, else any, forward and posts it `open-target` with the notification's target, which the page routes to (`shell/push-client.svelte.ts`); with no window open it opens one at the target. A target that isn't a path in the app opens Home. The page reaches the worker's registration through `navigator.serviceWorker.ready` in builds, and in the dev server only through a registration that already exists, so Settings → Notifications says the worker isn't running there.

**Developing with it.** The dev server never registers a worker. A worker that a preview registered on a port stays there: unregister it under Application, Service workers in the browser's tools before running the dev server on the same port. `e2e/smoke/service-worker.spec.ts` (`@preview`) covers registration, the offline reload, `/api` staying out of every cache and the update flow, with `POST /api/mock/rebuild` standing in for rebuilding the app. `e2e/smoke/push.spec.ts` (`@preview`) covers push: `fakePushService` in `e2e/support/push.ts` hands the page a subscription the mock accepts, since no push service answers in a test browser, and `deliverPush` sends a push through Chromium's DevTools protocol. It runs in full Chromium (`test.use({ channel: "chromium" })`): the default headless shell reports notifications denied and crashes on a push.

### Actions

`lib/action-registry.svelte.ts` holds the one list of named actions that the command palette (`shell/CommandPalette.svelte`), the composer's `/` menu and the rail's help menu draw from. A source is a function that builds actions from current state; `actionRegistry.register(key, source)` adds one (replacing the source under that key) and returns a function that removes it, and `actionRegistry.all` is every source's actions in registration order. The shell registers the app's sources (`shell/app-actions.svelte.ts`): the team places, every agent, the bound agent's places and live sessions, the settings sections, the chat actions, Start, Stop and Restart where they apply, Create an agent, and help. Another agent's places, settings and lifecycle actions are `searchOnly`: the palette lists them once something is typed. A unit adds actions by registering a source of its own, or by adding them to the source they belong with.

An action has a heading (`group`), a plain-language `label`, an optional `hint` and `terms` it is also found by, and `run(text?)`. A chat action has a `command`, its slash name: `/observe` in the composer, or typing `/observe` in the palette, finds "Summarize older messages now". `takesText` marks one that acts on what follows `/name`, and asks for text otherwise. `disabled` holds the reason it can't run now ("Start atlas first"); the palette and the `/` menu show the reason in place of the hint, and running it does nothing. `matchActions` finds the actions holding every typed word; `readCommandLine` reads a `/name text` line the composer sends.

Run actions with `actionRegistry.run(action, text?)`. It tells the registry's run listeners first, then runs the action once the page has settled: the shell closes the phone drawer there, so a dialog an action opens, or a place it goes to, never sits under the drawer. `installOffer.install`, in `app-actions.svelte.ts`, lists Install app among the help actions while it is set (see [Installing the app and code splitting](#installing-the-app-and-code-splitting)).

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

Home's sections (`places/home/`) read the store and the hub store directly; `home-model.ts` holds their words and times. An agent's state everywhere it is named or marked (Home, the rail, place headers, Settings, the palette, the Chat) is `hub.displayStateOf(name)`, or `displayState` in `lib/agent-display-state.ts` with the summary in hand: the hub's state, or `stopping` while a running or starting agent is in the hub's stopping set. A failed agent's fixes are `FailedAgentActions.svelte`, shared by the needs-you item and the Chat's state card: Restart, and by `last_error.kind`, Fix settings (`config`), Open Connections (`port_conflict`) or Report a bug (`crash`, `other`), the matching fix leading. Restart and Fix settings run through `agent-actions.svelte.ts`; Fix settings finds the Settings section and field from the agent's validate endpoints (`findSettingsFix` in `lib/agent-failure.ts`), flags that file's problems on the agent's settings and asks for the field to be focused, or opens Raw config when no diagnostic names a setting a form holds.

Each board row ends in a "…" menu (`AgentMenu.svelte`), headed by the agent's state: Open chat; Start, Stop and Restart, each offered where `lifecycleApplies` (`lib/agent-lifecycle.ts`, which the palette's lifecycle actions use too) says it applies, and none while the agent stops; Start automatically; Settings; and Delete. `agent-actions.svelte.ts` runs them through the hub store, which surfaces any failure, one action per agent at a time, whether they start from Home or from the Chat's state card. Start automatically shows the value being saved until the hub answers, then the hub's. Delete asks first (`confirmations.ask`), then drops the agent's staged settings (`settingsModel.drop`, which `App.svelte` also does for a deletion made elsewhere), and the hub's `agent_deleted` frame raises the "You deleted …" toast with Undo; a deletion that took no checkpoint says so in an error. Recently deleted (`RecentlyDeleted.svelte`), collapsed under the board, lists `hub.deleted` with Restore once there is something to list, or a failed load with Try again; restoring uses the checkpoint the deletion took. An agent's A2A card visibility is set in its Settings.

### Files

Files (an agent's workspace) and Shared files (the team's folder) are one place, `places/files/FilesPlace.svelte`, given a `FileSource` (`file-source.ts`): an agent and the `agent` scope, or no agent and the `team` scope. `fileSourceFor(place)` says which tree a `panel=file:<path>` names. The tree (`file-tree.svelte.ts`, drawn by `FileTreeView.svelte`) lists folders as they open, folders first, and follows the disk through the watch registry: the agent's socket (tied to that agent) or the hub's team watch. A change that adds or removes something lists that folder again; a resync, or the socket coming back after a drop (a watch owner's `reconnected`), lists every open folder again. Each file has History, Rename (inline; a `/` in the new name moves it into a folder below) and Delete, which acts at once and offers Undo from the checkpoint the delete returned.

A file opens in the context panel. `FileBuffer` (`file-buffer.svelte.ts`) is the open file: its text on disk and in the editor, saving with `If-Match`, the save conflict (Reload, discard my edits; Overwrite with my edits; or put the question off), and `refresh`, which takes a change made elsewhere when there are no edits and otherwise keeps them and says the file changed on disk. `PanelHost` holds the buffer while the panel shows a file from one tree, so the edits survive the frame redrawing its content at another width. `FilePanel.svelte` is its view: the editor (`TextEditor.svelte`, which Raw config shares) with live validation (diagnostics with a line jump to where they point), Save (also Ctrl or Cmd+S) and Discard, a missing-file state, a watch on the file, and the file's history (`FileHistoryDialog.svelte`, which restores through the config write coordinator and shows a version with `CheckpointText.svelte`, as Settings → History does). The tree tells the buffer the panel shows (`panelFile.shown`) when that file is renamed, deleted or restored.

While the buffer has unsaved edits, `FilePanel` registers a check with `router.guard`: any navigation that doesn't show the same file from the same tree asks first. That covers changing place or agent, opening another file, closing the panel (Close, Esc, Back on a phone), Back and Forward, and reload or tab close.

An agent's `config/config.toml`, `config/providers.toml` and `config/mcp.json` save through the config write coordinator as a whole-file write, so Settings hears about them, and the coordinator's re-read before a save asks the same question as the save conflict. Their history is in the agent-config repository (`config.toml`, `providers.toml`) or the workspace repository (`mcp.json`). The tree offers no Rename or Delete for them: the coordinator neither moves nor deletes, and a delete's checkpoint is the workspace's, which leaves `config.toml` and `providers.toml` out, so Undo couldn't bring them back.

### The Inbox

`places/inbox/` shows every agent's user inbox in one list, from `lib/inbox.svelte.ts` (`inbox`), which reads the hub's inbox routes through the API client. The place's URL is the list it shows: `inbox.show({ agent, tab })` fetches it each time the Inbox opens or the filter or tab changes, keeping what is on screen while the same list is fetched again, and `loadMore` pages older items in. Counts aren't kept here: the rail, the bottom bar, the tab and the filter read them from the overview store, and the place hands `overview.unreadCounts()` to `inbox.followCounts`, which fetches the list again whenever a count changes, so a new item shows up without polling.

The item in the URL (`?item=<agent>:<id>`) is open in place. `inbox.open(ref)` marks it read when it is unread; when the loaded pages don't hold it (a link to an older or archived item), it is fetched on its own through the read route and shown above the list (`openedApart`), and an item that's gone (`missing`) is corrected away. `markRead`, `archive` and `restore` each answer whether they worked, keep one action per item at a time (`pending`), and leave a plain-language reason in `problems` when they don't, which the item shows with Try again. A failed list or older page lands in `loadError` or `moreError`, and a link that couldn't be fetched in `openError`, each with Try again. Archiving offers Undo on its toast.

### The chat feed

An agent's Chat (`places/chat/ChatPlace.svelte`) is its header (`ChatHeader.svelte`: the agent and its role, a pill with the same running count as the rail's Activity row, shown only while the agent is up, that opens Activity, its settings, and a menu with Show conversation size, Restart and Stop, taken from the action registry with their reasons when they can't run), the conversation, and the composer floating over its foot (see [The composer](#the-composer)). Esc stops the reply in flight (`ws.stop`) on a second press (see [The composer](#the-composer)). While the agent runs, the feed's `tail` is the post-turn status line (`PostTurnStatus.svelte`), a status region under the last reply that says "Noting what matters from this conversation" while `store.memoryWorking` and "Reviewing the last reply" while `store.subconsciousWorking`.

While the agent isn't running (its shown state is `failed`, `stopped`, `starting` or `stopping`), the composer is gone and the conversation ends in its state card (`StateCard.svelte`, a region named by its title), alone in the middle when there is no conversation:

| State | Card |
|---|---|
| Failed | "<agent> couldn't start", the plain-language line for `last_error.kind` (`failureLine`), the fixes (`FailedAgentActions`, see [Home and the overview](#home-and-the-overview)), and `last_error.reason` behind Details. A restart from the card that ends failed again says so on the card |
| Stopped | "<agent> is stopped", Start <agent>, and the Start automatically switch |
| Starting, Stopping | A progress line, nothing to press |

The card's title is a status region, so a change of state is announced. Start and Restart move focus to the title, which stays through the state changes, and once the agent runs the composer that replaces the card takes focus. The past conversation stays readable: the coordinator (`lib/ws.svelte.ts`) loads history through the file-only routes whatever the agent's state, and opens the agent's socket only while the hub lists the agent running (or hasn't listed the agents yet). It closes the socket and ends any turn left in flight when the agent stops, so nothing reconnects to an agent that isn't running, and when the agent starts it connects and catches the chat up on what the start added.

The conversation is `feed/Feed.svelte`, made for any agent's conversation, the main chat or a session's transcript:

| Prop | Is |
|---|---|
| `agent` | The agent the conversation belongs to. Its links, Undo this turn and Open session act on this agent, never on the bound one by assumption |
| `items` | The `FeedItem`s to show |
| `label` | The scrolling region's name, such as "Conversation with atlas" |
| `history` | Optional `FeedHistory` (`feed/feed-history.ts`): older parts to load as the reader nears the top, and a `generation` bumped when the whole feed is replaced |
| `loading` | The items haven't arrived, so the empty state waits |
| `live` | Something at the tail is growing, such as a turn in progress |
| `liveTurnId` | The correlation id of the turn in flight, whose block is live |
| `observed` | What the page saw of each turn while it ran (`ObservedTurnLookup`), for the activity line's timing, failures and gap notes |
| `onStop` | Stops the turn in flight, from the head of its block; without it the head offers no Stop |
| `announcement` | What a screen reader is told as the agent works (`{ id, text }`, a new one each time): the feed draws it in a visually hidden polite status region outside the scrolling region. The chat's store (`store.announcement`) sets it when a turn starts ("atlas is working"), when its reply is complete ("atlas replied: " and the start of it as plain words, `lib/announce.ts`) and when it couldn't finish ("atlas couldn't finish"); whole messages only, never text as it streams in |
| `empty`, `tail` | Snippets: the one empty state, and live content after the items |
| `dock` | Optional snippet floating over the foot of the scrolling area, such as the chat's composer; the feed measures it and keeps the last lines clear of it |

**Turns.** The feed groups each turn's output into one block (`groupTurns` in `feed/turns.ts`, drawn by `feed/FeedTurn.svelte`), in the order it happened: the agent's texts, attachments and final reply, with each run of tool calls between them as one activity segment (an `ActivitySegment`; the `parts` of a `FeedTurn` are segments and messages). What the agent said stays beside the work it said it about, so a text it sent before running tools comes before them. A `FeedItem` carries the `turnId` of its turn: from history where messages carry `turn_id`, and from the turn in flight for live items (`turn_started` tags the user message that began it). A change of `turnId` ends a turn; where items carry none (episodes, older records), the next user message or agent message does. A user or agent message flagged `midTurn` reached the agent while its turn ran and stays inside that block; any other user or agent message starts a turn, whatever its id, because older history repeats turn ids (every page load once counted its message ids from `web-1` again). The flag is set where the item is built: the feed store sets it on a message that arrives while a turn is in flight, and history conversion (`convertHistory`) on a user message that follows a tool result of its own turn, which is where the agent takes a waiting message in. A divider or note ends a block. Each block is a `FeedTurn` entry with the turn's `turnId`, its `parts`, and `live` when it is `liveTurnId`'s (only the turn's latest block, and an empty one from `turn_started` on, before any output); everything else is a single entry. A turn's first block is keyed by its id, so it stays one block from start to end. Within a block, a message that follows another stands apart by more than the paragraphs inside one do.

**Activity lines** (`feed/ActivityLine.svelte`, one per segment) have three levels. Collapsed, a line is one summary of its steps (`summarizeSegment` in `feed/activity.ts`): friendly labels joined in order, repeats merged and counted ("Searched memory, read 2 files"), and for steps the page watched, how long they took and how many failed. A time under a second is left out. Opened, it lists the steps (`feed/ActivityStep.svelte`), each with its label and target: a workspace path links into the context panel, a session a spawn started opens that session, and while the turn is watched each shows whether it is running, failed or stopped. A step opens to its details (`feed/StepDetail.svelte`): its arguments, summarized per tool (`feed/step-args.ts`), and its result through `lib/format-tool-result.ts`. The labels come from one table in `activity.ts` keyed by tool name, covering every built-in tool; any other tool reads "Used <tool>", and a tool known to come from a tool server "Used <server>: <tool>". A step's start and end are stamped as the page sees its call and result, which is where a segment's time comes from; history records none. While the turn runs, the segment that is its newest content shows its steps open as they arrive, and a segment the agent has said something after collapses to its summary; focus on a step of the segment that collapses moves to its summary. **The turn's head** (`feed/TurnHead.svelte`) sits at the bottom of the running turn, below the newest content: a working mark, the time since the turn started, and Stop. When the turn ends the head goes, and a line in its place (`turnEndingLine`) says how it ended and how long it took: "Stopped by you" or "Didn't finish" with the time, or "Worked for 42s" for a turn that ended on its own after making tool calls and took a second or more. Failures belong to the segment that holds the failed step, as "1 step failed" on its summary. A turn with no tool calls that ended on its own has no segment and no ending line, and a turn from history has no ending line either. Focus on Stop when the turn ends moves to the newest summary.

**A turn that couldn't finish** leaves its account in the feed. An `error` frame that names a turn (`reply_to`) still raises its toast, and the store also adds a `turn-failure` item to that turn (`feed/TurnFailure.svelte`): "<agent> couldn't finish this reply", the plain-language message, Details for the technical cause, and Try again, which sends the user's own message that began the turn once more through `ws.sendChat` (offered only where the page holds that message, and on the bound agent's chat). The failure is the turn's last item, so the turn ends on it and says no "Worked for". An `error` with no `reply_to` is the toast alone. History has no field for a failure, so a reload shows what history holds and not the account.

**Text and reasoning as they stream.** The main chat's socket carries every main-agent turn, whatever channel started it. A model call's text arrives as `text_delta` pieces and its reasoning as `thinking_delta` pieces, ahead of the complete `broadcast_response` or `response` and `thinking` of the same call (`reply_to` and `call` name it). `LiveStreams` (`lib/feed-stream.ts`) keeps a draft item for each (turn, call): the first piece makes it, later pieces are held and added once per animation frame so the Markdown re-renders no more often than the screen draws, and the complete message replaces the draft's text in the same item, so nothing remounts. A streaming reply (`streaming: true`) is drawn by `Prose` with its `streaming` prop, which ends the text in a blinking caret in `vein-bright` (the `blink` tokens; no caret follows a closing code block, table or rule), and screen readers are told of the complete reply only, never the pieces (`announcement`, above). A `stream_restart` drops that call's drafts and puts "Retrying…" beside the working mark until something of the next attempt arrives (`ObservedTurn.retrying`). When a turn ends with a draft the complete message never replaced, the draft keeps what arrived and says so: "Stopped here" after the user's stop, "Cut off here" when the agent stopped or the turn failed. Reasoning is a step of the activity segment of the call that did it: while it streams it is muted and clipped to its last three lines under "Thinking", then it folds to "Thought for 6s" ("Thought" when no time is known or it took under a second), which opens to all of it; a segment of reasoning alone opens straight to the text, and its summary reads "Thought for 6s", or "Read 3 files, thought 6s" beside tool calls. Each model call's tool calls make a tool group of their own (`call` on the item), so reasoning that arrives late goes back before what its call wrote. History conversion makes reasoning steps from a message's `thinking` texts, ahead of its text and tool calls, with no timing.

**Turns from other places.** A `user_message` frame tells the page of every person's message entering the main conversation, before its `turn_started`: the page's own, by id, is already shown and is skipped (`MessageIds`), and any other shows as the message that starts the turn (or, when its `turn_id` is the turn in flight, joins it as a `midTurn` message), with the sender line a message from a chat interface has (`Alex · telegram · direct message`). Another tab's message therefore shows live, once in the tab that sent it. A `response` that was delivered to a chat interface (`endpoint` not `ws`, `background` or empty) says "Sent to Telegram" under the reply (`endpointName` in `feed/feed-words.ts`). A `response` with an empty `reply_to`, the agent's `send_message` to this page, is a standalone message of the conversation: it belongs to no turn, ends none, and is announced like a reply.

What the page saw of each turn is `ObservedTurns` (`lib/observed-turns.svelte.ts`), which the feed store (`store.observed`) and a session run keep: when the turn started and ended, how it ended (`finished`, `stopped` when the user asked, `interrupted` when the agent stopped under it), whether a stop was asked for, and its gaps. A frame of a turn the page didn't see start (it connected while the turn ran) starts that turn, joined partway, with a gap at 0 ("Earlier steps happened before this page connected"), under the turn's id, which every turn frame carries; the main chat times it from the hub's `busy_since`. A reconnect during a turn adds a gap where the page had got to. A gap before the first step is noted at the head of the turn, in an empty segment of its own when the turn starts with a text ("Worked before this page connected"), and one after N steps follows the Nth. History records neither timing nor failures, so loading history drops these records, and the next load shows the turn without them. The coordinator sends `set_verbose {enabled: true}` first on every agent connection, since the activity lines are built from the tool frames.

**The scrolling area and the dock.** The scrolling area (`.feed-scroll`) runs the whole height of the feed, so its scrollbar reaches the bottom of the page, and a `dock` floats over its foot: the chat's composer, on `stone-0` with the lines scrolling under it fading out above it. The dock sits beside the scrollbar, not over it. `Feed` measures the dock (a ResizeObserver, and the scrollbar's width) and keeps the column's bottom padding at the dock's height plus a buffer under the newest line, as CSS variables on `.feed` (`--feed-reserved`, `--feed-dock-height`, `--feed-scrollbar`). Above 760px the buffer is `--layout-feed-buffer`, a quarter of the view's height between 96px and 280px, so the end of the newest reply sits near eye level while the reader follows it; on a phone it is `--space-48`. A feed with no dock (a session's transcript, whose composer is under it) has neither. The room kept for the dock follows it up at once and down only once the reader has scrolled far enough from the bottom, or new content has landed, for that to go unseen, so a composer that shrinks (a sent message empties it) pulls nothing after it.

**Following.** The feed follows new content while the reader is within 120px (`FOLLOW_THRESHOLD_PX` in `lib/feed-scroll.svelte.ts`) of the end of the content, measured from the last line: the buffer and the dock's room are never counted against the reader, and a dock covering the foot counts as the edge of the view. Following pins to the true bottom, buffer included. The composer growing or shrinking leaves the thread where it is: only when it grows past the buffer so that it would cover the last line does the feed scroll, by just that much. A reader who has scrolled away isn't moved at all.

**Jump to latest.** Once the reader is away from the end a pill shows at the bottom centre of the feed, just above the dock, with a down arrow. When a new item lands at the tail below a reader who is away (not history prepended, nor a reload) it reads "New reply" with a vein dot, in a status region so it is announced, and goes with the pill when they are back at the end. Their own message always brings them down. Under `prefers-reduced-motion` the jump is instant rather than a smooth glide.

**Opening and closing.** Pressing any control with `aria-expanded` inside the feed (an activity summary or step, a card's Show all) keeps that control where it was on the screen: the feed stops following, and for half a second re-places the control every frame, whether it is moved by what it opened or redrawn as a new element (found again by its `aria-controls`, else its place among its item's controls, else its item). The content's height is held at what it was, so closing something at the very end can't pull the thread down after it; scrolling by hand lets it go. The reader follows again once they are back at the end.

Older parts load one at a time near the top, keeping the item the reader was at in place, until the view is filled or there are none; a reload under a reader who scrolled up finds the item they were at by its kind and text and puts it back, once the part holding it has loaded.

**History.** While the main chat's history loads, the feed shows a skeleton in the shape of a message and a reply (`Skeleton`, "Loading the conversation"). When it fails (`ws.historyError`, the reason in plain words, set by `ws.loadMainHistory` and cleared by a load that starts), the feed shows "Couldn't load the conversation" with the reason and a Retry button that runs `ws.loadMainHistory` again, instead of a toast.

`feed/FeedItemView.svelte` draws one item, and takes the same `agent`. A user message is a moss bubble with its sender line, images and Undo this turn; an agent's reply is unboxed prose; a message from a session or teammate is a card with its kind and sender (`feed/feed-words.ts`), a clamped body with Show all, and Open session for session senders; then day and episode dividers (named from today, "Today", "Yesterday", "Oct 6", and renamed as days pass; an episode divider keeps its id beside the date in the code face), the compressed-history note naming the agent, file attachments, and a session transcript's status lines. Message text goes through `feed/Prose.svelte`: sanitized Markdown (`renderMarkdown` in `lib/markdown.ts`) with its headings two levels below the page's own (a reply's `#` is an `h3`), each table in a labelled group the keyboard can focus and scroll, task items as "Done:" or "To do:" with a glyph in place of the bullet, each code block focusable and labelled with a Copy button (opaque, named "Copy code", and announced as "Copied" through a polite status), and links in a new tab (`rel="noopener noreferrer"`). Raw HTML in a reply keeps its text but loses every inline `style`, and `style`, form, frame and plug-in elements (`FORBIDDEN_TAGS` and `FORBIDDEN_ATTRS` in `lib/markdown.ts`), so a reply can't restyle or cover the app or show a form; the Copy button is added after the sanitizer for that reason. Inline code that is a whole workspace path (`isWorkspacePath`) is a link that opens the file in the context panel beside that agent's places, or beside its chat from anywhere else (`feed/feed-links.ts`). Open session finds the run on its agent (`lib/session-address.ts`).

### The composer

The composer is the feed's `dock` (see [The chat feed](#the-chat-feed)): it floats over the foot of the conversation, and its wrapper reports its top edge to `composerClearance` (`lib/composer-clearance.svelte.ts`, from `{@attach composerClearance.track}`, which the session panel's message box uses too), which is how toasts know where to sit.

`places/chat/Composer.svelte` takes the `agent`, whether a reply is under way (`replying`), whether the agent's connection is down and coming back (`reconnecting`, the transport's `lost`), how many of the user's messages wait for it (`queued`, `ws.queuedMessages`), and `onsend(text, images?)` and `onstop`. A line is a command only when its first word is `/` and the name of a chat action (`readCommandLine`): ChatPlace hands such a `/name text` line to the action registry and anything else, a pasted path like `/home/bear/app.log has the error` included, to `ws.sendChat`. `onsend` answers whether the line went; an action that can't run now says why in a toast and returns false, and the composer keeps the text and images in the box.

- The message box grows with its text up to 200px; Enter sends and Shift+Enter adds a line. On a touch screen (`(pointer: coarse)`, read live) Enter adds a line too, the box asks the soft keyboard for an Enter key (`enterkeyhint="enter"`), and only the Send button sends.
- Send and Stop are one button element. With nothing typed while a reply runs it is Stop reply, and typing brings Send back for a message that steers the reply. A press on it doesn't take focus from the message box (so a phone's keyboard stays up), and a keyboard press of Send or Stop puts focus back in the box.
- Esc takes two presses to stop a reply, since it also closes dialogs and menus. With a reply running, the first Esc in the composer shows "Press Esc again to stop" in the bar beside Send (a status region, so it is announced politely) and waits two seconds (`PressAgain`, `lib/press-again.svelte.ts`); a second Esc inside the window calls `onstop`. Any other key, focus leaving the composer, the reply ending or the window running out takes the hint back. An open `/` menu keeps the Esc to close itself, an open overlay (`overlayOpen` from `lib/ui`) does too, and neither arms the stop. A held key counts as one press. The Stop button and the activity line's Stop stop at once.
- The composer's boundary is a text field's: `control-border` at rest, and while focus is inside it `vein` at double weight with a `vein-faint` halo, the ring `Input` draws, so both meet 3:1.
- The text is a draft per agent in local storage (`lib/composer-drafts.ts`, `residuum-draft:<agent>`), read when the composer opens and cleared on send. Attached images are kept per agent for the life of the page only: a few base64 images would fill the storage a browser allows.
- Images attach by the paperclip, by paste and by drop. A drop works anywhere on the chat place (the header and the conversation too): while files are over it a "Drop images to attach" overlay covers the whole place, and the drop goes to the composer's exported `attach`. The place takes none while the agent has no composer. Everywhere else the app refuses a file drop (`refuseStrayFileDrops` in `lib/file-drop.ts`, on the window, skipped for a drop a place already took), so the browser can't open the file and leave the app. `lib/image-attachments.ts` takes JPEG, PNG, GIF and WebP up to 5 MB each and names any file it can't attach in a line that stays until the next attach or send. Each thumbnail has its own Remove; at phone width it is a 24px badge on the corner whose touch area reaches `--layout-touch-target`, so it doesn't cover the picture.
- `/` as the first character, or the `/` button, opens the chat actions from the registry (`SlashMenu.svelte`). The message box is itself the combobox (ARIA 1.2: `role="combobox"` on the textarea, `aria-expanded`, `aria-controls` only while the list exists); it keeps focus and points at the active option with `aria-activedescendant`. The arrow keys move, Tab fills in the command, Enter runs it, and Esc closes the list, claiming the key so it doesn't stop the reply. A disabled action shows its reason and does nothing.
- While the connection is down the composer says "Reconnecting — N messages will send once back online", and what is sent waits in the transport.

`ModelControl.svelte` is the model and thinking control: a `Popover` beside the composer, or a `Sheet` on phones. Its state is a `MainModel` (`main-model.svelte.ts`), which reads the agent's `providers.toml` through the config coordinator, follows every change made elsewhere, and lists the main provider's models. Choosing a model writes `models.main` with `configCoordinator.edit`, built from the file as it is with `modelRoleJson`, so the role's failover list and thinking stay; the thinking levels are toggle buttons, and pressing the one that is set clears it. After a write the agent is sent `reload`, asked for quietly (`quietReloads.expect()` in `lib/reload-notices.ts`): the popover says a change applies from the next reply, so the page shows neither "Reloading settings…" nor the "Settings reloaded." that follows, in the window after the request (the settings watcher's own pass included). A reload nobody asked to keep quiet, such as the palette's Reload settings, shows both in plain words with the agent's own report behind the detail in Recent notifications, and a reload that fails is always an error toast.

`ConversationSize.svelte` is what `panel=size` shows: the bound agent's totals (`store.sessionUsage`) in words, about three words to every four tokens (`formatApproxWords`). What goes to the model each reply is the size of the conversation; the total sent over its life is labeled "Sent to the model in total, counting repeats", since every reply sends the conversation again and the total counts the same words many times. The token counts are under Token counts, the reply in progress so far, and Summarize older messages now from the registry. The coordinator reads the totals when it binds an agent, whatever its state, and again on every connect, so a stopped agent shows the figures it last recorded; a read that fails is shown with Try again.

### Activity and the session panel

`places/activity/Activity.svelte` is what the bound agent is doing and has done, from `ws.sessions` (`lib/sessions.svelte.ts`), which the socket coordinator keeps current with the agent's `session_*` frames and loads again on every connect. Running now lists the live runs, each with its kind in plain words (`runKind` in `lib/session-format.ts`), its source, how it is doing and for how long, its last error, and Stop, and the tasks the agent sent to other agents, whose Stop task and Stop watching go through the overview store's `stopTask` and `stopWatching`, the commands Home uses, so a task its agent can't reach says why on both. Finished is one list of runs, 25 at a time with Show older, for every kind or the kind the filter picks; each kind pages on its own (`sessions.finished[kind]`, `showFinished`). A run opens in the context panel. A stopped or failed agent's Activity offers Start, since the session routes answer only while it runs.

`panel=session:<agent>:<runId>` shows `places/activity/SessionPanel.svelte`, for a run on any agent. Its state is a `SessionRun` (`lib/session-run.svelte.ts`), which `PanelHost` creates for the run the URL names and keeps while the frame draws its content again at another width, so the transcript, the subscription and the message box's draft stay. The transcript loads over HTTP, and live frames come through the hub socket's session relay whether or not the agent is the bound one: the run subscribes to its session's address (`subscribe_session`), again on every new hub connection (`hub_boot`), and reads the transcript again once a subscription is acknowledged and after `session_relay_lagged`, since frames may have been lost in between. Frames that arrive while the transcript loads are replayed after it, without repeating what it holds. Message and Stop go to the agent's session routes; a message to a finished run starts a new run, and the run follows it into that run, which `PanelHost` carries into the URL by replace. The panel shows the run's kind, title and state, Details (who started it, depth, what it spent, the episode it is remembered as, its address and run id), notes for an interrupted, failed or overlapping run, the transcript with the feed's components (a turn the panel watches has a live activity line, tagged by `session_turn_started`'s `turn_id`; between turns a working line shows), and the message box, disabled with "Start <agent> first" while the agent isn't running.

### The Schedule

`places/schedule/Schedule.svelte` shows the bound agent's pulses and scheduled actions from `lib/scheduled.svelte.ts` (`scheduled`), and `schedule-model.ts` holds its words, taking run names and next-run times from Home's `home-model.ts` so both read the same. The store is reset by the socket coordinator on an agent switch, so it can't import `ws`: the place passes it the socket's frames and watch registry with `startWatching({ onFrame, watches })`. While a place is open the store owns a watch on `HEARTBEAT.yml` and `scheduled_actions.json`, tied to the bound agent, and refetches when either changes, on a resync, and on a scheduled run's session frames. A failed load lands in `loadError`, which the place shows with Try again. The schedule routes answer only while the agent runs, so the place loads when the agent is running and otherwise shows that it is stopped, with Start.

### The Workbench

`places/workbench/Workbench.svelte` is a launcher: an artifact opens in a tab of its own on the artifacts origin, and nothing in the app embeds one. Its list is a `WorkbenchList` (`workbench-list.svelte.ts`), which reads the artifacts and where they open (`resolveArtifactsOrigin` in `lib/workbench.ts`: the relay's artifacts origin when the page is viewed through the relay, else this host on the artifacts port over plain HTTP, else the reason artifacts can't open), and follows the hub socket through `hub.onFrame`: `artifact_updated` marks the artifact "updating now" for a moment and reads the list again, `artifact_removed` reads it again, and `hub_boot` reads it again after a reconnect. Each fresh read settles the URL through `router.resolveArtifacts`, so an artifact that isn't there goes back to the list with a toast. While artifacts can't open, a banner says why and the list is read again every 10 seconds. A row (`ArtifactRow.svelte`) has Open, a link with `target="_blank"`, and a menu with Copy link and Delete, which acts at once and offers Undo from the team checkpoint. An artifact's running sessions come from the overview store's live sessions whose source label is `artifact:<name>` (`artifactRuns` in `workbench-model.ts`), on any agent; the selected row lists them with Stop, and each opens in the context panel.

### Agents in API calls

No request reads the viewed agent. Every agent-scoped function in `lib/api.ts` takes the agent name as its first argument, and its cache key includes that agent. Components take the agent from their props (`Settings`, the Files places' `FileSource`), stores hold the agent they were bound to (`ws.sessions`, `scheduled`), and the chat's controls use `ws.agent`, the agent the WebSocket is bound to.

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

`dirty`, `saving`, `lastResult` and `undoable` drive the save bar, and `unsaved` (staged changes or a raw draft) the reload guard and the scope picker's "(unsaved)". `file(name)` gives a file's `lockedBy` (`"form"` keeps its raw editor read-only, `"raw"` keeps its form read-only while the raw editor holds a draft set with `setRawDraft`), `rawDraftBase` (the text the draft started from, which a raw save checks the file against), `changedOnDisk`, `unreadable` and `loadError`. A change to a file that has staged changes leaves them alone, so the coordinator's re-read before Save finds the clash. A typed credential is stored under its name (`discord`, `webhook_<name>`, a provider's name) and the reference goes in the file. Immediate actions have no state here.

`lib/settings-fields.ts` is the one map from a form field to its key (`keyPathOf`), which diffing and diagnostic placement (`locateField`) share, so a field that saves to a key is the field that shows that key's error. A form field the map doesn't cover fails `settings-fields.test.ts`. Failover lists ride along with a role (`models.fallbacks`) so a save never shortens one.

The model's tests are `settings-model.component.test.ts`, so they run with live runes in jsdom.

### The Settings modal

`shell/SettingsModal.svelte` is open while the URL has a `settings` parameter. Its side (`settings/SettingsNav.svelte`) is the scope picker, "All agents" first and then each agent with its state, the line saying what the scope affects, and the scope's sections with the Advanced group under a heading; on a phone it is the screen a URL without a section shows, and a section opens over it with Back. The modal is never remounted while the scope or section changes: the content pane swaps, with a `--duration-swap` fade, once the new scope has loaded, and keeps the last section on screen until then. On a phone it stops above the bottom bar, which stays pressable. The frame's top bar has Reload from disk (which asks first when the scope has staged changes) and Close.

The save bar (`settings/SaveBar.svelte`) shows while the scope has staged changes. Save changes writes them; a save that wrote everything says so in a toast with Undo, and one that left files unsaved names them on the bar until those changes are saved or discarded. When a file changed on disk under the keys being saved, `ChangedOnDiskDialog` asks "Keep my changes" or "Use what's on disk"; closing it leaves that file unsaved with its changes staged. While the modal is open the palette lists Save changes, Discard changes and Reload settings from disk for its scope. Reloading the page with staged changes in any scope asks first.

**Writing a section.** A section is a component taking `{ scope, section }` (`AgentSectionProps` or `AllSectionProps` in `settings/sections.ts`), listed in `AGENT_SECTION_VIEWS` or `ALL_SECTION_VIEWS`, which name every section of their scope.

- Start with `SettingsSection` (title, a line on what it holds, and the save's problems no field shows), then the fields.
- Bind fields to the scope's forms (`scope.config.timeout_secs`, `scope.providers`, …); the frame loads the scope before the section renders, and the save bar follows the staged changes by itself.
- Put fields in `SettingsGroup` cards: an optional title with a state mark (`status`), a line on what the group is, and an optional `foot` action under the fields.
- A `config.toml` number or switch is `ConfigNumber` or `ConfigToggle` with the scope and the `ConfigFields` key (`settings/config-keys.ts` types it). The forms keep numbers as text, so `ConfigNumber` binds through `numberOfText` and `textOfNumber` from `lib/settings-bind.ts`, and a blank box is an unset key. `fallback` is the value the agent uses when the box is blank: it shows in the box and at the end of the hint with the `unit`; a section that words the default itself passes `placeholder` instead. Both components place the field's problems from the save by themselves. A control that isn't one of these (a text box, a select) takes `error={configFieldError(scope, "timeout_secs")}` itself (`fieldError` takes any `FieldRef`, for a webhook or a server).
- A setting that only applies while another is on is `disabled`, not hidden, so a problem on it is never out of sight; a closed `Disclosure` that holds fields opens when one of them has a problem (see `Memory.svelte`).
- Flag a value that is unreasonable, such as a limit of zero that blocks work, with a note beside the field. Never block the save.
- A credential field is `SecretConfigField`. It takes the form's value and the value on disk (`scope.configFile.baseline`), shows where a saved reference lives with Change or Replace, and leaves a typed value for the save to store as a secret. A list of folders is `PathList`, with a box to add one and staged removal.
- A part that needs the agent running goes inside `RunningOnly` (`<RunningOnly agent={scope.agent} subject="its status">…</RunningOnly>`), which says "Start atlas to see its status." with Start until it runs.
- Actions of the section's own register with `actionRegistry.register(key, source)` inside `untrack` in an `$effect` that returns the remover.
- Immediate actions (secrets, keys, Cloud, updates, an agent's visibility, its `a2a.json`) call their endpoints and report their own result; they have no part in the save bar. One that writes a file the scope holds, as visibility writes `config.toml` through the hub, calls `scope.load()` afterwards so the scope's other views read it again.
- While Raw config holds unsaved edits to a file the section edits (`sectionFiles` in `lib/settings-sections.ts`), `SettingsSection` says why and disables everything inside it.
- A field another place can send the user to carries `data-field={fieldMark(ref)}` (`settings/field-focus.ts`) on it or an element around it. The scope's `focusRequest` names the field to show on arrival: `scope.requestFocus(ref)` asks for it, and the section that holds it calls `scope.takeFocus()` once it shows, opens whatever hides the field, and calls `focusField(root, ref)`, which focuses the field's invalid control or its first one and marks it `data-arrived` until focus leaves. `scope.flagProblems(file, diagnostics)` shows problems found outside a save, such as by Fix settings' check, on their fields as a save's would show.

**Model** (`settings/ModelSection.svelte`) edits the agent's roles in `providers.toml` and the thinking and temperature for every model in `config.toml`. `ModelChoice` is one role's provider and model: a provider is an entry in the providers list or a type named directly (`providerOptions` in `lib/model-roles.ts`), the model list is the provider's own (`lib/models.ts`), loaded when the role shows, with common models and a note when it can't be read, and "Other model" takes any id. Changing a role's model or provider keeps its failover list, which the role lists. The other roles sit under "Use different models for specific jobs", named by their job (`JOB_ROLES`), and `ProvidersGroup` adds, edits and removes providers, all staged. Fix settings for a model, and Memory's "Choose the model that reviews replies", land here on the role through `focusRequest`.

**Raw config** (`settings/RawConfig.svelte`) edits each of the scope's files as text in the Files editor (`places/files/TextEditor.svelte`) as a field: line numbers, the lines with problems marked, and the problems listed under it, each with a position moving the cursor there. The text is checked through the file's validate route half a second after typing stops. Save writes the whole file through the coordinator, even with problems, checking the file against the text the draft started from.

**History** (`settings/HistorySection.svelte`) is `HistoryBrowser.svelte` for the scope: `<HistoryBrowser agent={name} />` shows an agent's workspace and config repositories, and `agent={null}` the team's and the hub's. A row opens in place (`CheckpointChanges.svelte`) to its paths, their diff and content, Restore, and Undo these changes, through `configCoordinator.restore` and `undo`.

**The All agents sections** (`GeneralSection`, `CloudSection`, `UpdatesSection`, `LimitsSection`, `DiagnosticsSection`) edit the hub's `config.toml`. General, Session limits and Diagnostics are plain forms over `scope.config`: the timezone, the gateway address and port under More options, the three session limits, and the log detail and the two trace switches, all staged and saved with Save changes. The trace switches are staged, and saved to `[tracing]`, because the hub's tracing endpoints change only the running daemon and a restart forgets them. Updates acts at once: it reads the update status, checks, installs, and after an install polls the status until it answers with a different version or a rollback notice. Residuum Cloud acts at once through `CloudConnection` in `lib/cloud.svelte.ts`, which reads the tunnel's status (on open, on window focus, on a hub config reload, and every few seconds while connecting or while it expects a change), and Reconnect and a pasted token write `[cloud]` through the config coordinator. Connect opens the relay's sign-in page, found from `[cloud] relay_url` (`connectTarget`) so a relay on this machine is signed in to there. Only the relay URL, the local port and Remove account are staged.

Saved keys (`KeysSection`: `AgentKeysGroup` and `SecretsGroup`) and the caller keys of the install's Agent-to-agent section (`ListenerSection`: the listener's switch, port and address are staged in `[a2a]`, and `CallerKeysGroup` acts at once) are lists the hub keeps, each shown with `KeyList` and added with `KeyForm`. Adding or removing an entry calls the hub's own endpoint and reports its own result, so none of it goes through the save bar or the settings model. Removing an agent key and revoking a caller key are single clicks that offer Undo from the checkpoint the hub returns (`notifyWithUndo`, a restore of the key store through the config coordinator). Removing a secret asks first, because the hub returns no checkpoint for it. No list ever shows a value: the agent-key and secret forms keep a typed value only until it is sent, and a new caller key's token shows in the group, with Copy, until the user presses Done. Notifications (`NotificationsSection`) is push: This device (`ThisDeviceGroup`) turns it on for this browser, renames it, sets its four kinds of notification and sends a test, all at once through the hub's push routes, and says why it can't when the page has no secure connection, runs on an iPhone outside the Home Screen app (with the Add to Home Screen steps), or has no service worker; Other devices lists the rest with how delivery to each is going (`PushDeliveryLine`, which shows a failure only while it is newer than the last success) and Remove; and `[push] contact` sits under More options, staged. The store is `lib/push.svelte.ts` (`push`), started before mount by `shell/push-client.svelte.ts`, which also reports presence (`lib/push-presence.ts`) and routes the worker's notification clicks. History for All agents is the same `HistorySection` as an agent's, with `agent={null}`.

## Code Quality

Before submitting changes, run:

```bash
npm run lint          # ESLint, then the style lint
npm run format        # Prettier auto-format
npm run format:check  # Prettier check, as the pre-commit hook runs it
npm run check         # TypeScript / Svelte type check; warnings fail it
npm test              # Vitest: lib unit tests and Svelte component tests
npm run test:coverage # The same tests with a coverage summary (HTML report in coverage/)
npm run e2e:fast      # Playwright specs in Chromium, visual comparisons left out (see Testing)
```

`just web-size` builds and prints the size of the initial route: the scripts and stylesheets `dist/index.html` loads before the first screen, raw and gzipped (`scripts/web-initial-route-size.sh`). The release workflow adds the same table to its job summary. It is a report with no threshold.

**TypeScript lint.** Every `.ts` module under `src/` and `e2e/`, including the rune store modules (`*.svelte.ts`), gets the strict type-aware ESLint rules. Only `.svelte` files get the relaxed set that fits runes. When a rule is wrong for one line, use a scoped `// eslint-disable-next-line <rule> -- <reason>`, never a blanket disable. `npm run lint` runs ESLint in two passes, the `.svelte` files and then everything else, because one run over both kinds rebuilds the TypeScript program over and over and takes several minutes instead of about half a minute.

**Style lint.** `npm run lint` also runs Stylelint over `src/**/*.css` and the `<style>` blocks of `.svelte` files. Outside the token file (`src/styles/tokens.css`, the design token set described in [AESTHETIC.md](./AESTHETIC.md)) it forbids literal colors (hex, named, `rgb()` and the like), raw `font-size` and `font` values, raw `z-index` values, literal durations and easing curves in `transition` and `animation`, and `transition: all`. Reference a token with `var(--…)` instead. Viewport media queries may use only the shell breakpoints, written as `min-width`/`max-width`; a component that needs its own responsive rule uses a container query. No stylesheet or component is exempt. When a value has no token, add a token to `tokens.css` rather than a literal.

**svelte-check.** It runs with `--fail-on-warnings` and filters nothing, so every warning, accessibility ones included, fails it. Fix the markup rather than suppressing the warning.

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

The unit and component layers run in `npm test`, and the pre-commit hook runs them. The other three run through Playwright and are not part of the hook. A pull request that changes `web/` runs the whole suite in CI (`.github/workflows/web-pr.yml`), and so does the release workflow; `just web-e2e` runs it locally first.

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

Playwright starts two mock servers for every worker and stops them when the run ends. One is the deterministic mock on the Vite dev server (the first worker's on port 5273, artifacts on 5280). The other is the production build, built once per run, served with the mock (the first worker's on port 4273, artifacts on 4280). Each further worker's pair sits 10 ports above the previous one (5283 and 4283, then 5293 and 4293, and so on). They sit apart from `just web-mock` (5173) and `just web-mock-preview` (4173), so a mock you started yourself can stay up. Two suites at once on one machine, from two worktrees for instance, each set their own `E2E_DEV_PORT` and `E2E_PREVIEW_PORT`, far enough apart for every worker's ports; a port that is already taken is an error, never a server to reuse.

A mock's state is global to its server, so every worker drives its own servers: the fixtures point `baseURL` (and with it `page.goto` and the `mock` controls) at the worker's dev or preview server, and `artifactsOrigin()` gives its artifacts listener. Each test starts with `POST /api/mock/reset` on its worker's mock, and tests run fully parallel, so don't rely on what another test did. The suite runs on `E2E_WORKERS` workers, by default one for every two CPUs up to six; set the variable rather than passing `--workers`, since the servers are started per worker. Requests to anything but localhost are aborted, so the internet can't change a run (a web font fails fast and falls back the same way everywhere).

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

`mock.post(path, { params, data })` calls one of the mock's test controls (see [Deterministic mode](#deterministic-mode)) and fails the test on any answer but 2xx. Locate elements by role and accessible name, and wait with web-first assertions (`expect(locator).toBeVisible()`), never with fixed sleeps. The mock runs with zero delays and a fixed clock, so what a spec waits for arrives at once and looks the same on every run; `POST /api/mock/delays` slows it down for a spec about waiting, and `POST /api/mock/turn-hold` keeps a turn running for as long as a spec looks at it.

**Waiting.** The suite has to pass when other suites share the machine, where every page load takes several seconds longer than on a quiet one. A spec that passes only because nothing was slow fails there, and passes again when rerun alone. So a step waits for the condition it depends on, and the harness supplies the conditions that every spec shares:

- Navigate with `page.goto` and `page.reload`. The fixtures wrap them to return only when the app is usable (`waitForApp` in `e2e/support/app.ts`): the shell, the setup wizard or the gallery drawn; the hub socket connected (the shell's `data-hub` attribute is `connected`, as opposed to `connecting` or `lost`, which the "Can't reach Residuum" banner shows) and the overview it announces fetched (`data-overview` is `loaded`: the rail's counts and which agents are working); the code behind a `?settings=` or `?panel=file:` URL mounted; and the fonts loaded. A spec never needs to wait for the shell before its first action. A page on another origin, such as an artifact's, isn't the app and isn't waited for. A test that cuts the hub off (`mock.post("/api/mock/hub-socket", { data: { online: false } })`, or `context.setOffline(true)`) changes what ready means: the loads and screenshots that follow expect the banner (`lost`) until the test brings it back.
- Open the parts of the app whose code loads the first time they open (Settings, the palette, the file view, the setup wizard) through the helpers in `e2e/support/lazy.ts` (`expectSettingsOpen`, `expectPaletteOpen`, `expectFileOpen`, `expectSetupOpen`). Each waits for the part to mount, with the bound for code that loads (`LOAD_TIMEOUT`), and returns its locator. The default of five seconds is for waits on the page reacting, not for waits that cross a module graph the dev server serves.
- Never use `waitForTimeout` or any other fixed sleep; ESLint rejects `waitForTimeout` in `e2e/`. Ask what the next step needs and wait on that: an element, an attribute, a URL (`expect.poll`), a response (`page.waitForResponse`, registered before the click that causes it).
- Don't act on a trigger that may not be ready. A control that opens a menu or a dialog is ready once it is visible, enabled and closed (`aria-expanded="false"`); and anything that was closing over it, such as a modal, has gone.
- A change made through a mock control (`mock.post("/api/mock/...")`) that the page learns of over a socket (`/api/mock/agent-file`, `/api/mock/team-file`) reaches it only if the page's watch has already reached the mock, which on a loaded machine can be after the spec's next step. Make the change inside `expect.poll`, the same each time, until the page shows it, as `schedule.spec.ts` and the file conflict spec in `files.spec.ts` do.
- A test that loads a page for each of many cases (a table of redirects) is one test per case, made by looping over the table around `test(...)`, not a loop inside one test. Every page load is seconds on a loaded machine, and a single test spends its timeout on the sum of them.

The dev server compiles a module the first time it is asked for, so Playwright's global setup (`e2e/support/warmup.ts`) asks for every module of the app, lazy chunks and the gallery included, before the first test. The first spec to reach a part of the app doesn't pay for it.

**Accessibility.** `expectNoAxeViolations(page)` from `e2e/support/axe.ts` scans the page as it stands and fails on serious and critical violations, naming the rule and the elements. Put the page in the state under test first (open the menu, then scan), and scan each place and overlay a change touches. The scan waits for animations that end, such as an overlay fading in, since text at part opacity reads as low contrast. `{ within: "[role=dialog]" }` limits the scan to a region. The full result is attached to the test. A violation that can't be fixed at once is listed in the spec with its reason: `{ allow: [{ rule: "color-contrast", reason: "…" }] }`. The scan fails when an allowed rule no longer fires, so an entry goes away with the problem it excuses. Minor and moderate findings don't fail; they are in the attached result.

**Visual.** `expectScreenshot(page, "name")` from `e2e/support/screenshot.ts` compares the page with the baseline `name` in `e2e/__screenshots__/`, one per project. Wait for the page to reach the state under test first. The helper itself waits until the app is usable (so it never shoots the "Can't reach Residuum" banner of a hub socket that is still connecting; a spec of the banner takes the hub down first, see Waiting), the animations that end have finished, and the fonts and images the page uses have loaded. Put `@visual` on the test, so it runs in the `visual-*` projects, where the page's clock is frozen at the mock's clock (so "2h ago" reads the same on every run) and reduced motion is on. The helper turns off animations and hides the caret, and it paints over anything carrying a `data-visual-mask` attribute; pass `mask: [locator]` to cover a region for one screenshot. Compare the viewport unless a spec needs `fullPage: true`.

Outside the container a `@visual` spec skips itself and says why, so `just web-e2e-fast` never compares screenshots. The WebKit project skips itself, naming the reason, when this machine can't start WebKit (run it through `just web-e2e-webkit` instead).

### Baselines

Text and image rendering differ between machines, so baselines are only ever made and compared inside the official Playwright container (`mcr.microsoft.com/playwright`, the `-noble` image for the installed Playwright version, pulled for `linux/amd64` on any host). `scripts/with-playwright-container.sh` starts it with a browser server, the recipes run the suite against that browser, and the mock stays on your machine. The image tag follows the exact version pinned in `package.json`, so bumping `@playwright/test` means running `just web-e2e-update` and reviewing every changed image.

`just web-e2e-update` rewrites the baselines. Look at each changed image under `e2e/__screenshots__/` before committing it: a baseline records whatever the page showed, including a bug.

### When a spec fails

The run keeps a trace and a screenshot of each failed test under `web/test-results/`, and writes the HTML report to `web/playwright-report/`. Open a trace with `npx playwright show-trace test-results/<test>/trace.zip` to step through the actions, the page at each one, the console and the network. A failed visual comparison leaves the actual, expected and diff images beside it. In CI, each shard that fails uploads its report and traces as a `playwright-report-shard-<n>` artifact.

### CI

The quality-checks workflow runs the suite on pull requests that change `web/` and on release tags. It splits the tests across four `Web End-to-End` jobs with Playwright's `--shard`, each on its own runner, beside the `Web Checks` job that runs the format, lint, type, unit and build checks. The CI runners have no Chromium and can't download one, so each shard runs every project in the Playwright container: it sets `E2E_ALL_IN_CONTAINER=1` and runs `npm run e2e -- --shard=<n>/4` through `scripts/with-playwright-container.sh`, which starts the container beside the runner and stops it afterwards. `PLAYWRIGHT_IMAGE_REGISTRY` points the wrapper at the cluster registry's mirror of mcr.microsoft.com, so the image comes over the LAN. Together the shards run every spec, the visual comparisons included. Turning the workflow's `visual` input off runs `npm run e2e:fast` instead, and each shard's summary says the comparisons were skipped. The workflow can also be started by hand from the Actions tab, to try a change on the runners without cutting a release.

`E2E_ALL_IN_CONTAINER=1` is the same switch locally. Under the wrapper it sends `desktop`, `phone` and the `preview-*` projects to the container's browser as well, so the run needs no Chromium on your machine: `E2E_ALL_IN_CONTAINER=1 scripts/with-playwright-container.sh npm --prefix web run e2e:fast`. Without a container browser the variable is an error, never a quiet fall-back to this machine's Chromium. The recipes leave it off, so `just web-e2e` and `just web-e2e-fast` keep using this machine's Chromium for those projects.

## Running Against the Real Backend

If you have the Rust backend running on port 7700:

```bash
npm run dev
```

This uses Vite's proxy to forward `/api` and `/ws` requests to `localhost:7700`.
