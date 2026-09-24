# Tool Contracts

This document is the source of truth for every tool exposed to the LLM. It must be kept in sync with the Rust `definition()` implementations in this directory.

---

## `read_file`

**Source:** `read.rs` · `ReadTool`

**Description sent to LLM:**
> Read the contents of a file. Each output line is prefixed with its line number and a tab (e.g. `   1\thello`); the prefix is not part of the file, so leave it out of edit_file's old_string. By default returns the first 2000 lines; use offset/limit for larger files. Lines longer than 2000 characters are truncated. Image files (JPEG, PNG, GIF, WebP) are returned as inline images for visual inspection instead of raw bytes.

### Input

| Parameter | Type    | Required | Description                                      |
|-----------|---------|----------|--------------------------------------------------|
| `path`    | string  | yes      | Absolute or relative path to the file to read    |
| `offset`  | integer | no       | Line number to start reading from (0-based, default: 0) |
| `limit`   | integer | no       | Maximum number of lines to read (default: 2000)  |

### Output

**Text files:** lines formatted as `{line_num:>4}\t{content}` joined by newlines, optionally preceded by warning lines.

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
- `PathPolicy` rejects the write path (targets a protected config or credential-store file)
- File already exists but has not been read via `read_file` first
- Directory creation fails
- Write fails

**Side effect:** Records the path in the `FileTracker` after a successful write.

---

## `edit_file`

**Source:** `edit.rs` · `EditTool`

**Description sent to LLM:**
> Edit an existing file by replacing exact text. Each entry in 'edits' replaces old_string with new_string; old_string must match the file exactly once (include surrounding lines to make it unique) unless replace_all is true. Edits apply in order, each seeing the result of the ones before it, and the file is only written if every edit succeeds. Copy old_string from read_file output without the line-number prefix. To delete text, use an empty new_string. The file must have been read with read_file first. Use this over write_file when changing part of an existing file.

### Input

| Parameter | Type   | Required | Description                                          |
|-----------|--------|----------|------------------------------------------------------|
| `path`    | string | yes      | Path to the file to edit                             |
| `edits`   | array  | yes      | Replacements to apply, in order (at least one entry) |

Each `edits` entry:

| Field         | Type    | Required | Description                                                                 |
|---------------|---------|----------|-----------------------------------------------------------------------------|
| `old_string`  | string  | yes      | Exact text to replace; must be non-empty and differ from `new_string`       |
| `new_string`  | string  | yes      | Replacement text; empty deletes `old_string`                                |
| `replace_all` | boolean | no       | Replace every occurrence instead of requiring exactly one (default `false`) |

### Matching

- Edits apply in order to an in-memory copy; each sees the result of the ones before it. The file is written once, only if every edit succeeds.
- `old_string` is matched exactly first. It must occur exactly once unless `replace_all` is set.
- Only when there is no exact match, the tool retries comparing whole lines with leading and trailing whitespace trimmed. That match is used only if it occurs exactly once; `new_string` is still written verbatim, and the result carries a note naming the edit.
- Line breaks in `old_string` and `new_string` are converted to the style most of the file's lines use, so CRLF files stay CRLF. A missing trailing newline is preserved.

### Output

On success: `"edited {path} ({N} replacement(s))"`, then one `note:` line per edit that matched only after ignoring whitespace, a blank line, and a preview of the changed regions in the final file. The preview uses `read_file`'s `{line_num:>4}\t{content}` format with 2 lines of context, separates regions with `   …`, and is capped at 60 lines.

