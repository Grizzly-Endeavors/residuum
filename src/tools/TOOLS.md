# Tool Contracts

This document is the source of truth for every tool exposed to the LLM. It must be kept in sync with the Rust `definition()` implementations in this directory.

---

## `read_file`

**Source:** `read.rs` · `ReadTool`

**Description sent to LLM:**
> Read the contents of a file. Each output line is tagged with a content hash (e.g. `1:f1a3\thello`) for use with edit_file. By default returns the first 2000 lines; use offset/limit for larger files. Lines longer than 2000 characters are truncated. Image files (JPEG, PNG, GIF, WebP) are returned as inline images for visual inspection instead of raw bytes.

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
- `PathPolicy` rejects the write path (targets a protected config file)
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
| `start_line` | string | yes      | Line anchor as `"N:hash"` (e.g. `"5:a3b2"`). Use `"0"` for insert at file start |
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
- `PathPolicy` rejects the path (targets a protected config file)
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
> Execute a shell command and return its output. Commands run via `sh -c` (Unix) or `cmd /C` (Windows) with a configurable timeout (default 120 seconds). The description sent to the LLM reflects the current platform.

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

### Side effects

Commands are resolved against the configured tool `PATH`: the directories in
`[tools].path` and the default `~/.residuum/bin`, prepended to the inherited
`PATH`. Binaries dropped into those dirs are runnable without a rebuild. See
[Tool PATH](../../docs/systems-usage/tools.md).

---

## `memory_search`

**Source:** `memory_search.rs` · `MemorySearchTool`

**Description sent to LLM (vector enabled):**
> Search past conversation observations, interaction chunks, and knowledge wiki pages using hybrid BM25 + vector similarity search. Returns matching results with relevance scores and snippets; a wiki result's ID is the page path to open with read_file. Supports filtering by source type, date range, and episode IDs.

**Description sent to LLM (BM25 only):**
> Search past conversation observations, interaction chunks, and knowledge wiki pages using BM25 full-text search. Returns matching results with relevance scores and snippets; a wiki result's ID is the page path to open with read_file. Supports filtering by source type, date range, and episode IDs.

### Input

| Parameter         | Type            | Required | Description                                                  |
|-------------------|-----------------|----------|--------------------------------------------------------------|
| `query`           | string          | yes      | Search query (supports AND, OR, phrase queries with quotes)  |
| `limit`           | integer         | no       | Maximum results to return (default: 5, max: 20)              |
| `source`          | string          | no       | Filter by source: `"observations"`, `"episodes"`, or `"wiki"`. Omit to search all three. |
| `date_from`       | string          | no       | Filter on or after date (YYYY-MM-DD, inclusive)              |
| `date_to`         | string          | no       | Filter on or before date (YYYY-MM-DD, inclusive)             |
| `episode_ids`     | array\<string\> | no       | Filter to results from these episode IDs (excludes wiki pages) |

### Output

On success with results:
```
Found {N} result(s):

1. [{source_type}] {id} | {date} | lines {s}-{e} (score: {score})
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
| `agent_name` | string          | no       | A skill name (e.g. `"memory-analyst"`) to fork the session with that skill as its role. Omit to fork a plain session with no skill. `"main"` is rejected. |
| `model_tier` | string (enum)   | no       | Model tier override for session actions: `"small"`, `"medium"`, `"large"`. Defaults to medium.                                                                 |

### Output

On success: `"Scheduled '{name}' (id: {id}). Fires at: {datetime}"` (datetime in user's local timezone)

On error: invalid datetime, `run_at` in the past, or save failure.

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
  {name} ({id}) — fires: {datetime} [agent info]
```

The agent label shows `[skill: {name}]` for skill-routed actions, or nothing for a plain session with no skill.

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

## `user_inbox_add`

**Source:** `inbox.rs` · `UserInboxAddTool`

**Description sent to LLM:**
> Add a new item to the user's inbox. Use this to explicitly send notes, reminders, or items for the human user to review later.

### Input

| Parameter     | Type             | Required | Description                        |
|---------------|------------------|----------|-------------------------------------|
| `title`       | string           | yes      | A short summary of the item        |
| `body`        | string           | yes      | The detailed content of the item   |
| `attachments` | array\<string\>  | no       | Paths to files already written to disk to attach to this item. Each file is copied into the item's own storage, so it's safe even if the source is later moved or deleted. |

### Output

On success, no `attachments` given: `"Added item to user inbox with ID: {filename stem}"`

