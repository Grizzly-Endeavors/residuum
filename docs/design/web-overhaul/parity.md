# Web UI Overhaul — Capability Parity

> **Status:** draft, pending owner sign-off. Design: [`design.md`](./design.md). Work units: [`phases.md`](./phases.md).

Every capability the current web UI offers, grouped by the surface that has it today, with where it lives after the overhaul. A work unit that replaces a surface checks off that surface's items. Cutover requires every item to be checked or marked **Changed** / **Dropped** with the reason given here. **Fix** marks a known defect in the current UI that the replacement must not carry over.

## Chat feed → Agent Chat (design §4; W22, W23, W24)

- [ ] User message text with inline image thumbnails
- [ ] Sender line for messages from another interface ("name · interface · location") and from a workbench artifact
- [ ] Agent replies as sanitized Markdown (GFM, line breaks) — **Changed:** unboxed prose; code blocks gain a copy button
- [ ] Session/teammate message cards: kind, sender, clamped body with Show all / Show less, Open session — **Fix:** Open session appears for session senders only, not teammates (`agent:<name>`)
- [ ] Inline output for chat-scoped actions (help, status, conversation size) — **Changed:** status and help open as dialogs; conversation size opens in the context panel
- [ ] Status lines with expandable details (session views)
- [ ] Day dividers; episode dividers ("ep-NNN · date") above lazily loaded episodes
- [ ] Compressed-history marker with an explanation — **Changed:** explained in plain words inline
- [ ] Tool calls with per-tool argument summaries and shaped results (text, JSON, file with gutter, list; long results collapse) — **Changed:** inside the activity line's step details; always available, no `/verbose`
- [ ] Agent file attachments: caption, inline image, audio player, download with filename and size
- [ ] Empty state — **Fix:** exactly one empty state (today two render)
- [ ] Lazy loading of older episodes near the top, with a loading slot, stable scroll anchor, and fill-until-overflow
- [ ] Newest episode loads with recent history so the compressed marker shows from the start
- [ ] Follow new content at the bottom; stop following when scrolled up
- [ ] Jump-to-latest pill showing the topmost visible divider's label
- [ ] Sending scrolls to the bottom
- [ ] Reconnect reconciliation: missed messages merge in; otherwise reload that keeps the in-flight turn and re-anchors a scrolled-up reader
- [ ] Undo this turn on user messages whose turn changed files; reports reverted and skipped paths
- [ ] Live turn indicator: elapsed, output, tool-call count, stop hint — **Changed:** part of the live activity line; token figures move to the conversation-size view
- [ ] Main history hides background turns unless they started with an agent message

## Composer → Agent Chat composer (design §4; W25, with the action registry from W21)

- [ ] Auto-growing input; Enter sends, Shift+Enter new line
- [x] Slash autocomplete when `/` is the first character: arrow keys, Tab completes, Enter runs, Esc closes; toolbar button toggles the full list; click outside closes — **Changed:** draws from the action registry
- [x] `/help` — **Changed:** Keyboard shortcuts dialog / palette
- [ ] `/verbose` — **Dropped:** tool activity is always shown, collapsed by default
- [x] `/status` — **Changed:** palette action "Show connection status"
- [x] `/observe`, `/reflect`, `/reload`, `/stop` — **Changed:** registry actions with plain labels ("Summarize older messages now", "Condense memories now", "Reload settings", "Stop reply"), still reachable from `/`
- [x] `/context` — **Changed:** "Show conversation size" opens the context panel
- [x] `/inbox <text>` — **Changed:** action "Add a note to <agent>'s inbox" with a text prompt
- [x] Unknown-command error
- [ ] Image attach by button, paste, drag-and-drop with highlight; JPEG/PNG/GIF/WebP ≤ 5 MB; rejection message; removable thumbnails; image-only sends
- [ ] Send becomes Stop while running and empty; typing restores Send for mid-turn steering
- [ ] Esc stops a running turn while the composer has focus
- [x] Feedback entry point — **Changed:** help menu and palette
- [ ] Model control: current main model, choose from the main provider's models, writes `models.main` keeping overrides, then reloads — **Fix:** refreshes after settings changes; goes through the config write coordinator
- [ ] Thinking control: Off / Low / Med / High; clicking the active level clears it — **Fix:** refreshes after settings changes
- [ ] Offline composing: messages queue with "Reconnecting — N messages will send once back online" (running agents only)
- [ ] Drafts — **Fix:** kept per agent across navigation and reload

