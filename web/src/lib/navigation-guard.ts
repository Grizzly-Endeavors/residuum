// The unsaved-edit guard (design §3). A view that holds work the user would
// lose by leaving (unsaved file edits, staged settings changes) registers a
// check. The router consults every check before it navigates, and the browser's
// own prompt covers reload and tab close. What the user is asked is injected:
// this module holds no dialog.

import type { AppLocation } from "./routes";

/**
 * What leaving for `target` would lose: a plain-language line per thing, or
 * none. `target` is null when the page itself is going away (reload, close).
 * A check answers for its own view only, and can tell a navigation that keeps
 * its work (another place while staged changes are kept per scope) from one
 * that loses it.
 */
export type LeaveCheck = (target: AppLocation | null) => string | null;

/** Asks the user whether to leave and lose `losses`; true to leave. */
export type ConfirmLeave = (losses: readonly string[]) => Promise<boolean>;

export class NavigationGuard {
  private readonly checks = new Set<LeaveCheck>();
  private confirm: ConfirmLeave | null = null;
  private reloadConfirmed = false;

  /** Register a check. Returns a function that removes it. */
  register(check: LeaveCheck): () => void {
    this.checks.add(check);
    return () => this.checks.delete(check);
  }

  /**
   * Set how the user is asked. Until one is set, leaving that would lose work
   * is refused, since there's no way to ask.
   */
  setConfirm(confirm: ConfirmLeave | null): void {
    this.confirm = confirm;
  }

  /** What leaving for `target` would lose, across every registered check. */
  losses(target: AppLocation | null): string[] {
    const found: string[] = [];
    for (const check of this.checks) {
      const loss = check(target);
      if (loss !== null) found.push(loss);
    }
    return found;
  }

  /** Ask whether to leave and lose `losses`: true when the user confirms, false when they decline or nothing can ask. */
  ask(losses: readonly string[]): Promise<boolean> {
    return this.confirm === null ? Promise.resolve(false) : this.confirm(losses);
  }

  /**
   * Ask before the app reloads the page itself, when that would lose work:
   * true to go ahead. A yes also stands in for the browser's prompt, which
   * would otherwise ask the same question again as the page unloads.
   */
  async confirmReload(): Promise<boolean> {
    const losses = this.losses(null);
    if (losses.length === 0) return true;
    if (!(await this.ask(losses))) return false;
    this.reloadConfirmed = true;
    return true;
  }

  /** The browser's prompt for reload and tab close, when something would be lost. */
  onBeforeUnload = (event: BeforeUnloadEvent): void => {
    if (this.reloadConfirmed || this.losses(null).length === 0) return;
    event.preventDefault();
  };
}