On success, with `attachments`: `"Added item to user inbox with ID: {filename stem} ({N} attachment(s) copied)"`

On error:
- Missing `title` or `body`
- An attachment path doesn't exist, isn't readable, or exceeds the 25 MB size cap — the whole add fails and no item is created (see side effects below)
- Failed to write the item to disk

**Side effect:** Writes a new `.json` file to `inbox/user/`, tagged with source `"agent"`. When `attachments` is given, each file is validated, copied into `inbox/user/attachments/{item id}/` (traversal-style source names are reduced to their basename; same-name collisions within one call get a `_2`, `_3`, ... suffix rather than clobbering), and the item's `attachments` field records the copies. If any attachment in the batch fails, every file already copied for that item is removed and no item is saved — a partial attachment set is never left behind. This is a separate inbox from the agent inbox (`inbox_list`/`inbox_read`/`inbox_archive`) — the agent has no tool to list, read, or archive items here; only the user reads and archives them via the web UI, where attachments are downloadable from `GET /api/inbox/{id}/attachments/{index}`.

---

## `send_message`

**Source:** `send_message.rs` · `SendMessageTool`

**Description sent to LLM:**
> Send a message and/or file attachment to an endpoint. When sharing a file with the user, always use the file_path parameter — the file will be delivered natively (inline image, audio player, or download link) rather than as a text path. Use list_endpoints to see available targets. On a chat endpoint (discord, telegram, teams) the message goes to the owner's direct message unless you pass a conversation from list_conversations, e.g. to post into a specific channel or group chat.

### Input

| Parameter   | Type   | Required        | Description                                                                    |
|-------------|--------|-----------------|--------------------------------------------------------------------------------|
| `endpoint`  | string | yes             | Target endpoint name (any interactive or notification endpoint)               |
| `message`   | string | no†             | Message body or caption text                                                   |
| `file_path` | string | no†             | Absolute path to a file to send. Images render inline, audio gets a player, other files appear as downloads. Always use this instead of pasting file paths as text. |
| `title`     | string | no              | Optional title for notifications (defaults to first 60 chars of message)      |
| `conversation` | string | no           | Conversation ID on a chat endpoint, from `list_conversations`. Omit to message the owner directly. |

† At least one of `message` or `file_path` must be provided.

### Output

On success:
- Text only: `"Message published to endpoint '{name}'"`
- File only: `"File '{filename}' published to endpoint '{name}'"`
- Text + file: `"Message and file '{filename}' published to endpoint '{name}'"`

With `conversation`, `{name}` reads `{endpoint} ({conversation label})`, e.g. `teams (#builds (Eng Team))`.

On error:
- Neither message nor file → `"at least one of 'message' or 'file_path' is required"`
- Unknown endpoint → `"unknown endpoint '{name}'; available: {list}"`
- Endpoint does not accept messages (e.g. inbox) → `"endpoint '{name}' does not accept messages; available: {list}"`
- File with notify endpoint → `"endpoint '{name}' does not support file attachments"`
- File not found → `"file not found: {path}"`
- File exceeds size limit → `"file '{name}' is {N}MB, exceeds {limit}MB limit for {endpoint}"`
- `conversation` on an endpoint with no running chat interface → `"'{name}' has no conversations to choose from; only running chat interfaces (discord, telegram, teams) do — omit 'conversation' to send there"`
- `conversation` not among the endpoint's conversations → `"no conversation '{id}' on '{name}'; use list_conversations to see the ones available"`
- The interface cannot list its conversations → `"couldn't list conversations on '{name}': {reason}"`
- Bus publish failure → execution error with details

**Side effects:**
- Notify endpoints: publishes `NotificationEvent` to the endpoint's topic
- Interactive endpoints: publishes `ResponseEvent` to the endpoint's topic (with optional `FileAttachment` and the validated `conversation` target). If delivery to a named conversation fails later, the owner gets an error message on that interface.
- **Cannot send to inbox** — the agent has no write path to inbox
- **File attachments require interactive endpoints** — Telegram allows up to 50MB, others 25MB

---

## `list_endpoints`

**Source:** `list_endpoints.rs` · `ListEndpointsTool`

**Description sent to LLM:**
> List available communication endpoints. Shows interactive endpoints (for switch_endpoint and send_message) and notification endpoints (for send_message only).

### Input

No parameters required (empty object accepted).

### Output

On success with endpoints:
```
Interactive endpoints (for switch_endpoint / send_message):
  ws — WebSocket
  discord — Discord

Notification endpoints (for send_message):
  my-ntfy — Ntfy (my-ntfy)
```

