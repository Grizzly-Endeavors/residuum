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

1. **The in-flight model call is cancelled immediately.** Since a turn spends most of its time waiting on the model, this is where a stop is felt — well under a second, not at the next tool boundary.
2. **A tool call already executing runs to completion.** A stop never severs a tool mid-flight — no half-written files, no orphaned processes. The tool loop only checks for a stop between iterations, after any in-flight tool has returned.
3. **The turn ends gracefully**, following the same path a normal turn does: partial assistant output and tool results already produced this turn are persisted, `TurnEnded` is published, and memory/observation hooks run as usual.
4. **A system note is added to history** recording that the user stopped the turn, so the next turn knows the work above was cut short and doesn't blindly re-run or re-report it.

## Correlation and Staleness

A stop request can name the specific turn to stop (its correlation id, as a WebSocket `Cancel` does) or, for a chat command that doesn't track ids, simply ask to stop "whichever turn is running" at its target. Either way, a request only ever applies to a turn that is actually active when the request is *handled* — never one that starts afterward, however briefly after. This holds by construction, not by timing, and the two targets get there differently:

- **A conversation session** is stopped through the session registry's synchronous stop-if-running check: whether a turn is running is read and, if so, cancelled under the same lock, with no channel for the request to sit in. There is no window in which the check could be stale by the time it takes effect.
- **Main** is stopped by sending the request down a channel the active turn's own loop watches, so a request that arrives mid-turn is applied (or, naming a since-finished turn's id, ignored) as soon as it's read. The channel is also drained on receipt at every point where nothing could legitimately be waiting on it — while genuinely idle, and again right before a new turn starts — so a request that arrives in the gap between two turns is answered "nothing running" there rather than being left to be misread, once the next turn's loop starts watching the same channel, as a request for that unrelated new turn.

Either way, a request that finds nothing running is discarded once answered; it never lingers to affect whatever runs next.

## Tool-Call Limit

There is no built-in cap on how many tool calls a turn may make. A user can bound this themselves with `max_tool_iterations` under `[agent]` in `config.toml` (also editable from the web UI's Settings page); left unset, a turn's tool loop runs unlimited, and the stop mechanisms above are the intended safety valve for a runaway turn. This applies to every turn loop: the main agent's own turns and every background/session/artifact turn.

When a configured `max_tool_iterations` is reached, the turn ends the same graceful way a user-initiated stop does — partial assistant output and tool results already produced are kept — except the agent's own final reply explains what happened: that it stopped after that many tool calls because of the configured limit, and how to raise or remove it. This is a normal turn completion, not an error: it produces a response like any other turn and does not surface as a system error notification. A `warn`-level log records the tool-call count whenever this limit is hit.
