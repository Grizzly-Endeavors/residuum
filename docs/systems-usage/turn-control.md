# Stopping a Turn

Every interface can stop a turn currently in progress: the main agent's own turn, or — on Discord, Telegram, and Teams — the turn of whichever [conversation session](background-tasks.md#conversation-routing) that interface's `/stop` was typed into. This is distinct from `stop_agent` (see [Background Tasks](background-tasks.md)), the tool an agent calls to cancel a session by address — the controls on this page are user-facing, typed or clicked from an interface, not called by the agent.

## How to Stop a Turn

| Interface | How | Targets |
|-----------|-----|---------|
| Web UI | A square stop button replaces send while a turn is generating and the composer is empty. Typing a message brings send back so a steering message can be sent instead. Pressing Escape while the composer has focus also stops the turn — scoped to the composer rather than a global shortcut, since several other overlays (modals, drawers, the command menu, the shortcuts overlay) already own Escape while they're open with no shared priority between them. | Main, always. |
| Telegram / Discord / Teams | `/stop` (in Teams, only the owner can run commands) | The same conversation an ordinary message from the same chat would reach: main for the owner's own DM, otherwise that conversation's own session — see [Conversation Routing](background-tasks.md#conversation-routing). Never main for a group chat or channel, even when the owner is the one who typed it. |
| WebSocket clients | Send a `Cancel` message carrying the `reply_to` correlation id of the turn to stop | Main, always. |

Every chat interface restricts commands (including `/stop`) to the owner, so this is about *where* the owner's own `/stop` lands, not about who else could send it.

If nothing is running in the target — main, or the conversation's session — the request gets a friendly "nothing is running right now" reply rather than an error or silence, and nothing is touched: see [Correlation and Staleness](#correlation-and-staleness).

## What Happens on Stop

A stop cancels work in progress immediately — it never waits for a tool boundary, a batch of tool calls, or the model to finish.

1. **The in-flight model call is cancelled immediately.** well under a second, not at the next tool boundary.
2. **A tool call already executing is interrupted, not run to completion.** The tool's own dispatch races against the stop: a built-in tool's `execute()` future is dropped the moment the stop lands, and `exec` additionally kills the command's whole process tree (see [Killing exec's Process Tree](#killing-execs-process-tree) below) so nothing is left running orphaned. Every other tool call still in that response's batch is skipped rather than started. Either way, the tool call still gets a result recorded in the transcript — worded as a cancellation, not a failure, so the model doesn't read it as something having gone wrong — so the transcript stays valid for the next provider call.
3. **The turn ends gracefully**, following the same path a normal turn does: partial assistant output and tool results already produced this turn — including the cancellation results from step 2 — are persisted, `TurnEnded` is published, and memory/observation hooks run as usual.
4. **A system note is added to history** recording that the user stopped the turn, so the next turn knows the work above was cut short and doesn't blindly re-run or re-report it.

The user sees which tool was interrupted and how many calls were skipped through the same tool-activity stream every tool call already publishes (call and result events) — a skipped call publishes both events too, with the result naming the cancellation, so a streaming client's live activity feed shows exactly what a stop cut short without needing a dedicated event of its own.

## Killing exec's Process Tree

`exec` runs its command via a shell (`sh -c` on Unix, `cmd /S /C` on Windows), so killing only that shell on a stop or a timeout would leave anything it spawned running orphaned. To kill the whole tree instead:

- **Unix**: the shell is spawned in its own process group (`pid == pgid`), and the group is sent `SIGKILL` via `killpg` — reaching the shell and everything it forked, never residuum's own process group.
- **Windows**: `taskkill /T /F /PID <pid>` is run against the shell's pid — `/T` walks the OS's own parent/child bookkeeping to kill the whole tree, since Windows has no direct equivalent of a process group here.

Either way, whatever stdout/stderr the command had already produced is kept and returned with the timeout or cancellation notice, instead of being discarded — the output pipes are drained on their own tasks the whole time the command runs, so a kill only stops new output, not what's already captured. See [`exec`](../../src/tools/TOOLS.md) for the exact message shapes.

This applies per tool call, not per command in general: a planned feature (tracked as a `gh` issue) will let `exec` start a command that outlives the tool call and hands it off elsewhere (e.g. a background session). That command is not killed by a turn stop, because the kill is tied to the tool call's own lifetime, not to the process it started.

## Daemon Shutdown and Restart

Stopping the gateway — `residuum stop`, a SIGTERM, the HTTP `/api/shutdown` request, or a restart triggered by an update — cancels whatever turn is currently running the same way a user stop does, persisting its partial state identically, before the gateway actually shuts down or re-execs. A turn blocks the event loop's own event processing for its whole duration, so this is watched for from inside the turn's own loop rather than the event loop's — otherwise the shutdown trigger would sit unobserved until the turn finished on its own, which is what made `residuum stop` give up with "did not stop" on a long-running turn. `residuum stop` now succeeds promptly regardless of whether a turn is running.

## Correlation and Staleness

A stop request can name the specific turn to stop (its correlation id, as a WebSocket `Cancel` does) or, for a chat command that doesn't track ids, simply ask to stop "whichever turn is running" at its target. Either way, a request only ever applies to a turn that is actually active when the request is *handled* — never one that starts afterward, however briefly after. This holds by construction, not by timing, and the two targets get there differently:

- **A conversation session** is stopped through the session registry's synchronous stop-if-running check: whether a turn is running is read and, if so, cancelled under the same lock, with no channel for the request to sit in. There is no window in which the check could be stale by the time it takes effect.
- **Main** is stopped by sending the request down a channel the active turn's own loop watches, so a request that arrives mid-turn is applied (or, naming a since-finished turn's id, ignored) as soon as it's read. The channel is also drained on receipt at every point where nothing could legitimately be waiting on it — while genuinely idle, and again right before a new turn starts — so a request that arrives in the gap between two turns is answered "nothing running" there rather than being left to be misread, once the next turn's loop starts watching the same channel, as a request for that unrelated new turn.

