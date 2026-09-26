# Memory System

The memory system gives the agent persistent recall across conversations. It has several distinct components that serve different purposes.

## Components

### Knowledge Wiki

Distilled long-term knowledge — facts about the user, their world, their work, and the machine — lives in the `wiki/` directory as one-concept Markdown pages. Only its root `index.md` is in the prompt; pages are read on demand. The memory pipeline below is the raw material the wiki is built from: the `memory_tending` pulse reads new episodes and files what they teach into wiki pages. See [wiki.md](wiki.md).

### USER.md — Core Facts

`USER.md` holds only durable identity and standing preferences the agent needs on every turn (name, timezone, how the user likes to be addressed and answered). It is hard-capped at roughly 15 entries and **replace, don't append**: once full, adding an entry means removing one. Everything longer-form about the user lives in wiki pages.

An entry belongs in `USER.md` only once it is corroborated — supported by at least two independent episodes. Knowledge seen in a single episode goes into a `draft` wiki page instead. The `memory_tending` pulse and the `learner` skill both apply this rule.

### Observer — Automatic Episode Extraction

Fires automatically after enough conversation accumulates (token threshold). The agent and user do not invoke it — the gateway handles timing.

Extraction (the LLM call that turns messages into observations and a narrative) and persistence (episode id allocation, writing the transcript and observation archives, indexing, embedding, and the reflector check) are separate steps. Persistence always goes through the memory merge writer, the single serialized writer for global memory — the main agent's own observation flow and every agent session's completion both call it, so episode numbering and observation-log appends never race between concurrent writers.

The main agent's own automatic cycle (after a turn ends, on the soft-threshold cooldown, or at an idle transition) runs off the gateway event loop, in a background worker — see `crate::gateway::post_turn` in the source — so the LLM call never delays the next inbound message, a stop request, or a shutdown signal from being handled. At most one cycle runs at a time; a trigger that arrives while one is already running coalesces into a single follow-up rather than stacking. Only reloading the agent's own observations/recent-context views needs to happen back on the main loop, once the cycle's result comes back — everything else (the extraction call, the merge, the file writes) happens entirely in the background. This is why memory can be "one step stale": a turn that starts before a still-in-flight cycle's result is applied sees the pre-cycle context. The web UI shows a quiet "updating memory…" indicator in the chat footer while a cycle is running. An idle transition's own cycle runs the same way — clearing the conversation buffer and switching interfaces waits for it, so an unobserved message is never wiped out by the clear racing ahead of the observe.

An automatic extraction or merge failure (a model call error, a write failure) is never a silent retry loop: it backs off exponentially (1m, 2m, 4m, ... capped at 1h) before the next threshold crossing attempts again, so a broken provider doesn't re-spend an LLM call on every subsequent crossing while unobserved messages keep accumulating — those messages are never discarded on a failed attempt, so they're still there once it recovers. The user is told once when a failure streak starts, in plain language, and once when it clears; repeat failures within the same streak stay quiet. A manually forced observe (the `/observe` chat command) always attempts regardless of this backoff, and a working manual retry clears it for the automatic path too.

### Agent Sessions and Memory

Every agent session — a pulse, a scheduled action, a webhook, or a `subagent_spawn`/learner sub-agent — has its own working memory and merges into this same global memory when it completes. A session's run is checked against the same observer thresholds as the main agent's; crossing the force threshold mid-run extracts and stages observations locally, invisible to any other agent until the run finishes. On completion, a run produces no episode only if it staged nothing and either its final turn ended with `HEARTBEAT_OK` or its transcript is below a configurable token floor (`episode_skip_token_floor` in `[background]`, default ~2000 tokens) — its transcript is still kept in the session store either way. Otherwise a final extraction runs over whatever wasn't staged, and the combined observations merge through the memory merge writer alongside the run's full transcript as one episode.

Merged observations and episodes carry the originating session's address, run id, and category, so `memory_search` and `memory_get` results stay traceable to their source. These fields are optional on read, so observation and episode files written before session memory existed still load. The main agent's own recent-context narrative (`recent_context.json`) is only ever replaced by the main agent's own observations — a session's captured narrative lives on its own episode's transcript instead, never in the shared recent-context file.

If the process exits mid-run, the run's transcript is not lost: it is appended to durably as the run progresses (after every model response and tool result), and at the next startup any run that never reached a terminal state goes through this same completion pipeline — skip check, extraction, merge — from its persisted transcript before normal operation resumes.

