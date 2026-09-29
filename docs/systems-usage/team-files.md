# Team files

Every agent sees one logical file tree: its own directory, plus a `team/` prefix that maps onto the shared team directory (`~/.residuum/team/`). The team directory holds what every agent in the hub shares: team rules (`AGENTS.md`), the user's core facts (`USER.md`), the wiki, the workbench, and team skills.

## The `team/` namespace

- A relative path starting with `team/` (or exactly `team`) names a file in the team directory. `team/wiki/people/sam.md` is `~/.residuum/team/wiki/people/sam.md`.
- Every other relative path resolves against the agent's own directory, so `SOUL.md` is the agent's own file.
- Absolute paths are used as given.
- The agent directory can never contain its own `team` entry, so the prefix is unambiguous. `write_file` and `edit_file` refuse a path inside `<agent>/team/` with an error explaining the reservation, and the web file API does the same.
- Another agent's directory is reachable only by absolute path. Agents share files by handing them over through the team area or by absolute path. Nothing adds a cross-agent file API.

The prefix applies in `read_file`, `write_file`, `edit_file`, the artifact paths of `a2a_task_update`, and the workspace file HTTP API (`/api/workspace/files`, `file`, `raw`, `tree`, `read`, `validate`, `dir`, `move`, and `DELETE /api/workspace/file`). A path is judged the way it is written: diagnostics for a strictly-parsed file (a team skill's `SKILL.md`, say) apply to its real location, and the write-scoping policy checks the real path.

In the web API the workspace root listing shows a `team` folder, a whole-workspace `tree` includes it, and a `team/...` path is read, written, moved and deleted like any other. The `team` folder itself can't be replaced, moved or deleted. Deleting, raw-overwriting, or moving a `team/...` path checkpoints the team repository first (a move between the agent directory and `team/` checkpoints both repositories), and the response names each checkpoint with its `checkpoint_repo` so Undo restores from the right one; see `checkpoints.md`. Paths in errors are shown as the client wrote them. What the access policy hides stays hidden under `team/`: `team/.index/`, `team/vectors.db` and its `-wal`/`-shm`/`-journal` sidecars, and atomic-write temp files.

## Write coordination

Several agents, and the user in the web UI, can write the same team file. Every write under `team/` goes through the team write coordinator, a hub-level service shared by every agent's tools and the web file API.

- **Versions at read.** When `read_file` reads a team file it records the file's version: the same mtime-and-size token the web API returns as `ETag`, plus the number of writes the coordinator has recorded for that path. The version is taken just before the read. The tools that share one tracker (an agent's main turn, or one background session) share what they recorded, and a session starts with an empty record.
- **Checked writes.** `write_file` and `edit_file` on a team path take that path's lock, compare the file's current version with the recorded one, and write only when they match. The write is atomic (temporary file, then rename), and the agent is recorded as the path's last writer. After its own write the agent's recorded version is updated, so its next edit needs no re-read.
- **Creating a file.** Writing a team file that did not exist when the tool checked succeeds only if it still does not exist under the lock.
- **Conflicts.** When the versions differ, nothing is written and the tool returns an error naming the path (`team/...`), saying it changed since the agent read it, and naming who changed it: `teammate <name>`, `the user`, `another session of yours`, or `an unknown writer (a change made outside Residuum)` when the change did not go through Residuum. It tells the agent to read the file again and reapply its change. The conflict is logged at `info` with the agent that was refused and the last writer.
- **Web UI.** Saves carry an `If-Match` precondition (`412` on a stale version), checked under the same per-path lock, and the user is recorded as the writer. A save without `If-Match` is unconditional. Moves and deletes of team files take the lock too, so an agent that read a file the user then moved or deleted is told the user changed it.
- **Deleted files.** When `read_file` finds a team file missing, it records that the file is absent. An agent whose earlier read of the file predates a deletion gets one conflict naming who deleted it; after it reads the path again (which reports not found), `write_file` creates the file. The conflict message says so when the file no longer exists.
- **Every team write is coordinated.** Besides the file tools and the web file API, these take the path locks and record their writer: a workbench artifact delete (the page, its folder and the files in it, and its `<name>.*` data files, recorded as the user), a checkpoint restore or undo into `team/` (every file it may write or remove, recorded as the user from the web UI and as the agent from `workspace_restore`; see `checkpoints.md`), and the role page, roster index and wiki log an agent's role-page creation writes (recorded as that agent). A restore or role-page write waits for a file tool holding the same path, and an agent that read a file any of them changed gets a conflict naming who changed it.
- **Agent-private files:** a file must be read before it is overwritten, and no version check runs.

The coordinator is one instance per hub. It is constructed once when the gateway starts from the `TeamPaths` of the workspace layout (`TeamWriteCoordinator::new(layout.team())`) and cloned into each agent's path policy (`TeamFiles`, the agent's view of the namespace and its identity as a writer) and into the web API state (the user's view). The checkpoint engine takes the same `layout.team()`, so the coordinator, the engine and the layout all locate `team/` the same way. A change made outside Residuum, such as an editor saving the file, is detected by the version and reported as an unknown writer.

## Change feed

The hub watches `team/` with the same watcher as an agent's directory (native notifications with a polling fallback, debouncing, blocked-path filtering, resyncs) and publishes on the same workspace topic with `team/`-prefixed paths. A WebSocket `watch_workspace` with `team/...` prefixes receives team changes; plain prefixes receive changes to the agent's own directory; `""` receives both. The workbench reload watcher and artifacts follow `team/workbench/` through this feed. See [Workbench](workbench.md#change-feed).
