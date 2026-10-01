// Which agent's places the rail shows. One agent is open at a time, and
// opening another closes it. The rail on wider screens and the one in the
// phone drawer read the same accordion.

export class RailAccordion {
  /** The agent whose places are listed, or null with every agent closed. */
  open = $state<string | null>(null);
  private viewed: string | null = null;

  /** A press on an agent's row: open it, or close it when it is the one open. Never navigates. */
  toggle(name: string): void {
    this.open = this.open === name ? null : name;
  }

  /**
   * The viewed agent changed. Arriving on an agent opens it, so the place
   * the user is on is in view: on load, and after reaching an agent from
   * anywhere else. Staying on the same agent leaves the user's choice alone.
   */
  follow(viewed: string | null): void {
    if (viewed !== null && viewed !== this.viewed) this.open = viewed;
    this.viewed = viewed;
  }
}
