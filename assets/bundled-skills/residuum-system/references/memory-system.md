# Memory System

The memory pipeline converts conversation turns into searchable long-term memory through three stages: observation, reflection, and search.

## USER.md

Core facts only: durable identity/standing preferences, hard-capped at ~15 entries, replace-don't-append. Longer-form, evolving knowledge about the user lives in wiki pages instead — see the `wiki` skill.

**Promotion rule**: knowledge supported by a single episode becomes a wiki page with `status: draft`. It becomes `stable` — and, if it is a core fact, earns a `USER.md` entry — once a second, independent episode supports it; each supporting episode is listed in the page's `sources`. Both the `wiki` and `learner` skills apply this rule.

## Observer

Fires after agent turns when accumulated unobserved message tokens exceed a threshold. Two trigger modes:

- **Soft threshold** (`threshold_tokens`): starts a cooldown timer; fires when cooldown expires.
- **Force threshold** (`force_threshold_tokens`): fires immediately, bypassing cooldown.

The main agent's own cycle runs off the event loop in a background worker, so it never delays the next message, a stop, or a shutdown — at most one cycle runs at a time, and a trigger arriving mid-cycle coalesces into one follow-up rather than stacking. Memory can be "one step stale" as a result: a turn that starts before a still-running cycle finishes sees the pre-cycle context. The observer calls an LLM to extract a structured `Episode` from recent messages. Each episode produces three files under `memory/episodes/YYYY-MM/DD/`:

| File | Format | Contents |
|------|--------|----------|
| `ep-NNN.jsonl` | JSONL — line 1 is meta JSON, subsequent lines are serialized Messages | Full conversation transcript |
| `ep-NNN.obs.json` | JSON array of Observation objects | Extracted observations |
| `ep-NNN.idx.jsonl` | JSONL of IndexChunk objects | Interaction-pair chunks for search indexing |

After extraction, observations are appended to `memory/observations.json` and recent messages are cleared from `memory/recent_messages.json`. The narrative context is saved to `memory/recent_context.json`. If an embedding provider is configured, `.obs` and `.idx` files are embedded for vector retrieval.

The bundled `OBSERVER.md` also extracts **interaction signals** — corrections/pushback, process preferences, frustration and its cause, praise and what earned it — as declarative facts about what happened, never as instructions. This is the evidence the `learner` skill corroborates against for a `preference` signal.

Customize extraction guidance by editing `memory/OBSERVER.md`.

Extraction (the LLM call) and persistence (episode id allocation, writing files, indexing, embedding, the reflector check) are separate steps. Persistence always goes through the memory merge writer — a single serialized writer shared by the main agent's own observation flow and every agent session's completion, so episode numbering and log appends never race.

An automatic extraction or merge failure backs off exponentially (1m, 2m, 4m, ... capped at 1h) instead of re-attempting — and re-spending an LLM call — on every later threshold crossing while unobserved messages keep piling up; those messages are never discarded on a failed attempt, so they're picked up once it recovers. You're told once when a failure streak starts, in plain language, and once when it clears. The `/observe` chat command always attempts regardless of this backoff, and a working manual retry clears it for the automatic path too.

## Agent Sessions and Memory

Every agent session (a pulse, a scheduled action, a webhook, or a `subagent_spawn`/learner sub-agent) merges its own findings into this same global memory when it completes — background work is not a memory dead end. A session's transcript is checked against the same observer thresholds; crossing the force threshold mid-run stages observations locally until the run finishes. On completion a run skips producing an episode only if it staged nothing and either its final turn ended with `HEARTBEAT_OK` or its transcript is under `episode_skip_token_floor` (`[background]` config, default ~2000 tokens) — its transcript is kept in the session store regardless. Otherwise the staged and final observations merge as one episode tagged with the session's address, run id, and category, so search results stay traceable to their source. A session's narrative lives on its own episode, never in the shared `recent_context.json`.

If the process crashes mid-run, nothing is lost: the transcript is appended to durably after every model response and tool result, and any run that never finished goes through this same skip-check/extract/merge pipeline from its saved transcript at the next startup.

## Reflector

Fires when `memory/observations.json` exceeds its token threshold. Calls an LLM to merge and deduplicate the observations, then writes the compressed result back to `observations.json`. The results are identical in structure, just denser.

**Critical**: The reflector reads from and writes to `observations.json` only. It does **not** touch the wiki. These are completely separate systems.

The original observations are backed up to `observations.json.bak` before replacement. Empty LLM responses are rejected (the reflector will not destroy existing content).

An automatic reflection failure follows the same backoff and once-per-streak notice as the observer's own, above — the reflector's tracker is shared globally across the main agent and every session, since the observation log it compresses is itself global. The `/reflect` chat command bypasses the backoff and always attempts, and its outcome updates the same tracker.

Customize compression guidance by editing `memory/REFLECTOR.md`.

## Search

Use `memory_search` to query past observations, episode chunks, and wiki pages. When an embedding provider is configured, hybrid search (BM25 + vector similarity) is used automatically. Otherwise, BM25 keyword search only.

| Parameter | Type | Description |
|-----------|------|-------------|
| `query` | string | **Required.** Free-text search query (supports AND, OR, phrase queries). |
| `source` | string | `"observations"`, `"episodes"`, or `"wiki"`. Omit to search all three. |
| `date_from` | string | ISO date lower bound (inclusive). |
| `date_to` | string | ISO date upper bound (inclusive). |
| `episode_ids` | array | Filter to specific episode IDs (excludes wiki pages). |

Use `memory_get` to retrieve the full transcript of a specific episode by ID, or a session run's transcript by run id (exactly one of `episode_id`/`run_id`) — the run-id mode reads from the session store, so it also works on a run that produced no episode, or one that's still in progress. An unknown run id points at `list_agents` and `memory_search`.

Episodes are indexed after each observer extraction and synced on startup. Wiki pages are resynced before every search, so a page you just wrote is searchable immediately; a wiki result's ID is the page path to open with `read_file`. Wiki pages are exempt from temporal decay.

A workbench artifact runs the same search via `GET /api/memory/search?q=<query>&limit=<1..50, default 10>&source=observations|episodes|wiki&date_from=&date_to=` (no `episode_ids` filter). It answers `{ results: [{ id, source, episode_id, date, line_start, line_end, snippet, score }], semantic }`, `semantic` saying whether vector search contributed. A blank `q`, an unrecognized `source`, or a malformed date answers `400`.

## Gotchas

- Episode IDs are zero-padded to 3 digits (`ep-001`, `ep-012`). The next ID is determined by scanning existing files for the highest number.
- `recent_messages.json` persists unobserved messages across restarts — there is no watermark.
- Observations have a `visibility` field (`User` or `Background`) that tracks their origin.
- The observer and reflector have independent model assignments via `[models] observer` and `[models] reflector` in config.toml. Unset roles fall back to `default`.
