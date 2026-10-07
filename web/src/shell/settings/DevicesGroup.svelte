<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import type { DeviceListResponse } from "../../lib/generated/DeviceListResponse";
  import type { PairLinkResponse } from "../../lib/generated/PairLinkResponse";
  import { userErrorMessage } from "../../lib/errors";
  import {
    approvePairing,
    createPairLink,
    denyPairing,
    fetchDevices,
    regenerateRecoveryCodes,
    revokeDevice,
  } from "../../lib/pairing-api";
  import { relativeTime } from "../../lib/time";
  import { toast } from "../../lib/toast.svelte";
  import { Badge, Banner, Button, Dialog, IconButton, Skeleton } from "../../lib/ui";
  import KeyList from "./KeyList.svelte";
  import SettingsGroup from "./SettingsGroup.svelte";

  // Who can reach this install through Residuum Cloud: the paired browsers, the
  // browsers waiting for an answer, and the recovery codes. The first browser is
  // paired from the machine Residuum runs on, with a link made here. Everything
  // acts at once through the hub's own endpoints and has no part in the save bar.
  // Waiting requests are polled for while the group is open, so an approval
  // can happen as soon as a browser asks.

  const POLL_MS = 4000;

  let listing = $state.raw<DeviceListResponse | null>(null);
  let loadError = $state("");
  let busy = $state<string | null>(null);

  /** The link and codes a dialog shows once. */
  let shown = $state<{
    link: PairLinkResponse | null;
    codes: readonly string[] | null;
  } | null>(null);
  let dialogOpen = $state(false);
  let copied = $state<"link" | "codes" | null>(null);
  let timer: ReturnType<typeof setInterval> | undefined;
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  const remote = $derived(listing?.remote === true);
  const qrSource = $derived(
    shown?.link ? `data:image/svg+xml;utf8,${encodeURIComponent(shown.link.qr_svg)}` : null,
  );

  async function load(): Promise<void> {
    try {
      listing = await fetchDevices();
      loadError = "";
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't load the paired browsers." });
    }
  }

  async function act(name: string, failure: string, step: () => Promise<void>): Promise<void> {
    busy = name;
    try {
      await step();
      await load();
    } catch (err) {
      toast.error(userErrorMessage(err, { action: failure }));
    } finally {
      busy = null;
    }
  }

  function enable(): Promise<void> {
    return act("link", "Couldn't make a pairing link.", async () => {
      const link = await createPairLink();
      shown = { link, codes: link.recovery_codes };
      dialogOpen = true;
    });
  }

  function newCodes(): Promise<void> {
    return act("codes", "Couldn't make new recovery codes.", async () => {
      const made = await regenerateRecoveryCodes();
      shown = { link: null, codes: made.recovery_codes };
      dialogOpen = true;
    });
  }

  async function copy(what: "link" | "codes", text: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      toast.error("Couldn't copy. Select the text and copy it by hand.");
      return;
    }
    copied = what;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => {
      copied = null;
    }, 1800);
  }

  function closeDialog(): void {
    shown = null;
    copied = null;
  }

  onMount(() => {
    void load();
    timer = setInterval(() => void load(), POLL_MS);
  });
  onDestroy(() => {
    clearInterval(timer);
    clearTimeout(copiedTimer);
  });
</script>

