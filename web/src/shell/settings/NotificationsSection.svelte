<script lang="ts">
  import { onMount } from "svelte";
  import { userErrorMessage } from "../../lib/errors";
  import { hub } from "../../lib/hub.svelte";
  import type { PushDevice } from "../../lib/hub-types";
  import { push } from "../../lib/push.svelte";
  import { toast } from "../../lib/toast.svelte";
  import { Banner, Button, Disclosure, EmptyState, Skeleton, TextField } from "../../lib/ui";
  import PushDeliveryLine from "./PushDeliveryLine.svelte";
  import { configFieldError, type AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";
  import ThisDeviceGroup from "./ThisDeviceGroup.svelte";

  // Notifications: push on this device, every other device that
  // gets notifications with how delivery to it is going, and under More
  // options the contact push services are given. The devices act at once;
  // only the contact is staged and saved with Save changes.

  let { scope, section }: AllSectionProps = $props();

  let removing = $state<string | null>(null);

  const others = $derived(push.devices?.filter((d) => d.id !== push.deviceId) ?? []);

  const contact = $derived(scope.config.push_contact.trim());
  // A flag, not a block: the hub drops a contact it can't use and says so.
  const contactError = $derived(
    configFieldError(scope, "push_contact") ??
      (contact !== "" && !/^(mailto:|https:\/\/)/i.test(contact)
        ? "Residuum uses only a mailto: or https:// contact, so it would use the default instead."
        : undefined),
  );
  let moreOpen = $state(false);
  $effect(() => {
    if (contactError !== undefined) moreOpen = true;
  });

  async function remove(device: PushDevice): Promise<void> {
    removing = device.id;
    try {
      await push.remove(device.id);
      toast.success(`${device.label} no longer gets notifications.`);
    } catch (err) {
      toast.error(userErrorMessage(err, { action: `Couldn't remove ${device.label}.` }));
    } finally {
      removing = null;
    }
  }

  const created = (at: string): string =>
    new Date(at).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });

  onMount(() => {
    void push.load();
    // A device the hub pruned (its subscription expired, or a rotation's
    // resubscribe failed) reaches the user as a plain hub notice; there's no
    // structured frame naming the device, so any notice is cause to read the
    // list again rather than leave it stale until the section remounts.
    return hub.onFrame((msg) => {
      if (msg.type === "notice") void push.load();
    });
  });
</script>

<SettingsSection
  {scope}
  {section}
  title="Notifications"
  lede="Hear from your agents on your phone or computer, even while Residuum is closed. Each device chooses what it's told about."
>
  <ThisDeviceGroup />

  <SettingsGroup
    title="Other devices"
    lede="Browsers and installed apps that get notifications. Removing one stops them there."
  >
    {#if push.devices === null && push.loadError !== ""}
      <Banner tone="error">
        {push.loadError}
        {#snippet actions()}
          <Button size="sm" onclick={() => void push.load()}>Try again</Button>
        {/snippet}
      </Banner>
    {:else if push.devices === null}
      <Skeleton lines={2} label="Loading devices" />
    {:else if others.length === 0}
      <EmptyState>No other devices get notifications.</EmptyState>
    {:else}
      <ul class="push-devices" aria-label="Other devices">
        {#each others as device (device.id)}
          <li class="push-device">
            <div class="push-device-text">
              <span class="push-device-name">{device.label}</span>
              <span class="push-device-added">Added {created(device.created_at)}</span>
              <PushDeliveryLine {device} />
            </div>
            <Button
              variant="quiet"
              size="sm"
              loading={removing === device.id}
              aria-label={`Remove ${device.label}`}
              onclick={() => void remove(device)}
            >
              Remove
            </Button>
          </li>
        {/each}
      </ul>
    {/if}
  </SettingsGroup>

  <Disclosure summary="More options" bind:open={moreOpen}>
    <SettingsGroup
      title="Contact for push services"
      lede="Google, Apple and Mozilla deliver the notifications. Each one is signed with a contact they can reach about this Residuum's traffic."
    >
      <TextField
        label="Contact"
        bind:value={scope.config.push_contact}
        placeholder="mailto:you@example.com"
        autocomplete="off"
        spellcheck={false}
        code
        hint="A mailto: address or an https:// link. Left blank, they're given Residuum's project page."
        error={contactError}
      />
    </SettingsGroup>
  </Disclosure>
</SettingsSection>

<style>
  .push-devices {
    display: flex;
    flex-direction: column;
    list-style: none;
  }

  .push-device {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
    padding: var(--space-10) 0;
    border-top: 1px solid var(--color-line-soft);

    &:first-child {
      padding-top: 0;
      border-top: 0;
    }
  }

  .push-device-text {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .push-device-name {
    font-size: var(--font-size-ui);
    font-weight: var(--font-weight-medium);
    overflow-wrap: anywhere;
  }

  .push-device-added {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }
</style>
