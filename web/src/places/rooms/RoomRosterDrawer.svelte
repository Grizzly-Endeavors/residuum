<script lang="ts">
  import { untrack } from "svelte";
  import type {
    RoomDetail,
    RoomRosterCandidate,
    RoomSeat,
    RoomSeatKind,
  } from "../../lib/hub-types";
  import { roomsStore } from "../../lib/rooms.svelte";
  import { Badge, Banner, Button, Dialog, StatusDot } from "../../lib/ui";

  interface Props {
    open: boolean;
    room: RoomDetail;
    candidates: readonly RoomRosterCandidate[];
  }

  let { open = $bindable(false), room, candidates }: Props = $props();

  let busySeatId = $state<string | null>(null);
  let actionError = $state<string | null>(null);

  $effect(() => {
    if (!open) return;
    untrack(() => {
      busySeatId = null;
      actionError = null;
      void roomsStore.refreshCandidates().then(() => {
        if (open && roomsStore.error !== null) {
          actionError ??= roomsStore.error;
          roomsStore.error = null;
        }
      });
    });
  });

  const archived = $derived(room.status === "archived");
  const lastSeat = $derived(room.roster.length <= 1);
  const rosterIds = $derived(new Set(room.roster.map((seat) => seat.seat_id)));
  const availableCandidates = $derived(
    candidates.filter((candidate) => !rosterIds.has(candidate.seat_id)),
  );

  function kindLabel(kind: RoomSeatKind, instance: string | null): string {
    switch (kind) {
      case "external_a2a":
        return "EXT A2A";
      case "sibling_a2a":
        return instance ?? "sibling";
      case "local":
        return "local";
    }
  }

  function kindTone(kind: RoomSeatKind): "danger" | "accent" | "neutral" {
    switch (kind) {
      case "external_a2a":
        return "danger";
      case "sibling_a2a":
        return "accent";
      case "local":
        return "neutral";
    }
  }

  async function addSeat(candidate: RoomRosterCandidate): Promise<void> {
    if (archived || !candidate.online || busySeatId !== null) return;
    busySeatId = candidate.seat_id;
    actionError = null;
    const updated = await roomsStore.patchRoom(room.id, { add_seats: [candidate.seat_id] });
    busySeatId = null;
    if (updated === null) {
      actionError = roomsStore.error ?? `Couldn't add ${candidate.display_name}.`;
      roomsStore.error = null;
    }
  }

  async function removeSeat(seat: RoomSeat): Promise<void> {
    if (archived || lastSeat || busySeatId !== null) return;
    busySeatId = seat.seat_id;
    actionError = null;
    const updated = await roomsStore.patchRoom(room.id, { remove_seats: [seat.seat_id] });
    busySeatId = null;
    if (updated === null) {
      actionError = roomsStore.error ?? `Couldn't remove ${seat.display_name}.`;
      roomsStore.error = null;
    }
  }
</script>

<Dialog
  bind:open
  title="Room roster"
  description="Manage active participants in {room.name}."
  size="md"
  fullscreenOnPhone
  onclose={() => {
    open = false;
  }}
>
  <div class="roster-drawer">
    {#if actionError !== null}
      <Banner tone="error" ondismiss={() => (actionError = null)}>{actionError}</Banner>
    {/if}

    {#if archived}
      <Banner tone="info">This room is archived. Reopen it to add or remove participants.</Banner>
    {/if}

    <section class="roster-section" aria-label="Active roster">
      <h3 class="roster-heading">Active roster ({room.roster.length})</h3>
      {#if lastSeat}
        <p class="roster-note">At least 1 active participant is required.</p>
      {/if}
      <ul class="roster-list">
        {#each room.roster as seat (seat.seat_id)}
          {@const used = room.window.per_seat_used[seat.seat_id] ?? 0}
          <li class="roster-item">
            <div class="roster-info">
              <div class="roster-title-row">
                <span class="roster-name">{seat.display_name}</span>
                <Badge tone={kindTone(seat.kind)}>{kindLabel(seat.kind, seat.instance)}</Badge>
                {#if used > 0}
                  <span class="roster-turns">{used}/3 turns</span>
                {/if}
              </div>
              {#if seat.role !== null && seat.role.trim() !== ""}
                <p class="roster-role">{seat.role}</p>
              {/if}
            </div>
            <Button
              size="sm"
              variant="quiet"
              aria-label="Remove {seat.display_name}"
              title={lastSeat ? "At least 1 active participant is required" : undefined}
              disabled={archived ||
                lastSeat ||
                (busySeatId !== null && busySeatId !== seat.seat_id)}
              loading={busySeatId === seat.seat_id}
              onclick={() => void removeSeat(seat)}
            >
              Remove
            </Button>
          </li>
        {/each}
      </ul>
    </section>

    <section class="roster-section" aria-label="Add participants">
      <h3 class="roster-heading">Add participants</h3>
      {#if availableCandidates.length === 0}
        <p class="roster-empty">All eligible agents are already on this roster.</p>
      {:else}
        <ul class="roster-list">
          {#each availableCandidates as candidate (candidate.seat_id)}
            <li class="roster-item">
              <StatusDot state={candidate.online ? "running" : "stopped"} />
              <div class="roster-info">
                <div class="roster-title-row">
                  <span class="roster-name">{candidate.display_name}</span>
                  <Badge tone={kindTone(candidate.kind)}>
                    {kindLabel(candidate.kind, candidate.instance)}
                  </Badge>
                  {#if !candidate.online}
                    <Badge tone="neutral">offline</Badge>
                  {/if}
                </div>
                {#if candidate.role !== null && candidate.role.trim() !== ""}
                  <p class="roster-role">{candidate.role}</p>
                {/if}
              </div>
              <Button
                size="sm"
                variant="secondary"
                icon="plus"
                aria-label="Add {candidate.display_name}"
                title={!candidate.online ? "Agent is currently offline" : undefined}
                disabled={archived ||
                  !candidate.online ||
                  (busySeatId !== null && busySeatId !== candidate.seat_id)}
                loading={busySeatId === candidate.seat_id}
                onclick={() => void addSeat(candidate)}
              >
                Add
              </Button>
            </li>
          {/each}
        </ul>
      {/if}
    </section>
  </div>

  {#snippet actions()}
    <Button variant="secondary" onclick={() => (open = false)}>Done</Button>
  {/snippet}
</Dialog>

<style>
  .roster-drawer {
    display: flex;
    flex-direction: column;
    gap: var(--space-18);
  }

  .roster-section {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  .roster-heading {
    color: var(--color-text);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
    --leading: var(--line-height-tight);
  }

  .roster-note,
  .roster-empty {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    --leading: var(--line-height-ui);
  }

  .roster-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    list-style: none;
  }

  .roster-item {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    padding: var(--space-8) var(--space-10);
    border: 1px solid var(--color-line-soft);
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
  }

  .roster-info {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .roster-title-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-6);
  }

  .roster-name {
    color: var(--color-text);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
  }

  .roster-turns {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
  }

  .roster-role {
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    --leading: var(--line-height-ui);
  }
</style>
