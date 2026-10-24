# Residuum Web UI Architecture

How the web app is put together: where code lives, how a URL becomes a screen, and the contracts between the shell, the places and the data layer. It describes shape and the non-obvious rules; the code is the source for details. [CONTRIBUTING.md](./CONTRIBUTING.md) covers running and testing the app, and [AESTHETIC.md](./AESTHETIC.md) the visual system.

The app is a Svelte 5 single-page app. The Rust binary embeds the production build (`web/dist`) and serves it from the hub.

## Project structure

| Path                                        | Holds                                                                                                                                                                                                                                            |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `src/main.ts`, `src/App.svelte`             | Entry point. `App` draws the setup wizard or the shell, plus toasts and tooltips in both                                                                                                                                                         |
| `src/shell/`                                | The frame around every place: rail, phone bottom bar and drawer, hub banner, update banner, place routing, command palette, app actions, dialogs. `shell/panel/` is the context panel, `shell/settings/` the Settings modal's parts and sections |
| `src/places/`                               | One folder per place: `home`, `inbox`, `chat`, `activity`, `schedule`, `files`, `workbench`, `not-found`                                                                                                                                         |
| `src/feed/`                                 | A conversation (turns, activity lines, messages), shared by Chat and session transcripts                                                                                                                                                         |
| `src/composer/`                             | The composer frame shared by the chat composer and a session's message box, its `/` menu, and staged file chips                                                                                                                                  |
| `src/lib/`                                  | The data layer: API client, stores, routing, the settings model, formatters. `lib/ui/` holds the primitive controls and overlays, `lib/generated/` the protocol types generated from Rust                                                        |
| `src/sw/`                                   | The service worker, a separate TypeScript program with the worker's globals                                                                                                                                                                      |
| `src/pairing/`                              | The `/pair` page an unpaired browser lands on through Residuum Cloud, mounted by `main.ts` instead of the app                                                                                                                                    |
| `src/Setup.svelte`, `src/components/setup/` | The first-run setup wizard                                                                                                                                                                                                                       |
| `src/styles/`                               | Design tokens, fonts, base styles                                                                                                                                                                                                                |
| `src/test/`                                 | Component-test helpers and harnesses                                                                                                                                                                                                             |
| `build/`                                    | The Vite plugin that builds the service worker into `dist/sw.js`                                                                                                                                                                                 |
| `mock/`                                     | The mock backend used by `dev:mock`, `preview:mock` and the tests                                                                                                                                                                                |
| `e2e/`                                      | Playwright specs (`smoke/`, `visual/`, `harness/`) and their support code (`support/`)                                                                                                                                                           |
| `scripts/`                                  | `visual-report.mjs`, which turns the visual comparisons' report into the PR comment                                                                                                                                                              |
| `public/`                                   | Served as is: manifest, icons, favicon, MCP catalog                                                                                                                                                                                              |

Two rules shape `src/`:

- **Stores don't navigate.** A store is a rune module (`*.svelte.ts`) in `lib/` that holds data and commands. Views read stores, draw, and navigate. ESLint forbids importing the router under `src/lib/` (except the router itself and `session-address.ts`).
- **No request reads the viewed agent.** Every agent-scoped function in `lib/api.ts` takes the agent name first, and its cache key includes it. Components take the agent from props, stores hold the agent they were bound to, and chat controls use `ws.agent`. Calls that serve several scopes take `agent: string | null` (`null` means the team's files or the hub's repositories); asking for an agent's own resource with `null` throws `NoAgentSelectedError` before any request.

## Routing

The URL is the source of truth for where the user is. A location is a place, an optional context panel and an optional Settings modal.

| URL                                            | Place                                                                         |
| ---------------------------------------------- | ----------------------------------------------------------------------------- |
| `/`                                            | Redirects to `/home`                                                          |
| `/home`                                        | Home                                                                          |
| `/inbox`                                       | Inbox, with `?folder=`, `?agent=`, `?thread=<ref>` or `?compose=1&to=<names>` |
| `/agent/:name`                                 | That agent's Chat                                                             |
| `/agent/:name/activity`, `/schedule`, `/files` | Its Activity, Schedule, Files                                                 |
| `/team/workbench[/:artifact]`                  | The Workbench, optionally with an artifact selected                           |
| `/team/files`                                  | Shared files                                                                  |
| anything else                                  | Not Found, keeping the address                                                |

