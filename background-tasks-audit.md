# Background Tasks Audit — Deferred Issues

Captured 2026-02-25 from post-implementation audit.

## Medium

### try_lock() in spawn() can fail under contention
**File:** `src/background/spawner.rs:85-88`

`spawn()` uses `try_lock()` on a tokio Mutex. If cancel/list holds the lock at that moment, spawn fails with an error. Should use `.lock().await` like every other call site. Under load (pulse fires multiple tasks while `list_agents` runs), this could spuriously fail.

### Background results go to queue_system_event, not the interrupt channel
**File:** `src/gateway/server/mod.rs:262-328`

Background results arriving via `bg_result_rx` call `agent.queue_system_event()`, which queues for the next turn. If the agent is mid-turn, results wait until the turn finishes. The `Interrupt::BackgroundResult` variant exists and `turn.rs` handles it, but nothing ever sends it through `interrupt_tx`. Background results are never injected mid-turn — defeating a key design goal.

### No useful transcript written on failure
**File:** `src/background/spawner.rs:147-149`

When a task fails, summary is set to empty string. `write_transcript` writes an empty file. The error message only appears in `BackgroundResult.status`. Users inspecting transcript files on disk see nothing useful.

## Low / Polish

### Generic default agent_name in subagent_spawn
**File:** `src/tools/background.rs:235-238`

If the LLM doesn't pass `agent_name`, task name defaults to `"subagent"`. `list_agents` output shows the generic name for all spawned tasks, making them hard to distinguish.

### Dead Interrupt::BackgroundResult code path
**File:** `src/agent/interrupt.rs:10-12` + `src/agent/turn.rs:62-72`

The variant and its handling exist but no producer ever sends it. Either wire it up (see mid-turn injection issue above) or remove it.

### tz field on BackgroundTaskSpawner is stored but never used
**File:** `src/background/spawner.rs:24-28`

`dead_code` annotation says "stored for future transcript timestamp formatting" but transcripts use `Utc::now()`. Either use `tz` for local-time transcript formatting or remove it.

### project_state not reset in cancellation cleanup
**File:** `src/background/spawner.rs:118-132`

Cancellation resets `path_policy`, `tool_filter`, and calls `mcp_registry.deactivate_project()`, but never clears the project from the sub-agent's own `ProjectState`. Since the sub-agent's `ProjectState` is isolated and about to be dropped, this is harmless — but the log claims deactivation happened.

### SubAgentConfig missing tools restriction field
**File:** `src/background/types.rs:44-55`

Design doc specifies `tools: Option<Vec<ToolName>>` for restricting sub-agent tools. Not implemented. `subagent_spawn` tool doesn't expose a `tools` parameter. All sub-agents get the full `build_subagent_registry()` tool set.
