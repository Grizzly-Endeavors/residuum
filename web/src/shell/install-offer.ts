import { installContext, watchInstallOffer } from "../lib/install";
import { notifications } from "../lib/notifications.svelte";
import { installHelp, installOffer } from "./app-actions.svelte";

/**
 * Start offering to install the app, in the palette and the help menu. This
 * runs before the app mounts: a browser fires its install prompt once, early,
 * and an offer started after mount can miss it.
 */
export function startInstallOffer(): () => void {
  return watchInstallOffer(installOffer, {
    context: installContext(),
    target: window,
    showIosHelp: () => {
      installHelp.open = true;
    },
    reportFailure: (message) => {
      notifications.surface("error", message);
    },
  });
}
