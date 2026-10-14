<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { hub } from "../lib/hub.svelte";
  import { overview } from "../lib/overview.svelte";
  import { router } from "../lib/router.svelte";
  import { APP_NAME, placeSubject, tabTitle } from "../lib/tab-title";

  // Titles the browser tab: the viewed agent or place, and while the tab is
  // hidden an unread count and whether the viewed agent is working or has
  // just finished. The hub's per-agent activity and the overview's inbox
  // count are the signals (the app icon's badge reads the same inbox count).
  // It draws nothing.

  let hidden = $state(document.visibilityState === "hidden");
  /** The viewed agent ended a turn while the tab was hidden. */
  let finished = $state(false);

  /** What is unread across the agents' chats and the inbox. */
  const unread = $derived(
    Object.values(hub.activity).reduce((sum, activity) => sum + activity.unread, 0) +
      overview.inboxUnread,
  );
  const viewed = $derived(router.viewedAgent);
  const working = $derived(viewed !== null && hub.activityOf(viewed).busy);

  function onvisibilitychange(): void {
    hidden = document.visibilityState === "hidden";
    // Coming back is seeing what happened.
    if (!hidden) finished = false;
  }

  // A turn ending under a hidden tab is the one thing the unread count can't
  // say: the open chat counts nothing as unread.
  let watched: string | null = null;
  let wasWorking = false;
  $effect(() => {
    const agent = viewed;
    const busy = working;
    untrack(() => {
      if (agent !== watched) {
        watched = agent;
        finished = false;
      } else if (busy) {
        finished = false;
      } else if (wasWorking && hidden) {
        finished = true;
      }
      wasWorking = busy;
    });
  });

  $effect(() => {
    document.title = tabTitle({
      subject: placeSubject(router.place, (name) => hub.shownName(name)),
      hidden,
      unread,
      working,
      finished,
    });
  });

  onMount(() => () => {
    document.title = APP_NAME;
  });
</script>

<svelte:document {onvisibilitychange} />
