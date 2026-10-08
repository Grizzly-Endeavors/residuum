<script lang="ts">
  import { onMount } from "svelte";
  import {
    CloudConnection,
    DEFAULT_RELAY_URL,
    connectTarget,
    phaseOf,
  } from "../../lib/cloud.svelte";
  import {
    Badge,
    Banner,
    Button,
    Disclosure,
    SecretField,
    Skeleton,
    TextField,
  } from "../../lib/ui";
  import DevicesGroup from "./DevicesGroup.svelte";
  import RemoteAccessGroup from "./RemoteAccessGroup.svelte";
  import { fieldError, type AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // Residuum Cloud: the tunnel's state and what can be done in each. Connect,
  // Cancel, Reconnect and Disconnect act at once (`CloudConnection`); the
  // relay address and removing the account are staged in the
  // hub's config with the rest of the install-wide settings.

  let { scope, section }: AllSectionProps = $props();

  const cloud = new CloudConnection();
  onMount(() => cloud.follow());

  const status = $derived(cloud.status);
  const phase = $derived(status === null ? null : phaseOf(status));
  const viaTunnel = $derived(status?.viewed_via_tunnel === true);
  /** What switching the tunnel off is called: disconnecting a connection, cancelling one that is starting. */
  const stopped = $derived(phase === "connected" ? "disconnected" : "cancelled");

  const baseline = $derived(scope.configFile.baseline);
  /** The saved account was removed in the form, and the removal isn't saved yet. */
  const removing = $derived(baseline.cloud_token !== "" && scope.config.cloud_token === "");
  const relayUnsaved = $derived(scope.config.cloud_relay_url !== baseline.cloud_relay_url);
  // The sign-in page sends the browser back to the gateway's port as it is now, so the saved one; the relay is the one being typed.
  const signIn = $derived(connectTarget(scope.config.cloud_relay_url, baseline.gateway_port));

  let tokenOpen = $state(false);
  let token = $state("");
  let moreOpen = $state(false);
  const relayError = $derived(fieldError(scope, { kind: "config", field: "cloud_relay_url" }));
  $effect(() => {
    if (relayError !== undefined) moreOpen = true;
  });

  function openSignIn(): void {
    if (signIn === null) return;
    window.open(signIn.url, "_blank", "noopener");
    cloud.expectChange();
  }

  async function connectWithToken(): Promise<void> {
    await cloud.connectWithToken(token);
    if (cloud.problem === "") {
      token = "";
      tokenOpen = false;
    }
  }

  function removeAccount(): void {
    scope.config.cloud_token = "";
    scope.config.cloud_enabled = false;
  }

  function keepAccount(): void {
    scope.config.cloud_token = baseline.cloud_token;
    scope.config.cloud_enabled = baseline.cloud_enabled;
  }
</script>

{#snippet mark()}
  {#if phase === "connected"}
    <Badge tone="positive" dot>Connected</Badge>
  {:else if phase === "connecting"}
    <Badge tone="accent" dot>Connecting…</Badge>
  {:else if phase === "disconnected"}
    <Badge dot>Disconnected</Badge>
  {:else if phase === "none"}
    <Badge dot>Not connected</Badge>
  {/if}
{/snippet}

<SettingsSection
  {scope}
  {section}
  title="Residuum Cloud"
  lede="Reach your agents from anywhere through a personal web address, with no port forwarding or VPN."
>
  <SettingsGroup title="Connection" status={mark}>
    {#if cloud.loadError !== ""}
      <Banner tone="error">
        {cloud.loadError}
        {#snippet actions()}
          <Button size="sm" onclick={() => void cloud.refresh()}>Try again</Button>
        {/snippet}
      </Banner>
    {/if}
    {#if status === null}
      {#if cloud.loadError === ""}
        <Skeleton lines={2} label="Loading the Cloud status" />
      {/if}
    {:else if phase === "connected"}
      <p class="cloud-line">
        {#if status.user_id !== null}Connected as <strong>{status.user_id}</strong>.{/if}
        Your agents can be reached through your Residuum Cloud address.
      </p>
    {:else if phase === "connecting"}
      <p class="cloud-line">
        Connecting to Residuum Cloud. This usually takes a few seconds, and Residuum keeps trying if
        it can't get through.
      </p>
    {:else if phase === "disconnected"}
      <p class="cloud-line">
        This install is signed in to Residuum Cloud, but the connection is switched off.
      </p>
    {:else}
      <p class="cloud-line">
        Sign in to Residuum Cloud to give this install a personal web address. The sign-in opens in
        a new tab; come back here when you're done.
      </p>
    {/if}

    {#if cloud.problem !== ""}
      <Banner tone="error">{cloud.problem}</Banner>
    {/if}

    {#if phase === "connected" || phase === "connecting"}
      {#if viaTunnel}
        <Banner>
          You're viewing Residuum through Residuum Cloud, so it can't be {stopped} from here. Do that
          on the machine running Residuum.
        </Banner>
      {:else}
        <div class="cloud-actions">
          <Button loading={cloud.busy === "disconnect"} onclick={() => void cloud.disconnect()}>
            {phase === "connected" ? "Disconnect" : "Cancel"}
          </Button>
        </div>
      {/if}
    {:else if phase === "disconnected"}
      {#if removing}
        <Banner>
          The account is removed when you save changes.
          {#snippet actions()}
            <Button size="sm" onclick={keepAccount}>Keep account</Button>
          {/snippet}
        </Banner>
      {:else}
        <div class="cloud-actions">
          <Button
            variant="primary"
            loading={cloud.busy === "reconnect"}
            onclick={() => void cloud.reconnect()}
          >
            Reconnect
          </Button>
          <Button variant="danger" onclick={removeAccount}>Remove account</Button>
        </div>
      {/if}
    {:else if phase === "none"}
      <div class="cloud-actions">
        <Button
          variant="primary"
          icon="external-link"
          disabled={signIn === null}
          onclick={openSignIn}
        >
          Connect to Residuum Cloud
        </Button>
      </div>
      {#if signIn === null}
        <Banner tone="warn">
          The relay address under More options isn't a ws:// or wss:// address, so there is nowhere
          to sign in.
        </Banner>
      {:else}
        <p class="cloud-hint">
          Opens {signIn.host} in a new tab.{relayUnsaved
            ? " Save your relay change first, so the sign-in uses it."
            : ""}
        </p>
      {/if}
      <Disclosure summary="Use a token instead" bind:open={tokenOpen}>
        <div class="cloud-token">
          <SecretField
            label="Tunnel token"
            source={{ kind: "none" }}
            bind:value={token}
            placeholder="rst_…"
            hint="A token from your Residuum Cloud account. It is stored as a secret, not in the config file."
          />
          <div class="cloud-actions">
            <Button
              variant="primary"
              loading={cloud.busy === "token"}
              disabled={token.trim() === ""}
              onclick={() => void connectWithToken()}
            >
              Connect with token
            </Button>
          </div>
        </div>
      </Disclosure>
    {/if}
  </SettingsGroup>

  <RemoteAccessGroup />

  {#if phase === "connected"}
    <DevicesGroup />
  {/if}

  <Disclosure summary="More options" bind:open={moreOpen}>
    <SettingsGroup
      title="Relay"
      lede="Where the tunnel connects. Leave it empty to use Residuum Cloud."
    >
      <TextField
        label="Relay URL"
        bind:value={scope.config.cloud_relay_url}
        placeholder={DEFAULT_RELAY_URL}
        autocomplete="off"
        spellcheck={false}
        code
        hint="Set this to run against a relay of your own, such as one on this machine while you develop. Signing in and connecting use it."
        error={relayError}
      />
    </SettingsGroup>
  </Disclosure>
</SettingsSection>

<style>
  .cloud-line {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
  }

  .cloud-line :global(strong) {
    color: var(--color-text);
    font-weight: var(--font-weight-medium);
  }

  .cloud-hint {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-ui);
  }

  .cloud-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
  }

  .cloud-token {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
  }
</style>
