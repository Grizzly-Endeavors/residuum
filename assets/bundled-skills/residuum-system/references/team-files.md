# Team Files

You see one file tree: your own directory, plus a `team/` prefix for the folder every agent on the team shares (team rules, the user's core facts, the wiki, the workbench, team skills).

## Paths

- A relative path starting with `team/` is in the shared team folder: `team/wiki/people/sam.md`. This works in `read_file`, `write_file`, `edit_file`, the `artifacts` of `a2a_task_update`, and the workspace file API (`/api/workspace/files`, `file`, `raw`, `tree`, `read`, `validate`, `dir`, `move`, and `DELETE /api/workspace/file`).
- Any other relative path is in your own directory, so `SOUL.md` is yours.
- Absolute paths work as usual.
- Your own directory can never contain a `team` entry. A write to a path inside `<your directory>/team/` is refused with an explanation; use `team/...` to work in the shared folder.
- A teammate's directory is reachable only by absolute path. To share a file, put a copy in the team folder or hand over its absolute path.
- The `team` folder itself can't be replaced, moved, or deleted through the file API. Deleting, raw-overwriting, or moving a `team/...` path through the file API checkpoints the team repository first (a move between your folder and `team/` checkpoints both), and the response names each checkpoint's repository (`checkpoint_repo`). `team/.index/`, `team/vectors.db` and its sidecars, and atomic-write temp files are hidden, as elsewhere.

## Shared writes

Other agents and the user can write the same team file. When you read a team file, Residuum records its version. `write_file` and `edit_file` on a team path then check, under that path's lock, that the file is still at the version you read, and write it atomically only if so.

If the file changed since you read it, nothing is written and the error tells you the path, that it changed since you read it, and who changed it: `teammate <name>`, `the user`, `another session of yours`, or `an unknown writer (a change made outside Residuum)`. Read the file again with `read_file`, then reapply your change to its current contents. After your own write you don't need to re-read before your next edit.

Creating a team file that someone else created first is the same conflict. Your private files (everything outside `team/`) are not checked this way.

The web UI keeps its `If-Match` precondition on saves (`412` when the file changed), checked under the same lock, and a save by the user is recorded as the user's write.

## Watching changes

`watch_workspace` prefixes starting with `team/` (for example `team/workbench` or `team/wiki`) receive changes to the team folder with `team/`-prefixed paths. Plain prefixes receive changes to your own directory, and `""` receives both.
