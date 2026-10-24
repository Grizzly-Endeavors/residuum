// ── "Undo this turn" ───────────────────────────────────────────────
//
// A turn's checkpoint pair (turn-start/turn-end) is tagged with the same
// turn id the live `turn_ended`/`session_turn_ended` frame carries — see
// `CheckpointContext::turn_id` and `docs/systems-usage/checkpoints.md`.
// Undoing a turn means undoing its turn-end checkpoint: that reverts every
// path the turn itself changed back to its content just before the turn
// started, skipping anything changed again since.
//
// A retried turn keeps its id, so each attempt (the one that failed and each
// rerun) records its own pair under that id. Undoing the turn undoes every
// attempt's turn-end checkpoint, newest first: once a rerun's changes are
// undone, a path both attempts touched holds the earlier attempt's recorded
// content again, so the earlier undo puts it back rather than skipping it.

import { fetchCheckpoints } from "./api";
import { configCoordinator } from "./config-coordinator";
import type { UndoOutcome } from "./types";

interface TurnEndCheckpoint {
  id: string;
  changedPathCount: number;
}

/** Every turn-end checkpoint recorded for `turnId`, newest first. Empty if
 * none is recorded (e.g. the turn predates checkpoints, or changed nothing so
 * no checkpoint was worth taking — see `docs/systems-usage/checkpoints.md`). */
async function findTurnEndCheckpoints(
  agent: string | null,
  turnId: string,
): Promise<TurnEndCheckpoint[]> {
  const found: TurnEndCheckpoint[] = [];
  let before: string | undefined;
  for (;;) {
    const page = await fetchCheckpoints(agent, { repo: "workspace", turnId, before, limit: 50 });
    for (const checkpoint of page.items) {
      if (checkpoint.trigger === "turn_end") {
        found.push({ id: checkpoint.id, changedPathCount: checkpoint.changed_path_count });
      }
    }
    if (page.next_cursor === null) return found;
    before = page.next_cursor;
  }
}

/** Whether the turn, which ran on `agent`, changed anything in its workspace,
 * for deciding whether to show or hide "Undo this turn". */
export async function turnChangedWorkspace(agent: string | null, turnId: string): Promise<boolean> {
  const checkpoints = await findTurnEndCheckpoints(agent, turnId);
  return checkpoints.some((checkpoint) => checkpoint.changedPathCount > 0);
}

/** Undo everything this turn changed, across every attempt of a retried
 * turn. `null` if there's no turn-end checkpoint to undo (the caller should
 * treat this the same as "changed nothing" — there's nothing to revert). The
 * outcome merges the attempts' undos: `checkpoint_id` is the last undo's, and
 * a path one attempt's undo had to leave alone is reported as left alone. */
export async function undoTurn(agent: string | null, turnId: string): Promise<UndoOutcome | null> {
  const checkpoints = await findTurnEndCheckpoints(agent, turnId);
  let checkpointId: string | null = null;
  const reverted = new Set<string>();
  const skipped = new Set<string>();
  for (const checkpoint of checkpoints) {
    const outcome = await configCoordinator.undo(agent, checkpoint.id, "workspace");
    checkpointId = outcome.checkpoint_id;
    for (const path of outcome.reverted_paths) reverted.add(path);
    for (const path of outcome.skipped_paths) skipped.add(path);
  }
  if (checkpointId === null) return null;
  return {
    checkpoint_id: checkpointId,
    reverted_paths: [...reverted].filter((path) => !skipped.has(path)),
    skipped_paths: [...skipped],
  };
}
