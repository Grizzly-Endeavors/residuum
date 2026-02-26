# Tool Contracts

This document is the source of truth for every tool exposed to the LLM. It must be kept in sync with the Rust `definition()` implementations in this directory.

**Gating note:** `exec` is a gated tool — only available when the active project's `tools` list opts it in. All other tools listed here are always available.

---

## `read_file`

**Source:** `read.rs` · `ReadTool`

**Description sent to LLM:**
> Read the contents of a file. Each output line is tagged with a content hash (e.g. `1:f1\thello`) for use with edit_file. By default returns the first 2000 lines; use offset/limit for larger files. Lines longer than 2000 characters are truncated.

### Input

| Parameter | Type    | Required | Description                                      |
|-----------|---------|----------|--------------------------------------------------|
| `path`    | string  | yes      | Absolute or relative path to the file to read    |
| `offset`  | integer | no       | Line number to start reading from (0-based, default: 0) |
| `limit`   | integer | no       | Maximum number of lines to read (default: 2000)  |

### Output

On success: lines formatted as `{line_num:>4}:{hash}\t{content}` joined by newlines, optionally preceded by warning lines.

Warnings prepended when:
- File exceeds 2000 lines and no explicit `limit`/`offset` was given
- Any lines exceed 2000 characters (they are truncated with `... (truncated)`)

On error (returned as `is_error = true`):
- File does not exist or cannot be read
- File exceeds 10 MB size cap

**Side effect:** Records the path in the `FileTracker` (enables subsequent `write_file`/`edit_file`).

---

## `write_file`

**Source:** `write.rs` · `WriteTool`

**Description sent to LLM:**
> Write content to a file. Creates parent directories if they don't exist. Overwrites the file if it already exists. Existing files must be read with read_file before overwriting.

### Input

| Parameter | Type   | Required | Description                                   |
|-----------|--------|----------|-----------------------------------------------|
| `path`    | string | yes      | Absolute or relative path to the file to write |
| `content` | string | yes      | The content to write to the file              |

### Output

On success: `"wrote {N} bytes to {path}"`

On error:
- `PathPolicy` rejects the write path (path is outside the active project root)
- File already exists but has not been read via `read_file` first
- Directory creation fails
- Write fails

**Side effect:** Records the path in the `FileTracker` after a successful write.

---

## `edit_file`

**Source:** `edit.rs` · `EditTool`

**Description sent to LLM:**
> Edit a file using line:hash anchors from read_file output. Validates content hashes before applying changes to detect stale edits. Operations: 'replace' (replace line or range), 'insert_after' (insert after a line; use start_line '0' to insert at file start), 'delete' (remove line or range). Use this over write_file when updating existing content.

### Input

| Parameter    | Type   | Required | Description                                                                 |
|--------------|--------|----------|-----------------------------------------------------------------------------|
| `path`       | string | yes      | Path to the file to edit                                                    |
| `operation`  | string | yes      | One of: `"replace"`, `"insert_after"`, `"delete"`                          |
| `start_line` | string | yes      | Line anchor as `"N:hash"` (e.g. `"5:a3"`). Use `"0"` for insert at file start |
| `end_line`   | string | no       | Optional end line anchor `"N:hash"` for range operations                   |
| `content`    | string | no*      | New content. Required for `replace` and `insert_after`; omitted for `delete` |

\* `content` is required when `operation` is `replace` or `insert_after`.

**Operations:**
- `replace` — replaces `start_line` through `end_line` (or just `start_line` if no range) with `content`
- `insert_after` — inserts `content` after `start_line` (use `"0"` to insert at file start)
- `delete` — removes `start_line` through `end_line`; cannot delete all lines from a file

### Output

On success: `"edited {path}: {description}"` where description is e.g. `"replaced line(s) 5"` or `"deleted line(s) 2-4"`.

On error:
- `PathPolicy` rejects the path
- File does not exist
- File has not been read via `read_file` first
- Hash mismatch on `start_line` or `end_line` (file changed since last read)
- Line number out of bounds
- Attempt to delete all lines from a file

