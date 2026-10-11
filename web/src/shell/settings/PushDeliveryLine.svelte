<script lang="ts">
  import { Icon } from "../../lib/icons";
  import type { PushDevice } from "../../lib/hub-types";
  import { deliveryOf } from "../../lib/push-devices";
  import { relativeTime } from "../../lib/time";

  // How delivery to a device is going: when a notification last reached its
  // push service, or the failure that came after, in the push service's words
  // as the hub put them.

  let { device }: { device: PushDevice } = $props();

  const delivery = $derived(deliveryOf(device));
</script>

{#if delivery.kind === "failing"}
  <p class="push-delivery" data-failing>
    <Icon name="warning" size={14} />
    <span>Last notification failed {relativeTime(delivery.at)}. {delivery.message}</span>
  </p>
{:else if delivery.kind === "delivered"}
  <p class="push-delivery">Last notification sent {relativeTime(delivery.at)}.</p>
{:else}
  <p class="push-delivery">No notifications sent yet.</p>
{/if}

<style>
  .push-delivery {
    display: flex;
    align-items: baseline;
    gap: var(--space-6);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);

    &[data-failing] {
      color: var(--color-err-text);
    }

    :global(svg) {
      flex: none;
      align-self: center;
    }
  }
</style>
