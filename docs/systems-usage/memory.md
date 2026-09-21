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

BM25 + vector results → normalize scores (min-max to [0,1]) → weighted merge → optional temporal decay → filter by min_score → return top N.

Temporal decay never applies to wiki pages: they hold maintained knowledge, and staleness is handled by their `stale_after` field and the `wiki_lint` pulse rather than by age.

## Tools

### `memory_search`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `query` | string | yes | Supports AND, OR, phrase queries |
| `limit` | integer | no | Max results. Default 5, cap 20 |
| `source` | string enum | no | `"observations"`, `"episodes"`, or `"wiki"`; omit to search all three |
| `date_from` | string | no | `YYYY-MM-DD`, inclusive lower bound |
| `date_to` | string | no | `YYYY-MM-DD`, inclusive upper bound |
| `episode_ids` | string[] | no | Limit to specific episode IDs (excludes wiki pages, which belong to no episode) |

A wiki result's ID is the page's workspace-relative path (`wiki/homelab/cluster.md`), ready for `read_file`.

### `memory_get`

Retrieves the full transcript of a specific episode.

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `episode_id` | string | yes | e.g. `"ep-001"`. Path traversal rejected. |
| `from_line` | integer | no | 1-indexed line offset. Default: beginning. |
| `lines` | integer | no | Lines to return. Default 50, max 200. |

## Persistence Across Restarts

- `memory/recent_messages.json` persists unobserved messages across restarts (no watermark system)
- Last-run timestamps for the observer are in-memory only; they reset on restart

## Context Assembly

Knowledge and memory appear in the agent's context, after `USER.md`, as:
1. `WIKI_INDEX` — the wiki's root `index.md`
2. `OBSERVATION_LOG` — the formatted observation log from `observations.json`
3. `RECENT_CONTEXT` — the narrative from the latest observation (`memory/recent_context.json`)

Sub-agents get `USER.md` and `WIKI_INDEX` but not the observation log or recent context.