{#snippet deviceMark(device: DeviceListResponse["devices"][number])}
  {#if device.current}<Badge tone="accent">This browser</Badge>{/if}
{/snippet}

{#snippet revokeAction(device: DeviceListResponse["devices"][number])}
  <IconButton
    icon="trash"
    label="Revoke {device.name}"
    loading={busy === `revoke:${device.id}`}
    onclick={() =>
      void act(`revoke:${device.id}`, `Couldn't revoke ${device.name}.`, async () => {
        await revokeDevice(device.id);
        toast.success(`Revoked ${device.name}. It can no longer reach Residuum.`);
      })}
  />
{/snippet}

<SettingsGroup
  title="Paired browsers"
  lede="Through Residuum Cloud, Residuum answers only browsers you have paired. Opening Residuum on this machine needs no pairing."
>
  {#if loadError !== ""}
    <Banner tone="error">
      {loadError}
      {#snippet actions()}
        <Button size="sm" onclick={() => void load()}>Try again</Button>
      {/snippet}
    </Banner>
  {/if}

  {#if listing === null}
    {#if loadError === ""}<Skeleton lines={2} label="Loading the paired browsers" />{/if}
  {:else}
    {#if listing.pending.length > 0}
      <section class="dev-pending" aria-label="Browsers asking to pair">
        {#each listing.pending as request (request.id)}
          <div class="dev-request">
            <div class="dev-request-text">
              <span class="dev-code">{request.code}</span>
              <span class="dev-name">{request.device_name}</span>
              <span class="dev-note"
                >Asked to pair. Approve only if the code matches the one on its screen.</span
              >
            </div>
            <div class="dev-request-actions">
              <Button
                size="sm"
                variant="primary"
                loading={busy === `approve:${request.id}`}
                onclick={() =>
                  void act(`approve:${request.id}`, "Couldn't approve that browser.", () =>
                    approvePairing(request.id),
                  )}
              >
                Approve
              </Button>
              <Button
                size="sm"
                loading={busy === `deny:${request.id}`}
                onclick={() =>
                  void act(`deny:${request.id}`, "Couldn't refuse that browser.", () =>
                    denyPairing(request.id),
                  )}
              >
                Refuse
              </Button>
            </div>
          </div>
        {/each}
      </section>
    {/if}

    {#if listing.devices.length === 0}
      <p class="dev-note">
        No browsers are paired. Make a pairing link, then open it in the browser you want to use
        through Residuum Cloud.
      </p>
    {:else}
      <KeyList
        label="Paired browsers"
        items={listing.devices.map((device) => ({ ...device, name: device.name }))}
        mark={deviceMark}
        note={(device) =>
          `Paired ${relativeTime(device.created_at)}, last used ${relativeTime(device.last_seen)}`}
        action={revokeAction}
      />
    {/if}

    {#if !remote}
      <div class="dev-actions">
        <Button
          variant={listing.devices.length === 0 ? "primary" : "secondary"}
          icon="plus"
          loading={busy === "link"}
          onclick={() => void enable()}
        >
          {listing.devices.length === 0 ? "Enable remote access" : "Pair another browser"}
        </Button>
      </div>
    {/if}

    <p class="dev-note">
      {#if listing.recovery_codes_remaining === 0}
        No recovery codes are left.
      {:else}
        {listing.recovery_codes_remaining} unused recovery
        {listing.recovery_codes_remaining === 1 ? "code" : "codes"}. A code pairs one browser if you
        lose access to every paired one.
      {/if}
      <Button size="sm" variant="quiet" loading={busy === "codes"} onclick={() => void newCodes()}>
        Make new recovery codes
      </Button>
    </p>
  {/if}
</SettingsGroup>

<Dialog
  bind:open={dialogOpen}
  title={shown?.link ? "Pair a browser" : "Recovery codes"}
  description={shown?.link
    ? "Open the link in the browser you want to pair, or scan the code with a phone."
    : "Save these now. They are shown only once, and the previous codes no longer work."}
  onclose={closeDialog}
>
  {#if shown?.link}
    <div class="dev-share">
      {#if qrSource !== null}
        <img class="dev-qr" src={qrSource} alt="QR code for the pairing link" />
      {/if}
      <code class="dev-link">{shown.link.link}</code>
      <p class="dev-note">
        The link works once and expires in {Math.round(shown.link.expires_in_secs / 60)} minutes. Anyone
        who opens it first can pair, so open it yourself.
      </p>
    </div>
  {/if}
  {#if shown?.codes}
    <div class="dev-share">
      {#if shown.link}
        <p class="dev-note">
          Recovery codes. Each pairs one browser if you lose access to every paired one. Save them
          now; they are shown only once.
        </p>
      {/if}
      <ul class="dev-codes" aria-label="Recovery codes">
        {#each shown.codes as code (code)}
          <li><code>{code}</code></li>
        {/each}
      </ul>
    </div>
  {/if}
  {#snippet actions()}
    {#if shown?.link}
      <Button
        icon={copied === "link" ? "check" : "copy"}
        onclick={() => shown?.link && void copy("link", shown.link.link)}
      >
        {copied === "link" ? "Copied" : "Copy link"}
      </Button>
    {/if}
    {#if shown?.codes}
      <Button
        icon={copied === "codes" ? "check" : "copy"}
        onclick={() => shown?.codes && void copy("codes", shown.codes.join("\n"))}
      >
        {copied === "codes" ? "Copied" : "Copy codes"}
      </Button>
    {/if}
    <Button
      variant="primary"
      onclick={() => {
        dialogOpen = false;
        closeDialog();
      }}
    >
      Done
    </Button>
  {/snippet}
</Dialog>

<style>
  .dev-pending {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  .dev-request {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
    padding: var(--space-12);
    border: 1px solid var(--color-vein-line);
    border-radius: var(--corner-md);
    background: var(--color-vein-faint);
  }

  .dev-request-text {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .dev-code {
    font-family: var(--font-code);
    font-size: var(--font-size-heading);
    font-weight: var(--font-weight-semibold);
    letter-spacing: 0.25em;
  }

  .dev-name {
    overflow-wrap: anywhere;
  }

  .dev-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
  }

  .dev-request-actions,
  .dev-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
  }

  .dev-share {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
  }

  .dev-qr {
    align-self: center;
    width: 220px;
    height: 220px;
    border-radius: var(--corner-sm);
    /* A QR code needs a light ground in either theme to scan. */
    background: var(--color-on-accent);
  }

  .dev-link {
    padding: var(--space-8) var(--space-10);
    border-radius: var(--corner-sm);
    background: var(--color-input);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    overflow-wrap: anywhere;
    user-select: all;
  }

  .dev-codes {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(180px, 1fr));
    gap: var(--space-6) var(--space-16);
    list-style: none;
    font-family: var(--font-code);
    font-size: var(--font-size-sm);
  }
</style>
