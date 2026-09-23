---
name: workbench
description: Build interactive artifacts the user opens in the Residuum web UI — charts, dashboards, calculators, explorers, forms, or any page for choosing between options — as HTML pages or folders in workbench/. Activate before creating or editing anything in workbench/, or when the user asks for something visual or interactive to open in a browser.
---

# Workbench

The workbench holds artifacts you build for the user: each artifact is one HTML page or a folder of files, shown in the web UI at `/workbench/<name>`. An artifact can read Residuum's API, save its own data, stream live events, and send you messages when the user clicks something. This skill does not cover files meant for download or chat attachments; send those as normal files.

## When to Use

- The user asks for a chart, diagram, dashboard, calculator, explorer, or "something I can open".
- An answer is clearer as an interactive page than as text: comparing options, exploring data, tuning parameters.
- You want the user to pick between options by clicking (see "Asking the user to choose").

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

5. **Tell the user where it is:** name the artifact's title and say it's in the web UI under Workbench (`/workbench/<name>`). If you know the address they use for the web UI, give the full link. The full view button (or `F`) lets the artifact fill the window. An open artifact reloads by itself when you save the file, so after an edit, say what changed rather than asking them to refresh.

## The `residuum` Object

| Call | Does |
|------|------|
| `await residuum.fetch(path, { method, headers, body })` | Calls Residuum's API and returns a standard `Response`. `path` starts with `/api/`. A plain object `body` is sent as JSON. |
| `await residuum.send(text)` | Sends `text` to you as a chat message, labelled with the artifact's name. Works only inside a click or key-press handler; otherwise it rejects. |
| `residuum.on(type, handler)` | Calls `handler(frame)` for each live event of that `type` (`"*"` for all). Returns an unsubscribe function. |
| `residuum.embedded` | `false` when the page is opened outside the web UI, where `fetch` and `send` reject. |
| `residuum.artifact` | This artifact's own name. |
| `residuum.version` | Residuum's version. |
| `residuum.features` | Frozen array of feature ids this build supports. |
| `await residuum.state.get()` | The artifact's own saved state (`workbench/<name>.state.json`), parsed, or `null` before the first `set()`. Rejects if the saved content isn't valid JSON. |
| `await residuum.state.set(value)` | Saves `value` as the artifact's state, overwriting whatever was there. |

Read `references/api.md` for the endpoints worth calling, the event types, and which routes are blocked.

## Asking the User to Choose

To have the user pick between options, build a page with one button per option and call `residuum.send` from each button's click handler with a message that names the choice:

```js
button.addEventListener("click", () => residuum.send(`Chose layout B: sidebar navigation`));
```

The message arrives as a user turn starting `[From workbench artifact "<name>"]`. Treat it as the user's answer.

## Verification

After writing an artifact, `read_file` it back and confirm:

- The page has a `<title>` and a `body` background.
- Every `residuum.fetch` path starts with `/api/` and appears in `references/api.md` as allowed.
- Each `residuum.send` call is inside an `addEventListener("click" | "keydown", …)` callback, not at the top level or in load, timer, or `residuum.on` code.
- Every data file the artifact writes is `workbench/<name>.<anything>`, beside the artifact, never a path under `workbench/<name>/`.
- Every relative URL names a file that exists in the artifact's folder (a page artifact has no other files), and no path starts with `/`, which would leave the artifact.
