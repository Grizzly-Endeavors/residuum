# Checkpoints

Checkpoints are a hidden git history of an agent's workspace and its config files, used for recovery — not a user-facing version control system, and not something the agent or the user manages directly as git.

There are three checkpoint repositories, each a bare git repository living outside the directory it tracks so a `.git` the user keeps in the workspace is never touched: the workspace repository (`hub/checkpoints/workspace.git`, tracking the agent's workspace root — `~/.residuum/<agent-name>/` — minus its own `config/config.toml` and `config/providers.toml`), the agent-config repository (`hub/checkpoints/agent-config.git`, tracking exactly those two files), and the hub-config repository (`hub/checkpoints/hub-config.git`, tracking the hub's `config.toml` and the encrypted key stores in `~/.residuum/hub/`). All three git-dirs live under `~/.residuum/hub/checkpoints/`, regardless of which repository's content lives elsewhere. Each repository holds a single linear branch, `refs/heads/checkpoints`; every checkpoint is a commit on it, rebuilt from scratch from whatever should currently be included rather than patched onto the parent, so a file that disappeared is simply absent from the new commit rather than needing separate deletion bookkeeping.

The agent-config and hub-config repositories are local-only forever: nothing in Residuum ever adds a remote to either or pushes them anywhere, because `config.toml` and `providers.toml` can hold plaintext API keys. The workspace repository has no remote either, but is designed so a user-configured remote could be added without changing its shape; pushing is not implemented.

## What's excluded

The workspace repository never includes data Residuum rebuilds itself (the search index, `.db`/`.sqlite` files and their sidecars — the same rule the workspace file API and change feed use), a `.git` directory the user keeps in the workspace, lock/PID files, atomic-write temp files, `config/*.last-known-good.toml` files (last-known-good copies live in `hub/`), or the agent's own `config/config.toml` and `config/providers.toml` (those belong to the agent-config repository instead, so plaintext provider keys never land in a repository designed to allow a remote). Symlinks are not followed or included.

The agent-config repository is a fixed allowlist relative to the agent's `config/` directory: `config.toml` and `providers.toml`, each included only when it currently exists. The hub-config repository is a fixed allowlist relative to `~/.residuum/hub/`: `config.toml`, `secrets.toml.enc`, `agent-keys.toml.enc`, and `a2a-keys.toml`, each included only when it currently exists. The machine key files these encrypted stores are decrypted with (`secrets.key`, `agent-keys.key`) are never in that list and are never checkpointed — restoring an encrypted store's `.enc` file only works because its machine key is untouched.

There is no retention or pruning. Every checkpoint stays forever; `GET /api/checkpoints/stats` and `/api/status` show each repository's on-disk size and checkpoint count so its growth is visible.

## When checkpoints are taken

The workspace repository is checkpointed at the start of every agent turn — main's and every background/session turn — if the tree changed since the last checkpoint, attributed as an outside edit; and at the end of every turn, attributed with a short summary of what the turn did. It is also checkpointed immediately before a destructive workspace API action: deleting a file or directory, overwriting via raw PUT, and moving or renaming with overwrite.

The agent-config repository is checkpointed immediately before any write to the agent's own `config.toml` or `providers.toml`: a raw PUT or a settings-patch save, or the setup wizard's writes. The hub-config repository is checkpointed immediately before any write to the hub's `config.toml` or an encrypted key store: every set/delete of a secret, an agent key (from the Settings UI), or an A2A caller key, the agent's own `agent_key_delete` tool and the `exec` tool's `store_output_as` parameter, and the equivalent `residuum secret`, `residuum agent-keys`, `residuum a2a keys`, and `residuum setup` CLI commands. The CLI commands open their own short-lived `CheckpointEngine` against the same on-disk repositories the gateway uses, rather than sharing the running gateway's instance.

Every checkpoint commit records which session/address caused it, its run id and turn/correlation id when it has them, why it was taken, and a short summary. Checkpointing never blocks or fails whatever triggered it: a failure is logged with structured fields and surfaces as a notice on the system notification channel (or, for a CLI command with no running gateway to notify, just a warning log), then the turn, action, or command proceeds as if nothing happened.

Concurrent commits to the same repository — from the gateway, a `residuum` CLI command, or several of either at once — are serialized rather than interleaved or lost. Each repository's git-dir holds a lock file that every committer acquires (with a brief bounded retry, never an indefinite wait) before reading the current tip, building the new commit, and moving the branch ref; the ref move itself is additionally a compare-and-swap against the exact tip that was read, so a commit can never silently overwrite a sibling written by another process in the gap between reading the tip and writing the ref.

