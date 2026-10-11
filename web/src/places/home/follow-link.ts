import { router } from "../../lib/router.svelte";
import { locationAt, type AppLocation, type Place } from "../../lib/routes";

/**
 * Open a link's place in the app instead of loading it, unless the click asks
 * for a new tab or window, which the browser handles as for any link.
 */
export function followLink(event: MouseEvent, target: AppLocation | Place): void {
  if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) {
    return;
  }
  event.preventDefault();
  const { place, panel } = "place" in target ? target : locationAt(target);
  void router.openPlace(place, panel === null ? {} : { panel });
}
