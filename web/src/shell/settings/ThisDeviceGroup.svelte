<script lang="ts">
  import { userErrorMessage } from "../../lib/errors";
  import type { PushPreferences } from "../../lib/hub-types";
  import { push, type PushAvailability } from "../../lib/push.svelte";
  import { toast } from "../../lib/toast.svelte";
  import { Badge, Banner, Button, Skeleton, TextField, Toggle } from "../../lib/ui";
  import { installHelp } from "../app-actions.svelte";
  import PushDeliveryLine from "./PushDeliveryLine.svelte";
  import SettingsGroup from "./SettingsGroup.svelte";

  // This browser's notifications: why it can't get them, or turning them on
  // under a name, and once on, its name, what it is told about, a test send
  // and how delivery is going. Every control acts at once through the hub's
  // push routes; none of it goes through the save bar.

  /** Each kind of notification: its switch's label, and when it is sent. */
  const PREFERENCES: readonly (readonly [keyof PushPreferences, string, string])[] = [
    ["inbox_item", "New inbox items", "An agent leaves something in your inbox."],
    ["agent_failed", "An agent stops working", "An agent can't start, or stops unexpectedly."],
    [
      "outbound_unreachable",
      "A task can't reach another agent",
      "A task an agent sent elsewhere has been waiting 10 minutes.",
    ],
    [
      "reply_while_away",
      "Replies while you're away",
      "An agent answers you while no Residuum window is open.",
    ],
  ];

  /** Why this device can't get notifications, where nothing here can change that. */
  const UNAVAILABLE: Partial<Record<PushAvailability, string>> = {
    insecure:
      "Browsers allow notifications only over a secure connection. Open Residuum through Residuum Cloud, over HTTPS, or at localhost on the computer it runs on, to get them here.",
    unsupported:
      "This browser can't show notifications from Residuum. A current Chrome, Edge, Firefox or Safari can.",
    "no-worker":
      "Residuum's background worker isn't running on this page, so it can't get notifications. Reload the page to start it.",
  };

  const unavailable = $derived(UNAVAILABLE[push.availability]);
  const device = $derived(push.thisDevice);
  const blocked = $derived(push.permission === "denied");

  let newLabel = $state(push.defaultLabel);
  let problem = $state<string | null>(null);

  const savedName = $derived(device?.label ?? "");
  // The box holds a new name until it is saved; a rename from elsewhere replaces it.
  let name = $derived(savedName);
  let renaming = $state(false);
  const nameTrimmed = $derived(name.trim());

  /** What each switch was just set to, shown until the hub answers. */
  let requested = $state<Partial<Record<keyof PushPreferences, boolean>>>({});
  let testing = $state(false);

  async function turnOn(): Promise<void> {
    problem = null;
    problem = await push.enable(newLabel.trim() || push.defaultLabel);
  }

  async function turnOff(): Promise<void> {
    problem = null;
    problem = await push.disable();
  }

  async function rename(): Promise<void> {
    if (nameTrimmed === "" || renaming) return;
    renaming = true;
    try {
      await push.rename(nameTrimmed);
      toast.success(`Renamed this device to ${nameTrimmed}.`);
    } catch (err) {
      toast.error(userErrorMessage(err, { action: "Couldn't rename this device." }));
    } finally {
      renaming = false;
    }
  }

  async function setPreference(key: keyof PushPreferences, on: boolean): Promise<void> {
    requested[key] = on;
    try {
      await push.setPreference(key, on);
    } catch (err) {
      toast.error(
        userErrorMessage(err, { action: "Couldn't change what this device is told about." }),
      );
    } finally {
      delete requested[key];
    }
  }

  async function sendTest(): Promise<void> {
    testing = true;
    try {
      const result = await push.sendTest();
      if (result.delivered) toast.success("Sent a test notification. It should appear here soon.");
      else toast.error(`The test notification wasn't delivered. ${result.error ?? ""}`.trim());
    } catch (err) {
      toast.error(userErrorMessage(err, { action: "Couldn't send the test notification." }));
    } finally {
      testing = false;
    }
  }
</script>

{#snippet state()}
  {#if device !== null && !blocked}
    <Badge tone="positive" dot>On</Badge>
  {:else if blocked}
    <Badge tone="danger" dot>Blocked</Badge>
  {:else if push.availability === "available"}
    <Badge dot>Off</Badge>
  {/if}
{/snippet}

<SettingsGroup title="This device" status={state}>
  {#if unavailable !== undefined}
    <p class="push-note">{unavailable}</p>
  {:else if push.availability === "install-first"}
    <Banner title="Add Residuum to your Home Screen first.">
      On iPhone and iPad, notifications work only in the app on your Home Screen. Add it, open it
      from there, and turn notifications on.
      {#snippet actions()}
        <Button size="sm" onclick={() => (installHelp.open = true)}>Show me how</Button>
      {/snippet}
    </Banner>
  {:else if push.deviceId !== null && device === null}
    <Skeleton lines={2} label="Loading this device" />
  {:else if device === null}
    {#if push.removedElsewhere}
      <p class="push-note">
        Notifications here were turned off from another device, or the browser's push service
        stopped taking them. Turn them on again to keep getting them.
      </p>
    {/if}
    {#if blocked}
      <Banner tone="warn">
        Notifications are blocked for this site. Allow them in your browser's site settings, then
        turn them on here.
      </Banner>
    {:else}
      <p class="push-note">
        Your browser asks for permission first. The name tells this device apart from your others in
        the list of devices.
      </p>
      <TextField
        label="Name for this device"
        bind:value={newLabel}
        placeholder={push.defaultLabel}
        autocomplete="off"
      />
      <div class="push-actions">
        <Button variant="primary" loading={push.busy} onclick={() => void turnOn()}>
          Turn on notifications
        </Button>
      </div>
    {/if}
  {:else}
    {#if blocked || push.permission === "default"}
      <Banner tone="warn">
        Your browser isn't letting Residuum show notifications here, so none will appear. Allow them
        in the browser's site settings, or turn them off here.
      </Banner>
    {/if}
    <div class="push-name">
      <TextField
        label="Device name"
        bind:value={name}
        autocomplete="off"
        error={nameTrimmed === "" ? "A device needs a name." : undefined}
        onkeydown={(event) => {
          if (event.key === "Enter") void rename();
        }}
      />
      {#if nameTrimmed !== "" && nameTrimmed !== device.label}
        <Button size="sm" loading={renaming} onclick={() => void rename()}>Rename</Button>
      {/if}
    </div>
    <PushDeliveryLine {device} />
    <div class="push-actions">
      <Button loading={testing} onclick={() => void sendTest()}>Send a test notification</Button>
      <Button variant="quiet" loading={push.busy} onclick={() => void turnOff()}>
        Turn off on this device
      </Button>
    </div>
  {/if}
  {#if problem !== null}
    <Banner tone="error">{problem}</Banner>
  {/if}
</SettingsGroup>

{#if device !== null}
  <SettingsGroup
    title="What to notify about"
    lede="For this device only. Each device chooses for itself."
  >
    {#each PREFERENCES as [key, label, hint] (key)}
      <Toggle
        {label}
        {hint}
        loading={requested[key] !== undefined}
        bind:checked={
          () => requested[key] ?? device?.preferences[key] ?? false,
          (on) => void setPreference(key, on)
        }
      />
    {/each}
  </SettingsGroup>
{/if}

<style>
  .push-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
  }

  .push-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
  }

  .push-name {
    display: flex;
    align-items: flex-end;
    gap: var(--space-8);

    > :global(:first-child) {
      flex: 1;
      min-width: 0;
    }
  }
</style>