On error (returned as `is_error = true`; nothing is written):
- `PathPolicy` rejects the path (targets a protected config or credential-store file)
- File does not exist (points to `write_file`)
- File has not been read via `read_file` first
- An edit fails, reported as `"edit {i} of {n}: {reason}. No changes were written to {path}"`, where the reason is one of:
  - `old_string` matches several places without `replace_all` (lists up to 10 line numbers)
  - `old_string` is not found (with a hint when it includes `read_file`'s line-number prefix)
  - no exact match, and the whitespace-insensitive retry matches several places (lists line numbers)

Malformed arguments (missing `path` or `edits`, an empty `edits` list, an entry missing `old_string`/`new_string`, an empty `old_string`, or identical `old_string` and `new_string`) return a `ToolError::InvalidArguments`.

---

## `exec`

**Source:** `exec.rs` · `ExecTool`

**Description sent to LLM:**
> Execute a shell command and return its output. Commands run via `sh -c` (Unix) or `cmd /C` (Windows) with a configurable timeout (default 120 seconds). Use `keys` to expose agent keys as environment variables (see agent_keys_list); use `store_output_as` to store stdout as a new agent key instead of returning it.

The description sent to the LLM reflects the current platform.

### Input

| Parameter         | Type     | Required | Description                               |
|-------------------|----------|----------|-------------------------------------------|
| `command`         | string   | yes      | The shell command to execute              |
| `timeout_secs`    | integer  | no       | Timeout in seconds (default: 120)         |
| `keys`            | string[] | no       | Agent key names to expose to this command, each as its uppercased name (`github_token` → `$GITHUB_TOKEN`) |
| `store_output_as` | object   | no       | `{ name, description? }` — store stdout as an agent key instead of returning it |

### Output

On success (exit code 0): stdout, followed by `STDERR:\n{stderr}` if stderr is non-empty. If both are empty, returns `"(no output)"`.

On error (exit code ≠ 0): `"command exited with code {N}\n{stdout+stderr}"`.

On timeout: the command's whole process tree is killed (not just the immediate shell), and the result is an error carrying whatever stdout/stderr it had already produced: `"command timed out after {N} seconds; its process tree was killed"`, followed by `STDOUT so far:\n{stdout}` and/or `STDERR so far:\n{stderr}` for whichever are non-empty.

On cancellation (the turn was stopped while the command was running): same process-tree kill and same partial-output shape, reported as a cancellation rather than a failure — `"the turn was stopped while this command was running; its process tree was killed"` plus whatever `STDOUT so far` / `STDERR so far` it had produced. See [Stopping a Turn](../../docs/systems-usage/turn-control.md).

Output is capped at 100 KB; larger output is truncated with `\n... (output truncated)`.

With `keys`: an unknown name returns `"unknown agent key(s): {names}. Available: {names}. Nothing was run."` without spawning anything. Without an agent key store: `"agent keys are not available in this context. Nothing was run."`

With `store_output_as`:
- Exit 0 with non-empty stdout: stdout (trailing newline trimmed) is stored as an agent-created key, and the result is `"stored agent key '{name}' ({N} bytes). Use it with keys: [\"{name}\"] as ${NAME}."` plus any stderr. **Stdout is never returned.**
- Non-zero exit: `"command exited with code {N}; nothing was stored and stdout was discarded"` plus stderr.
- Empty stdout: `"command produced no stdout; nothing was stored"`.
- A name that is invalid or belongs to a user-created key is refused before the command runs (`"... Nothing was run."`).
- A value that fails storage rules (shorter than 8 characters): `"command succeeded but its output was not stored: {reason}. stdout was discarded."`
- Timeout or cancellation: nothing is stored and stdout is discarded from the message too, the same as a non-zero exit — `"{reason}; nothing was stored and stdout was discarded"` plus stderr.

stderr in a `store_output_as` result is redacted against the new value as well as every existing key.

### Side effects

Commands are resolved against the configured tool `PATH`: the directories in
`[tools].path` and the default `~/.residuum/bin`, prepended to the inherited
`PATH`. Binaries dropped into those dirs are runnable without a rebuild. See
[Tool PATH](../../docs/systems-usage/tools.md).

Named keys are set only in the spawned child's environment. `store_output_as` writes to the agent key store. Every agent-key value in the result — this tool's or any other — is replaced with `[agent-key:<name>]` by the turn loop before the result is recorded or sent anywhere. See [Agent keys](../../docs/systems-usage/agent-keys.md).

The spawned command runs in its own process group (Unix) so a timeout or cancellation can kill the whole tree it started, not just the immediate shell — killed via `killpg` on Unix, `taskkill /T /F` on Windows. A stop or timeout races against the running command rather than waiting for the tool call to return; the call's own child process is what gets killed, not any process a future background-command feature hands off elsewhere. See [Stopping a Turn](../../docs/systems-usage/turn-control.md).

---

## `agent_keys_list`

**Source:** `agent_keys.rs` · `AgentKeysListTool`

**Description sent to LLM:**
> List the agent keys (API keys, tokens) available to exec. Shows each key's name, the environment variable it is exposed as, who created it, and its description. Values are never shown; use a key by naming it in exec's `keys` parameter.

### Input

None.

### Output

`"{N} agent key(s). Expose one to a command with exec's \`keys\` parameter; values are redacted from all output."` followed by one line per key: `"- {name} -> ${ENV_VAR} (created by user|agent): {description}"` (`(no description)` when empty).

With no keys: a message saying the user can add one with `residuum agent-keys set <name>` or in the web UI, and that the agent can mint one with exec's `store_output_as`.

On error: `"couldn't read the agent key store: {reason}"`.

---

## `agent_key_delete`

**Source:** `agent_keys.rs` · `AgentKeyDeleteTool`

**Description sent to LLM:**
> Delete an agent key you created (e.g. a minted token that is no longer needed). Keys the user created can't be deleted this way.

### Input

| Parameter | Type   | Required | Description                        |
|-----------|--------|----------|------------------------------------|
| `name`    | string | yes      | Name of the agent key to delete    |

### Output

On success: `"deleted agent key '{name}'"`.

On error: `"no agent key named '{name}'"`, or `"agent key '{name}' was created by the user; only keys the agent created can be replaced or deleted by the agent"`, or a store read/write failure.

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
> Retrieve a raw transcript by episode ID or session run ID — provide exactly one of the two. Use episode_id after memory_search to drill into a merged episode's full conversation. Use run_id to read a session run's transcript directly from the session store — e.g. to follow a resume pointer to a run that produced no episode, or to check on a run that's still in progress. Returns formatted message lines with role labels and line numbers.

### Input

| Parameter    | Type    | Required | Description                                              |
|--------------|---------|----------|----------------------------------------------------------|
| `episode_id` | string  | one of `episode_id`/`run_id` | The episode ID to retrieve (e.g., `"ep-001"`) |
| `run_id`     | string  | one of `episode_id`/`run_id` | The session run ID to retrieve (e.g., `"run-1234567890-abcd1234"`) |
| `from_line`  | integer | no       | Start reading from this line offset (1-indexed, default: start) |
| `lines`      | integer | no       | Number of message lines to return (default: 50, max: 200) |

**Security:** `episode_id`/`run_id` containing `/`, `\`, or `..` is rejected with a path-traversal error.

### Output

On success (episode mode): formatted transcript with header (`Episode: {id}`), message lines as `[line {N}] {Role}: {text}`, and an optional footer showing the range when `from_line`/`lines` are used.

On success (run mode): formatted transcript with header (`Run: {run_id} | address: {address} | category: {category} | state: {state}`, plus `| episode: {id}` once merged), the same `[line {N}] {Role}: {text}` message lines, and the same range footer. A run that hasn't completed yet is read from its live incremental transcript.

On error:
- Both `episode_id` and `run_id` given → `"provide exactly one of 'episode_id' or 'run_id', not both"`
- Neither given → `"missing required 'episode_id' or 'run_id' argument"`
- Episode not found → `"episode '{id}' not found"`
- Run not found → `"run '{id}' not found; use list_agents to find live session addresses, or memory_search for merged episodes"`
- `episode_id`/`run_id` is empty or contains invalid characters
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
>
> From a session's registry, the description carries one more sentence: "Running in a session: the owner's DM on every chat interface and the web UI are refused — message main instead so it can decide what to tell the owner. Posting to any other conversation or endpoint still works."

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
- From a session, target reaches the owner directly (the web UI endpoint, or the owner's DM on a chat interface — named explicitly or via the no-conversation default, including the default on a chat interface whose owner hasn't been claimed yet) → `"sessions cannot message the owner directly; message main instead so it can decide what to tell the owner"`
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

**Not available to sessions:** the one tool registered only on the main agent's registry — it redirects main's background-turn output, which is meaningless for a session.

---

## `stop_agent`

**Source:** `background.rs` · `StopAgentTool`

**Description sent to LLM:**
> Stop a live session by address, or cancel your open task with a remote agent (address "a2a:<name>"). Stopping a session cancels any in-flight turn and moves it to completing; its transcript is kept, not discarded. The main agent cannot be stopped this way. Use list_agents to find live addresses and remote agents.

### Input

| Parameter | Type   | Required | Description                                  |
|-----------|--------|----------|----------------------------------------------|
| `address` | string | yes      | The address of the session to stop, or `"a2a:<name>"` to cancel the caller's open task with that remote agent |

### Output

On success (session): `"Stopping session {address}."`

On success (remote agent): `"Canceling task {task_id} with remote agent a2a:{name}."`

On error (`address` is `"main"`): `InvalidArguments` — the main agent cannot be stopped this way.

On error (address not live): `"No live session with address {address}."` (returned as `is_error = true`)

On error (unknown remote agent, `is_error = true`): `"no remote agent named 'a2a:{name}'. Check config/a2a.json or list_agents."`

On error (no open task with that remote agent, `is_error = true`): `"no open task with remote agent a2a:{name}."`

On error (the remote agent can't be reached to cancel, `is_error = true`): the hub's plain-language reachability error.

**Side effect (session):** Cancels the session's stop token. A running turn ends at its next checkpoint (model-call boundary or tool-loop iteration); an idle session skips straight to completing. Either way the run's transcript so far is kept and merged like any other completed run.

**Side effect (remote agent):** Calls `CancelTask` on the caller's open task with that agent via the `A2aClientHub`/`RemoteTaskTracker` (`crate::a2a::client`). The task's outcome (typically `canceled`) is delivered back to the caller the same way any other outbound-task update is — see `message_agent` below and `docs/systems-usage/a2a.md`.

---

## `list_agents`

**Source:** `background.rs` · `ListAgentsTool`

**Description sent to LLM:**
> List the main agent, every live (running or idle) session, and every remote agent reachable over A2A (address "a2a:<name>"): for sessions, address, category, source, state, depth, spawner, elapsed time, and purpose; for remote agents, online status, description, skills, and your own open tasks with them. Completed sessions are not listed, but their addresses remain valid.

### Input

No parameters required (empty object accepted).

### Output

```
main — always live
{N} live session(s):
  [{address}] {source_label} — category: {scheduled|external|spawned} — state: {forking|running|idle|completing} — depth: {N} — spawner: {address|-} — running {elapsed}s — purpose: {prompt/task preview, up to 120 chars}

{N} remote agent(s):
  [a2a:{name}]{ (your instance)} {resolving its agent card|online — {description}|error — {reachability message}} — skills: {name} ({id}), ...
    task {task_id} — {state} — {last_status_text|(no status yet)}
```

`main` is always listed first, even when no sessions are live. `spawner` is `-` for `scheduled` and `external` sessions — only `spawned` sessions have one. Remote agents come from `config/a2a.json` and from relay-sibling discovery via the `A2aClientHub`; a sibling (one of the user's own other instances) carries the ` (your instance)` marker, a `config/a2a.json` entry doesn't. The skills line is omitted when the card hasn't resolved yet or declares none. Task lines list only the caller's own open (non-terminal) tasks with that agent, from the `RemoteTaskTracker`. See `docs/systems-usage/a2a.md`.

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

The session runs in the background via the session runtime. Every turn's outcome — completed, failed, cancelled, or panicked — is relayed to its **direct spawner** — the agent that called `subagent_spawn` — tagged with its address; for a nested spawn (a session spawning a session), that's the spawning session, not necessarily main.

### Nesting and the depth cap

`subagent_spawn` is registered both for the main agent and for every session, so sessions can spawn sessions. The tool instance carries the caller's own address and depth (main is `MAIN_ADDRESS`/`MAIN_DEPTH`; a session's own registry carries its own address/depth): a new spawn is refused once `depth + 1` would exceed the configured `subagent_depth_cap` (default 3, `[background]` config); the refusal names that setting. The spawned session's `spawner` field records the calling agent's address, and its `depth` is the caller's depth plus one.

### Errors

- Missing or empty `task` → `InvalidArguments`
- `skill` is `"main"` (reserved, case-insensitive) → `InvalidArguments`
- Invalid `model` value → `InvalidArguments`
- Unknown `skill` (not in the skill index) → `is_error = true` with the available skill list
- Spawning past the depth cap → `is_error = true`, e.g. `"cannot spawn: nesting depth cap (3) reached at depth 3 — handle this task directly instead of spawning further, or have a shallower agent spawn it. Raise the \`subagent_depth_cap\` setting under \`[background]\` in config.toml to allow deeper nesting"`
- Bus publish failure → `Execution` error

**Side effects:** Publishes a `SpawnRequestEvent` (carrying the pre-generated address, the caller's address as `spawner`, the computed `depth`, and a `hop_count` one more than the calling turn's current hop count) to the bus. The spawn listener picks it up, builds the session's fork resources, and hands it to the session runtime (visible via `list_agents`, cancellable via `stop_agent`). Every turn's outcome is relayed directly to the spawner via `AgentMessenger` (see `message_agent` below), not through the bus notification router.

---

## `message_agent`

**Source:** `message_agent.rs` · `MessageAgentTool`

**Description sent to LLM:**
> Send a text message to another agent by address — main, any session (running, idle, or previously completed), or a remote agent reachable over A2A (address "a2a:<name>"). A running session sees it as an interrupt at its next tool-call boundary; an idle one starts a new turn with it; a completed one is resumed as a new run at the same address. A remote agent's reply does not arrive immediately — it comes back later as an agent message from "a2a:<name>", once its task reaches a state that needs your attention. Every delivered message names your own address and category so the recipient can reply. Use list_agents to find addresses and remote agents.

### Input

| Parameter | Type   | Required | Description                                                  |
|-----------|--------|----------|----------------------------------------------------------------|
| `to`      | string | yes      | Address to message: `"main"`, a session address from `list_agents`, or `"a2a:<name>"` for a remote agent. |
| `message` | string | yes      | The message body.                                             |
| `skill`   | string | no       | Only meaningful when `to` is a remote agent: the id of one of its advertised skills, sent as `message.metadata.skill`. |

### Output

- Delivered to main: `"Message delivered to main."`
- Delivered to a live session: `"Message delivered to {address}."`
- The target session is completing: `"Session {address} is completing; your message will be delivered once it finishes, resuming it as a new run."` (`is_error = false`) — the call returns immediately; delivery itself happens on a detached task once the run clears (see Side effects).
- Delivered to a completed session: `"Session {address} had completed; message delivered by resuming it as a new run."` (`is_error = false` — the resume itself is not a failure)
- Sent to a remote agent: `"Sent to remote agent a2a:{name} (task {task_id}). Its reply will arrive as an agent message."` (`is_error = false`) — the call returns as soon as the remote agent accepts the task; it does not wait for the task to progress.
- Unknown address (`is_error = true`): `"no such agent '{to}'. Use list_agents to see live sessions; a completed session's address only works again once it has run at least once."`
- Unknown remote agent (`is_error = true`): `"no remote agent named 'a2a:{name}'. Check config/a2a.json or list_agents for known remote agents."`
- Remote agent not currently reachable (`is_error = true`): the hub's plain-language reachability error (e.g. its card hasn't resolved, or the last attempt failed).
- Remote agent rejected the request (`is_error = true`): `"remote agent a2a:{name} couldn't complete the request: {error}"`
- Messaging yourself (`is_error = true`): `"cannot message yourself"`
- An `artifact` session messaging `main` (`is_error = true`): `"artifact sessions can't reach the main conversation: your responses are shown to the artifact that started you. To bring something to the user's attention, file an inbox item with user_inbox_add instead."` Nothing is delivered. Every other target works as usual for an artifact session.
- Target's interrupt channel is saturated (`is_error = true`, vanishingly unlikely): `"agent {address} is busy, try again shortly"` — never falls back to a resume, which would double-register the address.
- Hop count at or above the configured hard limit (`is_error = true`): a message explaining the loop limit was reached and delivery was refused. The message never reaches `to`; logged at `warn` with both addresses and the hop count, and a best-effort note is recorded in the transcript of whichever side is a live, addressable session.
- A publish (to main, or as a resume spawn request) failed at the bus (`is_error = true`): a message naming what failed. The tool never reports success when delivery didn't actually happen.

### Errors

- Missing or empty `to`/`message` → `InvalidArguments`

### Remote agents (`to: "a2a:<name>"`)

Routed through `crate::a2a::client`'s `A2aClientHub` (resolves the agent's card and builds a protocol client) and `RemoteTaskTracker` (persists the outbound task and starts watching it). If the caller has an open task with that agent waiting on a reply (`INPUT_REQUIRED`/`AUTH_REQUIRED`), the message is sent as a follow-up on that task; otherwise a new task starts in the persisted (caller, agent) conversation context, if one exists. The send uses `configuration.return_immediately = true`, so the tool returns as soon as the remote agent accepts the task. The task's later outcome — it asks a question, needs auth, completes, fails, or is canceled — is delivered back to the caller via `AgentMessenger::send`, from address `a2a:{name}`, category `"remote"`. See `docs/systems-usage/a2a.md` for the delivery format and artifact handling.

**Hop counts:** every delivered message carries a hop count — one more than the highest hop count among the inputs driving the sender's current turn (its kickoff input, plus any agent messages drained mid-turn). A message that arrives mid-turn but isn't consumed before the turn ends carries its hop count forward into whichever turn picks it up next rather than losing it, so a loop can't reset the count to zero just by arriving at the wrong moment. At or above `hop_soft_limit` (`[background]`, default 8) the delivered content carries an added note asking the receiver to reply only if a reply is actually needed. At or above `hop_hard_limit` (default 32) delivery is refused outright, and the error names the `hop_hard_limit` setting (see Output above).

**Side effects:** Routes through the shared `AgentMessenger` (`crate::background::messaging`):
- **main** — publishes a `MessageEvent` on the `UserMessage` bus topic, formatted with the sender's address and category, reusing the main event loop's existing interrupt-if-running/new-turn-if-idle handling. The hop count is recorded under the published event's id, since `MessageEvent` itself carries no hop-count field.
- **running/idle session** — delivers an `Interrupt::AgentMessage` through the session's own interrupt channel (registered in the `SessionRegistry` at fork time). A running turn drains it at its next tool-call boundary; an idle session wakes and runs another turn in the same run, with the message as that turn's input.
- **completing session** — the run no longer accepts messages into itself. `AgentMessenger` hands the wait to a detached task (`wait_until_clear()` on a `tokio::sync::Notify`, not polling) and returns immediately; once the run actually leaves the registry (its resume point is always recorded before that happens), the task re-checks the address before acting: if another message already resumed it in the meantime, this one delivers straight into that live run; otherwise it resumes the session, same as a completed session below. That re-check is what lets two messages queued to one completing session both land, as exactly one new run, instead of the second racing a duplicate spawn and losing. A message still queued in a run's own interrupt channel when its teardown drains it (e.g. one delivered just as `stop_agent` lands) is handled the same way, combined into the resumed run's opening prompt if more than one arrived.
- **completed session** — publishes a fresh `SpawnRequestEvent` at the same address, carrying the previous run's model tier, the delivered message's own hop count as the new run's starting hop count, and a pointer to its episode id (or run id, retrievable with `memory_get`) in the new run's context. Goes through the ordinary spawn-listener path, exactly like any other session fork. `SessionRegistry::register` is a compare-and-swap: if a spawn/resume for this address raced another one that already registered it live, the loser delivers its own input into the winning run as a message instead of clobbering it. The listener applies the same fallback for a spawn/resume request that arrives for an address it can see is already running/idle/forking — the normal outcome of two messages resuming the same completing session, not an error case.

**Available to sessions:** registered in both the main agent's registry and `build_subagent_registry()`, each instance identifying itself with its own address and category (`"main"` for the main agent), and holding a clone of that agent's current-turn `HopCounter`.

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

**Conditional registration:** only registered when `web_search.standalone_backend.name == "ollama"` in config. Registered on both the main agent's registry and every session's (`build_subagent_registry()`), gated by the same check. Live on config reload: main's copy is added/removed in place by `Agent::reload_ollama_web_search_tool`, and a session forked after the reload picks up the new gating automatically since its registry is built fresh from the reloaded `SpawnContext`.

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

**Side effects:** Submits a sanitized OTLP trace dump and the runtime client context (version, OS, model, and an allowlist of configuration toggles, counts, and enums: never keys, paths, URLs, or names) to the developer ingest service via `agent-residuum.com/api/v1/bug-report`. Span content is forcibly sanitized regardless of the runtime `sanitize_content` toggle.

**Available to sessions:** registered in both the main agent's registry and `build_subagent_registry()`, against the same shared `TracingService` and a runtime client context snapshot taken at fork time.

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

**Available to sessions:** registered in both the main agent's registry and `build_subagent_registry()`, against the same shared `TracingService`.

---

## `a2a_task_update`

**Source:** `a2a_task_update.rs` · `A2aTaskUpdateTool`

**Description sent to LLM:**
> Report this A2A task's outcome to the caller that delegated it. Call this when the delegated task is done, when you need more input from the caller before you can continue, or when the task cannot be completed. Your final answer for the caller goes in `message` — the caller only ever sees what you put there, not the rest of your turn output.

### Input

| Parameter   | Type          | Required | Description                                                                                                   |
|-------------|---------------|----------|-----------------------------------------------------------------------------------------------------------------|
| `state`     | string (enum) | yes      | `"completed"` when the work is done, `"input_required"` when you need more information before continuing, `"failed"` when the task cannot be completed. |
| `message`   | string        | yes      | The message the caller sees: your final answer for `"completed"`, the question for `"input_required"`, or an explanation for `"failed"`. |
| `artifacts` | array<string> | no       | Workspace-relative paths of files to attach as artifacts.                                                       |

### Output

On success: `"Task marked {completed|marked as needing more input|marked failed}; the caller has been notified."`

On error: an unknown `state`, an empty `message`, or an artifact path that's empty, escapes the workspace, doesn't exist, or exceeds 20 MB — reported as `is_error = true` naming the specific artifact and reason.

**Side effect:** Publishes an `A2aTaskSignalEvent` on the bus, which the session's A2A task executor is waiting on to end the task's execution stream with the matching status. Each requested artifact is read into an A2A part first (UTF-8 text becomes a text part; anything else becomes a raw part with a detected media type) — a read failure fails the whole call before anything is published, so a caller never sees a partial update.

**Available to sessions:** registered only in a session whose `SubagentToolDeps.conversation_target` names the `a2a` endpoint — never in the main agent's registry, and never in a session started any other way. See `docs/systems-usage/a2a.md`.
