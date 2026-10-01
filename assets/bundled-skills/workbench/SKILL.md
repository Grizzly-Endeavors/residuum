---
name: workbench
description: Build interactive artifacts (charts, dashboards, calculators, explorers) as HTML pages or folders in team/workbench/. Activate before creating or editing anything in team/workbench/, or when the user asks for something visual or interactive to open in a browser.
---

# Workbench

The workbench is the team's, shared by every agent, and lives in `team/workbench/`. It holds artifacts you build for the user: each artifact is one HTML page or a folder of files. Every artifact is a page of its own on Residuum's artifacts address, at `/<name>/`, and the web UI lists them under Workbench (`/team/workbench`). An artifact can read Residuum's API, save its own data, stay current as workspace files change, follow an agent's live events, make one-shot calls to a small model, and run agent sessions whose results come back to the page. This skill does not cover files meant for download or chat attachments; send those as normal files.

## When to Use

- The user asks for a chart, diagram, dashboard, calculator, explorer, or "something I can open".
- An answer is clearer as an interactive page than as text: comparing options, exploring data, tuning parameters.

## Procedure

1. **Pick a name.** Lowercase letters, digits, and single hyphens, at most 64 characters: `pricing-explorer`, `sleep-chart`. `api` is reserved. Any other name is ignored by the workbench. To change an existing artifact, `read_file` the files you'll change (for a folder artifact, start with `index.html`) and edit them in place.

2. **Pick a shape.**
   - **Page:** `team/workbench/<name>.html`, everything inline. Use it for anything that fits comfortably in one file.
   - **Folder:** `team/workbench/<name>/index.html` plus the files it loads, referenced by relative URLs (`<script src="app.js">`, `import "./graph.js"`, `fetch("./data.json")`). Use it when the artifact has several scripts or modules, a web worker, or bundled data files.

3. **Write the artifact** with `write_file`:
   - Give the page a `<title>`: the workbench lists the artifact by it.
   - Set an explicit page `background` and text `color` on `body`. Unstyled pages render on white.
   - Load libraries from a CDN (jsdelivr, cdnjs, unpkg) or put them in the folder.
   - Through Residuum Cloud, a file or response over 10 MB fails to load, so keep bundled data under that.
   - Use the global `residuum` object for anything that talks to Residuum. It is injected into every page; do not add a script for it.

4. **Name the agent in every agent-specific call.** An artifact belongs to the team, not to an agent, so nothing defaults to one. Use `residuum.agent(name)` for an agent's live events and files, `{ agent }` in `residuum.ask` and `residuum.sessions.start`, and `/api/agents/<name>/...` paths for an agent's routes. A path that belongs to an agent but names none (`/api/status`) answers `400` without being sent. Name yourself unless the user asked for a teammate, and keep the name in one constant at the top of the page.

5. **Keep the artifact's own state** with `residuum.state.get()`/`residuum.state.set(value)` rather than hand-writing the state file path: it reads and writes `team/workbench/<name>.state.json` for you, beside the artifact (not inside its folder, where each save would reload the artifact). `get()` resolves to `null` before the first `set()`. Files with the artifact's name as prefix are deleted along with the artifact. For anything that doesn't fit that one file — other data files, conditional writes — use `residuum.fetch` against the team file API (`/api/team/workspace/...`, with paths relative to `team/`) directly. `localStorage` works for view preferences, but it lives in one browser (the user won't see it on another device, and you can't read it) and every artifact shares it: prefix keys with the artifact's name, and keep anything private to the artifact in the workspace instead.

   ```js
   async function load() {
     return (await residuum.state.get()) ?? {};
   }
   async function save(state) {
     await residuum.state.set(state);
   }
   ```

   Binary data (an uploaded image, a rendered chart export) goes through `/api/team/workspace/raw` instead: `PUT` with an `ArrayBuffer`, typed array, or `Blob` body writes it unchanged, and `GET` reads it back with a guessed `Content-Type`. Delete, create a directory, or move/rename a file with `DELETE /api/team/workspace/file`, `POST /api/team/workspace/dir`, and `POST /api/team/workspace/move` — see `references/api.md` for their exact contracts.

