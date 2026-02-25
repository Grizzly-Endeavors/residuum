# Memory Search — Design Document

## Problem

The current memory search is BM25-only over full episode transcripts (~30k tokens each). This has three compounding weaknesses:

1. **Diluted relevance.** A small relevant section buried in a 30k blob gets a weak BM25 score. Tool call results (file contents, command output) dominate the token count but carry almost no search signal.
2. **No semantic search.** BM25 only matches on exact terms. The agent can't find "that conversation about authentication" if the word "authentication" never appeared — even if the discussion was clearly about auth flows.
3. **No filtering.** Every search scans the entire corpus. The agent can't scope to a project, date range, or specific episodes it already knows are relevant.

The `EmbeddingProvider` trait and provider implementations (OpenAI, Ollama, Gemini) already exist but aren't wired to search.

---

## Design Decisions

### Observations as primary search targets

Observations are LLM-compressed, self-contained sentences (~50-200 tokens) with rich metadata: timestamp, project_context, source_episodes, visibility. They're already the highest signal-density unit in the memory system. Indexing them individually in both BM25 and vector gives search small, semantically dense targets that score well in both paradigms.

Episode transcripts remain available as secondary targets for deep retrieval, but observations should be the first thing search hits.

### Drop tool calls from indexed content

Tool call requests (function name + arguments) and tool results (file contents, command output, build logs) are stripped entirely from indexed content. Rationale:

- Tool results are the bulk of episode tokens (~60-80%) and are almost entirely noise for search: raw code, line numbers, boilerplate, duplicate of on-disk files.
- Tool call names carry minimal signal ("read_file", "exec") that doesn't help retrieval.
- The observer already captures the important conclusions from tool-heavy interactions ("discovered the bug was in middleware", "config parsing fails at line 42").
- The raw transcript (`ep-XXX.jsonl`) remains available via `memory_get` when truly needed.

### Interaction-pair chunking

Instead of indexing the full 30k episode blob, extract **interaction pairs**: one user message paired with the assistant's next text response, skipping all intermediate tool call/result turns. If an assistant turn involves multiple tool calls before responding with text, collapse into a single chunk containing only the user message and the final assistant text.

This produces chunks that are:
- Semantically coherent (question + answer)
- Appropriately sized for embedding (~200-2000 tokens typical)
- Free of noise from tool I/O

Chunks are stored in a new `ep-XXX.idx.jsonl` file, generated at observation time alongside the transcript and obs archive.

**Known gap:** If a user message is followed by a tool-heavy assistant stretch that never produces a text response before the next user message, that interaction produces zero chunks. The observations still capture the conclusions from those interactions, so search coverage is maintained through the observation layer. If search quality gaps emerge from this pattern, a fallback would be to capture user-message-only orphans as single-sided chunks. This is part of a larger architectural pattern where user messages cannot be injected during agent turns.

### Context field convention

The `context` field on indexed documents uses the active project name (e.g., `"aerohive-setup"`) or the workspace directory name (e.g., `"workspace"`) — never empty, never null. `project_context_label()` always produces a value; the `"general"` fallback in `derive_project_context()` exists only as a safety net for empty message batches.

Using a non-empty string keeps filtering clean: `context = ?` for project-scoped, omit the filter for global. No `IS NULL` gymnastics.

**Per-message granularity, not per-episode.** Each indexed document carries the context from its source message, not the episode-level majority vote. Specifically:

- **Observations**: tagged with the `project_context` from the `RecentMessage` closest to the observation's timestamp. This requires fixing the current observer — `build_episode_and_persist()` currently stamps every observation with the same episode-level context.
- **Chunks**: tagged with the `project_context` from the user message in the interaction pair (available directly on the `RecentMessage`).
- **Episode meta** (`EpisodeMeta.context`): retains the majority-vote value as a coarse label for the episode as a whole. Not used for search filtering.

This means a single episode that spans two projects produces observations and chunks tagged with the correct per-message context. Filtering by `project_context: "aerohive-setup"` finds only the interactions that actually involved that project, even if the episode also touched other work.

### Explicit index files (`idx.jsonl`)

The pre-processed chunks live in their own file rather than being derived on-the-fly during indexing. This means:
- The indexing pipeline reads clean, pre-processed input — no parsing/filtering logic in the indexer.
- Chunk boundaries are deterministic and inspectable on disk.
- Rebuilding the search index from scratch is a simple walk over `.idx.jsonl` and `.obs.json` files.
- The raw transcript stays untouched for full-fidelity retrieval.