Any place also takes:

- `panel=` for the context panel: `session:<agent>:<runId>`, `new-session:<agent>`, `file:<path>`, `agent-file:<agent>:<path>` or `size`. Each kind shows only on the places that can host it; the router removes a panel a place can't show. `file:` reads the tree the place shows; `agent-file:` names its agent and also shows on the Inbox, so mail attachments open beside their thread.
- `settings=<agent | _all>[/<section>]` for the Settings modal. `_all` is the install-wide scope. Section ids are in `lib/settings-sections.ts`.

`lib/routes.ts` parses and formats URLs and knows nothing of the browser. `lib/router.svelte.ts` holds the location. Legacy URLs redirect by replace (`routes.ts` lists them). An unknown agent or artifact, a settings scope that can't be an agent, or a panel the place can't show is corrected by replace, with a toast where the user should know.

**Backend notices.** When the backend can't serve a browser address (an expired file link, a stopped agent, a missing artifact), it redirects into the app with `notice=<code>` (see Browser landings in `docs/systems-usage/hub-http.md`). The router shows the toast once and drops the parameter. A new code is added to `AppNotice` in `crates/net/src/browser_landing.rs` and to `lib/landing-notices.ts` together.

**Push, replace, close.** `openPlace`, `openPanel` and `openSettings` push; `replace*`, `switchSettings*`, corrections and redirects replace. `closePanel`, `closeSettings` and `closeItem` go back in history when this page pushed the entry being closed, and replace otherwise (a deep link or reload). The result: Back closes the panel, modal, open thread or selected artifact before it leaves a place. In the Inbox and Workbench, opening a second item while one is open replaces, so the open item is always one history entry, and changing the Inbox folder or agent filter replaces too.

**Overlays.** A modal overlay calls `router.openOverlay(onDismiss)`, which pushes an entry with the same URL so Back closes it; the overlay closes itself through the returned handle. `ModalLayer` (under Dialog, Sheet and Drawer) does this for you; pass `historyEntry={false}` when a URL parameter already is the entry, as with Settings.

