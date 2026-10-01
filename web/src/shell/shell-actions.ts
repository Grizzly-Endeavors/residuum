// What the shell's own controls and the action registry ask of it: the
// search row and the phone's Search tab, the rail's "+" and gear, the phone
// bar, and the help actions. The shell answers each, so the rail, the bar and
// the registry hold no overlays of their own.

export type FeedbackTab = "bug" | "feedback";

export interface ShellActions {
  /** Open the command palette. */
  openSearch: () => void;
  /** Open Settings on the viewed agent's scope, or on All agents when no agent is viewed. */
  openSettings: () => void;
  openShortcuts: () => void;
  openNotifications: () => void;
  openFeedback: (tab: FeedbackTab) => void;
  /** Start creating an agent. */
  createAgent: () => void;
  /** Ask for a note to add to `agent`'s inbox. */
  addInboxNote: (agent: string) => void;
}
