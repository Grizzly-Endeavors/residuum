<script lang="ts">
  import type { PendingJoinInfo } from "../../lib/generated/PendingJoinInfo";
  import { approveJoin, denyJoin } from "../../lib/remote-access-api";
  import { toast } from "../../lib/toast.svelte";
  import { Button } from "../../lib/ui";
  import type { RemoteAct } from "./remote-access-act";

  // Other instances asking this one to approve them. What a request says about
  // itself (its slug and name) is whatever its sender chose, so it is drawn as
  // plain text and weighed against the six digits, which the person compares
  // with the ones shown on the other instance.

  interface Props {
    joins: readonly PendingJoinInfo[];
    busy: string | null;
    act: RemoteAct;
  }

  let { joins, busy, act }: Props = $props();

  function approve(join: PendingJoinInfo): Promise<void> {
    return act(`approve:${join.id}`, "Couldn't approve that instance.", async () => {
      await approveJoin(join.id);
      toast.success("Approved. That instance can now get certificates for your addresses.");
    });
  }

  function deny(join: PendingJoinInfo): Promise<void> {
    return act(`deny:${join.id}`, "Couldn't refuse that instance.", async () => {
      await denyJoin(join.id);
      toast.success("Refused.");
    });
  }
</script>

{#if joins.length > 0}
  <section class="pj" aria-label="Instances asking to join">
    <h4 class="pj-title">Instances asking to join</h4>
    <p class="pj-note">
      Approve only if the code matches the one shown on the instance that is asking. Approving lets
      it get certificates for your addresses.
    </p>
    <ul class="pj-list">
      {#each joins as join (join.id)}
        <li class="pj-item">
          <code class="pj-code" aria-label="Code {join.code}">{join.code}</code>
          <p class="pj-claim">
            Says it is <strong>{join.slug}</strong>, named <strong>{join.display_name}</strong>.
          </p>
          {#if join.in_relay_list === true}
            <p class="pj-hint">Residuum Cloud lists this instance.</p>
          {:else if join.in_relay_list === false}
            <p class="pj-hint" data-warn>
              Residuum Cloud does not list an instance with this name.
            </p>
          {/if}
          <div class="pj-actions">
            <Button
              variant="primary"
              loading={busy === `approve:${join.id}`}
              disabled={busy !== null && busy !== `approve:${join.id}`}
              onclick={() => void approve(join)}
            >
              Approve
            </Button>
            <Button
              variant="quiet"
              loading={busy === `deny:${join.id}`}
              disabled={busy !== null && busy !== `deny:${join.id}`}
              onclick={() => void deny(join)}
            >
              Deny
            </Button>
          </div>
        </li>
      {/each}
    </ul>
  </section>
{/if}

<style>
  .pj {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-12);
    border: 1px solid var(--color-vein-line);
    border-radius: var(--corner-md);
    background: var(--color-vein-faint);
  }

  .pj-title {
    font-size: var(--font-size-ui);
    font-weight: var(--font-weight-semibold);
  }

  .pj-note,
  .pj-claim,
  .pj-hint {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
    overflow-wrap: anywhere;
  }

  .pj-hint[data-warn] {
    color: var(--color-text);
  }

  .pj-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    list-style: none;
  }

  .pj-item {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    padding-top: var(--space-10);
    border-top: 1px solid var(--color-line-soft);
  }

  .pj-code {
    align-self: flex-start;
    padding: var(--space-6) var(--space-12);
    border-radius: var(--corner-sm);
    background: var(--color-input);
    font-family: var(--font-code);
    font-size: var(--font-size-title);
    letter-spacing: 0.3em;
  }

  .pj-claim strong {
    color: var(--color-text);
    font-weight: var(--font-weight-medium);
  }

  .pj-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
  }
</style>