**Unsaved work.** A view holding work the user would lose registers `router.guard.register(check)`; the check returns a line describing the loss for a target location, or null. In-app navigation, Back/Forward and reload all ask first (reload through the browser's own prompt).

**The bound agent** is the viewed agent on an agent place, and otherwise the most recently viewed one (remembered in local storage).

## The shell

`shell/Shell.svelte` frames every place: the rail beside the main region at medium and wide widths, and on phones a bottom bar with the rail in a drawer. The main region holds the hub banner (shown while the hub socket is down) and the place that `PlaceHost.svelte` picks from the location. The shell mounts the app-wide dialogs once each: confirm, Settings, the palette, Recent notifications, keyboard shortcuts, feedback, Add to Home Screen and Create agent. `ShellActions.createAgent` is the one way to create an agent.

The context panel (`panel/PanelHost.svelte`) is a resizable column at wide widths, floats over the main region at medium widths, and is a full-screen sheet on phones. `PanelHost` picks the content by panel kind and keeps per-panel state (an open file buffer, a session run) while the frame redraws at another width.

The rail's counts: Home is the needs-you count, Inbox the unread items across agents, and an agent's Activity its running count (`lib/running-count.ts`, the same number as the Chat header pill). The installed app's icon badge carries the inbox count where the Badging API exists. `TabTitle.svelte` titles the browser tab.

## Installing, code splitting and the service worker

**Installable.** `index.html` and `public/manifest.webmanifest` make the app installable. The manifest link carries `crossorigin="use-credentials"` because Residuum Cloud's relay answers a cookieless request with its login page. `lib/install.ts` decides whether Install app is offered (only in a secure context and outside an installed app; on iOS it opens the Add to Home Screen steps). Anything needing a secure context, push included, checks `installContext().secure`.

**Lazy chunks.** Settings, the command palette, the file panel and the setup wizard are separate chunks; the shell, Home and Chat are in the initial bundle. `LazyComponent` (`lib/lazy-component.svelte.ts`) loads a chunk on first use, shows a failure toast and retries on the next ask. Importing a lazy module statically from the initial bundle pulls its chunk back in, so import only its types or go through `LazyComponent`. `e2e/smoke/installable.spec.ts` finds chunks by name.

**Service worker.** `src/sw/worker.ts`, served as `/sw.js`, holds the app shell so the app opens with no network. It caches no data.

- `build/service-worker.ts` runs after the bundle is written, lists the shell files (`build/precache.ts`), derives a version from their contents and writes `dist/sw.js`. Any byte change in the shell yields a new worker.
- Page loads go network first, falling back to the cached `index.html` when the network fails or the hub answers 502–504. App files (`/assets/`, icons, favicon) come from the cache first.
- It never answers non-GET requests, other origins, `/api`, `/ws`, `/webhook` or `/cloud/callback`. The routing decisions are pure functions in `src/sw/rules.ts`.
- A new worker waits until the page posts `skip-waiting`. `lib/app-update.svelte.ts` checks for updates when the page becomes visible and every ten minutes, and `shell/UpdateBanner.svelte` offers Reload, which asks the unsaved-work guard first. Activation keeps this version's cache and the previous one, so a page opened before an update can still fetch its hashed files.
- Push: the worker shows every push as a notification (`src/sw/push.ts`) and, on click, focuses or opens a window at the notification's target.
- Registration happens in builds only (`__SERVICE_WORKER__`); the dev server never registers one. A worker a preview registered on a port stays there, so unregister it in the browser's devtools before running the dev server on the same port.

## Actions

`lib/action-registry.svelte.ts` holds the one list of named actions that the command palette, the composer's `/` menu and the rail's Help menu draw from. A source is a function that builds actions from current state; `actionRegistry.register(key, source)` adds one and returns its remover. The shell registers the app's sources in `shell/app-actions.svelte.ts`.

An action has a `group`, a plain-language `label`, optional `hint` and `terms`, and `run(text?)`. A chat action also has a `command` (its `/name`), and `takesText` when it acts on what follows. `disabled` holds the reason it can't run now, which the palette and menu show in place of the hint. Run actions through `actionRegistry.run(action, text?)`, which notifies run listeners (the shell closes the phone drawer) before running.

## Data layer

**Sockets.** `lib/transport.svelte.ts` connects, reconnects and holds outgoing frames while a connection is down. `lib/hub.svelte.ts` is the hub socket's state (each agent's state and activity, notices, deleted agents) and stays up across agent switches. `lib/ws.svelte.ts` is the coordinator for the bound agent's socket: it routes frames to the feed and sessions stores and is open only while the hub lists the agent running. History loads through file-only routes whatever the agent's state. Every agent connection sends `set_verbose {enabled: true}` (activity lines are built from tool frames), then `resync_turn`.

**Agent state.** Everywhere an agent's state is shown, use `hub.displayStateOf(name)` (or `displayState` in `lib/agent-display-state.ts`): the hub's state, or `stopping` while a stop is under way. `lib/agent-lifecycle.ts` says which of Start, Stop and Restart apply.

**Watches.** `lib/watch-registry.ts` merges every owner's path prefixes into one watch set per socket. `lib/agent-watches.svelte.ts` watches any agent's workspace: through the bound socket when that agent is bound, and otherwise through a watch-only connection (`?watch_only=true`, which the agent leaves out of its count of open pages, so unread counts and pushes are unaffected).

**Overview.** `lib/overview.svelte.ts` holds what Home and the rail counts show beyond the hub store: each agent's overview, recent team events, unread inbox threads, and `needsYou` (computed in `lib/needs-you.ts`). It follows the hub socket: `hub_boot` refetches (dropping everything when the boot id changed), a later `agents_snapshot` refetches what the connection may have missed, and `agent_overview` / `team_event` frames update in place.

### Config writes

Every write to a config file (an agent's `config.toml`, `providers.toml` and `mcp.json`, and the hub's `config.toml`) goes through `configCoordinator` in `lib/config-coordinator.ts`. Never call the `patch…`, `put…` or checkpoint restore functions in `lib/api.ts` for these files directly. Name files with `agentConfigFile(agent, "providers")` or `HUB_CONFIG_FILE`.

