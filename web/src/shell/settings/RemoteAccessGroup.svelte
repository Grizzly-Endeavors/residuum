<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import type { RemoteAccessState } from "../../lib/generated/RemoteAccessState";
  import type { RemoteAccessStatus } from "../../lib/generated/RemoteAccessStatus";
  import { userErrorMessage } from "../../lib/errors";
  import {
    acknowledgeRecoveryCode,
    fetchRemoteAccess,
    requestEmailReset,
    resetPins,
    retryRemoteAccess,
  } from "../../lib/remote-access-api";
  import { remoteAccess } from "../../lib/remote-access.svelte";
  import { toast } from "../../lib/toast.svelte";
  import type { BadgeTone } from "../../lib/ui";
  import { Badge, Banner, Button, Dialog, TextField } from "../../lib/ui";
  import PendingJoins from "./PendingJoins.svelte";
  import PendingReset from "./PendingReset.svelte";
  import RemoteInstances from "./RemoteInstances.svelte";
  import RemoteJoin from "./RemoteJoin.svelte";
  import SettingsGroup from "./SettingsGroup.svelte";

  // How Residuum Cloud reaches this install over the secure tunnel: whether it
  // is ready, its addresses and certificate, the recovery code that is shown
  // until it is saved, any certificate account nobody here approved, joining
  // other instances and approving theirs, and removing accounts of instances
  // that no longer exist, and a reset by email that is waiting to take effect.
  // It polls while open because setup runs in the
  // background for a few minutes.
  // Everything acts at once through the hub's endpoints and has no part in the
  // save bar.

  const POLL_MS = 5000;

  const LABELS: Record<RemoteAccessState, { text: string; tone: BadgeTone }> = {
    disabled: { text: "Off", tone: "neutral" },
    connecting: { text: "Connecting", tone: "neutral" },
    enrolling: { text: "Setting up", tone: "accent" },
    needs_join: { text: "Needs another instance", tone: "accent" },
    waiting_for_dns: { text: "Waiting for DNS", tone: "accent" },
    ordering: { text: "Getting a certificate", tone: "accent" },
    ready: { text: "Ready", tone: "positive" },
    refused: { text: "Refused", tone: "danger" },
    error: { text: "Needs attention", tone: "danger" },
  };

  let status = $state.raw<RemoteAccessStatus | null>(null);
  let loadError = $state("");
  let busy = $state<string | null>(null);
  let resetOpen = $state(false);
  let resetCode = $state("");
  let emailedTo = $state<string | null>(null);
  let copied = $state(false);
  let timer: ReturnType<typeof setInterval> | undefined;
  let copiedTimer: ReturnType<typeof setTimeout> | undefined;

  const unknownPins = $derived(status?.pins.filter((pin) => !pin.known) ?? []);
  const label = $derived(status ? LABELS[status.state] : null);
  const canRetry = $derived(
    status !== null &&
      ["error", "needs_join", "waiting_for_dns", "connecting"].includes(status.state),
  );
  /** Whether Residuum Cloud isn't set up, so there is nothing to show. */
  const hidden = $derived(status?.state === "disabled");

  async function load(): Promise<void> {
    try {
      const next = await fetchRemoteAccess();
      status = next;
      loadError = "";
      remoteAccess.accept(next);
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't load the remote access status." });
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

  async function copyCode(code: string): Promise<void> {
    try {
      await navigator.clipboard.writeText(code);
    } catch {
      toast.error("Couldn't copy. Select the code and copy it by hand.");
      return;
    }
    copied = true;
    clearTimeout(copiedTimer);
    copiedTimer = setTimeout(() => {
      copied = false;
    }, 1800);
  }

  function saved(): Promise<void> {
    return act("saved", "Couldn't record that the recovery code is saved.", async () => {
      await acknowledgeRecoveryCode();
      toast.success("Residuum no longer keeps the recovery code.");
    });
  }

  function reset(): Promise<void> {
    return act("reset", "Couldn't reset the pinned accounts.", async () => {
      await resetPins(resetCode.trim());
      resetOpen = false;
      resetCode = "";
      toast.success("This instance's certificate account now owns your address.");
    });
  }

  function emailReset(): Promise<void> {
    return act("email-reset", "Couldn't send the reset email.", async () => {
      emailedTo = await requestEmailReset();
    });
  }

  function openReset(): void {
    emailedTo = null;
    resetOpen = true;
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

{#if !hidden}
  <SettingsGroup
    title="Remote access"
    lede="Through the secure tunnel, your browser talks to Residuum directly and Residuum Cloud only passes encrypted traffic along. Residuum gets its own certificate for your addresses."
  >
    {#if loadError !== ""}
      <Banner tone="error">
        {loadError}
        {#snippet actions()}
          <Button size="sm" onclick={() => void load()}>Try again</Button>
        {/snippet}
      </Banner>
    {/if}

    {#if status !== null && label !== null}
      <p class="ra-state">
        <Badge tone={label.tone} dot>{label.text}</Badge>
        {#if status.detail}<span class="ra-note">{status.detail}</span>{/if}
      </p>

      {#if unknownPins.length > 0}
        <Banner tone="warn" title="Unrecognized certificate account">
          {#each unknownPins as pin (pin.account_uri)}
            <span class="ra-pin"
              >The instance "{pin.slug}" can get certificates for your addresses, and this instance
              never approved it.</span
            >
          {/each}
          If you didn't add it yourself, someone else may be able to impersonate your Residuum.
        </Banner>
      {/if}

      <PendingReset reset={status.pending_reset} {busy} {act} />

      <PendingJoins joins={status.pending_joins} {busy} {act} />

      {#if status.state === "needs_join"}
        <RemoteJoin {status} {busy} {act} />
      {/if}

      {#if status.recovery_code}
        <section class="ra-recovery" aria-label="Recovery code">
          <p class="ra-note">
            Save this recovery code somewhere safe. It is the only way to take your address back if
            every instance's certificate account is lost, and Residuum can't show it again once you
            confirm.
          </p>
          <code class="ra-code">{status.recovery_code}</code>
          <div class="ra-actions">
            <Button
              icon={copied ? "check" : "copy"}
              onclick={() => status?.recovery_code && void copyCode(status.recovery_code)}
            >
              {copied ? "Copied" : "Copy code"}
            </Button>
            <Button variant="primary" loading={busy === "saved"} onclick={() => void saved()}>
              I've saved it
            </Button>
          </div>
        </section>
      {/if}

      {#if status.hosts}
        <dl class="ra-hosts">
          <dt>Residuum</dt>
          <dd><code>https://{status.hosts.ui}</code></dd>
          <dt>Workbench</dt>
          <dd><code>https://{status.hosts.workbench}</code></dd>
          <dt>This instance (A2A, Teams)</dt>
          <dd><code>https://{status.hosts.instance}</code></dd>
        </dl>
      {/if}

      {#if status.certificate}
        <p class="ra-note">
          Certificate valid until {new Date(status.certificate.not_after).toLocaleDateString()}. It
          renews itself from {new Date(status.certificate.renews_at).toLocaleDateString()}.
        </p>
      {/if}

      <RemoteInstances siblings={status.siblings} pins={status.pins} {busy} {act} />

      {#if status.state !== "needs_join" && status.user !== null}
        <RemoteJoin {status} {busy} {act} />
      {/if}

      <div class="ra-actions">
        {#if canRetry}
          <Button
            loading={busy === "retry"}
            onclick={() =>
              void act("retry", "Couldn't ask Residuum to try again.", retryRemoteAccess)}
          >
            Try again now
          </Button>
        {/if}
        <Button variant="quiet" onclick={openReset}>Use a recovery code</Button>
      </div>
    {:else if loadError === ""}
      <p class="ra-note">Loading…</p>
    {/if}
  </SettingsGroup>

  <Dialog
    bind:open={resetOpen}
    title="Take your address back"
    description="Enter the recovery code from when remote access was first set up. This instance's certificate account then becomes the only one allowed to get certificates for your addresses."
  >
    <TextField
      label="Recovery code"
      bind:value={resetCode}
      placeholder="20 letters and digits"
      autocomplete="off"
      spellcheck={false}
      code
    />
    <section class="ra-email" aria-label="Lost your recovery code?">
      {#if emailedTo !== null}
        <p class="ra-note" role="status">
          A reset link was sent to {emailedTo}. Open it and confirm. After a waiting period, during
          which your other instances can cancel it, this instance's certificate account becomes the
          only one allowed for your addresses, and the new recovery code shows here.
        </p>
      {:else}
        <p class="ra-note">
          Lost your recovery code? Residuum Cloud can email the address on your account a link
          instead.
        </p>
        <div class="ra-actions">
          <Button loading={busy === "email-reset"} onclick={() => void emailReset()}>
            Lost your recovery code? Email me a reset link
          </Button>
        </div>
      {/if}
    </section>
    {#snippet actions()}
      <Button onclick={() => (resetOpen = false)}>Cancel</Button>
      <Button
        variant="primary"
        disabled={resetCode.trim().length !== 20}
        loading={busy === "reset"}
        onclick={() => void reset()}
      >
        Reset
      </Button>
    {/snippet}
  </Dialog>
{/if}

<style>
  .ra-state {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
  }

  .ra-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
  }

  .ra-pin {
    display: block;
  }

  .ra-recovery {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-12);
    border: 1px solid var(--color-vein-line);
    border-radius: var(--corner-md);
    background: var(--color-vein-faint);
  }

  .ra-code {
    padding: var(--space-8) var(--space-10);
    border-radius: var(--corner-sm);
    background: var(--color-input);
    font-family: var(--font-code);
    font-size: var(--font-size-heading);
    letter-spacing: 0.12em;
    overflow-wrap: anywhere;
    user-select: all;
  }

  .ra-email {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    margin-top: var(--space-12);
    padding-top: var(--space-12);
    border-top: 1px solid var(--color-line-soft);
  }

  .ra-hosts {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: var(--space-4) var(--space-16);
    font-size: var(--font-size-sm);
  }

  .ra-hosts dt {
    color: var(--color-text-2);
  }

  .ra-hosts code {
    font-family: var(--font-code);
    overflow-wrap: anywhere;
  }

  .ra-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
  }
</style>