**Trigger modes:**
- Soft threshold (`threshold_tokens`): starts a cooldown timer, fires when cooldown expires
- Force threshold (`force_threshold_tokens`): fires immediately, bypassing cooldown

**What it produces per episode (under `memory/episodes/YYYY-MM/DD/`):**
- `ep-NNN.jsonl` — line 1 is meta JSON, subsequent lines are serialized messages
- `ep-NNN.obs.json` — JSON array of extracted observations
- `ep-NNN.idx.jsonl` — JSONL of index chunks for search

**After extraction:**
- Observations appended to `memory/observations.json`
- Unobserved messages cleared from `memory/recent_messages.json`
- Narrative context saved to `memory/recent_context.json`
- If an embedding model is configured, .obs and .idx files are embedded for retrieval

Episode IDs are zero-padded to 3 digits (`ep-001`, `ep-012`). Next ID determined by scanning for the highest existing.

**Interaction signals**: the bundled `OBSERVER.md` also extracts a category of observations about how the user works and wants to be worked with — corrections and pushback, process preferences, frustration and its cause, praise and what earned it. These are recorded as contextualized, declarative facts about what happened ("the user prefers X"), never as instructions to the agent ("always do X"). This is the raw material the `learner` skill corroborates against when a `preference` signal fires — see [subconscious.md](subconscious.md#learning-trigger).

### Reflector — Observation Compression

Fires when `memory/observations.json` exceeds a token threshold. Calls the LLM to merge and deduplicate observations, then writes the compressed result back to `observations.json`. The results are identical in structure, just compressed.

**Critical**: The reflector reads from and writes to `observations.json` only. It does **not** touch the wiki or `USER.md`. These are completely separate systems.

Original observations are backed up before replacement. Empty LLM responses are rejected (the reflector will not destroy existing content).

An automatic reflection failure follows the same backoff and once-per-streak notice as the observer's own (see above) — the reflector's tracker is shared globally across the main agent and every session, since the observation log it compresses is itself global. The `/reflect` chat command bypasses the backoff and always attempts, and its outcome updates the same tracker.

### Prompt Customization

Both the observer and reflector use prompt files the agent owns:

- `memory/OBSERVER.md` — controls what the observer extracts from conversations
- `memory/REFLECTOR.md` — controls how the reflector compresses observations

These prompts contain only the customizable guidance portion. The output format specification is injected by the Rust code and cannot be lost by editing these files.

The intended workflow: the agent sets up a heartbeat to periodically review its own past episodes and extracted observations, evaluates the quality of extractions, and refines these prompts over time.

## Search

Two search backends, used together when both are available:

### BM25 (tantivy)

Full-text keyword search over observations, interaction-pair chunks, and wiki pages. Always available.

- Episode files are synced on startup (incremental via `.index_manifest.json`) and indexed after each observer extraction; the observer records what it indexed in the manifest, so startup does not index an episode twice
- Wiki pages are synced before every search: changed, new, and deleted pages are found by modification time, and only those are reindexed (see [wiki.md](wiki.md#how-it-reaches-the-model))
- Supports AND, OR, and phrase queries with quotes

### Vector (sqlite-vec)

Semantic similarity search via embeddings. Available when an embedding provider is configured in `[memory.search]`.

- Storage: `memory/vectors.db` (sqlite — deliberate exception to file-first philosophy since raw vectors aren't human-parsable), one table each for observations, chunks, and wiki pages
- A wiki page is re-embedded only when its text changes, so restarts reuse existing page embeddings
- When no embedding provider is configured, this branch is silently skipped (graceful degradation to BM25-only)

### Hybrid Search Flow

BM25 + vector results → normalize scores (min-max to [0,1]) → weighted merge (`vector_weight` and `text_weight`, rescaled to sum to 1 so the merged score stays in [0,1]; only their ratio matters) → optional temporal decay → filter by min_score → return top N. A result dropped by the min_score filter is not silently discarded: the caller learns how many were filtered (`memory_search`'s reply, or `below_threshold` on the HTTP endpoint), and can override the threshold for one search (`memory_search`'s `min_score` parameter, or the endpoint's `min_score` query parameter) to see them.

Temporal decay never applies to wiki pages: they hold maintained knowledge, and staleness is handled by their `stale_after` field and the `wiki_lint` pulse rather than by age.

## Tools

### `memory_search`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `query` | string | yes | Supports AND, OR, phrase queries |
| `limit` | integer | no | Max results. Default 5, no upper cap |
| `source` | string enum | no | `"observations"`, `"episodes"`, or `"wiki"`; omit to search all three |
| `date_from` | string | no | `YYYY-MM-DD`, inclusive lower bound |
| `date_to` | string | no | `YYYY-MM-DD`, inclusive upper bound |
| `episode_ids` | string[] | no | Limit to specific episode IDs (excludes wiki pages, which belong to no episode) |
| `min_score` | number | no | Override the configured `[search].min_score` relevance threshold for this search only |

A wiki result's ID is the page's workspace-relative path (`wiki/homelab/cluster.md`), ready for `read_file`. When every match falls below the relevance threshold, the reply says so and names how many weaker matches were filtered, instead of reporting a flat "no results" — pass `min_score` lower to see them.

`GET /api/memory/search?q=<query>&limit=<at least 1, default 10, no upper cap>&source=observations|episodes|wiki&date_from=&date_to=&min_score=<override>` runs the same hybrid search for workbench artifacts, with `episode_ids` unsupported (this endpoint has no equivalent parameter). It answers `{ results: [{ id, source, episode_id, date, line_start, line_end, snippet, score }], semantic, below_threshold }`, where `semantic` says whether vector search contributed to the results and `below_threshold` counts results that scored under the threshold and were dropped. A blank `q`, an unrecognized `source`, or a `date_from`/`date_to` that isn't `YYYY-MM-DD` answers `400`.

### `memory_get`

Retrieves the full transcript of a specific episode, or of a session run by its run id — provide exactly one of `episode_id`/`run_id`. The run-id mode reads from the session store rather than the episode store, so a run that produced no episode (skipped as a no-op, or not yet completed) can still be read; a run's own record names its episode id, if any, once merged. See [background-tasks.md](background-tasks.md#session-store) for the session store.

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `episode_id` | string | one of `episode_id`/`run_id` | e.g. `"ep-001"`. Path traversal rejected. |
| `run_id` | string | one of `episode_id`/`run_id` | e.g. `"run-1234567890-abcd1234"`. Path traversal rejected. Works on a still-running session too, reading its live incremental transcript. |
| `from_line` | integer | no | 1-indexed line offset. Default: beginning. |
| `lines` | integer | no | Lines to return. Default 50, max 200. |

An unknown run id returns an error pointing at `list_agents` (live sessions) and `memory_search` (merged episodes).

## Persistence Across Restarts

- `memory/recent_messages.json` persists unobserved messages across restarts (no watermark system)
- Last-run timestamps for the observer are in-memory only; they reset on restart

## Context Assembly

Knowledge and memory appear in the agent's context, after `USER.md`, as:
1. `WIKI_INDEX` — the wiki's root `index.md`
2. `OBSERVATION_LOG` — the formatted observation log from `observations.json`
3. `RECENT_CONTEXT` — the narrative from the latest observation (`memory/recent_context.json`)

An agent session's fork carries a snapshot of the observation log and the recent-context narrative taken at fork time, alongside `USER.md` and `WIKI_INDEX` — a session never sees observations merged after it forked; it sees them on its next run. See [background-tasks.md](background-tasks.md) for the full fork contents.

## Message Senders

A user message that arrives on a chat interface (Discord, Telegram, Teams) records who sent it: display name, stable ID, interface, and where on that interface it was sent (a direct message, a named channel). The sender is stored as its own field alongside the message in `recent_messages.json` and episode transcripts; the message text is never rewritten.

Wherever the conversation is read back — the agent's history on every model call, the observer's extraction transcript, `memory_get` output, and search chunks — a message with a sender is rendered with a leading `[From: Jane Doe via teams (#eng-team)]` line. That keeps attribution intact for every earlier message, not just the latest one, so the agent can tell participants apart in shared spaces. Messages from the web UI and from internal sources (background results, subconscious corrections) carry no sender. The web chat history shows the sender as a small label above the message.

A message one agent sends another (a session's result relayed to `main`, a `message_agent` call) records its sending agent the same way, as an `agent_sender` field (`address` and `category`) next to the message, in the recipient's history and in session transcripts. Its text already names the sender in an `[Agent Message from …]` header for the agent to read; the field is what the web UI trusts to show the message as coming from that session, since anyone can type the header. A message the owner sends a session from the web sidebar carries no `agent_sender`.