---

## `exec`

**Source:** `exec.rs` · `ExecTool`

**Gated:** requires an active project with `"exec"` in its `tools` list.

**Description sent to LLM:**
> Execute a shell command and return its output. Commands run via `sh -c` with a configurable timeout (default 120 seconds).

### Input

| Parameter      | Type    | Required | Description                               |
|----------------|---------|----------|-------------------------------------------|
| `command`      | string  | yes      | The shell command to execute              |
| `timeout_secs` | integer | no       | Timeout in seconds (default: 120)         |

### Output

On success (exit code 0): stdout, followed by `STDERR:\n{stderr}` if stderr is non-empty. If both are empty, returns `"(no output)"`.

On error (exit code ≠ 0): `"command exited with code {N}\n{stdout+stderr}"`.

On timeout: `"command timed out after {N} seconds"`.

Output is capped at 100 KB; larger output is truncated with `\n... (output truncated)`.

---

## `memory_search`

**Source:** `memory_search.rs` · `MemorySearchTool`

**Description sent to LLM (vector enabled):**
> Search past conversation observations and interaction chunks using hybrid BM25 + vector similarity search. Returns matching results with relevance scores and snippets. Supports filtering by source type, date range, project context, and episode IDs.

**Description sent to LLM (BM25 only):**
> Search past conversation observations and interaction chunks using BM25 full-text search. Returns matching results with relevance scores and snippets. Supports filtering by source type, date range, project context, and episode IDs.

### Input

| Parameter         | Type            | Required | Description                                                  |
|-------------------|-----------------|----------|--------------------------------------------------------------|
| `query`           | string          | yes      | Search query (supports AND, OR, phrase queries with quotes)  |
| `limit`           | integer         | no       | Maximum results to return (default: 5, max: 20)              |
| `source`          | string          | no       | Filter by source: `"observation"` or `"chunk"`               |
| `date_from`       | string          | no       | Filter on or after date (YYYY-MM-DD, inclusive)              |
| `date_to`         | string          | no       | Filter on or before date (YYYY-MM-DD, inclusive)             |
| `project_context` | string          | no       | Filter by project context (exact match)                      |
| `episode_ids`     | array\<string\> | no       | Filter to results from these episode IDs                     |

### Output

On success with results:
```
Found {N} result(s):

1. [{source_type}] {id} | {date} | {context} | lines {s}-{e} (score: {score})
   {snippet}
```

On success with no results: `"no results found"`

On error: `"search failed: {reason}"`

---

## `memory_get`

**Source:** `memory_get.rs` · `MemoryGetTool`

**Description sent to LLM:**
> Retrieve a raw episode transcript by ID. Use after memory_search to drill into the full conversation transcript of a specific episode. Returns formatted message lines with role labels and line numbers.

### Input

| Parameter    | Type    | Required | Description                                              |
|--------------|---------|----------|----------------------------------------------------------|
| `episode_id` | string  | yes      | The episode ID to retrieve (e.g., `"ep-001"`)            |
| `from_line`  | integer | no       | Start reading from this line offset (1-indexed, default: start) |
| `lines`      | integer | no       | Number of message lines to return (default: 50, max: 200) |