## Restore and undo

Restoring a path (a file or a directory) writes that path's content back from a checkpoint, making the on-disk subtree match the checkpoint's exactly for a directory (a file present now but absent from the checkpoint there is removed). Undoing a checkpoint reverts every path it changed back to its content just before it, except a path whose current on-disk content no longer matches what that checkpoint recorded — it changed again since, by a later checkpoint or the user — which is skipped and reported rather than clobbered. Both restore and undo checkpoint the result afterward, so either one can itself be undone. Both publish a notice naming what was restored, reverted, or skipped.

## Tools

The agent has two tools, scoped to the workspace repository only — they can never reach the agent-config or hub-config repositories, so they can't be used to read or restore something `PathPolicy` already blocks the agent from touching directly:

| Tool | Does |
|------|------|
| `workspace_history` | Lists checkpoints, optionally filtered to those that changed a given path, or shows what one checkpoint changed. |
| `workspace_restore` | Restores a path to a checkpoint, or undoes a checkpoint's changes. |

## HTTP API

Backs the web UI's history view: every route takes `repo` (`workspace`, `agent_config`, or `hub`) as a query parameter or request-body field.

| Route | Does |
|-------|------|
| `GET /api/checkpoints` | One page of checkpoints, newest first. `path` restricts to checkpoints that changed it; `turn_id` restricts to the checkpoints recorded against one turn (its turn-start/turn-end pair — how the UI finds a turn's checkpoints for "undo this turn"); `before` and `limit` page. |
| `GET /api/checkpoints/stats` | On-disk size, checkpoint count, and oldest checkpoint for a repository. |
| `GET /api/checkpoints/{id}` | A checkpoint's metadata plus the paths it changed. |
| `GET /api/checkpoints/{id}/diff` | Unified diff for one `path` at a checkpoint, relative to the checkpoint before it. |
| `GET /api/checkpoints/{id}/file` | A file's raw content at a checkpoint. |
| `POST /api/checkpoints/{id}/restore` | Restores `path` to this checkpoint. |
| `POST /api/checkpoints/{id}/undo` | Undoes this checkpoint's changes. |

`/api/status` additionally carries a `checkpoints` field with each repository's stats, so size is visible without the routes above.

## Web UI

Settings → History lists checkpoints from any of the three repositories (a tab per repo), filterable by path, with repo size and checkpoint count shown above the list. Selecting one shows the paths it changed, each with a diff or full-content view and a Restore action; a checkpoint can also be undone outright. The hub-config repo's encrypted key stores (`secrets.toml.enc`, `agent-keys.toml.enc`) never show a diff or content there — only that they changed, with a restore action described in plain language ("restores the saved keys to how they were at this point").

The Workspace file browser opens the same history, filtered to one file, from a History action on each file row — with a restore per version — alongside single-click Delete and an inline Rename.

A turn's own checkpoint pair is exposed as "Undo this turn" on the user message that started it, in both the main chat and a session view, once it's confirmed the turn changed the workspace — hidden while that's still being checked or once it's confirmed the turn changed nothing. This is only available for a turn observed live in the current connection: checkpoints don't persist a turn's id into chat history, so an older turn is only undoable from Settings → History (find its checkpoint, undo it there).

Every other destructive action reachable from Settings or the workspace browser fires on a single click. Agent key delete, A2A key revoke, and workspace file delete each return `checkpoint_id`: the checkpoint that holds the tree from just before the action. The result toast's Undo restores that checkpoint's copy of the affected path, even if a newer checkpoint was taken in the meantime. When the checkpoint could not be recorded the field is null, the action still completes, and the toast has no Undo. Removals from the Settings form (an MCP server, a provider, a webhook) autosave a moment later through `PATCH /api/config/patch`, `/api/hub/config/patch`, `/api/providers/patch`, or `/api/mcp/patch`, and each of those returns the same `checkpoint_id` for its write. Their Undo depends on whether that save has written the removal yet: before it has, Undo cancels the pending save and puts the entry back in the form; once it has, Undo restores the file from the checkpoint that save reported, which brings back the whole entry, including fields the form doesn't show. If that save's checkpoint failed, Undo says so instead of rebuilding the entry from the form.
