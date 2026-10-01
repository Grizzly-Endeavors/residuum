// What the shell's own controls ask of it: the rail's footer and "+", and the
// phone bar. The shell answers each, so the rail and the bar hold no overlays
// of their own.

export type FeedbackTab = "bug" | "feedback";

export interface ShellActions {
  /** Open Settings on the viewed agent's scope, or on All agents when no agent is viewed. */
  openSettings: () => void;
  openShortcuts: () => void;
  openNotifications: () => void;
  openFeedback: (tab: FeedbackTab) => void;
  /**
   * Open the Create agent dialog over the current place. Home's New agent, the
   * rail's "+" and the palette all open it through this.
   */
  createAgent: () => void;
}
