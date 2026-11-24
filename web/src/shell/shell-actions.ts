export type FeedbackTab = "bug" | "feedback";

export interface ShellActions {
  openSearch: () => void;
  openSettings: () => void;
  openShortcuts: () => void;
  openNotifications: () => void;
  openFeedback: (tab: FeedbackTab) => void;
  createAgent: () => void;
}