Either way, a request that finds nothing running is discarded once answered; it never lingers to affect whatever runs next.

## Mid-Turn Interrupt Queue

A stop request, a mid-turn user message, an agent message addressed to this turn, and a subconscious correction are all delivered the same way: queued on the turn's interrupt channel and drained at the tool loop's next checkpoint (see [What Happens on Stop](#what-happens-on-stop) above for the stop case specifically). The main agent's own channel is unbounded, so a message sent mid-turn is never dropped for want of queue space — there is no "busy" signal to give the user back if it were, so a dropped message would just silently vanish. A session's own interrupt channel stays bounded on purpose: an agent-to-agent message (`message_agent`) addressed to a *running* session that's stuck reports itself busy back to the sending agent rather than accepting unbounded backlog, a deliberate, visible signal rather than a silent drop — see [Messaging](background-tasks.md#messaging). A chat user's message routed to a conversation session's same channel is never refused this way, even when it's saturated: there's nobody on the other end of a chat message to hand a busy refusal to, so delivery retries on a detached task until the channel drains — see [Conversation Routing](background-tasks.md#conversation-routing).

## Output Truncation

When a model response is cut off by the provider's own output-token limit rather than ending naturally, the turn treats it as a completed (not failed) turn, but publishes a notice naming the limit and adds a system note to the transcript — `[Truncated] your previous response was cut off at the N-token output limit` — so the next turn's model call knows its own last response was incomplete. There is no automatic continuation: whether to pick up where it left off, and how, is left entirely to the agent's own judgment on its next turn, the same as any other turn decision.

## Tool-Call Limit

There is no built-in cap on how many tool calls a turn may make. A user can bound this themselves with `max_tool_iterations` under `[agent]` in `config.toml` (also editable from the web UI's Settings page); left unset, a turn's tool loop runs unlimited, and the stop mechanisms above are the intended safety valve for a runaway turn. This applies to every turn loop: the main agent's own turns and every background/session/artifact turn.

When a configured `max_tool_iterations` is reached, the turn ends the same graceful way a user-initiated stop does — partial assistant output and tool results already produced are kept — except the agent's own final reply explains what happened: that it stopped after that many tool calls because of the configured limit, and how to raise or remove it. This is a normal turn completion, not an error: it produces a response like any other turn and does not surface as a system error notification. A `warn`-level log records the tool-call count whenever this limit is hit.

## Running-Turn Indicator and Usage Totals

The web UI shows elapsed time and token progress for a turn while it runs, and cumulative session totals once it's done — quietly, for the person watching, and never for the agent: none of this reaches the agent's status line, context, or tool results, since a visible token/time budget would push it to stop long-running work early (this is the same reasoning behind [Tool-Call Limit](#tool-call-limit) above having no built-in cap).

**The running-turn indicator** replaces the bare "Thinking…" in both the main chat and an open session view with one quiet line, e.g. `Thinking… 1m 12s · ↓ 4.3k tokens · Esc to stop` (the session view omits the Esc hint, since it has its own visible Stop button instead). The elapsed clock ticks client-side from when the turn started; the token count is this turn's output tokens summed across every model call so far, updated after each call via a `turn_usage` frame (main) or `session_turn_usage` frame (a session), both carrying `output_tokens` and `has_usage`. When a call reports no usage (`has_usage: false`), the indicator shows the elapsed clock alone rather than a misleading `0 tokens`.

**The chat footer** is a quiet status line — model, session tokens in (`↑`) and out (`↓`), and context size — shown under the main chat's composer and in an open session view, updated after every model call. It reads `input_tokens`/`output_tokens` (cumulative for the whole session) and `context_tokens` (the most recent call's input tokens plus its cache reads/writes — the actual size the provider processed, not an estimate) from the same `turn_usage`/`session_turn_usage` frames' `session_totals` field. There is no per-model context-window size known to the codebase (no config or provider metadata carries one), so context size is always shown as an absolute token count, never a percentage of a window. A session view's footer omits the model segment: a session run has a model *tier* (small/medium/large), not one resolved model string, so there's nothing honest to show there.

**Persistence.** The main chat's totals live in `memory/usage_totals.json` in the workspace, written through after every model call and read back at startup so a restart doesn't reset the footer to zero; `GET /api/usage` serves the same file for the web client to seed the footer on connect/reconnect, the same way `GET /api/chat/history` seeds the feed from `recent_messages.json`. A session's totals live on its registry entry while it runs (mirrored into `SessionSummary.usage`, which both the live listing and a completed run's store record carry) and are copied into its store record at completion, so a finished run's session view still shows correct totals.

**Degradation.** A provider that reports no usage for a call leaves the numeric fields unchanged rather than showing a `0` or erroring, and is logged at `debug` — this can happen per call, not just per provider, so a session's or the main chat's totals may simply stop advancing for a stretch without any error surfacing.

## Repeat-Call Guard

An observed failure — GLM 5.3 Flash calling a tool with byte-identical arguments hundreds of times in a row, spinning instead of finishing — is guarded against directly, in every turn loop (main and background/session/artifact turns alike). The turn tracks a streak of consecutive calls whose tool name and raw argument JSON exactly match the previous call; any different call (a different tool, or different arguments) resets the streak. Calls within one model response's batch count in order, the same as calls from separate model responses.

- At `repeat_call_steer_after` (default `3`) consecutive identical calls, the call still runs, but its own result carries an appended note telling the model the exact call has repeated and to try something different or finish.
- At `repeat_call_stop_after` (default `6`), the call is not run at all: it gets a cancelled-style result explaining why, every remaining call in that batch is skipped the same way a user stop skips them, and the turn ends with a plain-language reply naming the repeated tool and how many times it repeated, plus which config setting to raise if the repetition was actually expected. A `warn`-level log records the tool name and streak length; the steering note at the lower threshold logs at `debug`.

Both thresholds are configurable under `[agent]` in `config.toml` (also editable from the web UI's Settings page), and the guard can be turned off entirely with `repeat_call_guard_enabled = false`.