**Security:** `episode_id` containing `/`, `\`, or `..` is rejected with a path-traversal error.

### Output

On success: formatted transcript with header (`Episode: {id}`), message lines as `[line {N}] {Role}: {text}`, and an optional footer showing the range when `from_line`/`lines` are used.

On error:
- Episode not found
- `episode_id` is empty or contains invalid characters
- Failed to read transcript file

---

## `project_activate`

**Source:** `projects.rs` · `ProjectActivateTool`

**Description sent to LLM:**
> Activate a project context. Loads the project's overview, manifest, and configuration into the agent's context.

### Input

| Parameter | Type   | Required | Description                                         |
|-----------|--------|----------|-----------------------------------------------------|
| `name`    | string | yes      | Name of the project to activate (case-insensitive)  |

### Output

On success: summary string like `"Activated project '{name}'. Manifest: {N} notes, {N} references, {N} workspace, {N} skills files."`

On error: project not found or activation failure message.

**Side effects:** Updates `PathPolicy` to scope writes to the project root; enables gated tools from the project's `tools` frontmatter list; activates MCP servers with reference counting (servers are shared when multiple agents activate the same project simultaneously — they are only stopped when the last agent deactivates); rescans skills to include project-scoped skills.

---

## `project_deactivate`

**Source:** `projects.rs` · `ProjectDeactivateTool`

**Description sent to LLM:**
> Deactivate the current project context. Requires a non-empty session summary log entry.

### Input

| Parameter | Type   | Required | Description                                           |
|-----------|--------|----------|-------------------------------------------------------|
| `log`     | string | yes      | Session summary log entry (required, must not be empty) |

### Output

On success: `"Deactivated project '{name}'. Log entry recorded."`

On error: no active project, or empty `log`.

**Side effects:** Resets `PathPolicy` to no active project; clears all gated tool permissions; decrements MCP server reference count (servers are only disconnected when the last agent deactivates — see `project_activate`); rescans skills without project dir.

---

## `project_create`

**Source:** `projects.rs` · `ProjectCreateTool`

**Description sent to LLM:**
> Create a new project with the standard directory structure and PROJECT.md.

### Input

| Parameter     | Type            | Required | Description                                               |
|---------------|-----------------|----------|-----------------------------------------------------------|
| `name`        | string          | yes      | Human-readable project name                               |
| `description` | string          | yes      | Brief summary of what this project covers                 |
| `tools`       | array\<string\> | no       | Optional list of tool names to associate with this project |

### Output

On success: `"Created project '{name}' at {path}"`

On error: creation failure message (e.g. name conflict, filesystem error).

**Side effect:** Triggers a project index rescan after successful creation.

---

## `project_archive`

**Source:** `projects.rs` · `ProjectArchiveTool`

**Description sent to LLM:**
> Archive a completed project. Updates frontmatter to archived status and moves it to the archive directory.

### Input

| Parameter | Type   | Required | Description                       |
|-----------|--------|----------|-----------------------------------|
| `name`    | string | yes      | Name of the project to archive    |

### Output

On success: `"Archived project '{name}'. Moved to archive/."`

On error:
- Project not found in index
- Project is currently active (must deactivate first)
- Filesystem error

**Side effect:** Triggers a project index rescan after successful archival.

---

## `project_list`

**Source:** `projects.rs` · `ProjectListTool`

**Description sent to LLM:**
> List all projects and their status.

### Input

| Parameter          | Type    | Required | Description                                       |
|--------------------|---------|----------|---------------------------------------------------|
| `include_archived` | boolean | no       | Include archived projects in the list (default false) |

### Output

On success: count header followed by one line per project:
```
{N} project(s):
  [{status}] {name}[ACTIVE] — {description}
