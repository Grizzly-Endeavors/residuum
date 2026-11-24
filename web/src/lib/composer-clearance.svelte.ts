interface Reported {
  readonly key: symbol;
  px: number;
}

class ComposerClearance {
  px = $state(0);

  private reports: Reported[] = [];

  private settle(): void {
    this.px = this.reports.findLast((entry) => entry.px > 0)?.px ?? 0;
  }

  readonly track = (el: HTMLElement): (() => void) => {
    const mine: Reported = { key: Symbol("composer"), px: 0 };
    this.reports.push(mine);
    const report = (): void => {
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
