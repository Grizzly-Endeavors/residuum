<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { applyUpdate, fetchUpdateStatus, triggerUpdateCheck } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import { relativeTime } from "../../lib/time";
  import type { RollbackNoticeResponse, UpdateStatusResponse } from "../../lib/types";
  import { Badge, Banner, Button, Skeleton } from "../../lib/ui";
  import type { AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // Updates: the running version, checking for a newer one, and installing it.
  // These act at once through their own endpoints and have no part in the save
  // bar. Installing restarts Residuum, so the section polls the status until
  // it answers again and says whether the new version came up or rolled back.

  let { scope, section }: AllSectionProps = $props();

  const RESTART_POLL_INTERVAL_MS = 1500;
  const RESTART_TIMEOUT_MS = 90_000;
  const timeoutSeconds = RESTART_TIMEOUT_MS / 1000;

  let status = $state<UpdateStatusResponse | null>(null);
  let loading = $state(true);
  let loadError = $state("");
  let problem = $state("");
  let checking = $state(false);
  let applying = $state(false);
  let restarting = $state(false);
  let waitedSeconds = $state(0);
  let outcome = $state<"updated" | "rolled_back" | "timed_out" | null>(null);
  let rollback = $state<RollbackNoticeResponse | null>(null);

  let pollHandle: ReturnType<typeof setInterval> | null = null;
  let restartStartedAt = 0;
  let versionBeforeRestart = "";

  /** Take a status, showing a rollback notice whenever there is one: it may have happened before this section opened. */
  function take(next: UpdateStatusResponse): void {
    status = next;
    if (next.rollback_notice !== null) {
      rollback = next.rollback_notice;
      outcome = "rolled_back";
    }
  }

  async function load(): Promise<void> {
    loading = true;
    try {
      take(await fetchUpdateStatus());
      loadError = "";
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't read the update status." });
    } finally {
      loading = false;
    }
  }

  function stopPolling(): void {
    if (pollHandle === null) return;
    clearInterval(pollHandle);
    pollHandle = null;
  }

  async function pollDuringRestart(): Promise<void> {
    waitedSeconds = Math.floor((Date.now() - restartStartedAt) / 1000);
    try {
      const next = await fetchUpdateStatus();
      // The old process can still answer for a moment after the install; only a new version or a rollback is an outcome.
      if (next.rollback_notice !== null || next.current !== versionBeforeRestart) {
        take(next);
        restarting = false;
        if (next.rollback_notice === null) outcome = "updated";
        stopPolling();
        return;
      }
    } catch {
      // Residuum is still down mid-restart, so keep waiting.
    }
    if (Date.now() - restartStartedAt > RESTART_TIMEOUT_MS) {
      restarting = false;
      outcome = "timed_out";
      stopPolling();
    }
  }

  async function check(): Promise<void> {
    checking = true;
    problem = "";
    try {
      take(await triggerUpdateCheck());
    } catch (err) {
      problem = userErrorMessage(err, { action: "Couldn't check for updates." });
    } finally {
      checking = false;
    }
  }

  async function install(): Promise<void> {
    applying = true;
    problem = "";
    try {
      await applyUpdate();
      versionBeforeRestart = status?.current ?? "";
      restartStartedAt = Date.now();
      waitedSeconds = 0;
      outcome = null;
      rollback = null;
      restarting = true;
      stopPolling();
      pollHandle = setInterval(() => void pollDuringRestart(), RESTART_POLL_INTERVAL_MS);
    } catch (err) {
      problem = userErrorMessage(err, { action: "Couldn't install the update." });
    } finally {
      applying = false;
    }
  }

  onMount(() => {
    void load();
  });
  onDestroy(stopPolling);
</script>

{#snippet versionMark()}
  {#if status !== null && !restarting}
    {#if status.update_available}
      <Badge tone="accent" dot>Update available</Badge>
    {:else if status.latest !== null}
      <Badge tone="positive" dot>Up to date</Badge>
    {:else}
      <Badge dot>Not checked yet</Badge>
    {/if}
  {/if}
{/snippet}

<SettingsSection
  {scope}
  {section}
  title="Updates"
  lede="Check for a newer version of Residuum and install it. Installing restarts Residuum, so every agent stops for a moment."
>
  <SettingsGroup title="Version" status={versionMark}>
    {#if loading}
      <Skeleton lines={3} label="Loading update status" />
    {:else if status === null}
      <Banner tone="error">
        {loadError}
        {#snippet actions()}
          <Button size="sm" onclick={() => void load()}>Try again</Button>
        {/snippet}
      </Banner>
    {:else if restarting}
      <Banner busy>
        Restarting Residuum… ({waitedSeconds}s). If it isn't back within {timeoutSeconds}s, it has
        probably rolled back to the previous version.
      </Banner>
    {:else}
      {#if outcome === "rolled_back" && rollback !== null}
        <Banner
          tone="error"
          title="Update to {rollback.attempted_version} failed and was rolled back."
        >
          {rollback.reason}. Now running {status.current}.
        </Banner>
      {:else if outcome === "updated"}
        <Banner icon="check">Updated to {status.current}.</Banner>
      {:else if outcome === "timed_out"}
        <Banner tone="warn" title="Still waiting for Residuum to come back.">
          It's been over {timeoutSeconds}s with no response. Check <code>residuum logs</code> on the machine
          running Residuum, or restart it by hand.
        </Banner>
      {/if}
      {#if status.unverified_update !== null && outcome !== "rolled_back"}
        <Banner tone="warn" title="This update couldn't be verified.">
          {status.unverified_update.version} was installed, but the release had no checksum to check the
          download against.
        </Banner>
      {/if}
      <dl class="update-facts">
        <dt>Running</dt>
        <dd>{status.current}</dd>
        {#if status.latest !== null}
          <dt>Latest</dt>
          <dd data-new={status.update_available || undefined}>{status.latest}</dd>
        {/if}
        {#if status.last_checked !== null}
          <dt>Checked</dt>
          <dd data-plain>{relativeTime(status.last_checked)}</dd>
        {/if}
      </dl>
      {#if problem !== ""}
        <Banner tone="error">{problem}</Banner>
      {/if}
      <div class="update-actions">
        <Button loading={checking} disabled={applying} onclick={() => void check()}>
          Check for updates
        </Button>
        {#if status.update_available}
          <Button
            variant="primary"
            loading={applying}
            disabled={checking}
            onclick={() => void install()}
          >
            Update and restart
          </Button>
        {/if}
      </div>
    {/if}
  </SettingsGroup>
</SettingsSection>

<style>
  .update-facts {
    display: grid;
    grid-template-columns: max-content minmax(0, 1fr);
    gap: var(--space-6) var(--space-16);
    font-size: var(--font-size-sm);
  }

  .update-facts dt {
    color: var(--color-text-3);
  }

  .update-facts dd {
    font-family: var(--font-code);
    overflow-wrap: anywhere;

    &[data-new] {
      color: var(--color-vein-bright);
    }

    &[data-plain] {
      font-family: inherit;
    }
  }

  .update-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
  }
</style>
