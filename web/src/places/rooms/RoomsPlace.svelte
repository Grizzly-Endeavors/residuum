<script lang="ts">
  import { onDestroy, tick, untrack } from "svelte";
  import Prose from "../../feed/Prose.svelte";
  import { displayState } from "../../lib/agent-display-state";
  import { hub } from "../../lib/hub.svelte";
  import type { RoomDetail, RoomMeetingRecord, RoomSeat, RoomSummary } from "../../lib/hub-types";
  import { Icon } from "../../lib/icons";
  import { roomsStore, type RoomTurnStateSnapshot } from "../../lib/rooms.svelte";
  import { router } from "../../lib/router.svelte";
  import { formatLocation, locationAt } from "../../lib/routes";
  import { ALL_SCOPE } from "../../lib/settings-sections";
  import { messageTimeLabel, relativeTime } from "../../lib/time";
  import {
    AttachmentChip,
    Badge,
    Banner,
    Button,
    ConfirmDialog,
    EmptyState,
    IconButton,
    Skeleton,
    Spinner,
    StatusDot,
    Tabs,
    type TabItem,
  } from "../../lib/ui";
  import PlaceHeader from "../../shell/PlaceHeader.svelte";
  import NewRoomDialog from "./NewRoomDialog.svelte";
  import RoomComposer from "./RoomComposer.svelte";
  import RoomRosterDrawer from "./RoomRosterDrawer.svelte";

  interface Props {
    room: string | null;
    tab: "active" | "archived";
  }

  let { room, tab }: Props = $props();

  const uid = $props.id();
  const minutesBodyId = `${uid}-minutes-body`;

  let newRoomOpen = $state(false);
  let rosterOpen = $state(false);
  let confirmDeleteOpen = $state(false);
  let pinning = $state(false);
  let capturingNotes = $state(false);
  let archiving = $state(false);
  let reopening = $state(false);
  let stopping = $state(false);
  let continuing = $state(false);
  let deleting = $state(false);
  let minutesExpanded = $state(false);
  let lastMinutesKey = "";
  let scrollEl = $state<HTMLDivElement>();
  let scrolledRoomId = "";
  let lastTailSeq = 0;
  let wasNearBottom = true;

  let prevRoomProp: string | null | undefined = undefined;

  $effect(() => {
    const id = room;
    untrack(() => {
      const prevViewing = roomsStore.viewingRoomId;
      if (prevRoomProp !== id) {
        if (prevRoomProp !== undefined) {
          rosterOpen = false;
          confirmDeleteOpen = false;
          roomsStore.error = null;
        }
        prevRoomProp = id;
      }
      roomsStore.setViewingRoom(id);
      if (id !== null && (prevViewing !== id || roomsStore.currentRoom?.id !== id)) {
        void roomsStore.openRoom(id);
      }
    });
  });

  onDestroy(() => {
    if (prevRoomProp !== undefined && roomsStore.viewingRoomId === prevRoomProp) {
      roomsStore.setViewingRoom(null);
    }
  });

  $effect(() => {
    if (room !== null) return;
    untrack(() => {
      if (!roomsStore.loaded) void roomsStore.refreshList();
    });
  });

  let now = $state(Date.now());
  $effect(() => {
    const timer = window.setInterval(() => {
      now = Date.now();
    }, 30_000);
    return () => window.clearInterval(timer);
  });

  const openRoom = $derived<RoomDetail | null>(
    room !== null && roomsStore.currentRoom?.id === room ? roomsStore.currentRoom : null,
  );

  $effect(() => {
    const key = openRoom === null ? "" : `${openRoom.id}:${openRoom.status}`;
    if (key === lastMinutesKey) return;
    lastMinutesKey = key;
    if (openRoom !== null) {
      minutesExpanded = openRoom.status === "archived";
    }
  });

  $effect.pre(() => {
    void roomsStore.messages;
    void openRoom?.id;
    wasNearBottom =
      scrollEl == null
        ? true
        : scrollEl.scrollHeight - scrollEl.scrollTop - scrollEl.clientHeight < 120;
  });

  $effect(() => {
    const roomId = openRoom?.id ?? null;
    const messages = roomsStore.messages;
    const tailSeq = messages.at(-1)?.seq ?? 0;
    if (roomId === null) {
      scrolledRoomId = "";
      lastTailSeq = 0;
      return;
    }
    if (messages.length === 0) return;
    const isInitialOpen = scrolledRoomId !== roomId;
    const hasNewTail = tailSeq > lastTailSeq;
    scrolledRoomId = roomId;
    lastTailSeq = tailSeq;
    if (isInitialOpen || (hasNewTail && wasNearBottom)) {
      void tick().then(() => {
        if (scrollEl != null) {
          scrollEl.scrollTop = scrollEl.scrollHeight;
        }
      });
    }
  });

  const title = $derived(openRoom !== null ? openRoom.name : "Rooms");
  const sub = $derived.by(() => {
    if (room !== null) return openRoom?.topic ?? undefined;
    return tab === "archived" ? "Archived meeting rooms" : "Multi-agent meeting rooms";
  });

  const listTabs = $derived<readonly TabItem<"active" | "archived">[]>([
    { value: "active", label: "Active", count: roomsStore.activeRooms.length },
    { value: "archived", label: "Archived", count: roomsStore.archivedRooms.length },
  ]);

  const displayedRooms = $derived(
    tab === "archived" ? roomsStore.archivedRooms : roomsStore.activeRooms,
  );

  const turnState = $derived.by<RoomTurnStateSnapshot>(() => {
    if (openRoom === null) {
      return {
        activeTurn: null,
        queuedSeatIds: [],
        pausedAtCap: false,
        windowUsed: 0,
        windowCap: 0,
        selectorError: null,
      };
    }
    return (
      roomsStore.turnStates[openRoom.id] ?? {
        activeTurn: openRoom.active_turn,
        queuedSeatIds: openRoom.window.queued_seats,
        pausedAtCap: openRoom.window.paused_at_cap,
        windowUsed: openRoom.window.window_used,
        windowCap: openRoom.window.window_cap,
        selectorError: openRoom.window.selector_error,
      }
    );
  });

  const hasQueuedTurns = $derived(!turnState.pausedAtCap && turnState.queuedSeatIds.length > 0);
  const turnBusy = $derived(turnState.activeTurn !== null || hasQueuedTurns);

  const minutesPanelPath = $derived.by<string | null>(() => {
    const raw = openRoom?.minutes.shared_path ?? null;
    return raw === null ? null : raw.replace(/^team\//, "");
  });

  const earlierMeetings = $derived.by<RoomMeetingRecord[]>(() => {
    if (openRoom === null || openRoom.meetings.length <= 1) return [];
    return openRoom.meetings.slice(0, -1).reverse();
  });

  function meetingForDividerSeq(detail: RoomDetail, seq: number): RoomMeetingRecord | null {
    return detail.meetings.find((m) => m.divider_seq === seq) ?? null;
  }

  function meetingFileLabel(sharedPath: string): string {
    return sharedPath.replace(/^team\/minutes\//, "").replace(/^team\//, "");
  }

  function followsInPlace(event: MouseEvent): boolean {
    if (event.metaKey || event.ctrlKey || event.shiftKey || event.altKey || event.button !== 0) {
      return false;
    }
    event.preventDefault();
    return true;
  }

  function roomHref(id: string): string {
    return formatLocation(locationAt({ kind: "rooms", room: id, tab: "active" }));
  }

  function backToList(): void {
    const nextTab = openRoom?.status === "archived" ? "archived" : tab;
    void router.openPlace({ kind: "rooms", room: null, tab: nextTab });
  }

  function switchTab(nextTab: "active" | "archived"): void {
    void router.replacePlace({ kind: "rooms", room: null, tab: nextTab });
  }

  function onRoomCreated(roomId: string): void {
    void router.openPlace({ kind: "rooms", room: roomId, tab: "active" });
  }

  function openMinutesFile(path: string): void {
    void router.openPanel({ kind: "file", path, agent: null });
  }

  function openDecisionModelSettings(): void {
    void router.openSettings({ scope: ALL_SCOPE, section: "system_one" });
  }

  function localAgentName(seatId: string | null): string | null {
    if (!seatId?.startsWith("agent:")) return null;
    return seatId.slice("agent:".length);
  }

  function seatDotState(
    seat: RoomSeat,
  ): "running" | "stopped" | "starting" | "stopping" | "failed" {
    const agentName = localAgentName(seat.seat_id);
    if (agentName !== null) {
      const agentSummary = hub.agent(agentName);
      if (agentSummary !== undefined) {
        return displayState(agentSummary.state, hub.isStopping(agentSummary.name));
      }
    }
    return "running";
  }

  function seatInstanceFor(detail: RoomDetail, seatId: string | null): string | null {
    if (seatId === null) return null;
    const inRoster = detail.roster.find((s) => s.seat_id === seatId)?.instance ?? null;
    if (inRoster !== null) return inRoster;
    if (!seatId.startsWith("a2a:")) return null;
    const rest = seatId.slice("a2a:".length);
    const slash = rest.indexOf("/");
    return slash > 0 ? rest.slice(0, slash) : null;
  }

  function hasExternalSeat(summary: RoomSummary): boolean {
    return summary.roster.some((seat) => seat.kind === "external_a2a");
  }

  async function togglePin(): Promise<void> {
    if (openRoom === null || pinning) return;
    pinning = true;
    await roomsStore.patchRoom(openRoom.id, { pinned: !openRoom.pinned });
    pinning = false;
  }

  async function handleCaptureNotes(): Promise<void> {
    if (openRoom === null || capturingNotes) return;
    capturingNotes = true;
    await roomsStore.captureNotes(openRoom.id);
    capturingNotes = false;
  }

  async function handleArchive(): Promise<void> {
    if (openRoom === null || archiving) return;
    archiving = true;
    await roomsStore.archiveRoom(openRoom.id);
    archiving = false;
  }

  async function handleReopen(): Promise<void> {
    if (openRoom === null || reopening) return;
    reopening = true;
    await roomsStore.reopenRoom(openRoom.id);
    reopening = false;
  }

  async function handleStop(): Promise<void> {
    if (openRoom === null || stopping) return;
    stopping = true;
    await roomsStore.stopRoom(openRoom.id);
    stopping = false;
  }

  async function handleContinue(): Promise<void> {
    if (openRoom === null || continuing) return;
    continuing = true;
    await roomsStore.continueRoom(openRoom.id);
    continuing = false;
  }

  async function handleDelete(): Promise<void> {
    if (openRoom === null || deleting) return;
    const id = openRoom.id;
    const nextTab = openRoom.status === "archived" ? "archived" : "active";
    deleting = true;
    const deleted = await roomsStore.deleteRoom(id);
    deleting = false;
    if (deleted) {
      await router.openPlace({ kind: "rooms", room: null, tab: nextTab });
    }
  }
</script>

{#if room === null}
  <PlaceHeader {title} {sub}>
    <div class="rooms-header-actions">
      <Button variant="primary" icon="plus" onclick={() => (newRoomOpen = true)}>New room</Button>
    </div>
  </PlaceHeader>

  <div class="rooms-index-scroll">
    <section class="rooms-index" aria-label="Meeting rooms">
      {#if roomsStore.error !== null}
        <Banner tone="error" ondismiss={() => (roomsStore.error = null)}>
          {roomsStore.error}
          {#snippet actions()}
            <Button variant="quiet" size="sm" onclick={() => void roomsStore.refreshList()}>
              Try again
            </Button>
          {/snippet}
        </Banner>
      {/if}

      <Tabs label="Room status" tabs={listTabs} selected={tab} onchange={switchTab}>
        {#snippet children(selectedTab)}
          {#if !roomsStore.loaded && roomsStore.rooms.length === 0}
            <div class="rooms-loading">
              <Skeleton shape="block" height="64px" label="Loading meeting rooms" />
              <Skeleton shape="block" height="64px" />
            </div>
          {:else if displayedRooms.length === 0}
            {#if selectedTab === "active"}
              <EmptyState
                variant="block"
                icon="users"
                title="No active meeting rooms"
                headingLevel={2}
              >
                Hold persistent team chats or focused meetings with multiple agents at once.
                {#snippet actions()}
                  <Button variant="primary" icon="plus" onclick={() => (newRoomOpen = true)}>
                    New room
                  </Button>
                {/snippet}
              </EmptyState>
            {:else}
              <EmptyState variant="block" icon="archive" title="No archived rooms" headingLevel={2}>
                Finished meetings that you archive appear here with their meeting minutes.
              </EmptyState>
            {/if}
          {:else}
            <ul class="rooms-list" aria-label="{selectedTab} rooms">
              {#each displayedRooms as summary (summary.id)}
                {@const updatedDate = new Date(summary.updated_at)}
                <li>
                  <a
                    class="room-row"
                    href={roomHref(summary.id)}
                    data-unread={summary.unread_count > 0 || undefined}
                    onclick={(event) => {
                      if (followsInPlace(event)) {
                        void router.openPlace({
                          kind: "rooms",
                          room: summary.id,
                          tab: selectedTab,
                        });
                      }
                    }}
                  >
                    <span
                      class="room-row-dot"
                      data-unread-dot={summary.unread_count > 0 || undefined}
                    ></span>
                    <span class="room-row-main">
                      <span class="room-row-top">
                        <span class="room-row-name">{summary.name}</span>
                        <Badge count={summary.unread_count} label="unread" />
                        {#if summary.pinned && summary.status === "active"}
                          <Badge tone="accent">Pinned</Badge>
                        {/if}
                        {#if summary.paused_at_cap && summary.status === "active"}
                          <Badge tone="neutral">Paused</Badge>
                        {/if}
                        {#if hasExternalSeat(summary)}
                          <Badge tone="danger">EXT A2A</Badge>
                        {/if}
                        <time
                          class="room-row-time"
                          datetime={summary.updated_at}
                          title={updatedDate.toLocaleString()}
                        >
                          {relativeTime(updatedDate, now)}
                        </time>
                      </span>
                      {#if summary.topic !== null && summary.topic.trim() !== ""}
                        <span class="room-row-topic">{summary.topic}</span>
                      {/if}
                      <span class="room-row-meta">
                        <span>{summary.roster.map((s) => s.display_name).join(", ")}</span>
                        <span
                          >{summary.last_seq}
                          {summary.last_seq === 1 ? "message" : "messages"}</span
                        >
                      </span>
                    </span>
                  </a>
                </li>
              {/each}
            </ul>
          {/if}
        {/snippet}
      </Tabs>
    </section>
  </div>
{:else}
  <PlaceHeader {title} {sub}>
    <div class="rooms-header-actions">
      <IconButton
        icon="back"
        label="Back to rooms"
        variant="quiet"
        size="sm"
        onclick={backToList}
      />
      {#if openRoom !== null}
        <Button
          size="sm"
          pressed={openRoom.pinned}
          loading={pinning}
          onclick={() => void togglePin()}
        >
          {openRoom.pinned ? "Unpin" : "Pin"}
        </Button>
        <Button size="sm" icon="users" onclick={() => (rosterOpen = true)}>
          Roster ({openRoom.roster.length})
        </Button>
        {#if openRoom.status === "active"}
          <Button
            size="sm"
            icon="document"
            loading={capturingNotes}
            disabled={capturingNotes ||
              archiving ||
              openRoom.last_seq <=
                Math.max(openRoom.last_captured_seq, openRoom.minutes.archived_at_seq)}
            onclick={() => void handleCaptureNotes()}
          >
            Capture notes
          </Button>
          <Button
            size="sm"
            icon="archive"
            loading={archiving}
            disabled={capturingNotes}
            onclick={() => void handleArchive()}
          >
            Archive room
          </Button>
        {:else}
          <Button size="sm" icon="restore" loading={reopening} onclick={() => void handleReopen()}>
            Reopen room
          </Button>
        {/if}
        <Button
          size="sm"
          variant="danger"
          icon="trash"
          loading={deleting}
          onclick={() => (confirmDeleteOpen = true)}
        >
          Delete
        </Button>
      {/if}
    </div>
  </PlaceHeader>

  {#if openRoom !== null}
    <div class="room-roster-bar" aria-label="Room participants">
      <ul class="room-seat-chips">
        {#each openRoom.roster as seat (seat.seat_id)}
          {@const isWorking = turnState.activeTurn?.seat_id === seat.seat_id}
          <li class="room-seat-chip" data-working={isWorking || undefined}>
            <StatusDot state={seatDotState(seat)} working={isWorking} />
            <span class="room-seat-name">{seat.display_name}</span>
            {#if seat.kind === "external_a2a"}
              <Badge tone="danger">EXT A2A</Badge>
            {:else if seat.kind === "sibling_a2a" && seat.instance !== null}
              <Badge tone="accent">{seat.instance}</Badge>
            {/if}
          </li>
        {/each}
      </ul>
      {#if openRoom.status === "archived"}
        <Badge tone="neutral">Archived</Badge>
      {/if}
    </div>

    {#if !roomsStore.decisionModelActive && openRoom.status === "active"}
      <Banner tone="warn" title="@mention-only mode" edge>
        No Decision Model is active, so unaddressed messages won't trigger automatic replies. Use
        <code>@agent</code> or <code>@everyone</code> to direct participants.
        {#snippet actions()}
          <Button size="sm" variant="secondary" icon="settings" onclick={openDecisionModelSettings}>
            Configure Decision model
          </Button>
        {/snippet}
      </Banner>
    {:else if openRoom.status === "active" && turnState.selectorError !== null}
      <Banner
        tone="error"
        title="Speaker selection failed — @mention-only for this message"
        edge
        ondismiss={() => roomsStore.dismissSelectorError(openRoom.id)}
      >
        {turnState.selectorError} Use <code>@agent</code> or <code>@everyone</code> to direct
        participants.
        {#snippet actions()}
          <Button size="sm" variant="secondary" icon="settings" onclick={openDecisionModelSettings}>
            Decision model settings
          </Button>
        {/snippet}
      </Banner>
    {/if}
  {/if}

  {#if roomsStore.error !== null && (openRoom !== null || roomsStore.loading)}
    <Banner tone="error" edge ondismiss={() => (roomsStore.error = null)}>
      {roomsStore.error}
    </Banner>
  {/if}

  <div class="room-scroll" aria-label="Room transcript" bind:this={scrollEl}>
    <div class="room-column">
      {#if roomsStore.loading && openRoom === null}
        <div class="rooms-loading">
          <Skeleton shape="block" height="64px" label="Loading meeting room" />
          <Skeleton shape="block" height="64px" />
        </div>
      {:else if openRoom === null}
        <EmptyState variant="block" icon="users" title="Room not found" headingLevel={2}>
          This meeting room could not be loaded or may have been deleted.
          {#snippet actions()}
            <Button variant="secondary" icon="back" onclick={backToList}>Back to rooms</Button>
          {/snippet}
        </EmptyState>
      {:else}
        {#if roomsStore.minutesMarkdown !== null || minutesPanelPath !== null}
          <section class="room-minutes-card" aria-label="Meeting minutes">
            <div class="room-minutes-bar">
              <button
                type="button"
                class="room-minutes-toggle"
                aria-expanded={minutesExpanded}
                aria-controls={minutesBodyId}
                onclick={() => (minutesExpanded = !minutesExpanded)}
              >
                <span class="room-minutes-chevron" data-open={minutesExpanded || undefined}>
                  <Icon name="chevron-right" size={15} />
                </span>
                <span>Meeting minutes</span>
              </button>
              {#if minutesPanelPath !== null}
                <Button
                  size="sm"
                  variant="secondary"
                  icon="document"
                  onclick={() => openMinutesFile(minutesPanelPath)}
                >
                  Open minutes file
                </Button>
              {/if}
            </div>
            {#if roomsStore.minutesMarkdown !== null}
              <div id={minutesBodyId} class="room-minutes-body" hidden={!minutesExpanded}>
                <Prose
                  content={roomsStore.minutesMarkdown}
                  agent={null}
                  teamFolder
                  filePath={minutesPanelPath ?? undefined}
                />
              </div>
            {/if}
            {#if earlierMeetings.length > 0}
              <div class="room-earlier-meetings">
                <span class="room-earlier-label">Earlier notes:</span>
                <ul class="room-earlier-list" aria-label="Earlier captured notes">
                  {#each earlierMeetings as meeting (meeting.id)}
                    <li>
                      <Button
                        size="sm"
                        variant="quiet"
                        icon="document"
                        onclick={() => openMinutesFile(meeting.shared_path.replace(/^team\//, ""))}
                      >
                        {meetingFileLabel(meeting.shared_path)}
                      </Button>
                    </li>
                  {/each}
                </ul>
              </div>
            {/if}
          </section>
        {/if}

        {#if roomsStore.hasMoreMessages}
          <div class="room-load-older">
            <Button
              variant="quiet"
              size="sm"
              loading={roomsStore.loadingOlder}
              onclick={() => void roomsStore.loadOlderMessages()}
            >
              Load earlier messages
            </Button>
          </div>
        {/if}

        {#if roomsStore.messages.length === 0}
          <div class="room-empty-feed">
            <EmptyState>
              No messages yet. Post a question or mention <code>@everyone</code> below to begin.
            </EmptyState>
          </div>
        {:else}
          <ol class="room-messages">
            {#each roomsStore.messages as msg (msg.id)}
              <li>
                {#if msg.kind === "system"}
                  {@const capturedMeeting = meetingForDividerSeq(openRoom, msg.seq)}
                  <div class="room-system-notice" role="status">
                    <Icon name="info" size={14} />
                    <span class="room-system-text">{msg.content}</span>
                    {#if capturedMeeting !== null}
                      <Button
                        variant="quiet"
                        size="sm"
                        icon="document"
                        onclick={() =>
                          openMinutesFile(capturedMeeting.shared_path.replace(/^team\//, ""))}
                      >
                        Open notes
                      </Button>
                    {/if}
                    <time class="room-msg-time" datetime={msg.timestamp}>
                      {messageTimeLabel(msg.timestamp, now)}
                    </time>
                  </div>
                {:else if msg.kind === "owner"}
                  <article class="room-owner-msg">
                    <div class="room-owner-meta">
                      <time class="room-msg-time" datetime={msg.timestamp}>
                        {messageTimeLabel(msg.timestamp, now)}
                      </time>
                    </div>
                    <div class="room-owner-bubble">
                      {#if msg.content.trim() !== ""}
                        <Prose content={msg.content} agent={null} />
                      {/if}
                      {#if msg.attachments.length > 0}
                        <ul class="room-attachments" aria-label="Attachments">
                          {#each msg.attachments as att (att.id)}
                            {@const attUrl =
                              att.url || `/api/hub/rooms/${openRoom.id}/attachments/${att.id}`}
                            <li>
                              {#if att.mime_type.startsWith("image/")}
                                <a
                                  class="room-att-image-link"
                                  href={attUrl}
                                  target="_blank"
                                  rel="noopener"
                                >
                                  <img class="room-att-image" src={attUrl} alt={att.filename} />
                                </a>
                              {/if}
                              <AttachmentChip
                                filename={att.filename}
                                size={att.size}
                                contentType={att.mime_type}
                                href={attUrl}
                              />
                            </li>
                          {/each}
                        </ul>
                      {/if}
                    </div>
                  </article>
                {:else}
                  <article class="room-agent-msg">
                    <header class="room-agent-head">
                      <span class="room-agent-author">{msg.author_name}</span>
                      {#if msg.seat_kind === "external_a2a"}
                        <Badge tone="danger">EXT A2A</Badge>
                      {:else if msg.seat_kind === "sibling_a2a"}
                        {@const instance = seatInstanceFor(openRoom, msg.seat_id)}
                        {#if instance !== null}
                          <Badge tone="accent">{instance}</Badge>
                        {/if}
                      {/if}
                      <time class="room-msg-time" datetime={msg.timestamp}>
                        {messageTimeLabel(msg.timestamp, now)}
                      </time>
                    </header>
                    {#if msg.content.trim() !== ""}
                      <Prose content={msg.content} agent={localAgentName(msg.seat_id)} />
                    {/if}
                    {#if msg.attachments.length > 0}
                      <ul class="room-attachments" aria-label="Attachments">
                        {#each msg.attachments as att (att.id)}
                          {@const attUrl =
                            att.url || `/api/hub/rooms/${openRoom.id}/attachments/${att.id}`}
                          <li>
                            {#if att.mime_type.startsWith("image/")}
                              <a
                                class="room-att-image-link"
                                href={attUrl}
                                target="_blank"
                                rel="noopener"
                              >
                                <img class="room-att-image" src={attUrl} alt={att.filename} />
                              </a>
                            {/if}
                            <AttachmentChip
                              filename={att.filename}
                              size={att.size}
                              contentType={att.mime_type}
                              href={attUrl}
                            />
                          </li>
                        {/each}
                      </ul>
                    {/if}
                  </article>
                {/if}
              </li>
            {/each}
          </ol>
        {/if}
      {/if}
    </div>
  </div>

  {#if openRoom !== null}
    <footer class="room-footer">
      {#if openRoom.status === "active" && turnBusy}
        <div class="room-turn-bar" role="status">
          <Spinner size={14} />
          {#if turnState.activeTurn !== null}
            <span class="room-turn-label">{turnState.activeTurn.display_name} is typing…</span>
            {#if turnState.activeTurn.tool_status !== null && turnState.activeTurn.tool_status !== ""}
              <Badge tone="accent">{turnState.activeTurn.tool_status}</Badge>
            {/if}
          {:else}
            <span class="room-turn-label">Next participant starting…</span>
          {/if}
          {#if turnState.queuedSeatIds.length > 0}
            <span class="room-turn-queued">({turnState.queuedSeatIds.length} queued)</span>
          {/if}
          <span class="room-turn-action">
            <Button
              size="sm"
              variant="secondary"
              icon="stop"
              loading={stopping}
              onclick={() => void handleStop()}
            >
              Stop
            </Button>
          </span>
        </div>
      {:else if openRoom.status === "active" && turnState.pausedAtCap}
        <div class="room-paused-bar" role="status">
          <span class="room-turn-label">
            Paused after {turnState.windowUsed} agent {turnState.windowUsed === 1
              ? "reply"
              : "replies"}
          </span>
          <span class="room-turn-action">
            <Button
              size="sm"
              variant="primary"
              icon="play"
              loading={continuing}
              onclick={() => void handleContinue()}
            >
              Continue
            </Button>
          </span>
        </div>
      {/if}

      <RoomComposer
        room={openRoom}
        turnActive={turnBusy}
        {reopening}
        onreopen={() => void handleReopen()}
      />
    </footer>

    <RoomRosterDrawer bind:open={rosterOpen} room={openRoom} candidates={roomsStore.candidates} />

    <ConfirmDialog
      bind:open={confirmDeleteOpen}
      title="Delete “{openRoom.name}”?"
      message="This permanently removes the room and its transcript. Any meeting minutes already saved in Shared files are kept."
      confirmLabel="Delete room"
      tone="danger"
      onconfirm={() => void handleDelete()}
    />
  {/if}
{/if}

<NewRoomDialog
  bind:open={newRoomOpen}
  candidates={roomsStore.candidates}
  oncreated={onRoomCreated}
/>

<style>
  .rooms-header-actions {
    display: flex;
    flex: none;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    margin-left: auto;
  }

  .rooms-index-scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .rooms-index {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
    width: 100%;
    max-width: calc(var(--layout-reading-width) + var(--space-64));
    margin-inline: auto;
    padding: var(--space-24) clamp(var(--space-16), 3vw, var(--space-32)) var(--space-64);
  }

  .rooms-loading {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  .rooms-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    list-style: none;
  }

  .room-row {
    display: flex;
    align-items: flex-start;
    gap: var(--space-10);
    padding: var(--space-12) var(--space-14);
    border: 1px solid var(--color-line-soft);
    border-radius: var(--corner-md);
    background: var(--color-stone-1);
    color: var(--color-text);
    text-decoration: none;
    transition:
      background-color var(--duration-fast) var(--ease-out),
      border-color var(--duration-fast) var(--ease-out);

    &:hover {
      border-color: var(--color-line);
      background: var(--color-stone-2);
    }
  }

  .room-row-dot {
    flex: none;
    width: var(--space-8);
    height: var(--space-8);
    margin-top: var(--space-6);
    border-radius: var(--corner-pill);

    &[data-unread-dot] {
      background: var(--color-vein-bright);
    }
  }

  .room-row-main {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-4);
    min-width: 0;
  }

  .room-row-top {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
  }

  .room-row-name {
    color: var(--color-text);
    font-size: var(--font-size-message);
    font-weight: var(--font-weight-semibold);
  }

  .room-row-time {
    margin-left: auto;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
  }

  .room-row-topic {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    --leading: var(--line-height-ui);
  }

  .room-row-meta {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-12);
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .room-roster-bar {
    display: flex;
    flex: none;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-8);
    padding: var(--space-8) var(--space-24);
    border-bottom: 1px solid var(--color-line-soft);
    background: var(--color-stone-1);
  }

  .room-seat-chips {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-6);
    list-style: none;
  }

  .room-seat-chip {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    padding: var(--space-2) var(--space-8) var(--space-2) var(--space-6);
    border: 1px solid var(--color-line-soft);
    border-radius: var(--corner-pill);
    background: var(--color-stone-2);
    font-size: var(--font-size-xs);

    &[data-working] {
      border-color: var(--color-vein-line);
      background: var(--color-vein-faint);
    }
  }

  .room-seat-name {
    color: var(--color-text);
    font-weight: var(--font-weight-medium);
  }

  .room-scroll {
    position: relative;
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .room-column {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
    width: 100%;
    max-width: calc(var(--layout-reading-width) + 2 * var(--space-24));
    margin-inline: auto;
    padding: var(--space-20) var(--space-24) var(--space-32);
  }

  .room-minutes-card {
    display: flex;
    flex-direction: column;
    gap: var(--space-10);
    padding: var(--space-12) var(--space-16);
    border: 1px solid var(--color-line);
    border-radius: var(--corner-md);
    background: var(--color-stone-1);
  }

  .room-minutes-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-10);
  }

  .room-minutes-toggle {
    display: inline-flex;
    align-items: center;
    gap: var(--space-8);
    min-height: 32px;
    padding: 0 var(--space-8) 0 var(--space-4);
    border-radius: var(--corner-sm);
    color: var(--color-text);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
    transition: color var(--duration-fast) var(--ease-out);

    &:hover {
      color: var(--color-vein-bright);
    }
  }

  .room-minutes-chevron {
    display: grid;
    transition: transform var(--duration-base) var(--ease-out);

    &[data-open] {
      transform: rotate(90deg);
    }
  }

  .room-minutes-body {
    width: 100%;
    padding-top: var(--space-4);

    &[hidden] {
      display: none;
    }
  }

  .room-earlier-meetings {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-6);
    padding-top: var(--space-8);
    border-top: 1px solid var(--color-line-soft);
  }

  .room-earlier-label {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
  }

  .room-earlier-list {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-4);
    list-style: none;
  }

  .room-load-older {
    display: flex;
    justify-content: center;
  }

  .room-empty-feed {
    padding: var(--space-32) 0;
  }

  .room-messages {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
    list-style: none;
  }

  .room-system-notice {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    padding: var(--space-8) var(--space-12);
    border: 1px solid var(--color-line-soft);
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .room-system-text {
    flex: 1 1 200px;
    min-width: 0;
    --leading: var(--line-height-ui);
  }

  .room-owner-msg {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: var(--space-4);
  }

  .room-owner-meta {
    display: flex;
    align-items: center;
    gap: var(--space-6);
  }

  .room-owner-bubble {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    max-width: min(82%, 560px);
    padding: var(--space-10) var(--space-14);
    border-radius: var(--corner-lg) var(--corner-lg) var(--corner-sm) var(--corner-lg);
    background: var(--color-moss-tint);
    color: var(--color-text);
  }

  .room-agent-msg {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-12) var(--space-16);
    border: 1px solid var(--color-line-soft);
    border-radius: var(--corner-lg);
    background: var(--color-stone-1);
  }

  .room-agent-head {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
  }

  .room-agent-author {
    color: var(--color-text);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  .room-msg-time {
    margin-left: auto;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    font-variant-numeric: tabular-nums;
  }

  .room-attachments {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    list-style: none;
  }

  .room-att-image-link {
    display: block;
    max-width: min(100%, 420px);
    margin-bottom: var(--space-4);
  }

  .room-att-image {
    display: block;
    max-width: 100%;
    max-height: 320px;
    border-radius: var(--corner-md);
  }

  .room-footer {
    display: flex;
    flex: none;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-10) var(--space-24) var(--space-16);
    border-top: 1px solid var(--color-line-soft);
    background: var(--color-stone-0);
  }

  .room-turn-bar,
  .room-paused-bar {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: var(--space-8);
    width: 100%;
    max-width: var(--layout-reading-width);
    margin: 0 auto;
    padding: var(--space-8) var(--space-12);
    border: 1px solid var(--color-line);
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    font-size: var(--font-size-sm);
  }

  .room-paused-bar {
    border-color: var(--color-vein-line);
    background: var(--color-vein-faint);
  }

  .room-turn-label {
    color: var(--color-text);
    font-weight: var(--font-weight-medium);
  }

  .room-turn-queued {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .room-turn-action {
    margin-left: auto;
  }

  @media (max-width: 760px) {
    .room-roster-bar {
      padding: var(--space-8) var(--space-16);
    }

    .room-column {
      padding: var(--space-16) var(--space-16) var(--space-24);
    }

    .room-footer {
      padding: var(--space-8) var(--space-16) var(--space-12);
    }

    .room-owner-bubble {
      max-width: 92%;
    }
  }
</style>