### SQLite + sqlite-vec for vector storage

Vector embeddings are stored in SQLite using the `sqlite-vec` extension, mirroring OpenClaw's approach. This is a deliberate exception to the file-first philosophy: raw embedding vectors aren't human-parsable regardless of format, so the auditability argument doesn't apply. SQLite gives us:
- Native cosine similarity via `vec_distance_cosine()`
- Single-file database (still portable and inspectable with standard tools)
- Atomic writes, concurrent reads
- Static compilation via the `sqlite-vec` crate (no runtime dependency)

### Filters on the search tool, not separate indexes

Project scoping, date ranges, episode ID filtering, and source type selection are all implemented as filters on a single `memory_search` tool rather than maintaining separate per-project or per-type indexes. The corpus size (thousands of observations, hundreds of episodes) doesn't warrant index partitioning.

### Temporal decay (opt-in)

Score decay based on age: `decayed_score = score * exp(-lambda * age_days)`. Default off. When enabled, a configurable half-life (default 30 days) gradually de-weights older results. Applied as a post-scoring adjustment before final ranking.

### Deferred: MMR re-ranking and query expansion

Both are valuable but add complexity. OpenClaw implements both as opt-in features. Deferred to a follow-up iteration once the core hybrid search is proven.

### Not implemented: visibility filtering

Visibility (user vs. background) is a signal for the LLM when reading observations in context, not a meaningful search filter. The agent rarely needs to exclude background observations from search results.

---

## System Map

### File Layout

```
memory/
├── observations.json                        # global observation log (reflector-compressed)
├── observations.json.bak                    # backup before reflection
├── recent_messages.json                     # unobserved messages
├── recent_context.json                      # narrative + episode ID
├── OBSERVER.md                              # user-editable extraction guidance
├── REFLECTOR.md                             # user-editable reflection guidance
├── vectors.db                               # SQLite + sqlite-vec embeddings
├── .index_manifest.json                     # NEW: tracks indexed files for incremental sync
├── episodes/
│   └── YYYY-MM/DD/
│       ├── ep-NNN.jsonl                     # raw transcript (meta + messages)
│       ├── ep-NNN.obs.json                  # per-episode observations archive
│       └── ep-NNN.idx.jsonl                 # NEW: interaction-pair chunks for indexing
└── .index/                                  # tantivy BM25 index (binary, mmap'd)
```

### idx.jsonl Format

Each line is a self-contained chunk with metadata:

```jsonl
{"chunk_id":"ep-001-c0","episode_id":"ep-001","date":"2026-02-19","context":"ironclaw","line_start":2,"line_end":14,"content":"user: How do I configure the memory system?\nassistant: The memory system uses..."}
{"chunk_id":"ep-001-c1","episode_id":"ep-001","date":"2026-02-19","context":"ironclaw","line_start":15,"line_end":38,"content":"user: What about the reflector?\nassistant: The reflector compresses..."}
```

Fields:
- `chunk_id`: `{episode_id}-c{N}` — stable identifier for the chunk within the episode
- `episode_id`: parent episode reference
- `date`: episode date (denormalized for filtering)
- `context`: project context tag (denormalized for filtering)
- `line_start`: 1-indexed line number of the user message in the raw `.jsonl` transcript
- `line_end`: 1-indexed line number of the closing assistant message in the raw `.jsonl` transcript
- `content`: user message + assistant text response, tool calls stripped

The `line_start`/`line_end` fields let `memory_get` jump directly to the relevant section of the raw transcript without the agent having to search within the file. A chunk search hit gives the agent everything needed to drill in: episode ID for the file path, line range for the offset.

### Vector Store Schema (SQLite)

```sql
-- Chunk embeddings (from idx.jsonl interaction pairs)
CREATE TABLE chunk_embeddings (
    chunk_id   TEXT PRIMARY KEY,
    episode_id TEXT NOT NULL,
    date       TEXT NOT NULL,      -- YYYY-MM-DD
    context    TEXT NOT NULL,      -- project context
    content    TEXT NOT NULL       -- chunk text (for snippet extraction)
);
CREATE VIRTUAL TABLE chunk_vectors USING vec0(
    embedding float[{dim}]        -- dimension from embedding provider
);

-- Observation embeddings
CREATE TABLE obs_embeddings (
    obs_id         TEXT PRIMARY KEY,   -- {episode_id}-o{N} or hash-based
    episode_id     TEXT NOT NULL,
    date           TEXT NOT NULL,
    context        TEXT NOT NULL,
    content        TEXT NOT NULL
);
CREATE VIRTUAL TABLE obs_vectors USING vec0(
    embedding float[{dim}]
);
```

