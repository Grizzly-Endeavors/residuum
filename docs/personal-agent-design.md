# Personal AI Agent — Memory & Proactivity Design

## Design Philosophy

This design targets two specific architectural weaknesses in OpenClaw's current systems: memory continuity across restarts and proactive behavior scheduling. Every other design decision in OpenClaw — the gateway pattern, channel normalization, Lane Queue, model-agnostic runtime, file-first workspace — serves the personal assistant use case well and is preserved as-is.

The guiding principle is **targeted improvement without sacrificing simplicity**. Both systems should remain inspectable, editable, and understandable by the user. Elegance that stops being practical is not a goal.

---

## Memory System

### Problem

OpenClaw's current memory has a day-boundary cliff. Identity files (SOUL.md, USER.md, MEMORY.md) and the last two days of daily logs are auto-loaded at startup. Anything older requires the agent to actively decide to call `memory_search` or `memory_get` — a judgment call LLMs are inconsistent at making.

The result: context from even the previous day can get dropped if it wasn't promoted to MEMORY.md or if the agent doesn't recognize it should search. Users end up building workarounds like scheduled summarization jobs to maintain continuity.

### Solution: Observational Memory Layer

Integrate an Observational Memory (OM) system on top of the existing workspace file structure. OM maintains a compressed, chronological event log that stays in the context window at all times, eliminating the retrieval dependency for recent-to-medium-term history.