6. **Keep workspace data current** when the artifact shows files that change (wiki pages, notes, inbox items, anything you or a background session edit): load the folder once with `GET /api/team/workspace/tree` (paths there are relative to `team/`), then follow it with `residuum.watch` and refresh only the changed files with one `POST /api/team/workspace/read`. Start watching before the first load so nothing slips between them. A `workspace_resync` means changes were missed: load everything again. A change to something you don't track as a file (a folder created, renamed, or removed stands for everything inside it) is simplest to handle the same way. `residuum.watch` follows team files only (`"team/..."`); an agent's own files are `residuum.agent(name).watch("notes", handler)`.

   ```js
   const pages = new Map(); // path (relative to team/) -> text
   const isPage = (path) => path.endsWith(".md");
   const inTeam = (path) => path.replace(/^team\//, ""); // change paths carry the team/ prefix

   async function loadAll() {
     const r = await residuum.fetch("/api/team/workspace/tree?path=wiki&content=true&glob=*.md");
     pages.clear();
     for (const e of (await r.json()).entries) if (e.content !== undefined) pages.set(e.path, e.content);
     render();
   }

   residuum.watch("team/wiki", async (frame) => {
     if (frame.type !== "workspace_changed" || frame.changes.some((c) => !isPage(c.path))) {
       return loadAll();
     }
     const gone = frame.changes.filter((c) => c.kind === "removed").map((c) => inTeam(c.path));
     const changed = frame.changes.filter((c) => c.kind !== "removed").map((c) => inTeam(c.path));
     for (const path of gone) pages.delete(path);
     if (changed.length > 0) {
       const r = await residuum.fetch("/api/team/workspace/read", { method: "POST", body: { paths: changed } });
       for (const f of (await r.json()).files) {
         if (f.content !== undefined) pages.set(f.path, f.content);
         else pages.delete(f.path);
       }
     }
     render();
   });
   loadAll();
   ```

7. **Tell the user where it is:** name the artifact's title and say it's under Workbench in the web UI (`/team/workbench/<name>`). If you know the address they use for Residuum, give the full link. An open artifact reloads by itself when you save its files, so after an edit, say what changed rather than asking them to refresh.

## The `residuum` Object

| Call | Does |
|------|------|
| `await residuum.fetch(path, { method, headers, body, signal })` | Calls Residuum's API and returns a standard `Response`. `path` starts with `/api/` and names its scope: `/api/team/...`, `/api/hub/...`, or `/api/agents/<name>/...`. A plain object `body` is sent as JSON; an `ArrayBuffer`, typed array, or `Blob` is sent as-is. A path that belongs to an agent but names none resolves to a `400` whose `error` says how to name it. |
| `await residuum.ask(promptOrRequest, { agent }?)` | One-shot call to a small model of one agent. A string is shorthand for `{ prompt: text }`. Name the agent in the request (`{ agent, prompt }`) or as the second argument; a call with none rejects with a `TypeError`. Resolves to `{ content, json?, model, usage }`; rejects with an `Error` on failure. |
| `residuum.on(type, handler)` | Residuum's own events: `artifact_updated`, `artifact_removed`, `connection`, or `"*"` for all three. Any other type throws a `TypeError`: an agent's events come from `residuum.agent(name).on`. Returns an unsubscribe function. |
| `residuum.watch(prefix, handler)` | Calls `handler(frame)` when team files under `prefix` change: `"team"` for all of them, `"team/wiki"`, or `"team/workbench/<name>.state.json"` for one file. Frames are `{ type: "workspace_changed", changes: [{ path, kind: "created" \| "modified" \| "removed" }] }`, or `{ type: "workspace_resync", reason }` when changes were missed. Any prefix outside `team/` throws a `TypeError`. Returns an unsubscribe function. |
| `residuum.agent(name)` | The handle for one agent: `name`, `on(type, handler)` for that agent's live events (`"*"` for all, tool calls included), and `watch(prefix, handler)` for its workspace (`"notes"`, `""` for all of it). Both return an unsubscribe function. |
| `await residuum.sessions.start({ agent, prompt, context, skill, model })` | Starts a session for the artifact on the named agent and returns a handle: `agent`, `address`, `on(type, handler)` for that session's frames only, `send(text)`, `stop()`. A call with no `agent` rejects with a `TypeError`. |
| `residuum.artifact` | This artifact's own name. |
| `residuum.version` | Residuum's version. |
| `residuum.features` | Frozen array of feature ids this build supports. |
| `await residuum.state.get()` | The artifact's own saved state (`team/workbench/<name>.state.json`), parsed, or `null` before the first `set()`. Rejects if the saved content isn't valid JSON. |
| `await residuum.state.set(value)` | Saves `value` as the artifact's state, overwriting whatever was there. |