```

When no projects exist: `"No projects found."`

---

## `skill_activate`

**Source:** `skills.rs` · `SkillActivateTool`

**Description sent to LLM:**
> Load a skill's full instructions into the system prompt. Use when a task matches an available skill's description.

### Input

| Parameter | Type   | Required | Description                                          |
|-----------|--------|----------|------------------------------------------------------|
| `name`    | string | yes      | Name of the skill to activate (case-insensitive)     |

### Output

On success: `"Activated skill '{name}'."`

On error: skill not found.

**Side effect:** Appends the skill's markdown body to the active system prompt.

---

## `skill_deactivate`

**Source:** `skills.rs` · `SkillDeactivateTool`

**Description sent to LLM:**
> Remove a skill's instructions from the system prompt when no longer needed.

### Input

| Parameter | Type   | Required | Description                         |
|-----------|--------|----------|-------------------------------------|
| `name`    | string | yes      | Name of the skill to deactivate     |

### Output

On success: `"Deactivated skill '{name}'."`

On error: skill is not currently active.

**Side effect:** Removes the skill's instructions from the active system prompt.

---

## `cron_add`

**Source:** `cron.rs` · `CronAddTool`

**Description sent to LLM:**
> Create a new scheduled cron job. The job will persist across restarts.

### Input

Required fields: `name`, `schedule_type`, `payload_type`

| Parameter            | Type    | Required   | Description                                                                          |
|----------------------|---------|------------|--------------------------------------------------------------------------------------|
| `name`               | string  | yes        | Human-readable name for this job                                                     |
| `schedule_type`      | string  | yes        | `"at"` / `"every"` / `"cron"`                                                       |
| `schedule_at`        | string  | if `at`    | Local datetime `YYYY-MM-DDTHH:MM:SS`, required when `schedule_type="at"`            |
| `schedule_every_ms`  | integer | if `every` | Interval in milliseconds, required when `schedule_type="every"`                     |
| `schedule_anchor_ms` | integer | no         | Anchor epoch ms (default 0 = Unix epoch), optional when `schedule_type="every"`     |
| `schedule_expr`      | string  | if `cron`  | 6-field cron expression including seconds, e.g. `"0 30 9 * * *"`; required when `schedule_type="cron"` |
| `schedule_tz`        | string  | no         | IANA timezone for cron evaluation; defaults to configured timezone                  |
| `payload_type`       | string  | yes        | `"system_event"` or `"agent_turn"`                                                  |
| `payload_text`       | string  | if `system_event` | Text to inject; required when `payload_type="system_event"`                |
| `payload_message`    | string  | if `agent_turn`   | Prompt for isolated agent turn; required when `payload_type="agent_turn"`  |
| `description`        | string  | no         | Optional description of what this job does                                          |
| `enabled`            | boolean | no         | Start the job enabled (default true)                                                |
| `delete_after_run`   | boolean | no         | Delete the job after it runs once                                                   |

### Output

On success: `"Created job '{name}' with id {id}. Next run: {datetime}"`

On error: invalid schedule, invalid payload, or save failure.

**Routing:** Results are routed via `NOTIFY.yml`. Add the job name to a channel list (`agent_feed`, `inbox`, or an external channel) to control where results are delivered.

**Side effect:** Persists the job to `jobs.json` and wakes the cron scheduler.

---

## `cron_list`

**Source:** `cron.rs` · `CronListTool`

**Description sent to LLM:**
> List all scheduled cron jobs with their status and next run time.

### Input

| Parameter          | Type    | Required | Description                                         |
|--------------------|---------|----------|-----------------------------------------------------|
| `include_disabled` | boolean | no       | Include disabled jobs in the list (default false)   |

### Output

On success: count header followed by one entry per job:
```
{N} job(s):
  [{enabled|disabled}] {name} ({id}) — last: {status} — next: {datetime}
    {description}