## Chat footer and thinking indicator → Agent Chat (design §4; W24, W25)

- [ ] Main model label — **Changed:** shown in the composer's model control
- [ ] Session input/output tokens, tool calls, context size — **Changed:** conversation-size view in the context panel
- [ ] Post-turn activity ("updating memory…", "reviewing turn…") — **Changed:** quiet status line under the last reply
- [ ] Usage seeded on connect and updated per turn
- [ ] Live elapsed timer, output tokens, tool calls, stop hint (main chat only)

## Sessions sidebar and session view → Activity + context panel (design §5; W26)

- [ ] Live count badge — **Changed:** running-sessions pill in the chat header and the Activity place
- [ ] Narrow-screen drawer behaviors (focus trap, inert page, Esc) — **Changed:** Activity is a place; the panel is a sheet on phones
- [ ] Categories external / scheduled / spawned / artifact with descriptions — **Changed:** plain-language kinds in one Running list and a filtered Finished list
- [ ] Finished paging with "N+" counts and Show older
- [ ] Row: kind, source, duration, purpose, state, outcome, start time, last error, artifact link, Stop (forking / queued / running / idle), selection
- [ ] Outbound tasks: agent, time since sent, status text, state (sent / working / waiting on reply / waiting on sign-in / can't reach for N, still retrying), Stop, Stop watching fallback
- [ ] Load errors with Try again; loading state
- [ ] Session view: back to chat, Stop session, heading focus on open, tags, purpose, details (kind, started by with links, depth > 1, remembered as, run id), notes (interrupted, failed with details, overlap)
- [ ] Transcript with loading, error with Try again, empty state, live frames (buffered and deduplicated during load), follow and Jump to latest
- [ ] Transcript status lines: outcome, stopping, delivery result, send/stop failures
- [ ] Message a session; resume a finished one ("Message this session to start it again…"); follow a resumed session into its new run
- [ ] Undo this turn in session transcripts
- [ ] Results as toasts when the session isn't open
- [ ] Open session from an agent message by address (live first, else looked up)

## Workspace → Files, Shared files, context panel (design §5; W31)

- [ ] Agent workspace and team files
- [ ] Lazy tree with expand/collapse and empty directories
- [ ] Identity-file tint — **Fix:** also applies in the team tree
- [ ] Per-file History, Rename (inline; `/` moves), Delete (immediate with Undo)
- [ ] Editor: filename, path, modified badge, Save / Discard when dirty; save toast with problems note
- [ ] Live validation and diagnostics
- [ ] Unsaved-edit prompt when switching files — **Fix:** also when leaving the place, closing the panel, or reloading
- [ ] Save-conflict dialog: reload and discard, or overwrite
- [ ] Phone: full-screen editor with back
- [ ] File history: checkpoints (≤ 100), relative time, trigger, summary, auto-select newest, diff, view full content, restore
- [ ] Team files have live updates — **Fix:** today the team view never subscribes to team change frames

## Workbench → Workbench launcher and standalone artifacts (design §5, §9.8–§9.10; W12b–W12d, W32a, W32b)

- [ ] List with loading, error with Try again, empty explanation; title, path, edited time (refreshes every 30 s); agent-edit glow "updating now" — **Fix:** one path format (`/team/workbench/<name>`); **Changed:** refreshes on hub artifact events, so it works with no agent running
- [ ] Open an artifact — **Changed:** opens on the artifacts origin in its own tab, never inside the app; Copy link added
- [ ] Delete with Undo (page and data files)
- [ ] Artifacts-unavailable warning — **Fix:** shows even when the list is empty, and covers the HTTPS-without-relay case (#308)
- [ ] Artifact bar (back, title, path), Stop page, Restart, Full view (button and F), Reload, deleted notice — **Dropped:** the page owns its own window; closing the tab unloads it, and the browser's reload and full screen apply
- [ ] Activity panel: the artifact's live sessions (open, stop) — **Changed:** in the Workbench row and detail, for sessions on any agent (#292); opening one shows it in the context panel
- [ ] In-flight model call count and Cancel calls — **Dropped:** a page manages its own calls
- [ ] Live reload on agent edits — **Changed:** the SDK reloads the page itself, unless the page handles `artifact_updated`
- [x] Unknown artifact — **Fix:** the route redirects to the list with a toast
- [ ] SDK: `fetch`, `ask`, `on`, `watch`, `state.get`/`set`, `sessions.start` with handle `on`/`send`/`stop`, connection events, `features`, `artifact`, `version` — **Changed:** direct access from the page's own origin; no implicit agent (agent-specific calls name their agent, with `agent(name)` added); `embedded`, `ready` and Esc forwarding removed; **Fix:** session frames arrive for sessions on any agent (#292); no reply crosses documents, because the bridge is gone (#307). Artifacts never shipped in a release, so no migration applies.
- [ ] Unscoped fetch paths: hub prefixes to the hub, `/api/workbench/` to team — **Changed:** agent paths must name the agent; one that doesn't gets a 400 with a clear error
- [ ] `watch` of `team/` prefixes, resync on reconnect — **Changed:** agent-workspace watches go through `agent(name).watch`
- [ ] Request lanes: 8 ordinary, 4 model calls per page; retry on the relay's "agent overloaded" 503 — kept, in the SDK
- [ ] Request limits — **Changed:** artifacts can call everything the UI can, except shutdown, stop-all, updates and setup, enforced where the artifacts origin forwards the API
- [ ] Artifacts origin and relay workbench host — **Changed:** they also forward the API and sockets (relay-project change)

## Scheduled → Schedule place (design §5; W30)

- [x] Reload
- [x] Pulses: enable toggle (disabled while pending or unscheduled), running and overlap badges, schedule, active hours, skill, next run, last result with error, problems — **Changed:** names and next-run times read as on Home ("Inbox check", "in 34m", "Due now"); the overlap is a line saying when the overlapped run started; the switch moves at once and back on failure; a notice explains when no enabled pulse will run
- [x] Actions: running badge, due time, skill, Cancel
- [x] Empty states explaining the agent creates these
- [x] Refetch on schedule-file changes and scheduled-session frames — **Fix:** the place owns its watch on the two files instead of relying on another view's
- [x] **Fix:** a failed load shows the error and Try again, never the empty state; a stopped or failed agent shows that it isn't running, with Start, instead of an error

## Team page → Home + Settings (design §6, §8; W27, W28, W38)

- [x] Agent state glyph, name link, state label, working chip, unread chip, role line or "No role page yet", last error with time — **Changed:** Home's agents board (a working agent's dot pulses and its Now line says how long it has been working); the last error is a needs-you item with its time, a plain-language line and the error behind Details
- [x] A2A card visibility per agent — **Changed:** set only in Settings → agent → Advanced → Agent-to-agent, "Who can find <agent>", which applies at once through the hub (no Save) and goes back to the hub's value on failure. Home no longer has it.
- [x] Start automatically toggle, reverting on failure — **Changed:** Home row menu, a checkbox item that shows the value being saved and then the hub's (the stopped state card's toggle is W23's)
- [x] Start / Stop / Restart with disabled reasons and pending labels — **Changed:** Home row menu, whose heading names the agent's state; an action the state doesn't take stays reachable but disabled, and the one in flight reads "Starting…", "Stopping…" or "Restarting…" while the row's state shows it too (state cards are W23's; needs-you Restart is W27's)
- [x] Delete with confirmation, then checkpoint note with Undo and Dismiss — **Changed:** Delete in the row menu asks first (a running agent stops first, and the question says so), then the hub's "You deleted X." toast carries Undo and dismiss; a deletion that took no checkpoint says so in an error; Recently deleted keeps Restore after the toast is gone. The checkpoint id is no longer shown.
- [x] Recently deleted with Restore, load error with Try again — **Changed:** collapsed disclosure under Home's board, shown once there is something to restore or a failed load
- [x] Create agent: live-validated name, description, copy model settings from, visibility; "Created X" — **Changed:** Create agent dialog (a sheet on phones) from Home's New agent, the rail's "+" and the palette; copy-from and "Who can find it" under More options; the hub's "You created X." toast, and the new agent's rail row takes focus beside the main region
- [x] Hub-wide toasts for created / restored / deleted (with Undo), failures and notices (team events never add toasts)

## Header, agent switcher, notifications, inbox, feedback, help → Shell (design §2, §7; W15b, W19, W21, W29)

- [x] Menu destinations (Chat, Workspace, Workbench, Scheduled, Agent settings, Team, Team files, Hub settings) — **Changed:** rail and bottom bar
- [x] Connection status text — **Changed:** shown only when degraded
- [x] Inbox button with unread badge — **Changed:** rail and bottom bar, across agents
- [x] Bug report entry — **Changed:** help menu and palette
- [x] Agent chips: state, busy dots, unread (99+), failed-agent error tooltip, current highlight, arrow-key movement — **Changed:** rail agent rows (accordion) with a working indicator and a 99+ chat-unread badge; Up/Down move between rows; the error shows on Home and in the state card; an unknown agent in the URL redirects to Home with a toast
- [x] Switching agents keeps the kind of page — **Changed:** agent places are explicit routes; the rail accordion links to them
- [x] Hub offline note — **Changed:** hub banner
- [x] Toasts: info/success auto-dismiss (4 s, 10 s with action), errors sticky, dismiss, action button
- [x] Recent notifications history with details, clear and Undo — **Changed:** a Recent notifications dialog from the help menu and palette
- [x] User inbox: unread count, tabs, read on open, body and attachment downloads, archive, restore, empty states — **Changed:** one cross-agent Inbox place with live counts from the overview (no polling); bodies render as Markdown; load errors are shown, not swallowed
- [x] Feedback dialog: bug and feedback tabs with drafts kept, required fields, severity, receipt with public id and Copy, friendly errors
- [x] Help overlay with shortcuts and commands — **Fix:** lists "Esc stops a running reply" and describes `/` accurately

## Settings → Settings modal (design §8; W33–W41)

- [x] Title and scope note — **Changed:** scope picker with "Applies to…" line
- [x] Saving / saved / saved-with-problems / reloaded status — **Changed:** save bar and results
- [x] Reload from disk with discard confirmation when there are unsaved changes
- [ ] Simple / Advanced / Raw — **Dropped:** Advanced sections, "More options" disclosures, and a Raw config section replace it
- [x] Section navigation and old-URL redirects — **Changed:** new section ids and mapping
- [x] Autosave — **Changed:** explicit Save changes / Discard per scope; items with their own endpoints stay immediate (design §8)
- [x] Partial-failure message naming saved and failed files
- [ ] Secret fields: store on save; Stored securely with Change; from environment variable with Replace
- [x] Undo after removing providers, webhooks, MCP servers, skills/tools folders, cloud account — **Changed:** removals are staged (Discard brings them back); after Save, Undo restores the returned checkpoint
- [x] Scope isolation: an agent page never shows editable install-wide fields, and a save writes only its own scope's files (today the agent-page save computes and ignores an install-wide diff, which is empty only because no agent section renders install-wide fields)
- [ ] **Fix:** History restore and undo refresh the open form
- [ ] **Fix:** field-level validation errors inline where the server gives a path

All-agents scope:

- [ ] General: timezone; bind address and port
- [ ] Cloud: connected (Disconnect, hidden via tunnel with explanation), connecting (Cancel), disconnected with token (Reconnect, Remove account with Undo), not connected (Connect link, token entry); relay URL, local port — **Fix:** correct the "Reconnect then Save" hint, re-poll status after Connect/Reconnect, surface a failed Disconnect
- [ ] A2A listener: toggle, port, own address; caller keys list, Revoke with Undo, add with validation, token shown once with Copy
- [ ] Session budget: concurrent turns, hop soft and hard limits
- [ ] Tracing: log detail, redaction, automatic error reports
- [ ] Update: status, current, latest, checked time, Check now, Update & restart with progress, outcomes (updated, rolled back, timed out with hint), unverified-update warning
- [ ] Secrets: list, remove with confirmation, add with replace warning
- [ ] Agent keys: list with env var and "saved by the agent", remove with Undo, add with name/env preview, replace note, short-value hint, description, warning toast
- [ ] History: team and hub repos

Agent scope:

- [ ] Runtime: timeout, max tokens, default temperature, default thinking; subconscious (enabled, watch mid-turn, cadence, max transcript tokens, learn, cooldown); learning fallback; retry (max, initial delay, max delay, backoff); abilities (allow MCP changes, allow channel changes, max tool calls, repeat-call guard, steer after, stop after); idle (timeout, channel) — **Changed:** split across Model, Memory and Advanced → Runtime per design §8
- [ ] Providers: name, type, API key, base URL, keep-alive; remove with Undo; add
- [ ] Model roles: main, observer, reflector, pulse, subconscious, embedding, background small/medium/large; provider, model (live list or custom id), fallback warning, temperature, thinking — **Fix:** subconscious role's model list loads on open; failover lists are preserved; `models.default` gets a control or is shown read-only
- [ ] Discord, Telegram, Teams fields and warnings
- [ ] Pulses & sessions: pulse enabled, idle timeouts per kind, episode floor, depth cap
- [ ] Memory thresholds and search tuning
- [ ] Skills folders, tools PATH folders, web search backend and keys, native search overrides
- [x] MCP: list, remove with Undo, add stdio/http, catalog with inputs — **Fix:** a failed catalog fetch shows an error with Try again, not "Reading catalog." forever — **Changed:** Advanced → Tool servers; a server's command, arguments (one per line), variables, address and headers can be edited in place; adds, edits and removals are staged
- [x] A2A visibility and client: status, URL with Copy, relay note, listener warning, card error, visibility, remote agents with raw editor, card preview, open workspace — **Changed:** Advanced → Agent-to-agent; visibility applies at once; the install's listener shows read-only with a way to All agents; open workspace opens `agent-card.json` in the agent's files; a stopped agent asks to start for status, card and reachability, and lists `a2a.json`'s agents with its editor
- [ ] Webhooks: route preview, name, secret, routing, format, content fields, remove with Undo, add
- [ ] History: workspace and agent-config repos
- [ ] History browser (both scopes): repo toggle, path filter, stats, paged list, detail, undo checkpoint with reverted/skipped report, changed paths with diff, view file, restore, encrypted-store hint

## Setup wizard → restyled, same flow (W42)

- [x] Six steps with draft autosave (without secrets), restore on reload, cleared on completion
- [x] Welcome, Providers, Assign models, MCP servers, Integrations, Save & Start — contents unchanged
- [x] Back and Next on every step

## Global (W15b toasts, W17 routing, W19 connection and errors, W21 keyboard)

- [x] Keyboard: `?` help, Enter/Shift+Enter, slash-menu keys, Esc (stop reply, close overlays), rail keyboard navigation, focus trapping in overlays, Enter/Space on disclosures — **Dropped:** F for artifact full view (artifacts open in their own tab)
- [ ] Agent socket reconnect with backoff, ping, queued sends; resync of sessions, history, usage, workspace watch and workbench on reconnect
- [x] Hub socket offline handling
- [ ] Gateway reloading toast and cache invalidation
- [ ] Plain-language error messages for unreachable, 404, 401/403, 5xx and server messages; error frames as toasts with details; notices as info
- [x] Time-of-day vein intensity — **Dropped:** see design §1
- [x] Deep links and back/forward — **Changed:** route model in design §3 with redirects from every current URL
- [x] macOS notification "Open" link lands on the last-used agent's Files, as today
- [ ] Persisted preferences: last agent, API cache, setup draft — sidebar open state, settings mode and verbose are **Dropped** with the features they served
- [x] Reduced motion respected
