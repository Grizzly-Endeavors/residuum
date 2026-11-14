<script lang="ts">
  import { untrack } from "svelte";
  import { roomsStore } from "../../lib/rooms.svelte";
  import { EmptyState } from "../../lib/ui";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";

  interface Props {
    room: string | null;
    tab: "active" | "archived";
  }

  let { room, tab }: Props = $props();

  $effect(() => {
    const id = room;
    untrack(() => {
      const prevViewing = roomsStore.viewingRoomId;
      roomsStore.setViewingRoom(id);
      if (id !== null && (prevViewing !== id || roomsStore.currentRoom?.id !== id)) {
        void roomsStore.openRoom(id);
      }
    });
    return () => {
      untrack(() => {
        if (roomsStore.viewingRoomId === id) roomsStore.setViewingRoom(null);
      });
    };
  });

  const title = $derived(
    room !== null && roomsStore.currentRoom?.id === room ? roomsStore.currentRoom.name : "Rooms",
  );
  const sub = $derived.by(() => {
    if (room !== null) return roomsStore.currentRoom?.topic ?? undefined;
    return tab === "archived" ? "Archived meeting rooms" : "Multi-agent meeting rooms";
  });
</script>

<PlaceHeader {title} {sub} />

<div class="rooms-stub">
  <EmptyState icon="users" title={room !== null ? title : "Meeting rooms"}>
    {room !== null
      ? `Room ${room} is ready.`
      : `${tab === "archived" ? "Archived" : "Active"} rooms (${String(
          tab === "archived" ? roomsStore.archivedRooms.length : roomsStore.activeRooms.length,
        )}).`}
  </EmptyState>
</div>

<style>
  .rooms-stub {
    display: flex;
    flex: 1;
    min-height: 0;
    align-items: center;
    justify-content: center;
    padding: var(--space-24);
  }
</style>