```

When no jobs match: `"No cron jobs found."`

---

## `cron_update`

**Source:** `cron.rs` · `CronUpdateTool`

**Description sent to LLM:**
> Update an existing cron job by ID. Only provided fields are changed.

### Input

| Parameter            | Type    | Required | Description                                                                         |
|----------------------|---------|----------|-------------------------------------------------------------------------------------|
| `id`                 | string  | yes      | Job ID to update                                                                    |
| `name`               | string  | no       | New name                                                                            |
| `description`        | string  | no       | New description                                                                     |
| `enabled`            | boolean | no       | Enable or disable the job                                                           |
| `delete_after_run`   | boolean | no       | Toggle delete-after-run                                                             |
| `schedule_type`      | string  | no       | New schedule type — replaces existing schedule (`"at"` / `"every"` / `"cron"`)     |
| `schedule_at`        | string  | no       | Required when updating to `schedule_type="at"`                                     |
| `schedule_every_ms`  | integer | no       | Required when updating to `schedule_type="every"`                                  |
| `schedule_anchor_ms` | integer | no       | Optional when updating to `schedule_type="every"`                                  |
| `schedule_expr`      | string  | no       | Required when updating to `schedule_type="cron"`                                   |
| `schedule_tz`        | string  | no       | IANA timezone for cron evaluation                                                  |
| `payload_type`       | string  | no       | New payload type — replaces existing payload (`"system_event"` / `"agent_turn"`)   |
| `payload_text`       | string  | no       | Required when updating to `payload_type="system_event"`                            |
| `payload_message`    | string  | no       | Required when updating to `payload_type="agent_turn"`                              |

### Output

On success: `"Updated job '{id}'"`

On error: job not found, invalid schedule/payload, or save failure.

**Side effect:** Persists changes to `jobs.json` and wakes the cron scheduler.

---

## `cron_remove`

**Source:** `cron.rs` · `CronRemoveTool`

**Description sent to LLM:**
> Remove a scheduled cron job by ID.

### Input

| Parameter | Type   | Required | Description         |
|-----------|--------|----------|---------------------|
| `id`      | string | yes      | Job ID to remove    |

### Output

On success: `"Removed job '{id}'"`

On error: job not found, or save failure.

**Side effect:** Persists removal to `jobs.json` and wakes the cron scheduler.

---

## `inbox_list`

**Source:** `inbox.rs` · `InboxListTool`

**Description sent to LLM:**
> List inbox items. Shows unread/read status, title, source, and timestamp for each item.

### Input

| Parameter     | Type    | Required | Description                                  |
|---------------|---------|----------|----------------------------------------------|
| `unread_only` | boolean | no       | Only show unread items (default false)       |

### Output

On success: count header followed by one entry per item:
```
{N} inbox item(s):
  [{read|unread}] {filename} — {title} ({source}, {timestamp})
```

When no items match: `"No inbox items found."`

---

## `inbox_read`

**Source:** `inbox.rs` · `InboxReadTool`

**Description sent to LLM:**
> Read a single inbox item by filename stem. Marks the item as read and returns its full content.

### Input

| Parameter | Type   | Required | Description                                              |
|-----------|--------|----------|----------------------------------------------------------|
| `id`      | string | yes      | Filename stem of the inbox item (without .json extension) |

### Output

On success: formatted item content:
```
Title: {title}
Source: {source}
Time: {timestamp}
Attachments: {paths}  (only if non-empty)

{body}
```

On error: item not found or read failure.

**Side effect:** Marks the item as read on disk.

---

## `inbox_add`

**Source:** `inbox.rs` · `InboxAddTool`

**Description sent to LLM:**
> Add a new item to the inbox. Use this to save reminders, notes, or anything to deal with later.

### Input

| Parameter | Type   | Required | Description                            |
|-----------|--------|----------|----------------------------------------|
| `title`   | string | yes      | Short summary of the inbox item        |
| `body`    | string | yes      | Full body text of the item             |
| `source`  | string | no       | Origin label (default: `"agent"`)      |

### Output

On success: `"Added inbox item '{title}' as {filename}"`

On error: save failure.

**Side effect:** Creates a new `.json` file in the inbox directory.

---

## `inbox_archive`

**Source:** `inbox.rs` · `InboxArchiveTool`

**Description sent to LLM:**
> Archive one or more inbox items by filename stem. Moves them to the archive directory.

### Input

| Parameter | Type            | Required | Description                                 |
|-----------|-----------------|----------|---------------------------------------------|
| `ids`     | array\<string\> | yes      | Filename stems of inbox items to archive    |

### Output

On success: `"Archived {N} item(s): {list}"`

On partial failure: success message plus `"Failed to archive {N} item(s): {errors}"`

On total failure: error with failure details.

**Side effect:** Moves `.json` files from inbox to `archive/inbox/`.

---

## `stop_agent`

**Source:** `background.rs` · `StopAgentTool`

**Description sent to LLM:**
> Cancel a running background task by ID. Returns an error if no task with that ID is active. Use list_agents to find active task IDs.

### Input

| Parameter | Type   | Required | Description                                  |
|-----------|--------|----------|----------------------------------------------|
| `task_id` | string | yes      | The ID of the background task to cancel      |

### Output

On success: `"Cancelled task {task_id}."`

On error (task not found): `"No active task with id {task_id}."` (returned as `is_error = true`)

---

## `list_agents`

**Source:** `background.rs` · `ListAgentsTool`

**Description sent to LLM:**
> List all currently running background tasks with their IDs, types, sources, prompt previews, and elapsed time.

### Input

No parameters required (empty object accepted).

### Output

When no tasks are running: `"No active background tasks."`

When tasks are running:
```
{N} active task(s):
  [{id}] {task_name} — type: {sub_agent|script} — source: {pulse|cron|agent} — running {elapsed}s
    preview: {prompt or command preview, up to 120 chars}
