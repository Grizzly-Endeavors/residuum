# Stopping a Turn

Every interface can stop the main agent turn currently in progress. This is distinct from `stop_agent` (see [Background Tasks](background-tasks.md)), which cancels a background **sub-agent** spawned via `subagent_spawn` — stopping a turn targets the one main-agent conversation loop, of which only one runs at a time.

## How to Stop a Turn

| Interface | How |
|-----------|-----|
| Web UI | A square stop button replaces send while a turn is generating and the composer is empty. Typing a message brings send back so a steering message can be sent instead. |
| Telegram / Discord / Teams | `/stop` (in Teams, only the owner can run commands) |
| WebSocket clients | Send a `Cancel` message carrying the `reply_to` correlation id of the turn to stop |

If nothing is running, a stop request gets a friendly "nothing is running right now" reply rather than an error or silence.

## What Happens on Stop

1. **The in-flight model call is cancelled immediately.** Since a turn spends most of its time waiting on the model, this is where a stop is felt — well under a second, not at the next tool boundary.
2. **A tool call already executing runs to completion.** A stop never severs a tool mid-flight — no half-written files, no orphaned processes. The tool loop only checks for a stop between iterations, after any in-flight tool has returned.
3. **The turn ends gracefully**, following the same path a normal turn does: partial assistant output and tool results already produced this turn are persisted, `TurnEnded` is published, and memory/observation hooks run as usual.
4. **A system note is added to history** recording that the user stopped the turn, so the next turn knows the work above was cut short and doesn't blindly re-run or re-report it.

## Correlation and Staleness

A stop request can name the specific turn to stop (its correlation id) or, for interfaces that don't track ids, simply ask to stop "whichever turn is running." Since only one main-agent turn runs at a time, a request naming a specific id is honored only if it matches the turn actually in progress — a stop for a turn that already finished is ignored rather than reaching forward and cutting off a newer, unrelated turn.

## Tool-Call Limit

There is no built-in cap on how many tool calls a turn may make. A user can bound this themselves with `max_tool_iterations` under `[agent]` in `config.toml` (also editable from the web UI's Settings page); left unset, a turn's tool loop runs unlimited, and the stop mechanisms above are the intended safety valve for a runaway turn. This applies to every turn loop: the main agent's own turns and every background/session/artifact turn.

When a configured `max_tool_iterations` is reached, the turn ends the same graceful way a user-initiated stop does — partial assistant output and tool results already produced are kept — except the agent's own final reply explains what happened: that it stopped after that many tool calls because of the configured limit, and how to raise or remove it. This is a normal turn completion, not an error: it produces a response like any other turn and does not surface as a system error notification. A `warn`-level log records the tool-call count whenever this limit is hit.
