# Contributing to the Residuum Web UI

Welcome! This guide will get you up and running with the frontend without needing the Rust backend.

## Prerequisites

- **Node.js 18+** — check with `node --version`
- **npm** — comes with Node.js

## Getting Started

```bash
cd web
npm install
npm run dev:mock
```

Open [http://localhost:5173](http://localhost:5173) in your browser. That's it — no backend required.

## Mock Mode

`npm run dev:mock` starts the Vite dev server with a built-in mock server that fakes all API endpoints and WebSocket connections. You'll see this in the terminal:

```
[mock] API mock server active
[mock] Mode: running (set VITE_MOCK_SETUP=1 for setup wizard)
[mock] WebSocket echo server on /ws
```

### What's mocked

- All REST endpoints return realistic fake data
- WebSocket simulates chat responses with tool calls and delays
- Agent sessions: live sessions (including a Discord conversation session) and a page-able list of finished ones. Messaging a session simulates a turn (include "busy" in the message to see a delivery failure), messaging a finished one resumes it, and a chat message starting with `spawn` starts a spawned session that relays its result to the main chat. Transcripts load after a short delay, so the loading state and anything racing it can be tried by hand
- `POST /api/mock/missed-relay` records a session result in the main chat's history and drops the WebSocket, to exercise catching up after a reconnect
- Main chat turns are recorded in history when they end. A chat message starting with `drop` loses the connection mid-turn: `drop finish …` ends the turn while disconnected, `drop compress …` also compresses history into a new episode (forcing a history reload), and any other `drop …` finishes the turn live after the page reconnects
- Config files are loaded from `../assets/*.example.*` and can be edited in the UI
- Secrets can be added and removed (stored in memory)
- Agent keys list with one user key and one agent-saved key, and can be added and removed (stored in memory)

### What's NOT mocked

- No real LLM calls happen — responses are canned
- Config saves don't persist across server restarts
- Some edge cases (rate limits, network errors) aren't simulated
- `POST /api/secrets` doesn't validate the value like the real server does — it accepts anything, including a `secret:` or `${ENV_VAR}` reference the real server would reject with a 400. The frontend already avoids sending those (see `lib/secrets.ts`), so this only matters if you're testing the rejection path itself

### Setup Wizard Mode

To test the first-run setup wizard:

```bash
VITE_MOCK_SETUP=1 npm run dev:mock
```

This starts the app in "setup" mode so you can walk through the onboarding flow.

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
│   │   ├── FeedItemView.svelte     # Renders one feed item; shared by chat and session views
│   │   ├── Message*.svelte         # Message components (user, assistant, agent message, status, …)
│   │   ├── ToolGroup.svelte        # Groups related tool calls together
│   │   ├── ToolItem.svelte         # Individual tool call display
│   │   ├── SessionsSidebar.svelte  # Live and finished agent sessions
│   │   ├── SessionView.svelte      # One session's transcript, live activity, message box, stop
│   │   ├── Header.svelte           # Top bar with navigation
│   │   ├── settings/               # Settings sub-panels
│   │   └── setup/                  # Setup wizard steps
│   └── lib/
│       ├── api.ts                # REST API client (typed fetch wrappers)
│       ├── ws.svelte.ts          # WebSocket coordinator: routes frames to the feed and sessions stores
│       ├── feed.svelte.ts        # Main chat feed state
│       ├── feed-items.ts         # History-to-feed conversion shared by chat and session views
│       ├── sessions.svelte.ts    # Agent sessions: listing, live frames, session view, commands
│       ├── routes.ts             # URL <-> location: which session, workspace flag, settings section
│       ├── router.svelte.ts      # Current location; push/replace history, back/forward
│       ├── relay.ts              # Recognizes agent-message headers in transcripts
│       ├── generated/            # Protocol types generated from Rust (cargo test --test ts_export)
│       ├── types.ts              # TypeScript types for API and messages
│       ├── commands.ts           # Slash command parser (/help, /reload, etc.)
│       ├── models.ts             # Model fetching and caching
│       ├── markdown.ts           # Markdown rendering
│       ├── settings-toml.ts      # Config serialization
│       └── secrets.ts            # secret:/${ENV_VAR} reference detection for settings fields
├── mock-server.ts            # Mock API + WebSocket (only used in dev:mock)
├── vite.config.ts
└── package.json
```

## Routing

The URL is the source of truth for where the user is:

| URL | Shows |
|-----|-------|
| `/` | Main chat |
| `/sessions/:runId` | A session's run in the main pane |
| `?workspace` (on either of the above) | Workspace panel open beside the main pane |
| `/settings/:section` | Settings, on one section |

`App.svelte` derives its layout state from `router` instead of mounting a component per route, so the chat, session view, and workspace stay mounted and every transition is the same CSS transition whether it came from a click or the back button. Navigate through `router` (or `sessions.openRun`), never by setting layout state directly.

History records places, not panel states. Opening a session, returning to the main chat, and opening or switching settings push an entry. Toggling the workspace replaces the current one, and so does a session view following its session into a new run. Back therefore moves between places the user visited. A settings URL says nothing about the chat side, so leaving settings returns to the session and workspace state that was showing before. Overlays (help, feedback, inbox) and the narrow-screen sessions drawer are not in the URL.

## Code Quality

Before submitting changes, run:

```bash
npm run lint          # ESLint check
npm run format        # Prettier auto-format
npm run check         # TypeScript / Svelte type check
npm test              # Vitest unit tests (*.test.ts next to the code they test)
```

## Running Against the Real Backend

If you have the Rust backend running on port 7700:

```bash
npm run dev
```

This uses Vite's proxy to forward `/api` and `/ws` requests to `localhost:7700`.