```

The `preview` line is omitted if the task has an empty prompt/command.

---

## `subagent_spawn`

**Source:** `background.rs` · `SubAgentSpawnTool`

**Description sent to LLM:**
> Spawn a background sub-agent to handle a task. The agent_name selects a preset that configures the sub-agent's instructions, model tier, and tool restrictions. Unknown preset names fail immediately with a list of available presets. By default runs asynchronously and delivers the result to the specified channels. Set wait=true to block until the sub-agent finishes and return its output directly.

### Input

| Parameter        | Type            | Required | Description                                                          |
|------------------|-----------------|----------|----------------------------------------------------------------------|
| `task`           | string          | yes      | The prompt/instructions for the sub-agent                            |
| `agent_name`     | string          | no       | Preset name to use (default: `"general-purpose"`). Must match a known preset or the call fails. |
| `model_override` | string          | no       | Override the preset's model tier: `"small"`, `"medium"`, `"large"`. If omitted, uses the preset's tier (default: `"medium"`). |
| `channels`       | array\<string\> | no       | Result delivery channels. If omitted, uses the preset's default channels (fallback: `["agent_feed"]`). Only used in async mode. |
| `wait`           | boolean         | no       | Block until the sub-agent finishes and return its output (default: `false`) |

Valid channel names: `agent_wake`, `agent_feed`, `inbox`, or any configured external notification channel.

### Subagent Presets

Presets are Markdown files in the `subagents/` directory at the workspace root (e.g., `subagents/researcher.md`). They configure sub-agent behaviour via YAML frontmatter:

```markdown
---
name: researcher
description: "Research specialist for gathering information"
model_tier: small          # small / medium / large (optional, default: medium)
denied_tools:              # permanently block these tools (mutually exclusive with allowed_tools)
  - exec
channels:                  # default result channels (overrideable at spawn time)
  - inbox
---

You are a research specialist. Focus on gathering and synthesising
information. Always cite sources.
```

**Built-in preset:** `general-purpose` — no tool restrictions, medium tier. Always present even with no `subagents/` directory.

**User-defined presets** with the same name as a built-in override the built-in.

**Unknown preset names** return a `ToolResult::error` listing available presets — the call does not proceed.

### Output

**Async mode** (`wait: false`, default):
On success: `"Subagent spawned: {task_id}"`

The sub-agent runs in the background. When it completes, the result is delivered to the specified `channels` via `ResultRouting::Direct`.

**Sync mode** (`wait: true`):
On success: the sub-agent's final text output.

On error: `"sub-agent failed: {reason}"` (returned as `is_error = true`).

### Errors

- Missing or empty `task` → `InvalidArguments`
- Invalid `model_override` value → `InvalidArguments`
- Unknown `agent_name` (preset not found) → `is_error = true` with available preset list
- Unknown channel name (async mode only) → `is_error = true` with message
- Provider construction failure → `Execution` error

**Side effects:**
- Async: registers a background task in the spawner (visible via `list_agents`, cancellable via `stop_agent`). Result delivered through the notification/channel system.
- Sync: runs a full sub-agent turn inline (not tracked by spawner). Any project activated during the turn is force-deactivated on completion.

**Not available to sub-agents:** this tool is only registered in the main agent's registry, not in `build_subagent_registry()`.
