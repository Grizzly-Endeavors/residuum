// The app icon's badge (the Badging API): the inbox unread total, on the
// installed app's icon. Browsers without the API show no badge, and the count
// is in the rail and the bottom bar either way.

/** The part of `navigator` the Badging API adds, where the browser has it. */
export interface BadgingNavigator {
  setAppBadge?: (count?: number) => Promise<void>;
  clearAppBadge?: () => Promise<void>;
}

/** Show `count` on the app icon, or clear the badge at zero. */
export function showAppBadge(count: number, nav: BadgingNavigator = navigator): void {
  const shown = count > 0 ? nav.setAppBadge?.(count) : nav.clearAppBadge?.();
  // A browser may refuse (the app isn't installed, or badges are off for it);
  // nothing in the app depends on the badge.
  shown?.catch(() => undefined);
}
