---
name: workbench
description: Build interactive artifacts the user opens in the Residuum web UI — charts, dashboards, calculators, explorers, forms, or any page for choosing between options — as HTML pages or folders in workbench/. Activate before creating or editing anything in workbench/, or when the user asks for something visual or interactive to open in a browser.
---

# Workbench

The workbench holds artifacts you build for the user: each artifact is one HTML page or a folder of files, shown in the web UI at `/workbench/<name>`. An artifact can read Residuum's API, save its own data, stay current as workspace files change, stream live events, make one-shot calls to a small model, and run agent sessions whose results come back to the page. This skill does not cover files meant for download or chat attachments; send those as normal files.

## When to Use

- The user asks for a chart, diagram, dashboard, calculator, explorer, or "something I can open".
- An answer is clearer as an interactive page than as text: comparing options, exploring data, tuning parameters.

## Procedure

1. **Pick a name.** Lowercase letters, digits, and single hyphens, at most 64 characters: `pricing-explorer`, `sleep-chart`. Any other name is ignored by the workbench. To change an existing artifact, `read_file` the files you'll change (for a folder artifact, start with `index.html`) and edit them in place.

2. **Pick a shape.**
   - **Page:** `workbench/<name>.html`, everything inline. Use it for anything that fits comfortably in one file.
   - **Folder:** `workbench/<name>/index.html` plus the files it loads, referenced by relative URLs (`<script src="app.js">`, `import "./graph.js"`, `fetch("./data.json")`). Use it when the artifact has several scripts or modules, a web worker, or bundled data files.

3. **Write the artifact** with `write_file`:
   - Give the page a `<title>`: the workbench lists the artifact by it.
   - Set an explicit page `background` and text `color` on `body`. Unstyled pages render on white.
   - Load libraries from a CDN (jsdelivr, cdnjs, unpkg) or put them in the folder.
   - Keep each file under 8 MiB; larger files are refused.
   - Use the global `residuum` object for anything that talks to Residuum. It is injected into every page; do not add a script for it.

4. **Keep the artifact's own state** with `residuum.state.get()`/`residuum.state.set(value)` rather than hand-writing the state file path: it reads and writes `workbench/<name>.state.json` for you, beside the artifact (not inside its folder, where each save would reload the artifact). `get()` resolves to `null` before the first `set()`. Files with the artifact's name as prefix are deleted along with the artifact. For anything that doesn't fit that one file — other data files, conditional writes — use `residuum.fetch` against the workspace file API directly. `localStorage` works for view preferences, but it lives in one browser (the user won't see it on another device, and you can't read it) and every artifact shares it: prefix keys with the artifact's name, and keep anything private to the artifact in the workspace instead.

   ```js
   async function load() {
     return (await residuum.state.get()) ?? {};
   }
   async function save(state) {
     await residuum.state.set(state);
   }
   ```

   Binary data (an uploaded image, a rendered chart export) goes through `/api/workspace/raw` instead: `PUT` with an `ArrayBuffer`, typed array, or `Blob` body writes it unchanged, and `GET` reads it back with a guessed `Content-Type`. Delete, create a directory, or move/rename a file with `DELETE /api/workspace/file`, `POST /api/workspace/dir`, and `POST /api/workspace/move` — see `references/api.md` for their exact contracts.

5. **Keep workspace data current** when the artifact shows files that change (wiki pages, notes, inbox items, anything you or a background session edit): load the folder once with `GET /api/workspace/tree`, then follow it with `residuum.watch` and refresh only the changed files with one `POST /api/workspace/read`. Start watching before the first load so nothing slips between them. A `workspace_resync` means changes were missed: load everything again. A change to something you don't track as a file (a folder created, renamed, or removed stands for everything inside it) is simplest to handle the same way.

   ```js
   const pages = new Map(); // path -> text
   const isPage = (path) => path.endsWith(".md");

   async function loadAll() {
     const r = await residuum.fetch("/api/workspace/tree?path=wiki&content=true&glob=*.md");
     pages.clear();
     for (const e of (await r.json()).entries) if (e.content !== undefined) pages.set(e.path, e.content);
     render();
   }

   residuum.watch("wiki", async (frame) => {
     if (frame.type === "workspace_resync" || frame.changes.some((c) => !isPage(c.path))) {
       return loadAll();
     }
     const gone = frame.changes.filter((c) => c.kind === "removed").map((c) => c.path);
     const changed = frame.changes.filter((c) => c.kind !== "removed").map((c) => c.path);
     for (const path of gone) pages.delete(path);
     if (changed.length > 0) {
       const r = await residuum.fetch("/api/workspace/read", { method: "POST", body: { paths: changed } });
       for (const f of (await r.json()).files) {
         if (f.content !== undefined) pages.set(f.path, f.content);
         else pages.delete(f.path);
       }
     }
     render();
   });
   loadAll();
   ```

   Check `residuum.features.includes("workspace-watch")` first if the artifact must also work on an older Residuum; without it, reload on a timer or a refresh button instead.

6. **Tell the user where it is:** name the artifact's title and say it's in the web UI under Workbench (`/workbench/<name>`). If you know the address they use for the web UI, give the full link. The full view button (or `F`) lets the artifact fill the window. An open artifact reloads by itself when you save the file, so after an edit, say what changed rather than asking them to refresh.