When no endpoints configured: `"No endpoints configured."`

Excludes inbox, webhook, and other input-only or system endpoints.

---

## `list_conversations`

**Source:** `list_conversations.rs` · `ListConversationsTool`

**Description sent to LLM:**
> List the direct messages, group chats, and channels you can post into on each chat interface (discord, telegram, teams). Pass an ID from here as send_message's 'conversation' to post there. Chats appear once the bot has been added to them or has heard from them; Discord server channels are listed directly.

### Input

| Parameter  | Type   | Required | Description                                                  |
|------------|--------|----------|--------------------------------------------------------------|
| `endpoint` | string | no       | Only list conversations on this chat endpoint (e.g. `"teams"`) |

### Output

One section per running chat interface, sorted by endpoint then label:
```
discord:
  1234567890 — #builds (Eng Team) (channel)
  9876543210 — direct message with bear (direct message)

teams:
  19:abc@thread.tacv2 — #general (Eng Team) (channel)
```

- An interface that knows no conversations yet shows `  (none yet)`.
- An interface that fails to list shows `  couldn't list conversations: {reason}` (the other sections still appear).
- No chat interface running → `"No chat interfaces are running (discord, telegram, and teams list conversations)."`
- `endpoint` that is not a running chat interface → error `"'{name}' is not a running chat interface; running: {list}"`

**Side effects:** none. Discord lists server channels through its API on every call; Teams and Telegram read their saved state.

---

## `switch_endpoint`

**Source:** `switch_endpoint.rs` · `SwitchEndpointTool`

**Description sent to LLM:**
> Switch the active endpoint for subsequent responses. Takes effect on the next turn. Use list_endpoints to see available interactive endpoints.

### Input

| Parameter  | Type   | Required | Description                                                       |
|------------|--------|----------|-------------------------------------------------------------------|
| `endpoint` | string | yes      | Endpoint identifier (e.g. `"discord"`, `"telegram"`, `"ws"`)    |

### Output

On success: `"Switched output to '{display_name}'. Subsequent responses will be sent there."`

On error:
- Unknown endpoint → `"unknown endpoint '{name}'; available interactive endpoints: {list}"`
- Non-interactive endpoint → `"endpoint '{name}' is not interactive; available: {list}"`

**Side effects:** Sets the output topic override via a `watch` channel. The gateway reads this before each turn and routes agent responses to the overridden endpoint. The switch takes effect on the **next turn**, not mid-turn — the confirmation response goes to the current endpoint.

---

## `stop_agent`

**Source:** `background.rs` · `StopAgentTool`

**Description sent to LLM:**
> Stop a live session by address. Cancels any in-flight turn and moves the session to completing; its transcript is kept, not discarded. The main agent cannot be stopped this way. Use list_agents to find live addresses.

### Input

| Parameter | Type   | Required | Description                                  |
|-----------|--------|----------|----------------------------------------------|
| `address` | string | yes      | The address of the session to stop           |

### Output

On success: `"Stopping session {address}."`

On error (`address` is `"main"`): `InvalidArguments` — the main agent cannot be stopped this way.

On error (address not live): `"No live session with address {address}."` (returned as `is_error = true`)

**Side effect:** Cancels the session's stop token. A running turn ends at its next checkpoint (model-call boundary or tool-loop iteration); an idle session skips straight to completing. Either way the run's transcript so far is kept and merged like any other completed run.

---

## `list_agents`

**Source:** `background.rs` · `ListAgentsTool`

**Description sent to LLM:**
> List the main agent plus every live (running or idle) session: address, category, source, state, depth, spawner, elapsed time, and purpose. Completed sessions are not listed, but their addresses remain valid.

### Input

No parameters required (empty object accepted).

### Output

```
main — always live
{N} live session(s):
  [{address}] {source_label} — category: {scheduled|external|spawned} — state: {forking|running|idle|completing} — depth: {N} — spawner: {address|-} — running {elapsed}s — purpose: {prompt/task preview, up to 120 chars}
```

`main` is always listed first, even when no sessions are live. `spawner` is `-` for `scheduled` and `external` sessions (they have no spawner in Phase 1).

---

## `subagent_spawn`

**Source:** `background.rs` · `SubagentSpawnTool`

**Description sent to LLM:**
> Fork a session to handle a task in the background. Optionally name a skill to give the session a role — its instructions become the session's brief. Runs asynchronously; each turn's result is relayed back to you tagged with the session's address. Returns the session's address immediately — use it with list_agents or stop_agent. A session's result is its own self-report, not verified fact — for verifiable work, ask it to return concrete handles (file paths, IDs, URLs) and verify them yourself before relying on the result.

