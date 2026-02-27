# Personal AI Agent — Background Tasks & Turn Loop Interrupts

## Overview

This document describes two related changes that share a common mechanism:

1. **Background tasks** — Pulse evaluations, scheduled action background jobs, and agent-spawned subagents all run on separate threads, decoupled from the main agent.
2. **Turn loop interrupts** — User messages and background task results can be injected into the main agent's turn loop between tool iterations, rather than queuing behind the entire turn.

These solve three concrete problems:

- **Pulse/scheduled actions block the main agent.** Currently, pulse evaluation and scheduled action execution run as main agent turns. If a pulse fires while the agent is mid-conversation, it waits. If the user sends a message while a pulse is evaluating, it waits. Everything is synchronous.
- **The user can't steer mid-turn.** When the agent is in a multi-tool loop (read file → exec → read again → ...), user messages queue behind the entire sequence. The user can't say "actually, use port 8080" until the agent finishes and delivers its response.
- **No fire-and-forget subagents.** The main agent has no way to delegate a self-contained task to a background worker and continue its conversation.

The solution is two primitives: a **BackgroundTask** (the lifecycle and routing envelope) and a **SubAgent** (the LLM-powered execution engine). BackgroundTask handles spawning, concurrency, cancellation, and result routing. SubAgent is one kind of thing a BackgroundTask runs — but not the only kind.

---

## Design Philosophy

Same principles as the rest of IronClaw:

1. **Put the right work in the right place.** Background scheduling is gateway work. Background evaluation is LLM work on a cheap model. Routing results is gateway work. The main agent only sees outcomes. And if the task doesn't need an LLM at all — just run it directly.
2. **Independent systems that compose through shared data.** Background tasks write transcripts to disk and deliver results through a channel. The main agent's observer compresses them naturally. No direct coupling between subsystems.
3. **Simplicity that stays practical.** One envelope (BackgroundTask), one result channel (interrupt_tx), one injection point (between turn loop iterations). Three problem sources, one mechanism.

---

## BackgroundTask: The Container

A BackgroundTask is the lifecycle envelope. It handles spawning, concurrency permits, cancellation, result routing, and transcript storage. It doesn't care what runs inside it.

### Definition

```rust
struct BackgroundTask {
    id: String,                      // unique ID (e.g., "bg-pulse-work_check-001")
    task_name: String,               // name used for routing lookup
    source: TaskSource,
    execution: Execution,
    routing: ResultRouting,          // how to route the result
}

enum ResultRouting {
    Notify,                          // look up task_name in NOTIFY.yml (pulses, scheduled actions)
    Direct(Vec<String>),             // dispatch to these channels (agent-spawned)
}

enum TaskSource {
    Pulse { pulse_name: String },
    Action { action_id: String },
    Agent { parent_turn_id: String },
}

enum Execution {
    SubAgent(SubAgentConfig),
    Script(ScriptConfig),
}
```

### Lifecycle

Every BackgroundTask, regardless of execution type, follows the same lifecycle:

1. **Acquire** a semaphore permit (bounded concurrency).
2. **Spawn** on a tokio task.
3. **Execute** (SubAgent turn loop, script invocation, etc.).
4. **Write** transcript/output to disk.
5. **Produce** a `BackgroundResult`.
6. **Send** to the interrupt channel.
7. **Release** permit on drop.

The spawner, concurrency control, cancellation token, result routing, and transcript storage are all properties of BackgroundTask. They work identically whether the task runs an LLM turn or a shell script.

---

## Execution: SubAgent

A SubAgent is an LLM-powered execution that runs a simplified turn loop. This is the execution type for pulse evaluations, scheduled actions that need reasoning, and agent-spawned work.

### Configuration

```rust
struct SubAgentConfig {
    prompt: String,                  // the task instructions
    context: Option<String>,         // optional inline context from the spawner
    context_files: Vec<PathBuf>,     // optional files to include in context
    tools: Option<Vec<ToolName>>,    // tool restriction (None = full default set)
    model_tier: ModelTier,           // small, medium, or large
}

enum ModelTier {
    Small,    // cheap/fast — extraction, simple checks
    Medium,   // balanced — most background work
    Large,    // frontier — complex reasoning tasks
}
```