| Call                                              | Does                                                                                                                                             |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------ |
| `save(file, { baseline, edit, choose, source? })` | Writes `{ patch }` or `{ text }`. Resolves `saved` (with the new text as the next baseline) or `used-disk`                                       |
| `edit(file, build, source?)`                      | Reads the file, builds a patch from its text and writes it with no other write in between; for one-key controls like the composer's model picker |
| `reload(file, source?)`                           | Rereads from disk and notifies subscribers                                                                                                       |
| `restore(...)`, `undo(...)`                       | Restore or undo a checkpoint and notify subscribers of any config file it wrote. Every checkpoint restore goes through these                     |
| `subscribe(file, listener)`                       | Hears `{ file, cause, source }` for every change; pass `source` to a write to recognise your own                                                 |

Writes to one file are serialized. Before a save the coordinator rereads the file; if it differs from `baseline` and the changed keys overlap the edit (a `{ text }` save overlaps everything), it calls `choose`, which answers `"keep-mine"` or `"use-disk"`. No lock is held while `choose` waits, so it can ask the user. Non-overlapping changes merge. `lib/config-sync.ts` feeds outside changes in: `workspace_changed` under the bound agent's `config/`, and the hub's `hub_config_reloaded`. Other agents' files have no change feed, so the pre-save reread is their only protection.

### The settings model

`lib/settings-model.svelte.ts` holds what the Settings modal edits, with no UI. `settingsModel.agent(name)` and `settingsModel.all()` return a scope. A scope never writes another scope's files; an agent scope shows the hub's values read-only as `scope.install`. Create scopes in script, not in template expressions, since creating one changes the registry.

Each file has a baseline (its text as last loaded or saved) and a form (a parsed copy the sections bind to). The staged change is the difference, so it survives closing the modal or switching scope. Read the form through the scope each time (`scope.config.timeout_secs`), because a reload replaces the objects.

- `load()` reads each file, keeping staged changes; `reload()` discards them and rereads; `discard()` drops them.
- `save(choose)` stores typed secrets, then writes each changed file's diff through the coordinator (providers, then config, then MCP). A failed file keeps its changes. The result names every checkpoint taken.
- `undo()` restores each file the last save wrote, in reverse order, skipping any that changed since.
- `fieldDiagnostics(ref)` and `sectionDiagnostics(section)` place a save's problems.
- `file(name).lockedBy` makes the raw editor and the form mutually exclusive: a form edit locks the raw editor, a raw draft locks the form.

`lib/settings-fields.ts` is the single map from form field to key path, shared by diffing and diagnostic placement; `settings-fields.test.ts` fails on a field it doesn't cover. Failover lists ride along with their role so a save never shortens one. The model's tests are `settings-model.component.test.ts`, so runes are live.

## The Settings modal

`shell/SettingsModal.svelte` is open while the URL has `settings`. Its side nav is the scope picker and section list (on phones, the list is its own screen). The modal isn't remounted when scope or section changes; the content swaps once the new scope has loaded. The save bar shows while the scope has staged changes, and a file that changed on disk under the keys being saved opens `ChangedOnDiskDialog`.

**Writing a section.** A section is a component taking `{ scope, section }` (`AgentSectionProps` or `AllSectionProps` in `shell/settings/sections.ts`), registered in `AGENT_SECTION_VIEWS` or `ALL_SECTION_VIEWS`.

- Start with `SettingsSection`, then group fields in `SettingsGroup` cards. Bind fields to the scope's forms; the save bar follows staged changes on its own.
- A `config.toml` number or switch is `ConfigNumber` or `ConfigToggle`, which place their own save problems. Forms keep numbers as text: a blank box is an unset key, and `fallback` shows the value the agent uses then. Other controls take `error={configFieldError(scope, key)}`.
- A setting that only applies while another is on is `disabled`, not hidden, so its problems stay visible. A `Disclosure` holding fields opens when one has a problem.
- Flag an unreasonable value with a note beside the field; never block the save.
- Credentials use `SecretConfigField`; folder lists use `PathList`; parts that need the agent running go inside `RunningOnly`.
- Section-local actions register with `actionRegistry.register` inside `untrack` in an `$effect` that returns the remover.
- Immediate actions (secrets, keys, Cloud, updates, visibility, `a2a.json`) call their endpoints and report their own result, outside the save bar. One that writes a file the scope holds calls `scope.load()` afterwards.
- A field other places can link to carries `data-field={fieldMark(ref)}`; `scope.requestFocus(ref)` asks for it and the owning section calls `scope.takeFocus()` and `focusField(root, ref)` once it shows.

