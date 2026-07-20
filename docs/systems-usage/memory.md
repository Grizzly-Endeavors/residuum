# Memory System

The memory system gives the agent persistent recall across conversations. It has several distinct components that serve different purposes.

## Components

### MEMORY.md — Persistent Scratchpad

A markdown file the agent owns and writes to directly. This is the agent's working notebook for cross-session context: important facts, ongoing threads, user preferences it wants to remember.

- Updated by the agent using `write_file` or `edit_file` tools
- Always loaded into the agent's context window
- Not touched by the observer or reflector — entirely agent-controlled
- Think of it as the agent's handwritten notes

### USER.md — Two-Tier User Model

`USER.md` splits into two sections with different churn and different rules:

- **Core Facts** — durable identity and standing preferences only. Hard-capped at roughly 15 entries and **replace, don't append**: once full, adding a new entry means removing one. This is the always-loaded summary, so it stays short and current by construction rather than growing without bound.
- **Profile** — the longer-form, evolving model: communication style, context about the user's work and life, preferences that haven't earned a Core Facts slot yet. Normal append/update churn.

**Promotion rule**: an observation is only durable enough for Core Facts (or for a new entry in Profile) once at least two supporting observations back it — the pattern must recur, not just be seen once. Promoted entries are annotated with the evidence count (e.g. "seen 3x"). A single, uncorroborated sighting is written to `MEMORY.md` as a provisional note instead, not into `USER.md`.

This rule is enforced by whichever agent is doing the tending — the `introspection` preset (via the `memory_tending` pulse) and the `learner` preset (see below) both apply it the same way.

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

**Interaction signals**: the bundled `OBSERVER.md` also extracts a category of observations about how the user works and wants to be worked with — corrections and pushback, process preferences, frustration and its cause, praise and what earned it. These are recorded as contextualized, declarative facts about what happened ("the user prefers X"), never as instructions to the agent ("always do X"). This is the raw material the `learner` preset corroborates against when a `preference` signal fires — see [subconscious.md](subconscious.md#learning-trigger).

### Reflector — Observation Compression

Fires when `memory/observations.json` exceeds a token threshold. Calls the LLM to merge and deduplicate observations, then writes the compressed result back to `observations.json`. The results are identical in structure, just compressed.

**Critical**: The reflector reads from and writes to `observations.json` only. It does **not** touch `MEMORY.md`. These are completely separate systems.

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

Full-text keyword search over observations and interaction-pair chunks. Always available.

- Index rebuilt on startup (incremental via `.index_manifest.json`)
- Updated after each observer extraction
- Supports AND, OR, and phrase queries with quotes

### Vector (sqlite-vec)

Semantic similarity search via embeddings. Available when an embedding provider is configured in `[memory.search]`.

- Storage: `memory/vectors.db` (sqlite — deliberate exception to file-first philosophy since raw vectors aren't human-parsable)
- When no embedding provider is configured, this branch is silently skipped (graceful degradation to BM25-only)

### Hybrid Search Flow

BM25 + vector results → normalize scores (min-max to [0,1]) → weighted merge → optional temporal decay → filter by min_score → return top N.

## Tools

### `memory_search`

| Parameter | Type | Required | Notes |
|-----------|------|----------|-------|
| `query` | string | yes | Supports AND, OR, phrase queries |
| `limit` | integer | no | Max results. Default 5, cap 20 |
| `source` | string enum | no | `"observations"`, `"episodes"`, or `"both"` |
| `date_from` | string | no | `YYYY-MM-DD`, inclusive lower bound |
| `date_to` | string | no | `YYYY-MM-DD`, inclusive upper bound |
| `project_context` | string | no | Exact match on project context field |
| `episode_ids` | string[] | no | Limit to specific episode IDs |

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

Memory appears in the agent's context as:
1. `MEMORY.md` content (the persistent scratchpad)
2. Formatted observation log from observations.json
2. Narrative context from the latest observation (`memory/recent_context.json`)
3. Unread inbox count (from inbox system, not memory — but surfaced in the same status area)
