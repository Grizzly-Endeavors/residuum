# Tool Contracts

This document is the source of truth for every tool exposed to the LLM. It must be kept in sync with the Rust `definition()` implementations in this directory.

---

## `read_file`

**Source:** `read.rs` · `ReadTool`

**Description sent to LLM:**
> Read the contents of a file. Each output line is tagged with a content hash (e.g. `1:f1\thello`) for use with edit_file. By default returns the first 2000 lines; use offset/limit for larger files. Lines longer than 2000 characters are truncated. Image files (JPEG, PNG, GIF, WebP) are returned as inline images for visual inspection instead of raw bytes.

### Input

| Parameter | Type    | Required | Description                                      |
|-----------|---------|----------|--------------------------------------------------|
| `path`    | string  | yes      | Absolute or relative path to the file to read    |
| `offset`  | integer | no       | Line number to start reading from (0-based, default: 0) |
| `limit`   | integer | no       | Maximum number of lines to read (default: 2000)  |

### Output

**Text files:** lines formatted as `{line_num:>4}:{hash}\t{content}` joined by newlines, optionally preceded by warning lines.

Warnings prepended when:
- File exceeds 2000 lines and no explicit `limit`/`offset` was given
- Any lines exceed 2000 characters (they are truncated with `... (truncated)`)

**Image files** (JPEG, PNG, GIF, WebP): returns a text summary (`[Image: {filename}, {size} KB]`) plus inline base64-encoded image data via `ToolResult.images`. The `offset`/`limit` parameters are ignored for images.

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
- `PathPolicy` rejects the write path (path is outside the active project root, or targets a protected config file)
- File already exists but has not been read via `read_file` first
- Directory creation fails
- Write fails

**Side effect:** Records the path in the `FileTracker` after a successful write.

---

## `edit_file`

**Source:** `edit.rs` · `EditTool`

**Description sent to LLM:**
> Edit a file using line:hash anchors from read_file output. Validates content hashes before applying changes to detect stale edits. Operations: 'replace' (replace exact range; end_line required — use the same anchor as start_line for a single-line replacement), 'insert_after' (insert after a line; use start_line '0' to insert at file start), 'delete' (remove line or range; end_line optional for ranges). Use this over write_file when updating existing content.

### Input

| Parameter    | Type   | Required | Description                                                                 |
|--------------|--------|----------|-----------------------------------------------------------------------------|
| `path`       | string | yes      | Path to the file to edit                                                    |
| `operation`  | string | yes      | One of: `"replace"`, `"insert_after"`, `"delete"`                          |
| `start_line` | string | yes      | Line anchor as `"N:hash"` (e.g. `"5:a3"`). Use `"0"` for insert at file start |
| `end_line`   | string | no*      | End line anchor `"N:hash"`. Required for `replace` (use same anchor as `start_line` for single-line). Optional for `delete`. Not used by `insert_after`. |
| `content`    | string | no**     | New content. Required for `replace` and `insert_after`; omitted for `delete` |

\* `end_line` is required when `operation` is `replace`.
\*\* `content` is required when `operation` is `replace` or `insert_after`.

**Operations:**
- `replace` — replaces `start_line` through `end_line` with `content`. Both anchors are required — pass the same anchor for both to replace a single line.
- `insert_after` — inserts `content` after `start_line` (use `"0"` to insert at file start)
- `delete` — removes `start_line` through `end_line`; cannot delete all lines from a file

### Output

On success: `"edited {path}: {description}"` where description is e.g. `"replaced line(s) 5"` or `"deleted line(s) 2-4"`.

On error:
- `PathPolicy` rejects the path (outside active project root, or targets a protected config file)
- File does not exist
- File has not been read via `read_file` first
- Hash mismatch on `start_line` or `end_line` (file changed since last read)
- Line number out of bounds
- Attempt to delete all lines from a file
- `replace` called without `end_line`

---

## `exec`