### Model tiers

Rather than configuring a model per subsystem (pulse model, action model, subagent model), background tasks use a tiered model configuration:

```toml
[background]
max_concurrent = 3

[background.models]
small = "gemini/gemini-2.5-flash"
medium = "anthropic/claude-haiku-4-5"
large = "anthropic/claude-sonnet-4-6"
```

All three are optional. Unset tiers fall back upward: small defaults to medium, medium defaults to large, large defaults to the main agent model.

| Source | Default tier | Rationale |
|--------|-------------|-----------|
| Pulse | `Small` | Extraction, monitoring — not reasoning |
| Scheduled action (background) | `Medium` | Varies by task |
| Agent-spawned | `Medium` | Worker tasks — agent can override to `Large` |

### Context assembly

SubAgents get minimal context by default:

| Included | Rationale |
|----------|-----------|
| Task prompt | The work to be done |
| USER.md | Timezone, preferences that affect judgment |
| ENVIRONMENT.md | Local environment notes |
| Projects index | SubAgents can activate/deactivate projects |
| `context` (if provided) | Inline context from the spawner |
| `context_files` (if provided) | Explicit file references from the spawner |

| Excluded | Rationale |
|----------|-----------|
| SOUL.md | Worker, not conversationalist |
| Observation log | No need for history — task is self-contained |
| Skills metadata | Not activating skills (can read files directly if needed) |
| MEMORY.md | Curated memory is for the main agent's judgment |
| Recent messages | Not part of the conversation |

### Available tools

SubAgents have access to the full tool set by default, including project management:

| Tool | Available | Notes |
|------|-----------|-------|
| `read` | Always | Read any workspace file |
| `write` | Always | Subject to project write scoping when a project is active |
| `edit` | Always | Subject to project write scoping |
| `exec` | Always | Shell command execution |
| `memory_search` | Always | Can search for relevant history |
| `memory_get` | Always | Can retrieve episode details |
| `project_activate` | Always | Can activate a project for scoped work |
| `project_deactivate` | Always | **Must** deactivate before returning |
| `project_list` | Always | Can browse available projects |
| `action_*` | No | SubAgents don't schedule actions |
| `skill_*` | Yes | SubAgents can use skills |
| `subagent_spawn` | No | No sub-to-sub delegation |
| `stop_agent` | No | SubAgents don't cancel other tasks |

The `subagent_spawn` tool's `tools` parameter can restrict this set further.

### Turn loop

Each SubAgent runs a simplified turn loop — same structure as the main agent's, but with the stripped-down context and no interrupt checking (these are fire-and-forget workers that run to completion or cancellation):

1. Assemble minimal context (task prompt + USER.md + ENVIRONMENT.md + projects index + context).
2. Call model provider.
3. Execute tool calls if any, loop back.
4. Check cancellation token between iterations.
5. On final response: verify no project is still active. If one is, force deactivation with auto-generated log.
6. Return summary text.

---

## Execution: Script

A ScriptConfig is a direct command execution with no LLM involvement. This is the execution type for scheduled actions that just need to run a command.

### Configuration

```rust
struct ScriptConfig {
    command: String,
    args: Vec<String>,
    working_dir: Option<PathBuf>,
    timeout_secs: u64,
}
```

### Execution

1. Spawn the command as a child process.
2. Capture stdout and stderr.
3. Wait for completion (or timeout/cancellation).
4. The `summary` in `BackgroundResult` is the command's stdout (truncated to a reasonable limit). Stderr is included if the exit code is non-zero.

No LLM call, no context assembly, no token cost. A scheduled action like "run this backup script at 3am" executes directly. The result flows through the same routing as any other BackgroundTask.

### When to use Script vs SubAgent

