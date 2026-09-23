# Workbench

The workbench is where the agent builds interactive artifacts for the user: charts, dashboards, calculators, explorers, and pages for choosing between options. Each artifact is an HTML page (`workbench/<name>.html`) or a folder (`workbench/<name>/index.html` plus the files it loads) in the workspace. The web UI lists artifacts at `/workbench` and shows one at `/workbench/<name>`.

## Intended use

**Agent:** activates the bundled `workbench` skill, writes a page or a folder with `write_file`, and tells the user the artifact's title and path. Editing an existing artifact is `read_file` then `edit_file`; an open artifact reloads by itself when any of its files change. An artifact talks to Residuum only through the `residuum` object injected into every HTML page it serves (see below).

**User:** opens **Workbench** from the header menu. The list shows every artifact by its `<title>`, newest edit first; an artifact's seam glows while the agent writes to it. Opening an artifact shows it under a slim bar with back, full view, and reload. **Full view** (the bar's button, or `F`) hides the Residuum UI so the artifact fills the window; `Esc` or the corner button returns. Full view is part of the URL (`/workbench/<name>?full`), so it survives reloads and bookmarks. Deleting an artifact from the list removes its page or folder and its data files.

## Files

| Path | Holds |
|------|-------|
| `workbench/<name>.html` | A single-page artifact. |
| `workbench/<name>/` | A folder artifact: `index.html` is its page, and any other files are loaded by relative URL. When both a page and a folder share a name, the folder wins. |
| `workbench/<name>.<anything>` | That artifact's saved data (for example `<name>.state.json`), written through the workspace file API. Not part of the artifact, not watched for reloads, deleted with the artifact. |

`<name>` is lowercase letters, digits, and single hyphens, at most 64 characters; anything else is ignored. Files over 8 MiB are refused. Symlinks are ignored, and a file must resolve inside its artifact's folder; names starting with `.` are never served.

## The artifacts listener

Artifacts are served by their own listener, never by the gateway's main listener: at startup Residuum binds the first free port after the gateway's (`7702` by default), skipping Teams' configured port and its default `7701` so enabling Teams later can't collide. The port is stable across restarts while the machine's ports don't change, which keeps each artifact's origin, and so its browser storage, stable. The listener is read-only: `/<name>/` is the artifact's page, `/<name>/<path>` a file in a folder artifact, and `/<name>` redirects to `/<name>/`. If no port can be bound, Residuum runs without it, logs the reason at `error`, and the Workbench page shows it.

`GET /api/workbench/info` tells the web UI where artifacts are: the local port and, when connected to the cloud relay, the UI and artifacts origins the relay announced. The UI frames artifacts from the relay's artifacts origin when it is itself being viewed through the relay, and from its own host on the artifacts port otherwise.

## The `residuum` object

| Call | Does |
|------|------|
| `residuum.fetch(path, init)` | Calls Residuum's API (`/api/...`) and returns a `Response`. A plain-object body is sent as JSON. |
| `residuum.send(text)` | Sends the agent a chat message labelled `[From workbench artifact "<name>"]`. Only works during a click or key press in the artifact. The user sees a notice that the artifact sent a message; the reply is in chat. |
| `residuum.on(type, handler)` | Streams the same live frames the web UI receives (`"*"` for all), except keepalives. |
| `residuum.embedded` | `false` when the page is opened outside the web UI; `fetch` and `send` then reject. |

The endpoint and event catalogue the agent works from is the skill's `references/api.md`.

## Security model

The web UI has no login of its own (the relay authenticates remote access), so anything running on the UI's origin can call every endpoint. Artifacts are agent-written pages that load third-party scripts, so they never get that origin:

- **Separate origin.** Artifacts run on the artifacts listener's origin (`localhost:7702` locally, `<user>.workbench.agent-residuum.com` remotely), framed with `sandbox="allow-scripts allow-same-origin allow-forms allow-modals allow-popups allow-downloads"`. `allow-same-origin` gives an artifact its own origin (browser storage, relative files), not the UI's. The browser won't let it read the gateway's responses, since the gateway sends no CORS headers.
- **Cross-site guard.** The gateway rejects state-changing requests and WebSocket upgrades whose `Sec-Fetch-Site` is anything but `same-origin` or `none` (or, without fetch metadata, whose `Origin` is `null`). An artifact's origin is the *same site* as the UI's (same host, or a sibling subdomain), so its direct requests arrive as `same-site` and are refused, as are any other website's.
- **Bridge.** `residuum.fetch` is relayed by the web UI (`web/src/lib/workbench-bridge.ts`), which accepts messages only from the artifact's frame on the artifacts origin and refuses with `403` and a reason: writing secrets or agent keys; anything under `/api/config/raw`, `/api/providers/raw`, `/api/mcp/raw`, and `/api/config/complete-setup`; `/api/shutdown`, `/api/update/check|apply|restart`, `/api/cloud/disconnect`; writes under `/api/tracing/`; and deleting workbench artifacts. Paths outside `/api/` are refused.
- **Messages need a gesture.** `residuum.send` requires the page's user activation, which a click or key press inside the artifact grants. An artifact cannot message the agent on its own.
- **Artifacts share one origin.** Every artifact runs on the same artifacts origin, so artifacts can read each other's browser storage. Anything that must stay private to an artifact belongs in the workspace, not the browser.

## Live reload

The gateway polls `workbench/` every second. An artifact counts as changed when its page, or any file in its folder, changes; the change is published on the bus and web UI clients receive `artifact_updated` and `artifact_removed` frames. An open artifact reloads in place, keeping full view. Saved data files sit beside artifacts and are not watched, so an artifact saving its own state never reloads itself.

## Through the relay

The relay serves each user's artifacts at `<user>.workbench.agent-residuum.com`. Requests for that host bypass the relay's own routes entirely and go through the tunnel tagged `surface: "workbench"`; Residuum's tunnel client forwards them to the artifacts listener, and answers `503` with an explanation when the listener isn't running rather than falling back to the main listener. The host is owner-only like the UI host, read-only, and never gets the relay's instance switcher. The relay only forwards workbench requests to Residuum versions that advertise the `workbench-surface` capability; older versions get a page asking to update. The relay announces both origins in the tunnel handshake, which is how the web UI learns the artifacts origin remotely.
