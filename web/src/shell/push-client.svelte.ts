// The page's side of Web Push, started once before the app mounts: this
// device's push state (`lib/push.svelte.ts`), its presence on the hub socket
// while the window is in front of the user (`lib/push-presence.ts`), and the
// windows a notification click brings forward, which the worker asks to show
// the notification's target.

import { hub } from "../lib/hub.svelte";
import { browserPresencePage, PresenceReporter } from "../lib/push-presence";
import { browserPush, push } from "../lib/push.svelte";
import { router } from "../lib/router.svelte";
import { parseUrl } from "../lib/routes";
import { isWorkerMessage } from "../sw/protocol";

/** Go where a clicked notification leads: an app path such as `/inbox?item=atlas:note-1`. */
export function openNotificationTarget(target: string): Promise<boolean> {
  const query = target.indexOf("?");
  const pathname = query < 0 ? target : target.slice(0, query);
  const search = query < 0 ? "" : target.slice(query);
  const { location } = parseUrl(pathname, search, { lastUsed: null });
  return router.openPlace(location.place, {
    panel: location.panel ?? undefined,
    settings: location.settings ?? undefined,
  });
}

/**
 * Follow what the worker tells this page: a notification was clicked, so go
 * where it leads, or `pushsubscriptionchange` rotated this device's
 * subscription, so the stored record of it must follow.
 */
function followWorkerMessages(): () => void {
  if (!("serviceWorker" in navigator)) return () => undefined;
  const container = navigator.serviceWorker;
  const onMessage = (event: MessageEvent): void => {
    if (!isWorkerMessage(event.data)) return;
    if (event.data.type === "open-target") void openNotificationTarget(event.data.target);
    else push.deviceRotated(event.data.endpoint);
  };
  container.addEventListener("message", onMessage);
  container.startMessages();
  return () => {
    container.removeEventListener("message", onMessage);
  };
}

export function startPushClient(): () => void {
  push.start(browserPush());
  const presence = new PresenceReporter(
    {
      connected: () => hub.transport.status === "connected",
      send: (message) => {
        hub.transport.send(message);
      },
      onConnect: (listener) =>
        hub.onFrame((msg) => {
          if (msg.type === "hub_boot") listener();
        }),
    },
    browserPresencePage(),
  );
  const stopPresence = presence.start();
  const stopFollowing = $effect.root(() => {
    $effect(() => {
      presence.setDevice(push.presenceDevice);
    });
  });
  const stopMessages = followWorkerMessages();
  return () => {
    stopFollowing();
    stopPresence();
    stopMessages();
  };
}
