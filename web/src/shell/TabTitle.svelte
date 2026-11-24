<script lang="ts">
  import { onMount, untrack } from "svelte";
  import { hub } from "../lib/hub.svelte";
  import { overview } from "../lib/overview.svelte";
  import { router } from "../lib/router.svelte";
  import { APP_NAME, placeSubject, tabTitle } from "../lib/tab-title";

  let hidden = $state(document.visibilityState === "hidden");
  let finished = $state(false);

  const unread = $derived(
    Object.values(hub.activity).reduce((sum, activity) => sum + activity.unread, 0) +
      overview.inboxUnread,
  );
  const viewed = $derived(router.viewedAgent);
  const working = $derived(viewed !== null && hub.activityOf(viewed).busy);

  function onvisibilitychange(): void {
    hidden = document.visibilityState === "hidden";
    if (!hidden) finished = false;
  }

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
