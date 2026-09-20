# Stopping a Turn

Every interface can stop the main agent turn currently in progress. This is distinct from `stop_agent` (see [Background Tasks](background-tasks.md)), which cancels a background **sub-agent** spawned via `subagent_spawn` — stopping a turn targets the one main-agent conversation loop, of which only one runs at a time.

## How to Stop a Turn

| Interface | How |
|-----------|-----|
| Web UI | A square stop button replaces send while a turn is generating and the composer is empty. Typing a message brings send back so a steering message can be sent instead. |
| Telegram / Discord | `/stop` |
| WebSocket clients | Send a `Cancel` message carrying the `reply_to` correlation id of the turn to stop |

If nothing is running, a stop request gets a friendly "nothing is running right now" reply rather than an error or silence.

## What Happens on Stop

1. **The in-flight model call is cancelled immediately.** Since a turn spends most of its time waiting on the model, this is where a stop is felt — well under a second, not at the next tool boundary.
2. **A tool call already executing runs to completion.** A stop never severs a tool mid-flight — no half-written files, no orphaned processes. The tool loop only checks for a stop between iterations, after any in-flight tool has returned.
3. **The turn ends gracefully**, following the same path a normal turn does: partial assistant output and tool results already produced this turn are persisted, `TurnEnded` is published, and memory/observation hooks run as usual.
4. **A system note is added to history** recording that the user stopped the turn, so the next turn knows the work above was cut short and doesn't blindly re-run or re-report it.

## Correlation and Staleness

A stop request can name the specific turn to stop (its correlation id) or, for interfaces that don't track ids, simply ask to stop "whichever turn is running." Since only one main-agent turn runs at a time, a request naming a specific id is honored only if it matches the turn actually in progress — a stop for a turn that already finished is ignored rather than reaching forward and cutting off a newer, unrelated turn.
