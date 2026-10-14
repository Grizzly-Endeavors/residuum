// How the page words a hub `notice` whose `kind` says what it reports. The
// hub's own text stays behind the expandable detail. A notice without a kind
// is shown as the hub wrote it.

import type { HubNoticeKind } from "./generated/HubNoticeKind";

/** A hub notice's toast text, and the hub's own words when the page reworded it. */
export interface HubNoticeText {
  message: string;
  details?: string;
}

export function hubNoticeText(kind: HubNoticeKind | undefined, message: string): HubNoticeText {
  switch (kind) {
    case undefined:
      return { message };
    case "gateway_rebind_failed":
      return {
        message:
          "Residuum couldn't move to the new address in its settings and is still reachable at the old one.",
        details: message,
      };
    case "background_limit_after_restart":
      return {
        message:
          "The new limit on background sessions running at once takes effect when Residuum restarts.",
        details: message,
      };
  }
}
