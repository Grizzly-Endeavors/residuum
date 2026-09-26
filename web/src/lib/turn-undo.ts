// ── "Undo this turn" ───────────────────────────────────────────────
//
// A turn's checkpoint pair (turn-start/turn-end) is tagged with the same
// turn id the live `turn_ended`/`session_turn_ended` frame carries — see
// `CheckpointContext::turn_id` and `docs/systems-usage/checkpoints.md`.
// Undoing a turn means undoing its turn-end checkpoint: that reverts every
// path the turn itself changed back to its content just before the turn
// started, skipping anything changed again since.

import { fetchCheckpoints, undoCheckpoint } from "./api";
import type { UndoOutcome } from "./types";

/** The turn-end checkpoint for `turnId`, or `null` if none is recorded
 * (e.g. the turn predates checkpoints, or changed nothing so no checkpoint
 * was worth taking — see `docs/systems-usage/checkpoints.md`). */
async function findTurnEndCheckpoint(turnId: string): Promise<{
  id: string;
  changedPathCount: number;
} | null> {
  const page = await fetchCheckpoints({ repo: "workspace", turnId, limit: 10 });
  const turnEnd = page.items.find((c) => c.trigger === "turn_end");
  if (!turnEnd) return null;
  return { id: turnEnd.id, changedPathCount: turnEnd.changed_path_count };
}

/** Whether the turn changed anything in the workspace, for deciding
 * whether to show or hide "Undo this turn". */
export async function turnChangedWorkspace(turnId: string): Promise<boolean> {
  const checkpoint = await findTurnEndCheckpoint(turnId);
  return (checkpoint?.changedPathCount ?? 0) > 0;
}

/** Undo everything this turn changed. `null` if there's no turn-end
 * checkpoint to undo (the caller should treat this the same as "changed
 * nothing" — there's nothing to revert). */
export async function undoTurn(turnId: string): Promise<UndoOutcome | null> {
  const checkpoint = await findTurnEndCheckpoint(turnId);
  if (!checkpoint) return null;
  return undoCheckpoint(checkpoint.id, "workspace");
}