Reference: [Mastra Observational Memory](https://mastra.ai/research/observational-memory) — 94.87% on LongMemEval, 5-40x compression on tool-heavy workloads.

### Architecture

#### What stays the same

- **Identity layer**: SOUL.md, USER.md, AGENTS.md, ENVIRONMENT.md — stable, curated, auto-loaded at startup. These define who the agent is and who the user is. OM does not touch these.
- **MEMORY.md**: Long-term curated facts and preferences. Still loaded in private contexts. Still manually maintained by the agent and user.
- **Hybrid search**: BM25 + vector retrieval over workspace files. Still available for deep retrieval beyond the observation window.

#### What changes

The `memory/YYYY-MM-DD.md` daily log system has been fully replaced by the OM observation log. Daily logs were removed once the observer was running — the agent stopped using them organically, as the observation log covered the same continuity needs with better compression and structure.

#### Context window structure

```
┌─────────────────────────────────────┐
│ System prompt                       │
│ (SOUL.md, AGENTS.md, USER.md, etc) │
├─────────────────────────────────────┤
│ MEMORY.md (curated long-term)       │
├─────────────────────────────────────┤
│ Observation log (OM)                │
│ - Compressed event history          │
│ - Chronological, dated entries      │
│ - Maintained by Observer/Reflector  │
├─────────────────────────────────────┤
│ Recent context (recent_context.json)│
│ - Narrative from latest observation │
│ - Bridges restarts / context gaps   │
├─────────────────────────────────────┤
│ Unread inbox count                  │
├─────────────────────────────────────┤
│ Raw message history                 │
│ (unobserved, verbatim)              │
└─────────────────────────────────────┘
```

### Two-Tier Compression

#### Tier 1: Observer

A background agent that watches the active conversation. The Observer uses a dual-threshold model to decide when to fire:

- **Soft threshold** (default ~30k tokens): When unobserved messages cross this level, a cooldown timer starts. If the cooldown expires without further activity, the Observer fires. This batches rapid exchanges into a single compression run rather than firing after every turn.
- **Force threshold** (default ~60k tokens): When unobserved messages reach this level, the Observer fires immediately, bypassing the cooldown. This prevents unbounded accumulation during long, continuous sessions.

When triggered, the Observer compresses unobserved messages into dated, structured observations.

**Input**: Raw messages, tool calls, tool results, corrections, decisions.

**Output**: A JSON observation log structured as a series of **episodes** — not prose summaries, but specific, dated entries capturing what happened, what was decided, and what changed. Each Observer compression run produces one episode with a unique ID.

```json
{
  "episodes": [
    {
      "id": "ep-001",
      "date": "2026-02-18",
      "start": "12:10",
      "end": "12:45",
      "observations": [
        "Working on Ansible playbook for AeroHive AP configuration",
        "Decided to use host_vars over group_vars for per-AP channel assignment",
        "Hit issue: aoscli module not recognizing enable mode — workaround using raw shell",
        "User correction: AeroHive uses HiveManager CLI, not aoscli",
        "Playbook tested successfully on AP-01, proceeding to remaining APs"
      ]
    }
  ]
}
```

After compression, the raw messages are dropped from context but **persisted as episode artifacts** under `memory/episodes/YYYY-MM/DD/`. Each episode produces three files:

- **`ep-NNN.jsonl`** — The transcript. Line 1 is a JSON meta object (episode ID, date, context tag); subsequent lines are serialized messages. Contains the full raw messages, tool calls, and results — everything the Observer compressed from.
- **`ep-NNN.obs.json`** — A JSON array of the extracted observations, archived alongside the transcript for independent retrieval.
- **`ep-NNN.idx.jsonl`** — JSONL of interaction-pair chunks extracted from the transcript, used by the search index.

The observations also append to the global observation log (`observations.json`). This gives the agent a trail to follow: the observation log tells it what happened, the episode ID tells it where to look for the full record.

Episode IDs are zero-padded to three digits (e.g., `ep-001`, `ep-012`) and monotonic within the global log. The ID is derived from the current max in the episodes directory, not from a separate counter file.

**Key properties:**
- Frequent, small-batch compression (soft threshold triggers cooldown, force threshold fires immediately — not at context overflow)
- Event-based structure preserved — reads like a decision log, not documentation
- Each compression run produces a discrete episode with a unique ID and persisted artifacts (transcript, observations archive, index chunks)
- Observation prefix stays stable between Observer runs, enabling prompt cache hits
- Observer can be run by a cheap, high-throughput model (e.g., Gemini Flash) since the work is extraction, not reasoning

#### Tier 2: Reflector

When the observation log itself grows past a second threshold (default ~40k tokens), the Reflector condenses it. It reads the full `observations.json`, sends it to an LLM for reorganization, and writes the compressed result back to `observations.json`. The structure stays the same — a flat array of observation objects with timestamps, project contexts, and content — it just gets denser.

The Reflector merges related observations, deduplicates overlapping entries, and drops superseded information (e.g., "decided to use Nginx" followed later by "switched to Traefik" — the Nginx entry can go). Original observations are backed up to `observations.json.bak` before replacement. If the LLM returns an empty result, the reflector rejects it and preserves the original log — this prevents a bad model response from wiping history.

The observation log never becomes a narrative blob. Even after reflection, it remains a structured series of dated observations. It just gets denser.

**Key properties:**
- Runs infrequently (only when observation log exceeds threshold)
- Operates in-place on `observations.json` — reads, compresses, writes back. Same format, fewer entries.
- Original log is backed up before replacement; empty LLM responses are rejected
- Only operation that fully invalidates prompt cache (acceptable given infrequency)
- Superseded information is dropped during merging
- Original episode transcripts remain in `memory/episodes/` — the Reflector compresses the observation log, not the raw record

**Threshold tuning:** The ~40k token default for Reflector triggering interacts with the target model's context window size and should be configurable relative to it rather than treated as a fixed constant. A model with a 200k context can afford a larger observation log before reflection is needed; a model with 100k may need a lower threshold. The config should express this as either an absolute token count or a percentage of the model's context budget allocated to the observation log.

### Interaction with Existing Systems

- **Hybrid search**: Episode artifacts persisted under `memory/episodes/YYYY-MM/DD/` are indexed for search. The `.idx.jsonl` chunks feed the BM25 index, and the `.obs.json` archives provide per-episode observation retrieval. When the Reflector compresses observations out of the active log, the agent can still retrieve the full detail by searching episode transcripts directly. This provides deep retrieval for older history without the agent needing to guess that it should look.
- **Restarts**: The observation log and recent context carry across restarts. A new run loads the existing observation block and `recent_context.json` (narrative from the latest observation), so context from last week is present without requiring a search decision.

### Model Selection

The Observer, Reflector, and pulse evaluation don't require frontier-level reasoning. Their jobs are structured extraction, reorganization, and quick checks — work well-suited to fast, cheap models. Recommended defaults:

- **Observer**: High-throughput model (e.g., Gemini Flash, GPT-5-mini, local model with sufficient context)
- **Reflector**: Same tier — slightly more nuanced work, but still extraction, not generation
- **Pulse evaluation**: Small model tier by default (cheap/fast). Pulses are ambient checks, not complex reasoning — most return HEARTBEAT_OK. The `agent` field on a pulse can override this when a specific pulse needs a different tier or the main agent.

Token cost per Observer run is modest relative to the tokens saved by not carrying raw history. The system is specifically designed to be a net cost reduction.

---

## Proactivity System

### Problem

OpenClaw's heartbeat fires a full agent turn every N minutes (default 30). The LLM reads HEARTBEAT.md — a freeform markdown checklist — and decides whether anything needs attention. Most of the time it returns HEARTBEAT_OK. This burns tokens on scheduling logic that doesn't require intelligence.

The heartbeat-state.json tracking pattern (checking which task is most overdue) is a community-invented workaround for the lack of per-task scheduling in the heartbeat system.

Additionally, the agent has no structured framework for deciding where results should be delivered — to the agent, to an external notification service, or silently to a log.

### Solution: Structured Pulse Scheduling + Channel-Based Notification Routing

Replace HEARTBEAT.md with **HEARTBEAT.yml** for machine-parseable scheduling, and add **CHANNELS.yml** for channel registry. Pulse routing is declared on each pulse in HEARTBEAT.yml via a `channels:` field. See [Notification Routing Design](notification-routing-design.md) for the full specification.

### HEARTBEAT.yml

A YAML file that defines **pulses** — groups of related tasks on a shared schedule.

```yaml
pulses:
  - name: work_check
    enabled: true
    schedule: "30m"
    active_hours: "08:00-18:00"
    tasks:
      - name: inbox_scan
        prompt: "Check for urgent unread emails"
      - name: pr_review
        prompt: "Any PRs waiting on my review?"
      - name: blocked_tasks
        prompt: "Any tasks stalled or waiting on input?"

  - name: daily_review
    enabled: true
    schedule: "24h"
    active_hours: "08:00-09:00"
    agent: main
    tasks:
      - name: morning_brief
        prompt: "Summarize today's calendar and top priorities"
      - name: follow_up_check
        prompt: "Any pending follow-ups from yesterday?"

  - name: evening_wind_down
    enabled: false
    schedule: "2h"
    active_hours: "18:00-22:00"
    tasks:
      - name: tomorrow_prep
        prompt: "Anything I should prep for tomorrow?"
```

#### Scheduling behavior

The **gateway** handles all scheduling logic:

1. Parse HEARTBEAT.yml on startup and on file change (hot-reload, consistent with existing config watching).
2. Track per-pulse last-run timestamps (replaces heartbeat-state.json).
3. On each scheduling tick, check which pulses are due based on their `schedule` and `active_hours`.
4. Only invoke the LLM for pulses that are actually due, passing the pulse's tasks as context.
5. If no pulses are due, no agent turn fires. Zero token cost.

The LLM is no longer a scheduler. It only receives focused task groups when there's actual work to evaluate.

#### Pulse fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Identifier for the pulse |
| `enabled` | bool | Whether the pulse is active. Defaults to `true`. Allows disabling a pulse without deleting its configuration. |
| `schedule` | duration string | How often the pulse fires (e.g., `30m`, `2h`, `24h`) |
| `active_hours` | time range | Window during which the pulse is eligible (respects user timezone from USER.md) |
| `agent` | string or null | Controls execution routing. `~` (null/omitted) = sub-agent with small model tier. `"main"` = main agent wake turn. `"<preset-name>"` = sub-agent with named preset from `subagents/`. |
| `tasks` | list | Tasks the LLM evaluates when the pulse fires |

#### Task fields

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Identifier for the task |
| `prompt` | string | Instruction passed to the LLM |

#### Agent self-modification

The agent can add, remove, or modify pulses and tasks by editing HEARTBEAT.yml, consistent with the existing self-evolving workspace pattern. Example: "Add a pulse that checks my GitHub notifications every hour during work hours."

### CHANNELS.yml

A YAML file defining the channel registry — what channels exist. Pulse routing is declared on each pulse in HEARTBEAT.yml via the `channels:` field:

```yaml
# In HEARTBEAT.yml:
pulses:
  - name: work_check
    schedule: "30m"
    channels: [agent_wake, ntfy]
    tasks:
      - name: inbox_scan
        prompt: "Check for urgent unread emails"

  - name: daily_review
    schedule: "24h"
    channels: [agent_feed, ntfy]
    agent: main
    tasks:
      - name: morning_brief
        prompt: "Summarize today's calendar and top priorities"
```

Built-in channels: `agent_wake` (inject into feed + start turn if idle), `agent_feed` (inject into feed passively), `inbox` (store silently). External channels (ntfy, webhook, etc.) are defined in `config.toml` under `[notifications.channels]`.

A pulse with no `channels` field has its results dropped after transcript storage and a warn-level log is emitted to surface the misconfiguration. HEARTBEAT_OK results (nothing actionable) are never routed regardless of channel configuration.

#### Self-evolution

Following the existing workspace pattern (mirroring SOUL.md's "this file is yours to evolve"), the agent refines pulse routing in HEARTBEAT.yml over time based on user feedback:

- User consistently ignores a pulse's results → agent changes its channels from `agent_feed` to `inbox` or removes the channel entirely
- User responds urgently to a specific type of finding → agent adds `agent_wake` to the pulse's channels
- User explicitly says "don't bother me with PR reviews from the docs repo" → agent removes the relevant channel from the pulse

### Interaction with Existing Systems

- **Scheduled actions**: Unchanged. Scheduled actions handle deterministic work ("run this backup script at 3am"). Pulses handle ambient awareness ("is anything worth my attention right now?"). These are complementary, not overlapping. Both heartbeat pulses and scheduled actions use direct channel routing — pulses declare channels in HEARTBEAT.yml, and scheduled actions specify channels at creation time via the `channels` parameter.
- **HEARTBEAT_OK**: Still used as the ack signal when a pulse evaluation finds nothing actionable. HEARTBEAT_OK results are never routed.
- **Observation log**: Findings from pulse evaluations feed into the OM observation log when they're routed to `agent_wake` or `agent_feed` and processed by the main agent. Results routed only to external channels or inbox enter the observation stream when the agent reviews them.

---

## How The Systems Compose

These two systems are designed independently but share a data layer, which means improvements to one naturally benefit the other.

**Memory → Proactivity**: A richer observation log means pulse task evaluations have better context. When the agent checks "any pending follow-ups?", it's reasoning over a dense event history, not hoping it remembered to write something to MEMORY.md.

**Proactivity → Memory**: Pulse findings routed to the agent feed append to the observation log through the standard compression path. The agent's ambient awareness of inbox state, calendar, task progress, etc. becomes part of the persistent record without explicit memory-write decisions.

The user and agent can deepen this integration over time (e.g., pulses that specifically reason over the observation log), but the systems function independently by default.

---

## Implementation Notes

### Priorities
1. HEARTBEAT.yml + gateway-level scheduling (highest impact-to-effort ratio)
2. CHANNELS.yml registry + notification channel infrastructure
3. Observer integration
4. Reflector integration

### Considerations
- Observer/Reflector model selection should be configurable but default to cheap/fast
- HEARTBEAT.yml schema should be validated at gateway startup with clear error messages
- CHANNELS.yml should ship with minimal defaults (empty channel registry) to avoid surprising behavior on first run
- Observation log storage location should be consistent with existing workspace layout
- Migration path from existing HEARTBEAT.md should be documented (or automated)
