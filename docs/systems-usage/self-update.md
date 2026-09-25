# Self-Update, Rollback, and Startup Health

Residuum can update its own binary and restart itself, from the web UI's Settings → Version page (`GET /api/update/status`, `POST /api/update/check`, `POST /api/update/apply`) or the CLI (`residuum update`, `residuum update -y`). Both paths funnel through `update::download_and_install`. This page covers what happens after that call, including how a bad update rolls itself back and how `residuum serve`/`residuum stop` know whether the gateway is actually healthy rather than merely running.

## Readiness: What "Healthy" Means

A gateway process is healthy once its providers and workspace have finished initializing and its HTTP listener is bound and accepting connections — reaching the PID lock alone is not enough, since that happens before any of that initialization runs. The gateway marks this moment by writing a readiness file (`residuum.ready` in the config directory, `daemon::write_ready_file`) right after its listener binds. That file is cleared at the start of every fresh startup attempt and on exit, so a waiter never reads a marker left by a previous run.

Two things wait on this marker with the same 60-second timeout (`daemon::READINESS_TIMEOUT`) and the same polling primitive (`daemon::wait_for_ready`), each also watching whether the process they spawned has exited early:

- **`residuum serve`** (the daemon spawner, `commands::serve::daemon`) spawns the foreground gateway as a detached child and doesn't report "started" until this marker appears. If the process exits first or the timeout elapses, it reports the plain-language reason instead — read from a startup-error marker the failed process leaves behind (`daemon::write_startup_error`/`read_startup_error`) when one is available, falling back to a generic message otherwise. Either way it also points at the log files: `residuum logs`, and the daemon's raw stderr at `daemon::stderr_log_path` (`<config dir>/logs/serve.stderr.log`) — stderr is appended there rather than discarded, so a panic or an error from before tracing initializes is never silently lost.
- **The update-rollback watchdog** (below) waits on the exact same marker to decide whether the new version came up.

If a last-known-good config fallback is in play elsewhere in the startup path, it still counts as healthy here: readiness only cares that the listener ends up bound, not which config path got it there.

## Preserving the Previous Binary

`update::download_and_install` never leaves only a possibly-broken binary on disk. Before installing the new one, it renames the currently-running binary to `update::previous_binary_path(exe)` — the same path with `.prev` appended (`residuum` → `residuum.prev`, `residuum.exe` → `residuum.exe.prev`) — clearing any stale backup from an earlier update that was never confirmed healthy first. Only after that succeeds does the new binary take the original path. If the final swap fails, the preserved binary is restored immediately so the install failure never leaves nothing executable at all.

GitHub Releases publishes no checksum or signature for these binaries (see `.github/workflows/release.yml`), so there is nothing to verify the download against beyond the HTTPS connection itself.

A successful install leaves a `PendingRollback` marker (`residuum.update-pending.json`) recording where the previous binary went and which versions are involved. `commands::serve::foreground::relaunch` reads and deletes this marker the next time a restart happens, and that presence is what decides which of the two restart paths below runs.

## Two Restart Paths

**Plain restart** — no `PendingRollback` marker present, e.g. `POST /api/update/restart` on its own. The binary didn't change, so there's nothing to roll back to if it fails: Unix `exec()`s the same process image in place (the PID and PID file stay valid); Windows starts a new process and this one exits, since Windows can't replace a running process image.

**Rollback-capable restart** — a `PendingRollback` marker is present. Rather than exec'ing directly into the new (possibly broken) binary, the current process hands off to the update-rollback watchdog (`commands::update_watchdog`, invoked as the hidden `residuum update-watchdog` subcommand) and exits. Critically, the watchdog runs *from the preserved previous binary*, not the new one — so even a new binary that can't execute at all (wrong platform, truncated download, corrupted file) still gets supervised and rolled back, instead of taking the supervisor down with it.

The watchdog:

1. Starts the new binary with the same arguments the restart it took over from was using, with stdout discarded and stderr appended to the same daemon stderr log.
2. Waits up to `READINESS_TIMEOUT` for the readiness marker, the same way `residuum serve` does — bailing out early if the new process exits first, or if it couldn't even be spawned.
3. **On success**: deletes the preserved previous binary and exits. Nothing further to do.
4. **On failure** (crashed, timed out, or couldn't execute): kills the new process if it's still running, restores the preserved binary over the gateway's canonical path, writes a `RollbackNotice` (`residuum.rollback-notice.json` — attempted version, plain-language reason, timestamp) via `update::write_rollback_notice`, and starts the restored binary so the gateway comes back up on the version that was working before.

## Surfacing a Rollback

`GET /api/update/status` includes `rollback_notice` whenever one is on disk — read fresh on every call, so it's visible even to someone who opens Settings well after the rollback happened, not just whoever was watching the update in progress. It's cleared automatically at the start of the next successful `download_and_install`, so it only ever describes the most recent attempt.

The Settings → Version page in the web UI reflects this instead of claiming an update in progress will "reconnect automatically" forever: while restarting, it polls `/api/update/status` and shows elapsed time; once the gateway answers again it shows one of three outcomes — updated to the new version, rolled back with the recorded reason, or (past a 90-second client-side window with no response at all) a prompt to check the logs on the machine running Residuum.