The page reloads itself when any of its own files change. To keep the page's state instead (a long form, a running session), register a `residuum.on("artifact_updated", handler)`: the page then stays loaded and the handler is called, with `{ name }`, for every artifact that changes.

Read `references/api.md` for the endpoints worth calling, the event types, and which routes are blocked.

## Running Agent Work from an Artifact

When the page needs an agent to do something (research a topic, write or reorganize files, summarize a folder, fill in data) and show the result in the page, start a session with `residuum.sessions.start`. It is a full fork of the agent you name, with that agent's tools, and its output comes back only to the page: nothing posts in the main chat or the inbox. Use `residuum.ask` instead for one-shot text work that needs no tools.

The agent has to be running: the call rejects with the gateway's message if it doesn't exist or is stopped, so show that message in the page. The page follows the session over Residuum's live connection; if that connection isn't available within 10 seconds, the start rejects (`code: "no_live_connection"`) and no session runs, so show that message too.

```js
const AGENT = "scout"; // the agent that runs this artifact's sessions and model calls: your own name

const session = await residuum.sessions.start({
  agent: AGENT,
  prompt: `Write a wiki page about ${topic} in team/wiki/${slug}.md, then reply with one sentence saying what you wrote.`,
});
session.on("session_state_changed", (f) => showStatus(f.state)); // "running", "idle", …
session.on("session_response", (f) => showResult(f.content));
session.on("session_error", (f) => showError(f.message));
session.on("resync", (f) => showStatus(f.session ? f.session.state : "unknown")); // updates were missed
// Later, to follow up or cancel:
await session.send("Add a section on sources.");
await session.stop();
```

Write the prompt as a complete task brief: the session can't see the page or the main chat. Ask for the result in the shape the page will show (a sentence, a list, JSON). Show the session's progress and errors in the page, and give the user a way to stop it; the session keeps running if the page closes or reloads, and it idles for 10 minutes after its last turn so follow-up `send` calls land in the same run. The user can also watch or stop it from that agent's Activity in the web UI, where it shows as From a workbench page.

## One-shot Model Calls

Use `residuum.ask` when the artifact itself needs a small piece of text intelligence — summarizing a note, classifying input, extracting fields, rewriting a passage — without involving you. The call goes to a small background model that sees only what the artifact sends: no tools, no memory, no identity files, none of the workspace context you have. Give it everything it needs in the prompt.

```js
const summary = await residuum.ask(`Summarize this in one sentence:\n\n${noteText}`, { agent: AGENT });
render(summary.content);
```

For structured output, pass a JSON Schema and read the parsed result:

```js
const result = await residuum.ask({
  agent: AGENT,
  prompt: `Classify the sentiment of: "${feedback}"`,
  schema: { type: "object", properties: { sentiment: { type: "string" } }, required: ["sentiment"] },
});
render(result.json.sentiment);
```

Reach for `residuum.ask` only for genuinely one-shot work. Anything that needs your judgment, your tools, or multiple turns belongs in an agent session (`residuum.sessions.start`), not a model call. Through Residuum Cloud a call that takes longer than 25 seconds fails, so keep prompts and `max_tokens` small enough to answer in that time.

## Verification

After writing an artifact, `read_file` it back and confirm:

- The page has a `<title>` and a `body` background.
- Every `residuum.fetch` path starts with `/api/team/`, `/api/hub/`, or `/api/agents/<name>/`, and appears in `references/api.md` as allowed.
- Every agent-specific call names its agent: `residuum.agent(name)`, `{ agent }` in `residuum.sessions.start` and `residuum.ask`. `residuum.on` is used only for `artifact_updated`, `artifact_removed`, and `connection`, and `residuum.watch` only for `team/` paths.
- Every `residuum.sessions.start` and `residuum.ask` shows its error in the page when it rejects.
- Every `residuum.sessions.start` result shows its `session_response` and `session_error` frames in the page, and the page can stop the session.
- Every `residuum.ask` prompt includes whatever context the model needs to answer — it sees nothing beyond what's in the call.
- Every data file the artifact writes is `team/workbench/<name>.<anything>`, beside the artifact, never a path under `team/workbench/<name>/`.
- An artifact that shows workspace files that can change calls `residuum.watch` before its first load and loads everything again on `workspace_resync`.
- Every relative URL names a file that exists in the artifact's folder (a page artifact has no other files), and no path starts with `/`, which would leave the artifact.