**Source:** `exec.rs` · `ExecTool`

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
| `source`          | string          | no       | Filter by source: `"observations"`, `"episodes"`, or `"both"` |
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

**Side effects:** Updates `PathPolicy` to scope writes to the project root; resolves MCP server name references from `mcp.json` files (project-local `mcp.json` alongside `PROJECT.md` takes precedence over global `workspace/config/mcp.json`) and activates them with reference counting (servers are shared when multiple agents activate the same project simultaneously — they are only stopped when the last agent deactivates); rescans skills to include project-scoped skills.

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

**Side effects:** Resets `PathPolicy` to no active project; decrements MCP server reference count (servers are only disconnected when the last agent deactivates — see `project_activate`); rescans skills without project dir.

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

## `schedule_action`

**Source:** `actions.rs` · `ScheduleActionTool`

**Description sent to LLM:**
> Schedule a one-off action to fire at a specific time. The action runs once and is removed after firing.

### Input

| Parameter    | Type            | Required | Description                                                                                                                                                       |
|--------------|-----------------|----------|-------------------------------------------------------------------------------------------------------------------------------------------------------------------|
| `name`       | string          | yes      | Human-readable name for this action                                                                                                                               |
| `prompt`     | string          | yes      | The prompt to execute when the action fires                                                                                                                       |
| `run_at`     | string          | yes      | Always use local time without an offset (e.g. `2026-03-01T09:00:00`). Interpreted in the user's configured timezone. Must be in the future. |
| `agent_name` | string          | no       | Agent routing: `"main"` runs a full wake turn with conversation context; a preset name (e.g. `"memory-agent"`) spawns a sub-agent using that preset. Omit for default sub-agent behavior. |
| `model_tier` | string (enum)   | no       | Model tier override for sub-agent actions: `"small"`, `"medium"`, `"large"`. Defaults to medium.                                                                 |
| `channels`   | array\<string\> | no       | Result delivery channels for sub-agent actions. Defaults to `["agent_feed"]`. Not used when `agent_name="main"`.                                                  |

**Mutual exclusion:** if `agent_name="main"`, `channels` must not be provided — main-turn actions inject directly into the agent.

### Output

