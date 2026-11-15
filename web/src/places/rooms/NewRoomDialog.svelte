<script lang="ts">
  import { untrack } from "svelte";
  import type { RoomRosterCandidate } from "../../lib/hub-types";
  import { roomsStore } from "../../lib/rooms.svelte";
  import { Badge, Banner, Button, Dialog, StatusDot, TextField, Toggle } from "../../lib/ui";

  interface Props {
    open: boolean;
    candidates: readonly RoomRosterCandidate[];
    oncreated?: (roomId: string) => void;
  }

  let { open = $bindable(false), candidates, oncreated }: Props = $props();

  const uid = $props.id();

  let name = $state("");
  let topic = $state("");
  let selectedSeatIds = $state<string[]>([]);
  let pinned = $state(false);
  let formError = $state<string | null>(null);
  let submitting = $state(false);

  $effect(() => {
    if (!open) return;
    untrack(() => {
      name = "";
      topic = "";
      selectedSeatIds = [];
      pinned = false;
      formError = null;
      submitting = false;
      void roomsStore.refreshCandidates().then(() => {
        if (open && roomsStore.error !== null) {
          formError ??= roomsStore.error;
          roomsStore.error = null;
        }
      });
    });
  });

  function toggleSeat(seatId: string): void {
    if (selectedSeatIds.includes(seatId)) {
      selectedSeatIds = selectedSeatIds.filter((id) => id !== seatId);
    } else {
      selectedSeatIds = [...selectedSeatIds, seatId];
    }
    if (formError !== null) formError = null;
  }

  function leafSlugOf(seatId: string): string {
    const stripped = seatId.replace(/^(?:agent:|a2a:)/, "");
    const slash = stripped.lastIndexOf("/");
    return slash >= 0 ? stripped.slice(slash + 1) : stripped;
  }

  function seatKindLabel(candidate: RoomRosterCandidate): string {
    switch (candidate.kind) {
      case "external_a2a":
        return "EXT A2A";
      case "sibling_a2a":
        return candidate.instance ?? "sibling";
      case "local":
        return "local";
    }
  }

  function seatKindTone(candidate: RoomRosterCandidate): "danger" | "accent" | "neutral" {
    switch (candidate.kind) {
      case "external_a2a":
        return "danger";
      case "sibling_a2a":
        return "accent";
      case "local":
        return "neutral";
    }
  }

  async function submit(): Promise<void> {
    const trimmed = name.trim();
    if (trimmed === "") {
      formError = "Room name is required.";
      return;
    }
    if (trimmed.length > 120) {
      formError = "Room name must be 120 characters or fewer.";
      return;
    }
    if (selectedSeatIds.length === 0) {
      formError = "Select at least 1 participant.";
      return;
    }
    submitting = true;
    formError = null;
    const response = await roomsStore.createRoom({
      name: trimmed,
      topic: topic.trim() === "" ? null : topic.trim(),
      seat_ids: selectedSeatIds,
      pinned,
    });
    submitting = false;
    if (response === null) {
      formError = roomsStore.error ?? "Couldn't create the meeting room.";
      roomsStore.error = null;
      return;
    }
    open = false;
    oncreated?.(response.room.id);
  }

  function onsubmit(event: SubmitEvent): void {
    event.preventDefault();
    void submit();
  }

  function close(): void {
    if (submitting) return;
    open = false;
  }
</script>

<Dialog
  bind:open
  title="New room"
  description="Create a multi-agent meeting room across local and connected agents."
  size="md"
  fullscreenOnPhone
  onclose={close}
>
  <form id="{uid}-form" class="new-room-form" {onsubmit}>
    {#if formError !== null}
      <Banner tone="error" ondismiss={() => (formError = null)}>{formError}</Banner>
    {/if}

    <TextField
      label="Room name"
      bind:value={name}
      maxlength={120}
      placeholder="e.g., Architecture Sync"
      data-autofocus
      required
    />

    <TextField
      label="Topic (optional)"
      bind:value={topic}
      multiline
      rows={2}
      placeholder="Agenda or focus for this room"
    />

    <fieldset class="new-room-roster">
      <legend class="new-room-legend">Participants</legend>
      <p class="new-room-hint">Select at least 1 active or online agent.</p>
      {#if candidates.length === 0}
        <p class="new-room-empty">No running local agents or online A2A agents available.</p>
      {:else}
        <ul class="new-room-candidates">
          {#each candidates as candidate (candidate.seat_id)}
            {@const checked = selectedSeatIds.includes(candidate.seat_id)}
            {@const leafSlug = leafSlugOf(candidate.seat_id)}
            <li>
              <label class="candidate-row" data-checked={checked || undefined}>
                <input
                  type="checkbox"
                  {checked}
                  disabled={submitting || !candidate.online}
                  onchange={() => toggleSeat(candidate.seat_id)}
                />
                <StatusDot state={candidate.online ? "running" : "stopped"} />
                <span class="candidate-main">
                  <span class="candidate-head">
                    <span class="candidate-name">{candidate.display_name}</span>
                    {#if leafSlug !== "" && candidate.display_name.toLowerCase() !== leafSlug.toLowerCase()}
                      <span class="candidate-handle">@{leafSlug}</span>
                    {/if}
                    <Badge tone={seatKindTone(candidate)}>{seatKindLabel(candidate)}</Badge>
                    {#if !candidate.online}
                      <Badge tone="neutral">offline</Badge>
                    {/if}
                  </span>
                  {#if candidate.role !== null && candidate.role.trim() !== ""}
                    <span class="candidate-role">{candidate.role}</span>
                  {/if}
                </span>
              </label>
            </li>
          {/each}
        </ul>
      {/if}
    </fieldset>

    <Toggle
      label="Pin to sidebar"
      hint="Keep this room listed directly under Rooms in the left rail."
      bind:checked={pinned}
    />
  </form>

  {#snippet actions()}
    <Button variant="quiet" disabled={submitting} onclick={close}>Cancel</Button>
    <Button type="submit" form="{uid}-form" variant="primary" loading={submitting}>
      Create room
    </Button>
  {/snippet}
</Dialog>

<style>
  .new-room-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
  }

  .new-room-roster {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    min-width: 0;
    margin: 0;
    padding: 0;
    border: 0;
  }

  .new-room-legend {
    padding: 0;
    color: var(--color-text);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
  }

  .new-room-hint,
  .new-room-empty {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    --leading: var(--line-height-ui);
  }

  .new-room-candidates {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
    max-height: 220px;
    padding: var(--space-4);
    overflow-y: auto;
    border: 1px solid var(--color-line);
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    list-style: none;
  }

  .candidate-row {
    display: flex;
    align-items: flex-start;
    gap: var(--space-10);
    padding: var(--space-8) var(--space-10);
    border-radius: var(--corner-sm);
    cursor: pointer;
    transition: background-color var(--duration-fast) var(--ease-out);

    &:hover {
      background: var(--color-stone-3);
    }

    &[data-checked] {
      background: var(--color-vein-tint);
    }

    & input[type="checkbox"] {
      flex: none;
      margin-top: var(--space-2);
      accent-color: var(--color-vein);
    }
  }

  .candidate-main {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .candidate-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-6);
  }

  .candidate-name {
    color: var(--color-text);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
  }

  .candidate-handle {
    color: var(--color-text-3);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
  }

  .candidate-role {
    color: var(--color-text-2);
    font-size: var(--font-size-xs);
    --leading: var(--line-height-ui);
  }
</style>