### Input

| Parameter        | Type            | Required | Description                                                          |
|------------------|-----------------|----------|-----------------------------------------------------------------------|
| `task`           | string          | yes      | The prompt/instructions for the session                              |
| `skill`          | string          | no       | Name of a skill to activate as the session's role. Omit to run on the task prompt alone. Must match a known skill or the call fails. `"main"` is rejected. |
| `model`          | string          | no       | Model tier: `"small"`, `"medium"`, `"large"`. Default: `"medium"`. |

### Session Roles

A spawned session is a `spawned`-category fork of the main agent, running off the main thread with its own identity, memory snapshot, and tool registry — see `docs/systems-usage/background-tasks.md` for the full fork contents. Naming a `skill` activates that skill on the session's own skill state, so its body arrives as the session's role instructions through the normal active-skill path. Roles are ordinary skills in `skills/<name>/SKILL.md` — there is no separate preset format, and the same file can be activated in-turn by the main agent.

**Unknown skill names** return a `ToolResult::error` listing available skills — the call does not proceed. The check runs against the in-memory skill index, so it costs no disk I/O.

### Output

On success: `"Session {address} spawned with skill '{name}'."`, or `"Session {address} spawned."` when no skill was named. `{address}` is generated synchronously (e.g. `spawned-researcher-3f9a`) and returned before the session actually starts running.

The session runs in the background via the session runtime. Its result from each turn is relayed back to the main agent, tagged with its address.

### Errors

- Missing or empty `task` → `InvalidArguments`
- `skill` is `"main"` (reserved, case-insensitive) → `InvalidArguments`
- Invalid `model` value → `InvalidArguments`
- Unknown `skill` (not in the skill index) → `is_error = true` with the available skill list
- Bus publish failure → `Execution` error

**Side effects:** Publishes a `SpawnRequestEvent` (carrying the pre-generated address) to the bus. The spawn listener picks it up, builds the session's fork resources, and hands it to the session runtime (visible via `list_agents`, cancellable via `stop_agent`). Each turn's result is delivered through the bus notification system.

**Not available to sessions:** this tool is only registered in the main agent's registry, not in `build_subagent_registry()` — nesting arrives in a later phase.

---

## `message_agent`

**Source:** `message_agent.rs` · `MessageAgentTool`

**Description sent to LLM:**
> Send a text message to another agent by address — main, or any session (running, idle, or previously completed). A running session sees it as an interrupt at its next tool-call boundary; an idle one starts a new turn with it; a completed one is resumed as a new run at the same address. Every delivered message names your own address and category so the recipient can reply. Use list_agents to find addresses.

### Input

| Parameter | Type   | Required | Description                                                  |
|-----------|--------|----------|----------------------------------------------------------------|
| `to`      | string | yes      | Address to message: `"main"`, or a session address from `list_agents`. |
| `message` | string | yes      | The message body.                                             |

### Output

- Delivered to main: `"Message delivered to main."`
- Delivered to a live session: `"Message delivered to {address}."`
- Delivered to a completed (or completing) session: `"Session {address} had completed; message delivered by resuming it as a new run."` (`is_error = false` — the resume itself is not a failure)
- Unknown address (`is_error = true`): `"no such agent '{to}'. Use list_agents to see live sessions; a completed session's address only works again once it has run at least once."`
- Messaging yourself (`is_error = true`): `"cannot message yourself"`
- Target's interrupt channel is saturated (`is_error = true`, vanishingly unlikely): `"agent {address} is busy, try again shortly"` — never falls back to a resume, which would double-register the address.
- A publish (to main, or as a resume spawn request) failed at the bus (`is_error = true`): a message naming what failed. The tool never reports success when delivery didn't actually happen.

### Errors

- Missing or empty `to`/`message` → `InvalidArguments`