Raw config edits each file as text in the Files editor with live validation, and saves the whole file even with problems. History (`HistoryBrowser.svelte`) shows an agent's workspace and config repositories, or with `agent={null}` the team's and hub's, and restores through the coordinator. Saved keys and A2A caller keys are lists the hub keeps; adding and removing act at once, and removing an agent key or caller key offers Undo from the checkpoint the hub returns. Values are never shown after they are sent.

## Places

**Home** (`places/home/`) reads the overview and hub stores directly; `home-model.ts` holds its wording. A failed agent's fixes (`FailedAgentActions.svelte`) are shared by Home's needs-you item and the Chat's state card, and pick the fix from `last_error.kind`. Agent actions run through `agent-actions.svelte.ts`, one per agent at a time. Delete asks first; Recently deleted restores from the deletion's checkpoint.

**Files and Shared files** are one place (`places/files/FilesPlace.svelte`) given a `FileSource`: an agent's workspace or the team folder. The tree follows the disk through the watch registry. Rename and Delete act at once; Delete offers Undo from the checkpoint it returned. A file opens in the context panel as a `FileBuffer` (`file-buffer.svelte.ts`), which saves with `If-Match` and handles save conflicts and changes on disk. Markdown and plain text open rendered, with Edit switching to the editor. An agent's `config/config.toml`, `providers.toml` and `mcp.json` save through the config coordinator, and the tree offers no Rename or Delete for them, because the coordinator neither moves nor deletes and the workspace checkpoint doesn't cover the config files.

**Inbox** (`places/inbox/`) is mail between the user and their agents, through the hub's mail routes and `lib/mail.svelte.ts`. The list sits beside the open thread or New message; when the place is too narrow for both (a container query on the place, `@container mail`, kept in sync with the layout tokens by `mail-layout.test.ts`) it shows one at a time. The store refetches when unread counts change, drops answers that predate a user action, and keeps drafts in memory (one New message, one reply per thread). Attachments are a `StagedAttachments` (`lib/staged-attachments.svelte.ts`, shared with the chat composer): each file uploads at once, with retry and remove, and a send waits for uploads.

**Workbench** (`places/workbench/`) is a launcher: an artifact opens in its own tab on the artifacts origin, never embedded. `lib/workbench.ts` resolves that origin (the relay's when viewed through the relay, else this host on the artifacts port over plain HTTP, else the reason it can't). The list rereads on `artifact_updated`, `artifact_removed` and `hub_boot`. An artifact's running sessions are the live sessions whose source is `artifact:<name>`.

**Schedule** (`places/schedule/`) shows the bound agent's pulses and scheduled actions from `lib/scheduled.svelte.ts`. The store can't import `ws` (the coordinator resets it), so the place hands it frames and the watch registry with `startWatching`. It watches `HEARTBEAT.yml` and `scheduled_actions.json`, merges `pulse_fire_changed` frames at once, and drops answers from loads older than the newest applied. Schedule routes answer only while the agent runs.

**Activity** (`places/activity/`) lists the bound agent's live runs, its tasks on other agents, and finished runs paged by kind, from `lib/sessions.svelte.ts`. New session opens `panel=new-session:<agent>`; the panel hands over to the run's session panel once `session_started` arrives, or takes the task back on `session_start_failed`.

**The session panel** (`SessionPanel.svelte`, state in `lib/session-run.svelte.ts`) shows a run on any agent. The transcript loads over HTTP; live frames come through the hub socket's session relay (`subscribe_session`), resubscribed on each `hub_boot` and reread after acknowledgement or `session_relay_lagged`. A message to a finished run starts a new run, which the panel follows into the URL. Messages wait in a `waiting` list until the session reads them.

## The chat

`places/chat/ChatPlace.svelte` is the header, the feed, and the composer floating over the feed's foot. While the agent isn't running, the composer is replaced by a state card (`StateCard.svelte`) and the past conversation stays readable.

