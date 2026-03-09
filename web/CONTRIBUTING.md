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
- Config files are loaded from `../assets/*.example.*` and can be edited in the UI
- Secrets can be added and removed (stored in memory)

### What's NOT mocked

- No real LLM calls happen — responses are canned
- Config saves don't persist across server restarts
- Some edge cases (rate limits, network errors) aren't simulated

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
│   ├── main.ts              # App entry point
│   ├── App.svelte            # Main router — switches between Chat, Setup, Settings
│   ├── Chat.svelte           # Chat view
│   ├── Setup.svelte          # Setup wizard
│   ├── Settings.svelte       # Settings panel
│   ├── app.css               # Global styles
│   ├── components/
│   │   ├── ChatFeed.svelte       # Message list
│   │   ├── ChatInput.svelte      # Input box with slash commands
│   │   ├── Header.svelte         # Top bar with navigation
│   │   ├── MessageAssistant.svelte
│   │   ├── MessageUser.svelte
│   │   ├── MessageSystem.svelte
│   │   ├── ThinkingIndicator.svelte
│   │   ├── ToolGroup.svelte      # Groups related tool calls together
│   │   ├── ToolItem.svelte       # Individual tool call display
│   │   ├── settings/             # Settings sub-panels
│   │   └── setup/                # Setup wizard steps
│   └── lib/
│       ├── api.ts            # REST API client (typed fetch wrappers)
│       ├── ws.svelte.ts      # WebSocket connection (reactive state)
│       ├── types.ts          # TypeScript types for API and messages
│       ├── commands.ts       # Slash command parser (/help, /reload, etc.)
│       ├── models.ts         # Model fetching and caching
│       ├── markdown.ts       # Markdown rendering
│       └── settings-toml.ts  # Config serialization
├── mock-server.ts            # Mock API + WebSocket (only used in dev:mock)
├── vite.config.ts
└── package.json
```

## Code Quality

Before submitting changes, run:

```bash
npm run lint          # ESLint check
npm run format        # Prettier auto-format
npm run check         # TypeScript / Svelte type check
```

## Running Against the Real Backend

If you have the Rust backend running on port 7700:

```bash
npm run dev
```

This uses Vite's proxy to forward `/api` and `/ws` requests to `localhost:7700`.