The `rowid` in `vec0` virtual tables corresponds to the `rowid` in the metadata tables, joining vectors to their metadata.

### BM25 Index Schema (Tantivy)

Tantivy indexes both observations and chunks. Each document has:

| Field | Type | Purpose |
|-------|------|---------|
| `id` | STRING, STORED | chunk_id or obs_id |
| `source_type` | STRING, STORED | `"observation"` or `"chunk"` |
| `episode_id` | STRING, STORED | parent episode |
| `date` | STRING, STORED | YYYY-MM-DD |
| `context` | STRING, STORED | project context |
| `content` | TEXT, STORED | searchable body |

### Interaction-Pair Extraction

Given a sequence of messages from an episode:

```
user: "How do I fix the build?"
assistant: [tool_call: exec("cargo build")]        ← skipped
tool: "error[E0308]: mismatched types..."           ← skipped
assistant: [tool_call: read("src/main.rs")]         ← skipped
tool: "fn main() { ... }"                           ← skipped
assistant: "The build fails because..."             ← captured
user: "What about the tests?"
assistant: "Tests pass after..."                    ← captured
```

Algorithm:
1. Walk messages in order
2. On each `user` message, start a new pending pair
3. Skip `assistant` messages that contain only tool calls (no text content)
4. Skip `tool` role messages entirely
5. On the first `assistant` message with non-empty text content, close the pair
6. If a new `user` message arrives before the pair closes, discard the incomplete pair
7. Write each completed pair as one idx.jsonl line

### Hybrid Search Flow

```
memory_search(query, filters)
│
├─ BM25 Search (tantivy)
│  ├─ Parse query (lenient)
│  ├─ Apply source_type filter (observation/chunk/both)
│  ├─ Apply date range filter
│  ├─ Apply context filter
│  ├─ Apply episode_id filter
│  ├─ Fetch top N * candidate_multiplier results
│  └─ Return (id, score, content, metadata)
│
├─ Vector Search (sqlite-vec)
│  ├─ Embed query via EmbeddingProvider
│  ├─ Search chunk_vectors + obs_vectors (based on source filter)
│  ├─ Join to metadata tables for filtering
│  ├─ Fetch top N * candidate_multiplier results
│  └─ Return (id, score, content, metadata)
│
├─ Normalize scores
│  ├─ BM25 scores: min-max normalize to [0, 1] within result set
│  ├─ Vector scores: min-max normalize to [0, 1] within result set
│  │   (BM25 is [0, unbounded], cosine is [-1, 1] — raw scales are incomparable)
│  │   (single-result sets normalize to 1.0)
│
├─ Merge
│  ├─ Combine by document id
│  ├─ hybrid_score = vector_weight * norm_vec_score + text_weight * norm_bm25_score
│  ├─ Apply temporal decay (if enabled)
│  ├─ Filter by min_score threshold
│  ├─ Sort by final score descending
│  └─ Return top N results
│
└─ Format results with metadata
```

When no embedding provider is configured, the vector search branch is skipped and results come from BM25 only (graceful degradation).

### Search Tool Interface

```json
{
  "name": "memory_search",
  "parameters": {
    "query": {
      "type": "string",
      "description": "Search query (supports AND, OR, phrase queries with quotes)"
    },
    "limit": {
      "type": "integer",
      "description": "Maximum results to return (default: 5, max: 20)"
    },
    "filters": {
      "type": "object",
      "properties": {
        "source": {
          "type": "string",
          "enum": ["observations", "episodes", "both"],
          "description": "Search observations, episode chunks, or both (default: both)"
        },
        "date_from": {
          "type": "string",
          "description": "Earliest date to include (YYYY-MM-DD)"
        },
        "date_to": {
          "type": "string",
          "description": "Latest date to include (YYYY-MM-DD)"
        },
        "project_context": {
          "type": "string",
          "description": "Filter to a specific project context"
        },
        "episode_ids": {
          "type": "array",
          "items": { "type": "string" },
          "description": "Filter to specific episodes (e.g. [\"ep-001\", \"ep-003\"])"
        }
      }
    }
  },
  "required": ["query"]
}
```

