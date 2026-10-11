// How much of the screen's foot a composer covers, for what floats above it.
//
// A composer reports its own box while it is on screen. The toasts read the
// distance from the foot of the viewport to the top of the composer showing,
// so they sit above it whatever the breakpoint, the bottom bar or the
// composer's height at the moment. With two composers up (the chat's and a
// session panel's), the one mounted last is the one in front.

interface Reported {
  readonly key: symbol;
  px: number;
}

class ComposerClearance {
  /** From the foot of the viewport to the top of the composer in front; 0 with none. */
  px = $state(0);

  /** In the order they mounted. Plain data, so reporting from an effect reads no state. */
  private reports: Reported[] = [];

  private settle(): void {
    this.px = this.reports.findLast((entry) => entry.px > 0)?.px ?? 0;
  }

  /** Report `el` for as long as it is mounted: `{@attach composerClearance.track}`. */
  readonly track = (el: HTMLElement): (() => void) => {
    const mine: Reported = { key: Symbol("composer"), px: 0 };
    this.reports.push(mine);
    const report = (): void => {
      // Not laid out (display: none), or under a modal layer that made the page inert: it covers nothing the user can reach.
      mine.px =
        el.getClientRects().length === 0 || el.closest("[inert]") !== null
          ? 0
          : Math.max(
              0,
              Math.round(document.documentElement.clientHeight - el.getBoundingClientRect().top),
            );
      this.settle();
    };
    const resizes = new ResizeObserver(report);
    resizes.observe(el);
    // A modal layer marks what is under it `inert`, on an ancestor of the composer.
    const inertness = new MutationObserver(report);
    inertness.observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["inert"],
      subtree: true,
    });
    window.addEventListener("resize", report);
    report();
    return () => {
      resizes.disconnect();
      inertness.disconnect();
      window.removeEventListener("resize", report);
      this.reports = this.reports.filter((entry) => entry !== mine);
      this.settle();
    };
  };
}

export const composerClearance = new ComposerClearance();
