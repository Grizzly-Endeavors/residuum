# Memory System

The memory pipeline converts conversation turns into searchable long-term memory through three stages: observation, reflection, and search.

## USER.md

Core facts only: durable identity/standing preferences, hard-capped at ~15 entries, replace-don't-append. Longer-form, evolving knowledge about the user lives in wiki pages instead — see the `wiki` skill.

**Promotion rule**: knowledge supported by a single episode becomes a wiki page with `status: draft`. It becomes `stable` — and, if it is a core fact, earns a `USER.md` entry — once a second, independent episode supports it; each supporting episode is listed in the page's `sources`. Both the `wiki` and `learner` skills apply this rule.

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

The bundled `OBSERVER.md` also extracts **interaction signals** — corrections/pushback, process preferences, frustration and its cause, praise and what earned it — as declarative facts about what happened, never as instructions. This is the evidence the `learner` skill corroborates against for a `preference` signal.

Customize extraction guidance by editing `memory/OBSERVER.md`.

## Reflector

Fires when `memory/observations.json` exceeds its token threshold. Calls an LLM to merge and deduplicate the observations, then writes the compressed result back to `observations.json`. The results are identical in structure, just denser.

**Critical**: The reflector reads from and writes to `observations.json` only. It does **not** touch the wiki. These are completely separate systems.

The original observations are backed up to `observations.json.bak` before replacement. Empty LLM responses are rejected (the reflector will not destroy existing content).

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

Use `memory_get` to retrieve the full transcript of a specific episode by ID.

Episodes are indexed after each observer extraction and synced on startup. Wiki pages are resynced before every search, so a page you just wrote is searchable immediately; a wiki result's ID is the page path to open with `read_file`. Wiki pages are exempt from temporal decay.

## Gotchas

- Episode IDs are zero-padded to 3 digits (`ep-001`, `ep-012`). The next ID is determined by scanning existing files for the highest number.
- `recent_messages.json` persists unobserved messages across restarts — there is no watermark.
- Observations have a `visibility` field (`User` or `Background`) that tracks their origin.
- The observer and reflector have independent model assignments via `[models] observer` and `[models] reflector` in config.toml. Unset roles fall back to `default`.