The distinction maps to the existing scheduled action design's delivery modes:

- Scheduled actions that need judgment (evaluate, summarize, decide) → SubAgent
- Scheduled actions that need execution (run script, call API, invoke command) → Script
- Pulse tasks → always SubAgent (the whole point is LLM evaluation)
- Agent-spawned tasks → always SubAgent

---

## Task Spawning and Lifecycle

### BackgroundTaskSpawner

The spawner manages the lifecycle of all background tasks regardless of execution type:

```rust
struct BackgroundTaskSpawner {
    interrupt_tx: mpsc::Sender<Interrupt>,
    semaphore: Arc<Semaphore>,
    active_tasks: Arc<Mutex<HashMap<String, CancellationToken>>>,
}

impl BackgroundTaskSpawner {
    async fn spawn(&self, task: BackgroundTask) -> Result<String> {
        let permit = self.semaphore.clone().acquire_owned().await?;
        let tx = self.interrupt_tx.clone();
        let cancel = CancellationToken::new();
        let cancel_clone = cancel.clone();
        let id = task.id.clone();

        tokio::spawn(async move {
            let _permit = permit;
            let result = tokio::select! {
                res = execute_task(task) => res,
                _ = cancel_clone.cancelled() => {
                    BackgroundResult::cancelled(task)
                }
            };
            let _ = tx.send(Interrupt::BackgroundResult(result)).await;
        });

        self.active_tasks.lock().await.insert(id.clone(), cancel);
        Ok(id)
    }

    async fn cancel(&self, id: &str) -> Result<bool> {
        if let Some(token) = self.active_tasks.lock().await.remove(id) {
            token.cancel();
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

async fn execute_task(task: BackgroundTask) -> BackgroundResult {
    match task.execution {
        Execution::SubAgent(config) => execute_subagent(task.id, task.source, config).await,
        Execution::Script(config) => execute_script(task.id, task.source, config).await,
    }
}
```

### Concurrency limit

A bounded semaphore (default: 3) caps concurrent background tasks of any type. SubAgents and scripts share the same pool.

```toml
[background]
max_concurrent = 3
```

### Cancellation

Any running background task can be cancelled by the main agent via the `stop_agent` tool or by the gateway on shutdown.

The cancellation token is checked at different granularities depending on execution type:

| Execution type | Check point | Cleanup on cancel |
|---------------|-------------|-------------------|
| SubAgent | Between tool iterations | Force-deactivate any active project with log: `"[cancelled] SubAgent {id} was stopped."` Write partial transcript. |
| Script | Periodically during process wait | Kill child process (SIGTERM → SIGKILL). Capture partial output. |

Cancellation is cooperative for SubAgents — the worst-case delay is one LLM round-trip. For scripts, the child process is killed directly.

### Transcript storage

All background task output is written to a dedicated directory:

```
memory/
├── episodes/              # conversation episodes (observer output)
│   └── YYYY-MM/DD/
└── background/            # background task transcripts and output
    └── YYYY-MM/DD/
        ├── bg-{id}.jsonl  # SubAgent transcripts (same format as episodes)
        └── bg-{id}.log    # Script output (stdout + stderr)
```

These are not episodes in the OM sense — the observer does not compress them directly. When background results are injected into the main agent's message stream, the observer captures the summaries naturally alongside regular conversation. The full transcript/output on disk serves as a retrieval target.

Background transcripts are indexed by the memory search system alongside episode transcripts. The `source_type` in the search index distinguishes them.

---

## Result Flow

A background task produces a `BackgroundResult` when it completes:

```rust
struct BackgroundResult {
    id: String,
    task_name: String,             // used for NOTIFY.yml routing lookup
    source: TaskSource,
    summary: String,               // SubAgent: LLM's final text. Script: stdout.
    transcript_path: PathBuf,
    status: TaskStatus,
    timestamp: DateTime<Utc>,
}

enum TaskStatus {
    Completed,
    Cancelled,
    Failed { error: String },
}
```