## The `residuum` Object

| Call | Does |
|------|------|
| `await residuum.fetch(path, { method, headers, body })` | Calls Residuum's API and returns a standard `Response`. `path` starts with `/api/`. A plain object `body` is sent as JSON; an `ArrayBuffer`, typed array, or `Blob` is sent as-is. |
| `await residuum.ask(promptOrRequest)` | One-shot call to a small model. A string is shorthand for `{ prompt: text }`. Resolves to `{ content, json?, model, usage }`; rejects with an `Error` on failure. |
| `residuum.on(type, handler)` | Calls `handler(frame)` for each live event of that `type` (`"*"` for all), including `{ type: "connection", state: "connected" \| "disconnected" }` when Residuum's connection drops or returns. Returns an unsubscribe function. |
| `residuum.watch(prefix, handler)` | Calls `handler(frame)` when workspace files under `prefix` (a workspace-relative path like `"wiki"`, or `""` for everything) change: `{ type: "workspace_changed", changes: [{ path, kind: "created" \| "modified" \| "removed" }] }`, or `{ type: "workspace_resync", reason }` when changes were missed. Returns an unsubscribe function. |
| `await residuum.sessions.start({ prompt, context, skill, model })` | Starts an agent session for the artifact and returns a handle: `address`, `on(type, handler)` for that session's frames only, `send(text)`, `stop()`. |
| `residuum.embedded` | `false` when the page is opened outside the web UI, where `fetch`, `ask`, and `sessions.start` reject. |
| `residuum.artifact` | This artifact's own name. |
| `residuum.version` | Residuum's version. |
| `residuum.features` | Frozen array of feature ids this build supports. |
| `await residuum.state.get()` | The artifact's own saved state (`workbench/<name>.state.json`), parsed, or `null` before the first `set()`. Rejects if the saved content isn't valid JSON. |
| `await residuum.state.set(value)` | Saves `value` as the artifact's state, overwriting whatever was there. |

Read `references/api.md` for the endpoints worth calling, the event types, and which routes are blocked.

## Running Agent Work from an Artifact

When the page needs an agent to do something (research a topic, write or reorganize files, summarize a folder, fill in data) and show the result in the page, start a session with `residuum.sessions.start`. It is a full fork of you, with your tools, and its output comes back only to the page: nothing posts in the main chat or the inbox. Use `residuum.ask` instead for one-shot text work that needs no tools.

Check `residuum.features.includes("artifact-sessions")` before relying on it.

```js
const session = await residuum.sessions.start({
  prompt: `Write a wiki page about ${topic} in wiki/${slug}.md, then reply with one sentence saying what you wrote.`,
});
session.on("session_state_changed", (f) => showStatus(f.state)); // "running", "idle", …
session.on("session_response", (f) => showResult(f.content));
session.on("session_error", (f) => showError(f.message));
// Later, to follow up or cancel:
await session.send("Add a section on sources.");
await session.stop();
```

Write the prompt as a complete task brief: the session can't see the page or the main chat. Ask for the result in the shape the page will show (a sentence, a list, JSON). Show the session's progress and errors in the page, and give the user a way to stop it; the session keeps running if the page closes, and it idles for 10 minutes after its last turn so follow-up `send` calls land in the same run. The user can also watch or stop it from the artifact's own activity panel (the bar above the page) or the web UI's sessions sidebar, under Artifacts — stopping the page itself (Stop page, on that same bar) never stops the sessions it started.

## One-shot Model Calls

Use `residuum.ask` when the artifact itself needs a small piece of text intelligence — summarizing a note, classifying input, extracting fields, rewriting a passage — without involving you. The call goes to a small background model that sees only what the artifact sends: no tools, no memory, no identity files, none of the workspace context you have. Give it everything it needs in the prompt.

```js
const summary = await residuum.ask(`Summarize this in one sentence:\n\n${noteText}`);
render(summary.content);
```

For structured output, pass a JSON Schema and read the parsed result:

```js
const result = await residuum.ask({
  prompt: `Classify the sentiment of: "${feedback}"`,
  schema: { type: "object", properties: { sentiment: { type: "string" } }, required: ["sentiment"] },
});
render(result.json.sentiment);
```

Reach for `residuum.ask` only for genuinely one-shot work. Anything that needs your judgment, your tools, or multiple turns belongs in an agent session (`residuum.sessions.start`), not a model call.

## Verification

After writing an artifact, `read_file` it back and confirm:

- The page has a `<title>` and a `body` background.
- Every `residuum.fetch` path starts with `/api/` and appears in `references/api.md` as allowed.
- Every `residuum.sessions.start` result shows its `session_response` and `session_error` frames in the page, and the page can stop the session.
- Every `residuum.ask` prompt includes whatever context the model needs to answer — it sees nothing beyond what's in the call.
- Every data file the artifact writes is `workbench/<name>.<anything>`, beside the artifact, never a path under `workbench/<name>/`.
- An artifact that shows workspace files that can change calls `residuum.watch` before its first load and loads everything again on `workspace_resync`.
- Every relative URL names a file that exists in the artifact's folder (a page artifact has no other files), and no path starts with `/`, which would leave the artifact.