### memory_get Tool Interface

```json
{
  "name": "memory_get",
  "parameters": {
    "episode_id": {
      "type": "string",
      "description": "Episode identifier (e.g. \"ep-001\")"
    },
    "from_line": {
      "type": "integer",
      "description": "Starting line number (1-indexed)"
    },
    "lines": {
      "type": "integer",
      "description": "Number of lines to read (default: 50, max: 200)"
    }
  },
  "required": ["episode_id"]
}
```

Takes an episode ID, resolves it to the transcript path on disk (`episodes/YYYY-MM/DD/{episode_id}.jsonl`), and returns a bounded snippet. The agent never needs to know or construct file paths — search results provide episode IDs and line offsets, and `memory_get` handles resolution.

Without `from_line`, returns from the start of the transcript (meta line + first messages). With `from_line` (typically from a chunk's `line_start`), jumps directly to the relevant section.

### Search Result Format

```
Found 3 result(s):

1. [observation] ep-001 | 2026-02-19 | ironclaw (score: 0.87)
   The memory system uses a two-tier compression model with observer and reflector stages.

2. [chunk] ep-003-c2 | 2026-02-21 | ironclaw | lines 15-38 (score: 0.72)
   user: How does the observer decide when to fire?
   assistant: The observer tracks token accumulation across messages...

3. [observation] ep-005 | 2026-02-23 | ironclaw (score: 0.65)
   Observer threshold was increased to 40k tokens after testing showed premature triggers.
```

Chunk results include `lines X-Y` so the agent can immediately call `memory_get` with the right offset to drill into the raw transcript.

### Configuration

```toml
[memory.search]
# Embedding provider for vector search (optional — BM25-only if omitted)
provider = "openai"               # "openai" | "ollama" | "gemini"
model = "text-embedding-3-small"

# Hybrid scoring weights (must sum to 1.0)
vector_weight = 0.7
text_weight = 0.3

# Result filtering
min_score = 0.35
candidate_multiplier = 4          # fetch 4x limit for post-processing

# Temporal decay (opt-in)
temporal_decay = false
temporal_decay_half_life_days = 30
```

### Indexing Pipeline

#### At observation time (online)

When the observer fires and writes an episode:

1. Write `ep-NNN.jsonl` (raw transcript) — existing behavior
2. Write `ep-NNN.obs.json` (observations) — existing behavior
3. **NEW:** Extract interaction pairs from messages → write `ep-NNN.idx.jsonl`
4. **NEW:** Index observations in tantivy (one document per observation)
5. **NEW:** Index interaction-pair chunks in tantivy (one document per chunk)
6. **NEW:** If embedding provider configured:
   a. Embed each observation → insert into obs_vectors + obs_embeddings
   b. Embed each chunk → insert into chunk_vectors + chunk_embeddings

#### At startup (incremental sync)

Full rebuild on every boot is too expensive — embedding hundreds of episodes via a remote provider is slow and wasteful when most content is archival and unchanged. Instead, startup performs an incremental sync:

1. Read `.index_manifest.json` from the memory directory — a registry of every document ID (chunk and observation) that has been indexed, with the source file path and its mtime at index time.
2. Walk `episodes/` directory tree, collecting all `.obs.json` and `.idx.jsonl` files.
3. Compare against the manifest:
   - **New files** (path not in manifest): index all documents, embed if provider configured.
   - **Modified files** (mtime differs): re-index all documents from that file (delete old entries first).
   - **Deleted files** (in manifest but not on disk): remove stale entries from tantivy and vector store.
   - **Unchanged files**: skip entirely.
4. Write updated `.index_manifest.json`.

This makes startup O(changed files) rather than O(all files). For a workspace with hundreds of archival episodes and one or two new ones, startup indexing touches only the new episodes.

**Full rebuild** is available as an explicit operation (e.g., `--rebuild-index` flag or when the manifest is missing/corrupt). This clears both indexes and re-processes everything from scratch.

#### .index_manifest.json format

```json
{
  "last_rebuild": "2026-02-24T10:30:00",
  "embedding_model": "text-embedding-3-small",
  "embedding_dim": 1536,
  "files": {
    "episodes/2026-02/19/ep-001.obs.json": {
      "mtime": "2026-02-19T14:30:00",
      "doc_ids": ["ep-001-o0", "ep-001-o1", "ep-001-o2"]
    },
    "episodes/2026-02/19/ep-001.idx.jsonl": {
      "mtime": "2026-02-19T14:30:00",
      "doc_ids": ["ep-001-c0", "ep-001-c1"]
    }
  }
}
```

The `embedding_model` and `embedding_dim` fields detect provider changes — if the configured embedding model differs from the manifest, a full vector re-embed is required (BM25 index is unaffected and can be synced incrementally regardless).

#### Dependencies

New crates:
- `rusqlite` (with `bundled` feature) — SQLite access
- `sqlite-vec` — vector similarity extension
- `zerocopy` — zero-copy vector serialization

---

## Implementation Phases

### Phase 1: idx.jsonl generation and expanded BM25 indexing

**Goal:** Produce the new index files, expand tantivy to index observations and chunks separately, and add filters. No vector search yet.

- Implement interaction-pair extraction from episode messages (with line_start/line_end tracking)
- Write `ep-NNN.idx.jsonl` at observation time in the existing observer pipeline
- Expand tantivy schema: add `id`, `source_type`, `episode_id`, `context` fields
- Index individual observations from `.obs.json` files (one tantivy doc per observation)
- Index interaction-pair chunks from `.idx.jsonl` files (one tantivy doc per chunk, replacing raw `.jsonl` indexing)
- Implement `.index_manifest.json` for incremental sync at startup
- Update `rebuild()` to walk `.obs.json` + `.idx.jsonl` instead of `.jsonl`
- Update `memory_search` tool with filters (source, date, project_context, episode_ids)
- Update search result format to show source type, episode metadata, and line offsets for chunks
- Tests for extraction, indexing, filtering, incremental sync, full rebuild

### Phase 2: memory_get tool

**Goal:** Let the agent pull specific sections from any memory file after search.

- Implement `MemoryGetTool` with path, from_line, lines parameters
- Bounded reads with sane defaults and limits
- Path validation (must be within memory directory)
- Wire into tool registry
- Tests

### Phase 3: Hybrid search (SQLite + sqlite-vec)

**Goal:** Add vector similarity search alongside BM25.

- Add `rusqlite`, `sqlite-vec`, `zerocopy` dependencies
- Implement vector store module: schema creation, insert, cosine search
- Wire `EmbeddingProvider` into the search pipeline
- Embed observations and chunks at observation time (online)
- Incremental vector sync at startup (via `.index_manifest.json` — same manifest, extended with vector state)
- Embed query at search time
- Min-max score normalization within each result set before weighting
- Implement hybrid merge: weighted combination of normalized BM25 + vector scores
- Graceful degradation when no embedding provider is configured (BM25-only)
- Re-embed trigger when `embedding_model` in manifest differs from config
- Config parsing for `[memory.search]` section
- Tests (with mock embedding provider)

### Phase 4: Temporal decay

**Goal:** Opt-in recency bias for search results.

- Implement score decay function: `score * exp(-lambda * age_days)`
- Parse `temporal_decay` and `temporal_decay_half_life_days` from config
- Apply as post-scoring adjustment after hybrid merge, before final ranking
- Tests

### Phase 5: Backfill ~~and migration~~ (simplified)

**Goal:** Generate idx.jsonl files for the handful of existing episodes that predate the new pipeline.

**Original plan** called for a startup migration pass that walks the episodes directory and generates missing `.idx.jsonl` files automatically. This is unnecessary — at the time of implementation there are only 4 pre-existing episodes. A one-off Python script generates the missing files, and the existing incremental sync (Phase 1) picks them up on next startup.

If the corpus ever grows large enough that a proper migration path matters, it can be added then. The extraction algorithm is deterministic and the raw `.jsonl` transcripts are always available.

- One-off script: `scripts/backfill_idx.py` — parses existing `.jsonl` transcripts and writes `.idx.jsonl` using the same interaction-pair algorithm as `chunk_extractor.rs`
- Run once, verify output, done
- Incremental sync handles the rest

### Deferred

- **MMR re-ranking**: diversity-aware result selection (OpenClaw has this as opt-in)
- **Query expansion**: keyword extraction from natural language queries to improve BM25 recall
- **Orphan chunk capture**: index user messages with no paired assistant text response as single-sided chunks (only if search quality gaps emerge from tool-heavy episodes)
