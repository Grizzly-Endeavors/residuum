// ── Tracking a debounced autosave, for a safe "Undo" on what it just saved ──
//
// Settings' form autosave is debounced: a change sits in a timer for 800ms
// before the actual PATCH goes out. Undoing a removal from that form
// (Providers, MCP servers, webhooks) is only safe to do by reverting local
// state when that save hasn't been sent yet — once it has, the removed
// entry's unmodeled fields (whatever the form doesn't parse back out) are
// already gone from the file on disk, and rebuilding the entry from form
// state alone would silently drop them. Past that point, undoing means
// restoring the file from the checkpoint the server took just before the
// save that removed it — see `notifyFormUndo` in `form-undo.ts`.
//
// This class is the shared, testable piece: it doesn't know about Svelte,
// forms, or specific files, just "is a scheduled save still pending, which
// saves started after a given moment, and which checkpoint each save's
// write reported".

/** What a save that started after a given point did to one file. */
export type SaveWriteLookup =
  /** No save that started after that point has written this file. */
  | { kind: "not-written" }
  /** A save wrote it; `checkpointId` is null when the server's checkpoint
   * failed, so there is nothing correct to restore. */
  | { kind: "written"; checkpointId: string | null };

interface RecordedWrite {
  save: number;
  file: string;
  checkpointId: string | null;
}

// Enough to cover every save a user could make while one toast's Undo is
// still on screen; older entries only matter to toasts long since gone.
const MAX_RECORDED_WRITES = 50;

export class PendingSaveTracker {
  private timer: ReturnType<typeof setTimeout> | undefined;
  private saving = false;
  private settleWaiters: (() => void)[] = [];
  private savesStarted = 0;
  private writes: RecordedWrite[] = [];

  /** Call when a debounced save is scheduled (right after `setTimeout`). */
  markScheduled(timer: ReturnType<typeof setTimeout>): void {
    this.timer = timer;
  }

  /** Call right before the actual save request starts. */
  markSaving(): void {
    this.timer = undefined;
    this.saving = true;
    this.savesStarted += 1;
  }

  /** Call once for each file the current save wrote, with the checkpoint
   * id the server reported for that write (null when it reported none). */
  recordWrite(file: string, checkpointId: string | null): void {
    this.writes.push({ save: this.savesStarted, file, checkpointId });
    if (this.writes.length > MAX_RECORDED_WRITES) this.writes.shift();
  }

  /** Call once the save request has fully resolved, success or failure. */
  markSettled(): void {
    this.saving = false;
    const waiters = this.settleWaiters;
    this.settleWaiters = [];
    for (const resolve of waiters) resolve();
  }

  /**
   * A marker for "now", to pass to {@link firstWriteAfter} later. A save
   * already in flight at this moment snapshotted the form before it, so
   * only saves started after it count.
   */
  mark(): number {
    return this.savesStarted;
  }

  /** The first write to `file` by a save started after `mark`. */
  firstWriteAfter(mark: number, file: string): SaveWriteLookup {
    const write = this.writes.find((w) => w.save > mark && w.file === file);
    return write ? { kind: "written", checkpointId: write.checkpointId } : { kind: "not-written" };
  }

  /**
   * Try to cancel a save that hasn't started yet. Returns `true` if it
   * succeeded — nothing was sent, so the caller can safely revert local
   * form state instead of touching the server. Returns `false` if a save
   * already started (or already finished, or none was ever scheduled) —
   * the caller must wait for it with {@link waitForSettled} and then check
   * {@link firstWriteAfter}.
   */
  cancelIfPending(): boolean {
    if (this.timer !== undefined && !this.saving) {
      clearTimeout(this.timer);
      this.timer = undefined;
      return true;
    }
    return false;
  }

  /** Resolves once any save currently in flight has settled; resolves
   * immediately if none is. */
  async waitForSettled(): Promise<void> {
    if (!this.saving) return;
    await new Promise<void>((resolve) => {
      this.settleWaiters.push(resolve);
    });
  }
}
