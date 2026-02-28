# Memory System

The memory pipeline converts conversation turns into searchable long-term memory through three stages: observation, reflection, and search.

## MEMORY.md

A persistent scratchpad the agent owns and writes to directly. This is the agent's working notebook for cross-session context. The observer and reflector never touch it — it is entirely agent-controlled.

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

After extraction, observations are appended to `memory/observations.json` and recent messages are cleared from `memory/recent_messages.json`. The narrative context is saved to `memory/recent_context.json`. If an embedding provider is configured, `.obs` and `.idx` files are embedded for vector retrieval.

Customize extraction guidance by editing `memory/OBSERVER.md`.

## Reflector

Fires when `memory/observations.json` exceeds its token threshold. Calls an LLM to merge and deduplicate the observations, then writes the compressed result back to `observations.json`. The results are identical in structure, just denser.

**Critical**: The reflector reads from and writes to `observations.json` only. It does **not** touch `MEMORY.md`. These are completely separate systems.

The original observations are backed up to `observations.json.bak` before replacement. Empty LLM responses are rejected (the reflector will not destroy existing content).

Customize compression guidance by editing `memory/REFLECTOR.md`.

## Search

Use `memory_search` to query past observations and episode chunks. When an embedding provider is configured, hybrid search (BM25 + vector similarity) is used automatically. Otherwise, BM25 keyword search only.

| Parameter | Type | Description |
|-----------|------|-------------|
| `query` | string | **Required.** Free-text search query (supports AND, OR, phrase queries). |
| `source` | string | `"observations"`, `"episodes"`, or `"both"` (default: `"both"`). |
| `date_from` | string | ISO date lower bound (inclusive). |
| `date_to` | string | ISO date upper bound (inclusive). |
| `project_context` | string | Filter to observations/chunks from a specific project. |
| `episode_ids` | array | Filter to specific episode IDs. |

Use `memory_get` to retrieve the full transcript of a specific episode by ID.

The search index is rebuilt on startup (incrementally) and updated after each observer extraction.

## Gotchas

- Episode IDs are zero-padded to 3 digits (`ep-001`, `ep-012`). The next ID is determined by scanning existing files for the highest number.
- `recent_messages.json` persists unobserved messages across restarts — there is no watermark.
- Observations have a `visibility` field (`User` or `Background`) that tracks their origin.
- The observer and reflector have independent model assignments via `[models] observer` and `[models] reflector` in config.toml. Unset roles fall back to `default`.