**The feed** (`feed/Feed.svelte`) draws any agent's conversation, the main chat or a session transcript. It takes the owning `agent` explicitly, so links and actions never assume the bound agent.

- **Turns.** `groupTurns` (`feed/turns.ts`) groups each turn's output into one block, in order: texts and the reply, with each run of tool calls between them as one activity segment. A change of `turnId` ends a turn. Turn ids aren't unique across older history, so a user or agent message starts a new turn unless it is flagged `midTurn` (it reached the agent while that turn ran). The feed store sets the flag from `user_message` frames, history conversion from position after a tool result.
- **Activity lines** (`feed/ActivityLine.svelte`) collapse to a summary ("Searched memory, read 2 files"), open to steps, and steps open to arguments and results. Labels come from one table in `feed/activity.ts` keyed by tool name; unknown tools read "Used <tool>". Timing exists only for steps the page watched, since history records none.
- **Streaming.** `text_delta` and `thinking_delta` pieces build a draft per (turn, model call) in `lib/feed-stream.ts`, applied once per animation frame; the complete message replaces the draft's text in the same item, so nothing remounts. `stream_restart` drops the call's drafts. Screen readers hear only complete replies (`lib/announce.ts`).
- **Failures.** An `error` frame naming a turn adds a `turn-failure` item with Try again. History has no record of failures, so a reload drops it.
- **Waiting messages.** A message sent while a turn runs waits in the store's `waiting` list (shown with Send now and Multitask) until its `user_message` frame places it where the agent read it, which is where history puts it after a reload.
- **Other senders.** Messages from other tabs, chat interfaces, teammates and the agent's sessions arrive as `user_message` frames (`agent_sender` for agents) and show live.
- **Observed turns.** `lib/observed-turns.svelte.ts` records what the page saw of each turn (timing, how it ended, gaps when it connected partway). A reconnect sends `resync_turn`, and the `turn_snapshot` answer is replayed through the normal frame handler, so a page opened mid-turn shows everything the turn did. The mock keeps the same journal (`mock/turn-journal.ts`).
- **Timestamps.** Messages carry a local timestamp with no zone. Live messages take the moment their frame arrived; archived episodes and transcripts show none, because history gives them placeholder times.
- **Markdown** goes through `renderMarkdown` (`lib/markdown.ts`) and `feed/Prose.svelte`. Only an agent's own files and `data:` images render as images; any other image becomes a link, because loading it could leak what the agent read or trigger a hub `GET`. The sanitizer also strips inline styles, forms, frames and media, and the hub's CSP blocks remote images too.

**Scrolling.** `FeedScroller` (`lib/feed-scroll.svelte.ts`) is the only code that moves the feed's scroll position, and it records every position it sets; any other position change is the reader's. The feed follows new content until the reader scrolls up, and follows again within 120px of the end, on End, on Jump to latest, or on sending. Glides run on `requestAnimationFrame` rather than browser smooth scroll, which script can't follow or reliably stop, and they use the frame timestamp so they still run while tests hold the clock. Pressing a control with `aria-expanded` keeps that control in place for half a second. The composer is the feed's `dock`; the feed measures it and reserves room so the last line stays clear.

**The composer** (`places/chat/Composer.svelte`, built on `composer/ComposerFrame.svelte`) sends on Enter (Shift+Enter for a newline; on touch screens only the Send button sends). A line is a command only when it starts with `/` plus a chat action's name (`lib/command-line.ts`); anything else, a pasted path included, is a message. Esc takes two presses to stop a reply, since Esc also closes menus and dialogs. Drafts persist per agent in local storage (`lib/composer-drafts.ts`); attached files persist per agent for the page's life. Files upload through `POST /api/hub/uploads` as soon as they're attached. When the agent refuses a message (`message_refused`), it goes back into the box with its files, which upload again.

**Model control.** `ModelControl.svelte` writes `models.main` through `configCoordinator.edit` and then asks the agent to `reload`, quietly (`quietReloads.expect()` in `lib/reload-notices.ts`), since the popover already says the change applies from the next reply. Reload notices are worded from the outcome the frame reports, never from its text.
