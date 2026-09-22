# Workbench

The workbench is where the agent builds interactive tools for the user: charts, dashboards, calculators, explorers, and pages for choosing between options. Each tool is a single HTML page the agent writes to `workbench/<name>.html` in the workspace. The web UI lists tools at `/workbench` and shows one at `/workbench/<name>`.

## Intended use

**Agent:** activates the bundled `workbench` skill, writes one self-contained page per tool with `write_file`, and tells the user the tool's title and path. Editing an existing tool is `read_file` then `edit_file`; an open tool reloads by itself when its page changes. A tool talks to Residuum only through the `residuum` object injected into every page (see below).

**User:** opens **Workbench** from the header menu. The list shows every tool by its `<title>`, newest edit first; a tool's seam glows while the agent writes to it. Opening a tool shows it under a slim bar with back, full view, and reload. **Full view** (the bar's button, or `F`) hides the Residuum UI so the tool fills the window; `Esc` or the corner button returns. Full view is part of the URL (`/workbench/<name>?full`), so it survives reloads and bookmarks. Deleting a tool from the list removes its page and its data files.

## Files

| Path | Holds |
|------|-------|
| `workbench/<name>.html` | A tool's page. `<name>` is lowercase letters, digits, and single hyphens, at most 64 characters; anything else is ignored. |
| `workbench/<name>.<anything>` | That tool's data (for example `<name>.state.json`), written through the workspace file API. Not listed as tools; deleted with the tool. |

Tool pages are limited to 8 MiB. Symlinked pages are ignored. Everything a tool needs beyond CDN libraries must be inline in its one file: a tool page cannot load other workbench files.

## The `residuum` object

| Call | Does |
|------|------|
| `residuum.fetch(path, init)` | Calls Residuum's API (`/api/...`) and returns a `Response`. A plain-object body is sent as JSON. |
| `residuum.send(text)` | Sends the agent a chat message labelled `[From workbench tool "<name>"]`. Only works during a click or key press in the tool. The user sees a notice that the tool sent a message; the reply is in chat. |
| `residuum.on(type, handler)` | Streams the same live frames the web UI receives (`"*"` for all), except keepalives. |
| `residuum.embedded` | `false` when the page is opened outside the web UI; `fetch` and `send` then reject. |

The endpoint and event catalogue the agent works from is the skill's `references/api.md`.

## Security model

The web UI has no login of its own (the relay authenticates remote access), so anything running on the UI's origin can call every endpoint. Tools never get that origin:

- **Sandboxed origin.** `GET /api/workbench/tools/<name>` serves the page with `Content-Security-Policy: sandbox allow-scripts allow-forms allow-modals allow-popups allow-downloads`, and the web UI frames it with the same `sandbox` flags. The page runs in an opaque origin whether it is framed or opened directly: it cannot read gateway responses, and it has no `localStorage` or cookies.
- **Cross-site guard.** The gateway rejects state-changing requests and WebSocket upgrades whose `Sec-Fetch-Site` is anything but `same-origin` or `none` (or, without fetch metadata, whose `Origin` is `null`). A tool calling the API directly is refused; so is any other website open in the browser.
- **Bridge.** `residuum.fetch` is relayed by the web UI (`web/src/lib/workbench-bridge.ts`), which refuses with `403` and a reason: writing secrets or agent keys; anything under `/api/config/raw`, `/api/providers/raw`, `/api/mcp/raw`, and `/api/config/complete-setup`; `/api/shutdown`, `/api/update/check|apply|restart`, `/api/cloud/disconnect`; writes under `/api/tracing/`; and deleting workbench tools. Paths outside `/api/` are refused.
- **Messages need a gesture.** `residuum.send` requires the page's user activation, which a click or key press inside the tool grants. A tool cannot message the agent on its own.

## Live reload

The gateway polls `workbench/` every second for tool pages (`*.html` only) and publishes each change on the bus; web UI clients receive `workbench_tool_updated` and `workbench_tool_removed` frames. An open tool reloads in place, keeping full view. Data files are not watched, so a tool saving its own state never reloads itself.

## Through the relay

Remote access serves the workbench exactly as locally. The relay injects its instance switcher only into top-level pages, never into tool frames. Tools stay single-file because browsers drop the relay's `SameSite=Lax` session cookie on requests made from inside a sandboxed frame: a tool's own sub-requests to the gateway would be sent to the login page. `residuum.fetch` is unaffected, since the web UI makes those requests.