On success: `"Scheduled '{name}' (id: {id}). Fires at: {datetime}"` (datetime in user's local timezone)

On error: invalid datetime, `run_at` in the past, invalid channel, or save failure.

**Side effect:** Persists the action to `scheduled_actions.json` and wakes the action scheduler.

---

## `list_actions`

**Source:** `actions.rs` · `ListActionsTool`

**Description sent to LLM:**
> List all pending scheduled actions with their IDs, names, and fire times.

### Input

No parameters required (empty object accepted).

### Output

On success: count header followed by one entry per action (fire times displayed in user's local timezone):
```
{N} action(s):
  {name} ({id}) — fires: {datetime} [agent info] → [channels]
```

The agent label shows `[main turn]` for main-turn actions, `[preset: {name}]` for preset-routed actions, or nothing for default sub-agent actions. The channels suffix is omitted for main-turn actions.

When no actions exist: `"No pending scheduled actions."`

---

## `cancel_action`

**Source:** `actions.rs` · `CancelActionTool`

**Description sent to LLM:**
> Cancel a pending scheduled action by ID.

### Input

| Parameter | Type   | Required | Description          |
|-----------|--------|----------|----------------------|
| `id`      | string | yes      | Action ID to cancel  |

### Output

On success: `"Cancelled action '{id}'"`

On error: action not found, or save failure.

**Side effect:** Persists removal to `scheduled_actions.json` and wakes the action scheduler.

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

## `send_message`

**Source:** `send_message.rs` · `SendMessageTool`

**Description sent to LLM:**
> Send a message to an external notification channel or the inbox. Use this to proactively notify the user via configured channels (ntfy, webhook) or to save a message to the inbox for later review.

### Input

| Parameter | Type   | Required | Description                                                                    |
|-----------|--------|----------|--------------------------------------------------------------------------------|
| `channel` | string | yes      | Target channel name: `"inbox"` or any configured external channel             |
| `message` | string | yes      | The message body to send                                                       |
| `title`   | string | no       | Optional title (used for inbox items; defaults to first 60 chars of message)  |

### Output

On success (inbox): `"Message saved to inbox as {filename}"`

On success (external): `"Message sent to channel '{name}'"`

On error:
- Channel is `agent_wake` or `agent_feed` → `"send_message cannot target internal routing channels..."`
- Unknown external channel → `"unknown external channel '{name}'; available: {list}"`
- Delivery failure → execution error with details

**Side effects:**
- Inbox: creates a new `.json` file in the inbox directory with source `"agent"`
- External: sends the message via the configured channel transport (HTTP POST, etc.)

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
  [{id}] {task_name} — type: sub_agent — source: {pulse|action|agent} — running {elapsed}s
    preview: {prompt preview, up to 120 chars}
```

The `preview` line is omitted if the task has an empty prompt/command.

---

## `subagent_spawn`

**Source:** `background.rs` · `SubAgentSpawnTool`

**Description sent to LLM:**
> Spawn a background sub-agent to handle a task. The agent_name selects a preset that configures the sub-agent's instructions, model tier, and tool restrictions. Unknown preset names fail immediately with a list of available presets. Runs asynchronously and delivers the result to the specified channels.

### Input

| Parameter        | Type            | Required | Description                                                          |
|------------------|-----------------|----------|----------------------------------------------------------------------|
| `task`           | string          | yes      | The prompt/instructions for the sub-agent                            |
| `agent_name`     | string          | no       | Preset name to use (default: `"general-purpose"`). Must match a known preset or the call fails. `"main"` is reserved for scheduled tasks and will be rejected. |
| `model_override` | string          | no       | Override the preset's model tier: `"small"`, `"medium"`, `"large"`. If omitted, uses the preset's tier (default: `"medium"`). |
| `channels`       | array\<string\> | no       | Result delivery channels. If omitted, uses the preset's default channels (fallback: `["agent_feed"]`). |

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

On success: `"Subagent spawned: {task_id}"`

The sub-agent runs in the background. When it completes, the result is delivered to the specified `channels` via `ResultRouting::Direct`.

### Errors

- Missing or empty `task` → `InvalidArguments`
- `agent_name` is `"main"` (reserved, case-insensitive) → `InvalidArguments`
- Invalid `model_override` value → `InvalidArguments`
- Unknown `agent_name` (preset not found) → `is_error = true` with available preset list
- Unknown channel name → `is_error = true` with message
- Provider construction failure → `Execution` error

**Side effects:** Registers a background task in the spawner (visible via `list_agents`, cancellable via `stop_agent`). Result delivered through the notification/channel system.

**Not available to sub-agents:** this tool is only registered in the main agent's registry, not in `build_subagent_registry()`.

---

## `web_fetch`

**Source:** `web_fetch.rs` · `WebFetchTool`

**Description sent to LLM:**
> Fetch a web page and extract its main content as readable text. Returns the page title and cleaned content, optimized for reading. Use this to read articles, documentation, or any web page.

### Input

| Parameter | Type   | Required | Description          |
|-----------|--------|----------|----------------------|
| `url`     | string | yes      | The URL to fetch     |

### Output

On success: extracted readable text from the page, with the title as a markdown heading if available. Content is truncated at 50,000 characters with a `[content truncated]` notice if exceeded.

For `text/plain` responses: returns the raw text content (truncated if needed).

On error (`is_error = true`):
- HTTP error status: `"HTTP {status} fetching {url}"`
- Unsupported content type (not `text/html` or `text/plain`): `"unsupported content type: {type}"`

On execution error:
- Network/connection failure: `"failed to fetch {url}: {details}"`
- Response body read failure: `"failed to read response body: {details}"`

**No side effects.** Read-only tool with a 30-second timeout and 5-redirect limit.