### Routing via NOTIFY.yml

Results are routed by task name, not by urgency level. When a `BackgroundResult` arrives, the gateway looks up `task_name` in `NOTIFY.yml` and dispatches to every channel that lists it.

See [Notification Routing Design](notification-routing-design.md) for the full NOTIFY.yml specification. Summary:

- **`agent_wake`** — Inject into message feed. Start a turn if idle.
- **`agent_feed`** — Inject into message feed. Wait for next interaction.
- **`inbox`** — Create an `InboxItem`. Never enters the message feed.
- **External channels** (ntfy, webhook) — Deliver via HTTP to the configured service.
- **Not listed** — Result is not delivered. Transcript is preserved on disk.

A task can appear in multiple channels. Dispatch to all listed channels happens in parallel.

### InboxItem

Results routed to the `inbox` channel produce inbox items:

```rust
struct InboxItem {
    id: String,
    source: TaskSource,
    title: String,
    summary: String, // Truncated contents
    content_path: PathBuf,
    timestamp: DateTime<Utc>,
    read: bool,
}
```

Inbox items are persisted to `workspace/inbox/inbox.json` — a simple JSON array. The agent has `inbox_list` and `inbox_clear` tools for management. Unread inbox items are surfaced in context assembly as a count: `"You have 3 unread inbox items."` — not their contents, so they don't consume token budget.

### What the main agent sees

For results routed to `agent_wake` or `agent_feed`:

```
[Background: work_check] Found 2 urgent emails requiring response.
PR #421 from @alice has been waiting 3 days for review.
```

For script results:

```
[Background: nightly_backup] Exit 0. Backup completed: 2.3GB written to /mnt/backup/2026-02-25.tar.gz
```

### Subagent results from `subagent_spawn`

Agent-spawned subagent results are routed to the channels specified in the `subagent_spawn` tool call (default: `["agent_feed"]`). This bypasses NOTIFY.yml — the main agent decides at spawn time where the result goes, and the gateway validates the channel names against built-in channels and `config.toml`.

The exception is `wait: true` mode: the main agent's turn loop blocks at that tool call until the subagent completes, then returns the result as the tool response. This is the synchronous escape hatch for "compute this before I respond."

---

## Turn Loop Interrupts

### The interrupt channel

A single `mpsc` channel carries all interrupts to the active turn loop:

```rust
enum Interrupt {
    UserMessage(InboundMessage),
    BackgroundResult(BackgroundResult),
}
```

The gateway, channels, and BackgroundTaskSpawner are all producers. The turn loop is the sole consumer.

### Injection point

Between every tool-execution-→-LLM-call boundary, the turn loop drains the interrupt channel:

```rust
for iteration in 0..MAX_ITERATIONS {
    // === Interrupt check point ===
    while let Ok(interrupt) = interrupt_rx.try_recv() {
        match interrupt {
            Interrupt::UserMessage(msg) => {
                messages.push(Message::user(msg.content));
            }
            Interrupt::BackgroundResult(result) => {
                // NOTIFY.yml routing already dispatched to external channels
                // and inbox. Only agent_wake and agent_feed results arrive here.
                messages.push(Message::system(format_background_result(&result)));
            }
        }
    }

    let response = provider.complete(&messages).await?;

    if response.tool_calls.is_empty() {
        break;  // final response, exit loop
    }

    for call in &response.tool_calls {
        let result = execute_tool(call).await?;
        messages.push(Message::tool_result(call.id, result));
    }
}
```

`try_recv()` is non-blocking. If nothing is pending, the loop continues without delay. If multiple interrupts have accumulated, they're all drained and injected before the next LLM call.

Only results routed to `agent_wake` or `agent_feed` via NOTIFY.yml are sent through the interrupt channel. Inbox items and external channel deliveries are handled at the routing step and never enter the turn loop.

### What the LLM sees

After interrupt injection, the message sequence looks like:

```
... (previous messages)
assistant: [tool_call: exec("cargo build --release")]
tool: "Compiling ironclaw v0.1.0 ..."
user: "Actually, can you add the --features discord flag?"        ← injected
system: "[Background: inbox_scan] 2 urgent emails found."        ← injected (medium/high)
assistant: (next LLM response — sees the amendment and background context)
```

The LLM handles this naturally. It sees the tool result, the user's amendment, and any background context, then produces its next response accounting for all of it.

### Mid-completion messages

Messages that arrive during an LLM completion (while streaming a response) are not injected into that completion. They wait for the next check point:

- **If the LLM produces tool calls:** The interrupt is injected after tool execution, before the next LLM call.
- **If the LLM produces a final text response (no tool calls):** The response is delivered, the turn ends, and the queued message starts a new turn.

This covers the real pain point (user can't steer during long multi-tool sequences) without the complexity and wasted tokens of aborting mid-stream completions.

---

## Interaction with Projects

SubAgents are full participants in the project system. They can activate projects, read project files, write to project workspaces, and use project-scoped tools and MCP servers. Multiple agents (main + subagents, or multiple subagents) can have the same project active simultaneously.

### No project locking

There is no locking or mutual exclusion on project activation. Two subagents can work in the same project workspace concurrently. Rationale:

- Writes to **different files** have no conflict.
- Writes to the **same file** are last-write-wins — consistent with the file-first model everywhere else.
- **Deactivation logs** append to a daily log file (`notes/log/YYYY-MM/log-DD.md`). Multiple appends are safe.

The filesystem is the concurrency model, same as it is for the user editing files while the agent works.

### MCP server reference counting

The one edge case is project-scoped MCP servers. When a project activates, its MCP servers start. When it deactivates, they should tear down — but only if no other agent still has that project active.

The MCP registry tracks activation counts per project. A server starts on the first activation (count 0 → 1) and tears down when the last deactivation drops the count to zero. This is lightweight reference counting, not locking — activations never block.

```rust
// In McpRegistry
struct ProjectMcpState {
    active_count: usize,
    servers: Vec<McpServerHandle>,
}
```

### Mandatory deactivation

A SubAgent must deactivate any active project before returning its final result. The gateway enforces this — if a SubAgent's turn loop ends with a project still active, the gateway forces deactivation with an auto-generated log entry: `"[auto] SubAgent {id} completed without deactivating. Task: {prompt truncated to 200 chars}"`.

This ensures every SubAgent interaction with a project gets logged, maintaining the session log continuity the Projects system depends on. The auto-generated log is a safety net, not the intended path — the SubAgent's system prompt instructs it to deactivate explicitly with a proper session log.

### Cancellation with active project

If a SubAgent is cancelled while a project is active, the cancellation handler force-deactivates the project with a log entry: `"[cancelled] SubAgent {id} was stopped. Work may be incomplete."` The MCP ref count is decremented normally. This maintains project log continuity even on abnormal termination.

### A typical project-aware SubAgent flow

1. SubAgent receives task: "Run the test suite for the aerohive playbooks."
2. SubAgent calls `project_activate("aerohive-setup")`.
3. Project context loads — workspace files, tools (exec now available), MCP servers (ref count incremented).
4. SubAgent executes the work within the project's write scope.
5. SubAgent calls `project_deactivate` with a session log summarizing what it did (ref count decremented; MCP servers torn down if count hits zero).
6. SubAgent returns its final result.

---

## Changes to Existing Systems

### Pulse scheduling (`pulse/`)

Pulse evaluation moves from a main agent turn to a background task:

**Before:**
```
Pulse due → full main agent turn (blocks everything)
```

**After:**
```
Pulse due → BackgroundTaskSpawner.spawn(BackgroundTask {
    task_name: pulse.name,
    source: Pulse { pulse_name },
    execution: SubAgent(SubAgentConfig {
        prompt: concatenated task prompts,
        agent: memory,
        ..
    }),
})
```

HEARTBEAT_OK is the only gate. If the SubAgent's summary contains the HEARTBEAT_OK sentinel, the result is logged silently and not routed. Otherwise, the result is dispatched to every channel that lists the pulse name in `NOTIFY.yml`.

### Scheduled action execution (`actions/`)

Scheduled actions already have `UserVisible` and `Background` delivery modes:

| Mode | Current behavior | New behavior |
|------|-----------------|--------------|
| `UserVisible` | Enqueue as system event in main agent turn | Routed via NOTIFY.yml — list the action in `agent_wake` or `agent_feed` |
| `Background` | Dedicated agent thread (partially implemented) | `BackgroundTaskSpawner.spawn()` — uses Script execution for simple commands, SubAgent for tasks needing judgment. Routing determined by NOTIFY.yml. |

### Gateway event loop

The `tokio::select!` loop simplifies. Pulse and scheduled actions no longer need executor arms that run full agent turns. They spawn background tasks and return immediately.

Background results are routed via NOTIFY.yml. Results destined for `agent_wake` or `agent_feed` flow through `interrupt_tx`. If no turn is active, the gateway's idle-state handler picks them up: `agent_feed` results queue for the next turn, `agent_wake` results start a new turn.

### Agent state tracking

The gateway tracks whether a turn is active to route interrupts correctly:

```rust
enum AgentState {
    Idle,
    Busy { interrupt_tx: mpsc::Sender<Interrupt> },
}
```

When a turn starts, the gateway transitions to `Busy` and holds the sender half. When the turn ends, it transitions back to `Idle`, and pending `agent_feed` results queue for the next turn.

---

## Agent-Facing Tools

### subagent_spawn

```json
{
    "name": "subagent_spawn",
    "parameters": {
        "task": {
            "type": "string",
            "description": "What to do — self-contained task description"
        },
        "agent_name": {
            "type": "string",
            "description": "Name of agent to call. Default: None."
        },
        "model_override": {
            "type": "string",
            "enum": ["small", "medium", "large"],
            "description": "Model tier override for the subagent. Default: medium."
        },
        "channels": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Notification channels for the result (e.g., [\"agent_feed\", \"ntfy\"]). Must be built-in or defined in config.toml. Default: [\"agent_feed\"]."
        },
        "wait": {
            "type": "boolean",
            "description": "If true, block until subagent completes and return its result. Default: false."
        }
    },
    "required": ["task"]
}
```

**Synchronous mode** (`wait: true`): The tool call blocks the main agent's turn loop until the SubAgent finishes. The SubAgent's summary is returned as the tool result. The `channels` parameter is ignored — the result is returned directly as the tool response.

Note: while blocked on a synchronous subagent, the main turn loop is not draining interrupts. User messages that arrive during this window are queued and injected after the subagent result is processed. If this becomes a pain point, the wait can be restructured to poll both the subagent completion and the interrupt channel.

**Asynchronous mode** (`wait: false`, default): Returns immediately with `"Subagent spawned: {id}."` The result is dispatched to the channels specified in the `channels` parameter. The gateway validates that each channel name is either built-in (`agent_wake`, `agent_feed`, `inbox`) or defined in `config.toml` — invalid channel names are rejected at spawn time with an error.

### stop_agent

```json
{
    "name": "stop_agent",
    "parameters": {
        "task_id": {
            "type": "string",
            "description": "ID of the background task to cancel"
        }
    },
    "required": ["task_id"]
}
```

Returns `"Cancelled task {id}."` if the task was running, or `"No active task with id {id}."` if it already completed.

### list_agents

```json
{
    "name": "list_agents",
    "parameters": {}
}
```

Returns a list of active background tasks with their IDs, sources, execution types, prompts (truncated), and how long they've been running.

### inbox_list

```json
{
    "name": "inbox_list",
    "parameters": {
        "unread_only": {
            "type": "boolean",
            "description": "Only show unread items. Default: true."
        }
    }
}
```

### inbox_clear

```json
{
    "name": "inbox_clear",
    "parameters": {
        "item_ids": {
            "type": "array",
            "items": { "type": "string" },
            "description": "Specific item IDs to clear. If omitted, clears all read items."
        }
    }
}
```

---

## Interaction with Observational Memory

Background results flow into the observation log through the standard path:

1. Background task completes → result routed via NOTIFY.yml.
2. Results routed to `agent_wake` or `agent_feed` are injected into the main agent's message stream as system messages.
3. The main agent processes them. The observer eventually compresses this exchange into an episode, tagged with `Background` visibility.
4. Results routed only to `inbox` or external channels enter the observation stream when the agent reviews them (e.g., via `inbox_list`).

There is no direct coupling between background tasks and the observer. The observer sees background results the same way it sees any other message in the conversation stream. The `Background` visibility tag (already in the OM design) ensures these observations don't pollute the user-facing conversation record.

---

## Configuration

```toml
[background]
max_concurrent = 3                    # max simultaneous background tasks
transcript_retention_days = 30        # auto-cleanup for background transcripts

[background.models]
small = "gemini/gemini-2.5-flash"
medium = "anthropic/claude-haiku-4-5"
large = "anthropic/claude-sonnet-4-6"
# Unset tiers fall back upward: small → medium → large → main agent model
```

---

## Data Flow

### Pulse evaluation

```
Scheduler tick
      │
      ▼
Check due pulses (HEARTBEAT.yml + timestamps)
      │
      ├── Nothing due → no-op (zero cost)
      │
      └── Pulse due → BackgroundTaskSpawner.spawn()
                          │
                     (runs on thread pool)
                          │
                          ▼
                    SubAgent turn
                    (minimal context, small model)
                          │
                    ┌─────┴──────┐
                    │            │
              HEARTBEAT_OK    Result
                    │            │
              Log silently   Route via NOTIFY.yml
                             (dispatch to all channels
                              listing this pulse name)
```

### Scheduled action script execution

```
Action tick (background mode, script)
      │
      ▼
BackgroundTaskSpawner.spawn(Script { command, args })
      │
      ▼
Child process executes (no LLM, zero tokens)
      │
      ▼
BackgroundResult { summary: stdout }
      │
      ▼
Route via NOTIFY.yml (same as any background task)
```

### User message mid-turn

```
User sends message during multi-tool sequence
      │
      ▼
Gateway sees AgentState::Busy
      │
      ▼
Send Interrupt::UserMessage to interrupt_tx
      │
      ▼
Turn loop: after current tool execution completes
      │
      ▼
try_recv() drains interrupt channel
      │
      ▼
User message appended to messages array
      │
      ▼
Next LLM call sees: original request + tool results + user amendment
      │
      ▼
LLM adjusts its approach based on the new input
```

### Subagent with project work (async)

```
Main agent calls subagent_spawn(
    task: "Run aerohive test suite",
    channels: ["agent_feed", "ntfy"],
    wait: false
)
      │
      ▼
Gateway validates channel names → all valid
      │
      ▼
BackgroundTaskSpawner.spawn(SubAgent, routing: Direct(["agent_feed", "ntfy"]))
      │
      ├── Tool returns immediately: "Subagent spawned: bg-042"
      │   (main agent continues its turn)
      │
      └── SubAgent runs on thread pool
                │
                ├── project_activate("aerohive-setup")
                │   (MCP ref count: 0 → 1, servers start)
                │
                ├── read, exec, write within project workspace
                │
                ├── project_deactivate(log: "Ran test suite. 2 failures...")
                │   (MCP ref count: 1 → 0, servers tear down)
                │
                ▼
          BackgroundResult
                │
                ▼
          Dispatch to specified channels: agent_feed + ntfy
```

---

## Implementation Phases

Prereqs: Inbox system, NOTIFY.yml parsing and routing (see [Notification Routing Design](notification-routing-design.md)).

### Phase 1: Interrupt channel and turn loop check points

The highest-impact change: user messages can be injected mid-turn.

- Define `Interrupt` enum and create the `mpsc` channel.
- Add `AgentState` tracking to the gateway.
- Modify `execute_turn()` to accept an `interrupt_rx` and drain it between iterations.
- Route user messages through `interrupt_tx` when agent is busy.
- Tests: simulate user message during multi-tool turn, verify injection.

**Milestone: User can steer the agent mid-turn.**

### Phase 2: BackgroundTask primitive and spawner

- Define `BackgroundTask`, `BackgroundResult`, `TaskSource`, `TaskStatus`, `Execution`, `ModelTier`.
- Implement `BackgroundTaskSpawner` with semaphore-bounded `tokio::spawn` and `CancellationToken`.
- Implement SubAgent execution: minimal context assembly (USER.md + ENVIRONMENT.md + projects index + task prompt), simplified turn loop, transcript writing.
- Implement Script execution: child process spawning, output capture, timeout.
- NOTIFY.yml parsing and routing: load routing config, dispatch results to listed channels by task name.
- `NotificationChannel` trait and built-in channel implementations (`agent_wake`, `agent_feed`, `inbox`).
- `InboxItem` struct and `inbox.json` persistence.
- Inbox item count note in context assembly.
- Tests: spawn both execution types, verify result delivery via NOTIFY.yml routing, verify concurrency limit.

**Milestone: Background tasks run independently and deliver results.**

### Phase 3: Cancellation and management tools

- Implement cancellation via `CancellationToken` (SubAgent) and process kill (Script).
- Implement `stop_agent` tool.
- Implement `list_agents` tool.
- Tests: cancel running SubAgent mid-turn, cancel running script, verify cleanup.

**Milestone: Main agent can monitor and cancel background work.**

### Phase 4: Project-aware subagents

- Enable project tools in SubAgent tool set.
- Implement MCP server reference counting in `McpRegistry`.
- Implement mandatory deactivation enforcement: check on SubAgent turn completion, force-deactivate with auto-log if needed.
- Implement cancellation cleanup: force-deactivate with cancellation log, decrement MCP ref count.
- Tests: SubAgent activates project, does work, deactivates. Two SubAgents in same project simultaneously. Cancellation mid-project. MCP ref count lifecycle.

**Milestone: SubAgents can work within project contexts.**

### Phase 5: Pulse and scheduled action migration

- Modify pulse executor to spawn BackgroundTask (SubAgent execution) instead of running a main agent turn. Remove `load_alerts()` and Alerts.md concatenation from pulse execution path.
- Modify scheduled action background-mode executor to use BackgroundTaskSpawner — Script execution for simple commands, SubAgent for reasoning tasks.
- Remove scheduled action `UserVisible`/`Background` delivery modes — routing is now determined entirely by NOTIFY.yml.
- Remove pulse and scheduled action executor arms from the gateway event loop that ran full agent turns.
- Tests: pulse fires → SubAgent → result routed via NOTIFY.yml. Scheduled action script → direct execution → result routed via NOTIFY.yml.

**Milestone: Pulse and scheduled actions no longer block the main agent.**

### Phase 6: Agent-facing spawn tool

- Implement `subagent_spawn` tool with sync (`wait: true`) and async (`wait: false`) modes.
- Sync mode: block the tool call, run the SubAgent, return result as tool response.
- Async mode: spawn and return immediately, result flows through interrupt channel.
- Wire `model_tier`, `context`, `files`, and `tools` parameters.
- Tests: both spawn modes, context passing, tool restriction.

**Milestone: Main agent can delegate work to background workers.**

---

## What's Not Included

- **Sub-to-sub delegation.** SubAgents cannot spawn their own SubAgents. Orchestration chains are a complexity nightmare. If a task needs decomposition, the main agent does the decomposition and spawns multiple tasks itself.
- **Streaming background results.** Background tasks run to completion before delivering results.
- **Priority queuing.** All background tasks compete equally for semaphore permits. No task preempts a running task regardless of routing configuration.
