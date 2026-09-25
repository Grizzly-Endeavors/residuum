# Checkpoints

Checkpoints are a hidden git history of the workspace, used for recovery. Not a user-facing version control system — there's no staging, no branches, no commit messages to write.

Residuum takes a checkpoint automatically at the start and end of every turn (start captures edits made outside Residuum since the last checkpoint; end captures what the turn itself did) and before a destructive workspace action (delete, overwrite, move/rename with overwrite, workbench artifact delete). Checkpointing never blocks or fails a turn or action — a failure is logged and surfaces as a notice, then things proceed normally.

There's a second, separate checkpoint repository for the root config files (`config.toml`, `providers.toml`, and the encrypted key stores), checkpointed before writes to those. It's local-only and never reachable from either tool below.

## Tools

| Tool | Key Parameters | Description |
|------|---------------|-------------|
| `workspace_history` | `action` (`list`\|`show`), `path`, `limit`, `checkpoint_id` | List checkpoints (optionally filtered to ones that changed a path), or show what one checkpoint changed. |
| `workspace_restore` | `action` (`restore_path`\|`undo_turn`), `checkpoint_id`, `path` | Restore a file/directory to a checkpoint, or undo a checkpoint's changes. |

Both tools only ever see the workspace checkpoint repository — never the config one.

## Restoring vs. undoing

`restore_path` writes a file or directory back to exactly what it looked like at a checkpoint. For a directory, this makes the on-disk subtree match the checkpoint exactly — a file that exists now but didn't exist there gets removed.

`undo_turn` reverts everything a checkpoint changed back to its content just before that checkpoint, in one call — useful for "undo what I just did." A path that was touched again since (by a later turn or the user) is skipped rather than clobbered, and the tool result names what was skipped.

Both actions checkpoint the resulting state afterward, so a restore or an undo can itself be undone with another `undo_turn` call against the new checkpoint id the tool result returns.

## Gotchas

- `checkpoint_id` comes from `workspace_history`'s `list`/`show` output — there's no way to guess one.
- `path` must be relative to the workspace root and can't contain `..` — both tools reject a path that would escape it.
- There's no retention or pruning: every checkpoint is kept. If workspace history feels heavy, that's expected for now.