**Side effects:** Routes through the shared `AgentMessenger` (`crate::background::messaging`):
- **main** — publishes a `MessageEvent` on the `UserMessage` bus topic, formatted with the sender's address and category, reusing the main event loop's existing interrupt-if-running/new-turn-if-idle handling.
- **running/idle session** — delivers an `Interrupt::AgentMessage` through the session's own interrupt channel (registered in the `SessionRegistry` at fork time). A running turn drains it at its next tool-call boundary; an idle session wakes and runs another turn in the same run, with the message as that turn's input.
- **completing session** — the run no longer accepts messages into itself; `AgentMessenger` waits for it to fully leave the registry (its resume point is always recorded before that happens) and then resumes it, same as a completed session below. A message still queued in a run's own interrupt channel when its teardown drains it (e.g. one delivered just as `stop_agent` lands) is handled the same way, combined into the resumed run's opening prompt if more than one arrived.
- **completed session** — publishes a fresh `SpawnRequestEvent` at the same address, carrying the previous run's model tier and a pointer to its episode id (or run id, retrievable with `memory_get`) in the new run's context. Goes through the ordinary spawn-listener path, exactly like any other session fork. The spawn listener itself refuses a spawn/resume for an address still registered as running/idle/forking (logged as an error — should only happen if something else raced the registry), and defers one for a completing address until it clears.

**Available to sessions:** registered in both the main agent's registry and `build_subagent_registry()`, each instance identifying itself with its own address and category (`"main"` for the main agent).

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

---

## `ollama_web_search`

**Source:** `ollama_web_search.rs` · `OllamaWebSearchTool`

**Conditional registration:** only registered when `web_search.standalone_backend.name == "ollama"` in config.

**Description sent to LLM:**
> Search the web using Ollama Cloud. Returns search results with titles, URLs, and snippets.

### Input

| Parameter     | Type    | Required | Description                                      |
|---------------|---------|----------|--------------------------------------------------|
| `query`       | string  | yes      | The search query                                 |
| `max_results` | integer | no       | Maximum number of results to return (default: 5) |

### Output

On success with results:
```
Found {N} result(s):

1. {title}
   URL: {url}
   {snippet}
```

On success with no results: `"No search results found."`

On non-2xx HTTP response: `"ollama web search API returned HTTP {status}: {body}"` (returned as `is_error = true`)

On execution error:
- API call failure: `"failed to call ollama web search API: {details}"`
- Response parse failure: `"failed to parse ollama web search response: {details}"`

**No side effects.** Read-only tool with a 30-second timeout.

---

## `file_bug_report`

**Source:** `file_bug_report.rs` · `FileBugReportTool`

**Description sent to LLM:**
> File a structured bug report when you observe broken behavior in residuum itself (a crash, a tool returning the wrong result, a model reply that violates a hard constraint, etc.). Use this for things that are clearly wrong, not for confusion or usability friction — use submit_feedback for those. Returns a public reference ID.

### Input

| Parameter        | Type   | Required | Description                                                       |
|------------------|--------|----------|-------------------------------------------------------------------|
| `what_happened`  | string | yes      | What actually happened (the broken behavior)                      |
| `what_expected`  | string | yes      | What should have happened instead                                 |
| `what_doing`     | string | yes      | What you (or the user) were doing when it happened                |
| `severity`       | string | yes      | One of: `broken`, `wrong`, `annoying`                             |

### Output

On success: `"bug report submitted: RR-XXXXXXXXXX"`.

On submission failure (network issue, upstream rejection, rate limit): `is_error = true` with the upstream error message; for 429 responses the `Retry-After` header is included.

**Side effects:** Submits a sanitized OTLP trace dump and the runtime client context (version, OS, model) to the developer ingest service via `agent-residuum.com/api/v1/bug-report`. Span content is forcibly sanitized regardless of the runtime `sanitize_content` toggle.

**Not available to sub-agents:** registered only on the main agent's tool registry (sub-agent registry omitted in v1).

---

## `submit_feedback`

**Source:** `submit_feedback.rs` · `SubmitFeedbackTool`

**Description sent to LLM:**
> Submit short, free-form feedback to the developer. Use this when you notice confusion, usability friction, surprising behavior, or patterns in your own actions that seem worth surfacing — anything that isn't unambiguously broken (use file_bug_report for that). You are encouraged to use this proactively. Returns a public reference ID.

### Input

| Parameter  | Type   | Required | Description                                                |
|------------|--------|----------|------------------------------------------------------------|
| `message`  | string | yes      | The feedback message                                       |
| `category` | string | no       | Optional free-form category tag (e.g. `ui`, `docs`, `tools`) |

### Output

On success: `"feedback submitted: RR-XXXXXXXXXX"`.

On submission failure: `is_error = true` with the upstream error message; for 429 responses the `Retry-After` header is included.

**Side effects:** Submits the message + version-only client context to `agent-residuum.com/api/v1/feedback`. No trace dump is attached.

**Not available to sub-agents:** registered only on the main agent's tool registry (sub-agent registry omitted in v1).
