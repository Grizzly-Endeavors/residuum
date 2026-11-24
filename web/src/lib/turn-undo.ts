import { fetchCheckpoints } from "./api";
import { configCoordinator } from "./config-coordinator";
import type { UndoOutcome } from "./types";

interface TurnEndCheckpoint {
  id: string;
  changedPathCount: number;
}

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

export async function turnChangedWorkspace(agent: string | null, turnId: string): Promise<boolean> {
  const checkpoints = await findTurnEndCheckpoints(agent, turnId);
  return checkpoints.some((checkpoint) => checkpoint.changedPathCount > 0);
}

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
