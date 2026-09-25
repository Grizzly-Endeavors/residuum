# Checkpoints

Checkpoints are a hidden git history of the workspace and the root config files, used for recovery — not a user-facing version control system, and not something the agent or the user manages directly as git.

There are two checkpoint repositories, each a bare git repository living outside the directory it tracks so a `.git` the user keeps in the workspace is never touched: the workspace repository (`~/.residuum/checkpoints/workspace.git`, tracking the workspace root) and the config repository (`~/.residuum/checkpoints/config.git`, tracking the root config files and the encrypted key stores that live alongside `config.toml` in `~/.residuum/`). Each repository holds a single linear branch, `refs/heads/checkpoints`; every checkpoint is a commit on it, rebuilt from scratch from whatever should currently be included rather than patched onto the parent, so a file that disappeared is simply absent from the new commit rather than needing separate deletion bookkeeping.

The config repository is local-only forever: nothing in Residuum ever adds a remote to it or pushes it anywhere, because `config.toml` and `providers.toml` can hold plaintext API keys. The workspace repository has no remote today either, but is designed so a user-configured remote could be added later without changing its shape; pushing is not implemented yet.

## What's excluded

The workspace repository never includes data Residuum rebuilds itself (the search index, `.db`/`.sqlite` files and their sidecars — the same rule the workspace file API and change feed use), a `.git` directory the user keeps in the workspace, or lock/PID files. Symlinks are not followed or included.

The config repository is a fixed allowlist, not a directory walk: `config.toml`, `providers.toml`, `secrets.toml.enc`, `agent-keys.toml.enc`, and `a2a-keys.toml`, each included only when it currently exists. The machine key files these encrypted stores are decrypted with (`secrets.key`, `agent-keys.key`) are never in that list and are never checkpointed — restoring an encrypted store's `.enc` file only works because its machine key is untouched.

There is no retention or pruning. Every checkpoint stays forever; `GET /api/checkpoints/stats` and `/api/status` show each repository's on-disk size and checkpoint count so its growth is visible.

## When checkpoints are taken

The workspace repository is checkpointed at the start of every agent turn — main's and every background/session turn — if the tree changed since the last checkpoint, attributed as an outside edit; and at the end of every turn, attributed with a short summary of what the turn did. It is also checkpointed immediately before a destructive workspace API action: deleting a file or directory, overwriting via raw PUT, moving or renaming with overwrite, and deleting a workbench artifact.

The config repository is checkpointed immediately before any write to a root config file or an encrypted key store: a raw PUT or a settings-patch save to `config.toml` or `providers.toml`, the complete-setup endpoint, and every set/delete of a secret, an agent key (from the Settings UI), or an A2A caller key. A change made through the `residuum secret`/`residuum agent-keys` CLI commands, or the agent's own agent-key tools, is not checkpointed today — those paths don't share the running gateway's engine instance.

Every checkpoint commit records which session/address caused it, its run id and turn/correlation id when it has them, why it was taken, and a short summary. Checkpointing never blocks or fails whatever triggered it: a failure is logged with structured fields and surfaces as a notice on the system notification channel, then the turn or action proceeds as if nothing happened. Concurrent sessions checkpointing the same repository at once are serialized, never interleaved.

## Restore and undo

Restoring a path (a file or a directory) writes that path's content back from a checkpoint, making the on-disk subtree match the checkpoint's exactly for a directory (a file present now but absent from the checkpoint there is removed). Undoing a checkpoint reverts every path it changed back to its content just before it, except a path whose current on-disk content no longer matches what that checkpoint recorded — it changed again since, by a later checkpoint or the user — which is skipped and reported rather than clobbered. Both restore and undo checkpoint the result afterward, so either one can itself be undone. Both publish a notice naming what was restored, reverted, or skipped.

## Tools

The agent has two tools, scoped to the workspace repository only — they can never reach the config repository, so they can't be used to read or restore something `PathPolicy` already blocks the agent from touching directly:

| Tool | Does |
|------|------|
| `workspace_history` | Lists checkpoints, optionally filtered to those that changed a given path, or shows what one checkpoint changed. |
| `workspace_restore` | Restores a path to a checkpoint, or undoes a checkpoint's changes. |

## HTTP API

For the web UI (not yet built): every route takes `repo` (`workspace` or `config`) as a query parameter or request-body field.

| Route | Does |
|-------|------|
| `GET /api/checkpoints` | One page of checkpoints, newest first. `path` restricts to checkpoints that changed it; `before` and `limit` page. |
| `GET /api/checkpoints/stats` | On-disk size, checkpoint count, and oldest checkpoint for a repository. |
| `GET /api/checkpoints/{id}` | A checkpoint's metadata plus the paths it changed. |
| `GET /api/checkpoints/{id}/diff` | Unified diff for one `path` at a checkpoint, relative to the checkpoint before it. |
| `GET /api/checkpoints/{id}/file` | A file's raw content at a checkpoint. |
| `POST /api/checkpoints/{id}/restore` | Restores `path` to this checkpoint. |
| `POST /api/checkpoints/{id}/undo` | Undoes this checkpoint's changes. |

`/api/status` additionally carries a `checkpoints` field with each repository's stats, so size is visible without the routes above.
