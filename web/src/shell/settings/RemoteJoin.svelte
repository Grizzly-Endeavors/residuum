<script lang="ts">
  import type { RemoteAccessStatus } from "../../lib/generated/RemoteAccessStatus";
  import { isInstanceSlug } from "../../lib/instance-slug";
  import { startJoin } from "../../lib/remote-access-api";
  import { Banner, Button, Disclosure, TextField } from "../../lib/ui";
  import type { RemoteAct } from "./remote-access-act";

  // Joining another instance: this instance asks one that is already set up
  // for remote access to approve it, and shows six digits to compare with the
  // ones that instance shows. Until it is approved a new instance can't serve
  // anything over the tunnel, so the form is open and prominent then, and a
  // quiet disclosure otherwise.

  interface Props {
    status: RemoteAccessStatus;
    busy: string | null;
    act: RemoteAct;
  }

  let { status, busy, act }: Props = $props();

  const uid = $props.id();

  let instance = $state("");
  let open = $state(false);

  const needsJoin = $derived(status.state === "needs_join");
  const join = $derived(status.join);
  const waiting = $derived(join?.state === "waiting");
  /** The other instances this one has a certificate account for, offered as suggestions. */
  const suggestions = $derived([
    ...new Set(
      status.pins.filter((pin) => !pin.own && isInstanceSlug(pin.slug)).map((pin) => pin.slug),
    ),
  ]);
  const chosen = $derived(instance.trim());
  const problem = $derived(
    chosen !== "" && !isInstanceSlug(chosen)
      ? "An instance name is 1 to 24 lowercase letters, digits or hyphens."
      : "",
  );

  function submit(event: SubmitEvent): void {
    event.preventDefault();
    if (!isInstanceSlug(chosen) || busy !== null) return;
    void act("join", "Couldn't ask that instance to approve this one.", () => startJoin(chosen));
  }
</script>

{#snippet progress()}
  {#if join !== null}
    {#if join.state === "waiting"}
      <div class="rj-wait" role="status">
        <p class="rj-note">
          Asking <strong>{join.instance}</strong> to approve this instance. Check that it shows the same
          code, and approve it there.
        </p>
        {#if join.code}<code class="rj-code" aria-label="Code to compare">{join.code}</code>{/if}
        {#if join.detail}<p class="rj-note">{join.detail}</p>{/if}
      </div>
    {:else if join.state === "approved"}
      <Banner tone="info" icon="check" title="Approved">
        {join.detail ?? `${join.instance} approved this instance.`}
      </Banner>
    {:else}
      <Banner tone="error" title={join.state === "denied" ? "Refused" : "Couldn't join"}>
        {join.detail ?? `${join.instance} didn't approve this instance.`}
      </Banner>
    {/if}
  {/if}
{/snippet}

{#snippet form()}
  {#if !waiting}
    <form class="rj-form" onsubmit={submit}>
      <TextField
        label="Instance to join"
        hint="The name of an instance that already works over the tunnel."
        error={problem}
        bind:value={instance}
        list="{uid}-suggestions"
        autocomplete="off"
        autocapitalize="none"
        spellcheck={false}
        maxlength={24}
        code
      />
      <datalist id="{uid}-suggestions">
        {#each suggestions as slug (slug)}<option value={slug}></option>{/each}
      </datalist>
      <div class="rj-actions">
        <Button
          type="submit"
          variant="primary"
          loading={busy === "join"}
          disabled={!isInstanceSlug(chosen)}
        >
          Join
        </Button>
      </div>
    </form>
  {/if}
{/snippet}

{#if needsJoin}
  <section class="rj-prominent" aria-label="Join another instance">
    <h4 class="rj-title">Join another instance</h4>
    <p class="rj-note">
      Another instance of yours already serves your addresses. This one can't be reached over the
      tunnel until that instance approves it.
    </p>
    {@render progress()}
    {@render form()}
  </section>
{:else}
  <Disclosure summary="Join another instance" bind:open>
    <div class="rj-body">
      <p class="rj-note">
        Ask an instance that already works over the tunnel to approve this one, so both can serve
        your addresses.
      </p>
      {@render progress()}
      {@render form()}
    </div>
  </Disclosure>
{/if}

<style>
  .rj-prominent {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
    padding: var(--space-12);
    border: 1px solid var(--color-vein-line);
    border-radius: var(--corner-md);
    background: var(--color-vein-faint);
  }

  .rj-title {
    font-size: var(--font-size-ui);
    font-weight: var(--font-weight-semibold);
  }

  .rj-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
    padding-top: var(--space-8);
  }

  .rj-note {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
    overflow-wrap: anywhere;
  }

  .rj-wait {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  .rj-code {
    align-self: flex-start;
    padding: var(--space-8) var(--space-12);
    border-radius: var(--corner-sm);
    background: var(--color-input);
    font-family: var(--font-code);
    font-size: var(--font-size-title);
    letter-spacing: 0.3em;
    user-select: all;
  }

  .rj-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
  }

  .rj-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
  }
</style>
