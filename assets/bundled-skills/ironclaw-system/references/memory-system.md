# Memory System

The memory pipeline converts conversation turns into searchable long-term memory through three stages: observation, reflection, and search.

## Observer

Fires after agent turns when accumulated unobserved message tokens exceed a threshold. Two trigger modes:

- **Soft threshold** (`threshold_tokens`): starts a cooldown timer; fires when cooldown expires.
- **Force threshold** (`force_threshold_tokens`): fires immediately, bypassing cooldown.

The observer calls an LLM to extract a structured `Episode` from recent messages. Each episode produces three files under `memory/episodes/YYYY-MM/DD/`:

| File | Format | Contents |
|------|--------|----------|
| `ep-NNN.jsonl` | JSONL — line 1 is meta JSON, subsequent lines are serialized Messages | Full conversation transcript |
| `ep-NNN.obs.json` | JSON array of Observation objects | Extracted observations |
| `ep-NNN.idx.jsonl` | JSONL of IndexChunk objects | Interaction-pair chunks for search indexing |

After extraction, observations are appended to `memory/observations.json` and recent messages are cleared from `memory/recent_messages.json`. The narrative context is saved to `memory/recent_context.json`.

Customize extraction guidance by creating `memory/OBSERVER.md`.

## Reflector

Fires when `memory/observations.json` exceeds its token threshold. Calls an LLM to compress the observation log into a summary, which replaces `MEMORY.md`. The original observations file is backed up before replacement. Empty LLM responses are rejected (the reflector will not destroy existing content).

Customize compression guidance by creating `memory/REFLECTOR.md`.

## Search

Use `memory_search` to query the tantivy BM25 index. Available filters:

| Filter | Type | Description |
|--------|------|-------------|
| `query` | string | **Required.** Free-text search query. |
| `source` | string | Filter by source (e.g., `"user"`, `"background"`). |
| `date_from` | string | ISO date lower bound (inclusive). |
| `date_to` | string | ISO date upper bound (inclusive). |
| `project_context` | string | Filter to episodes from a specific project. |
| `episode_ids` | array | Filter to specific episode IDs. |

Use `memory_get` to retrieve the full transcript of a specific episode by ID.

The search index is rebuilt on startup and updated after each observer extraction. If `memory/vectors.db` exists, vector search results are merged with BM25 results.

## Gotchas

- Episode IDs are zero-padded to 3 digits (`ep-001`, `ep-012`). The next ID is determined by scanning existing files for the highest number.
- `recent_messages.json` persists unobserved messages across restarts — there is no watermark.
- Observations have a `visibility` field (`User` or `Background`) that tracks their origin.
- The observer and reflector share the same LLM provider configured in `[memory]`.
