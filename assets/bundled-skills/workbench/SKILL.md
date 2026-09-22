---
name: workbench
description: Build interactive tools the user opens in the Residuum web UI — charts, dashboards, calculators, explorers, forms, or any page for choosing between options — as single HTML files in workbench/. Activate before creating or editing anything in workbench/, or when the user asks for something visual or interactive to open in a browser.
---

# Workbench

The workbench holds tools you build for the user: one self-contained HTML page per tool, shown in the web UI at `/workbench/<name>`. A tool can read Residuum's API, save its own data, stream live events, and send you messages when the user clicks something. This skill does not cover files meant for download or chat attachments; send those as normal files.

## When to Use

- The user asks for a chart, diagram, dashboard, calculator, explorer, or "something I can open".
- An answer is clearer as an interactive page than as text: comparing options, exploring data, tuning parameters.
- You want the user to pick between options by clicking (see "Asking the user to choose").

## Procedure

1. **Pick a name.** Lowercase letters, digits, and single hyphens, at most 64 characters: `pricing-explorer`, `sleep-chart`. Any other name is ignored by the workbench. To change an existing tool, `read_file` it first and edit it in place.

2. **Write the page** with `write_file` to `workbench/<name>.html`. Put all HTML, CSS, and JavaScript in that one file:
   - Give it a `<title>`: the workbench lists the tool by it.
   - Set an explicit page `background` and text `color` on `body`. Unstyled pages render on white.
   - Load libraries from a CDN with `<script src>` (jsdelivr, cdnjs, unpkg). Everything else stays inline. The page cannot load other files from the workbench folder, so embed data in the page or fetch it with `residuum.fetch`.
   - Keep the page under 8 MiB; larger pages are refused. Put bulky data in a workspace file and load it with `residuum.fetch` instead of inlining it.
   - Use the global `residuum` object for anything that talks to Residuum. It is injected into every tool; do not add a script for it.

3. **Save state through the workspace API**, never `localStorage` (the sandbox blocks it). Store a tool's data in `workbench/<name>.<anything>.json`: files with the tool's name as prefix are deleted along with the tool.

   ```js
   const STATE = "workbench/pricing-explorer.state.json";
   async function load() {
     const r = await residuum.fetch(`/api/workspace/file?path=${encodeURIComponent(STATE)}`);
     return r.ok ? JSON.parse(await r.text()) : {};
   }
   async function save(state) {
     await residuum.fetch("/api/workspace/file", {
       method: "PUT",
       body: { path: STATE, content: JSON.stringify(state) },
     });
   }
   ```

4. **Tell the user where it is:** name the tool's title and say it's in the web UI under Workbench (`/workbench/<name>`). If you know the address they use for the web UI, give the full link. The full view button (or `F`) lets the tool fill the window. An open tool reloads by itself when you save the file, so after an edit, say what changed rather than asking them to refresh.

## The `residuum` Object

| Call | Does |
|------|------|
| `await residuum.fetch(path, { method, headers, body })` | Calls Residuum's API and returns a standard `Response`. `path` starts with `/api/`. A plain object `body` is sent as JSON. |
| `await residuum.send(text)` | Sends `text` to you as a chat message, labelled with the tool's name. Works only inside a click or key-press handler; otherwise it rejects. |
| `residuum.on(type, handler)` | Calls `handler(frame)` for each live event of that `type` (`"*"` for all). Returns an unsubscribe function. |
| `residuum.embedded` | `false` when the page is opened outside the web UI, where `fetch` and `send` reject. |

Read `references/api.md` for the endpoints worth calling, the event types, and which routes are blocked.

## Asking the User to Choose

To have the user pick between options, build a page with one button per option and call `residuum.send` from each button's click handler with a message that names the choice:

```js
button.addEventListener("click", () => residuum.send(`Chose layout B: sidebar navigation`));
```

The message arrives as a user turn starting `[From workbench tool "<name>"]`. Treat it as the user's answer.

## Verification

After writing a tool, `read_file` it back and confirm:

- It has a `<title>` and a `body` background.
- Every `residuum.fetch` path starts with `/api/` and appears in `references/api.md` as allowed.
- Each `residuum.send` call is inside an `addEventListener("click" | "keydown", …)` callback, not at the top level or in load, timer, or `residuum.on` code.
- It references no local files besides CDN URLs.
