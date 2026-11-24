import type { AppLocation } from "./routes";

export type LeaveCheck = (target: AppLocation | null) => string | null;

export type ConfirmLeave = (losses: readonly string[]) => Promise<boolean>;

export class NavigationGuard {
  private readonly checks = new Set<LeaveCheck>();
  private confirm: ConfirmLeave | null = null;
  private reloadConfirmed = false;

  register(check: LeaveCheck): () => void {
    this.checks.add(check);
    return () => this.checks.delete(check);
  }

  setConfirm(confirm: ConfirmLeave | null): void {
    this.confirm = confirm;
  }

  losses(target: AppLocation | null): string[] {
    const found: string[] = [];
    for (const check of this.checks) {
      const loss = check(target);
      if (loss !== null) found.push(loss);
    }
    return found;
  }

  ask(losses: readonly string[]): Promise<boolean> {
    return this.confirm === null ? Promise.resolve(false) : this.confirm(losses);
  }

  async confirmReload(): Promise<boolean> {
    const losses = this.losses(null);
    if (losses.length === 0) return true;
    if (!(await this.ask(losses))) return false;
    this.reloadConfirmed = true;
    return true;
  }

  onBeforeUnload = (event: BeforeUnloadEvent): void => {
    if (this.reloadConfirmed || this.losses(null).length === 0) return;
    event.preventDefault();
  };
}
