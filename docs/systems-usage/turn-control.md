# Stopping a Turn

Every interface can stop a turn currently in progress: the main agent's own turn, or — on Discord, Telegram, and Teams — the turn of whichever [conversation session](background-tasks.md#conversation-routing) that interface's `/stop` was typed into. This is distinct from `stop_agent` (see [Background Tasks](background-tasks.md)), the tool an agent calls to cancel a session by address — the controls on this page are user-facing, typed or clicked from an interface, not called by the agent.

## How to Stop a Turn

| Interface | How | Targets |
|-----------|-----|---------|
| Web UI | A square stop button replaces send while a turn is generating and the composer is empty. Typing a message brings send back so a steering message can be sent instead. | Main, always. |
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

## Tool-Call Limit

There is no built-in cap on how many tool calls a turn may make. A user can bound this themselves with `max_tool_iterations` under `[agent]` in `config.toml` (also editable from the web UI's Settings page); left unset, a turn's tool loop runs unlimited, and the stop mechanisms above are the intended safety valve for a runaway turn. This applies to every turn loop: the main agent's own turns and every background/session/artifact turn.

When a configured `max_tool_iterations` is reached, the turn ends the same graceful way a user-initiated stop does — partial assistant output and tool results already produced are kept — except the agent's own final reply explains what happened: that it stopped after that many tool calls because of the configured limit, and how to raise or remove it. This is a normal turn completion, not an error: it produces a response like any other turn and does not surface as a system error notification. A `warn`-level log records the tool-call count whenever this limit is hit.
