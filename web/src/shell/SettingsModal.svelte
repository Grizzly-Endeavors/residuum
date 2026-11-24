<script lang="ts">
  import { tick, untrack } from "svelte";
  import { MediaQuery } from "svelte/reactivity";
  import { actionRegistry } from "../lib/action-registry.svelte";
  import { router } from "../lib/router.svelte";
  import { settingsModel } from "../lib/settings-model.svelte";
  import {
    defaultSection,
    scopeKind,
    type AgentSectionId,
    type AllSectionId,
    type SectionId,
  } from "../lib/settings-sections";
  import { Banner, Button, IconButton, ModalLayer, Skeleton } from "../lib/ui";
  import { PHONE_QUERY } from "../styles/breakpoints";
  import ChangedOnDiskDialog from "./settings/ChangedOnDiskDialog.svelte";
  import SaveBar from "./settings/SaveBar.svelte";
  import { modalActions, reloadScope } from "./settings/scope-actions";
  import { AGENT_SECTION_VIEWS, ALL_SECTION_VIEWS, type SettingsScope } from "./settings/sections";
  import SettingsNav from "./settings/SettingsNav.svelte";

  const SLOW_LOAD_MS = 250;

  const phone = new MediaQuery(PHONE_QUERY);
  const openScope = $derived(router.settings?.scope ?? null);
  const openSection = $derived(router.settings?.section ?? null);
  const listing = $derived(phone.current && openSection === null);

  let scope = $state.raw<SettingsScope | null>(null);
  let shown = $state.raw<{ scope: SettingsScope; section: SectionId | null } | null>(null);

  $effect(() => {
    const id = openScope;
    if (id === null) {
      scope = null;
      return;
    }
    const next = untrack(() => settingsModel.scope(id));
    scope = next;
    void next.load();
  });

  $effect(() => {
    const target = scope;
    const section = openSection;
    if (target === null) {
      shown = null;
      return;
    }
    const reveal = (): void => {
      shown = { scope: target, section };
    };
    if (target.loaded || untrack(() => shown?.scope) === target) {
      reveal();
      return;
    }
    const timer = setTimeout(reveal, SLOW_LOAD_MS);
    return () => {
      clearTimeout(timer);
    };
  });

  const section = $derived.by((): SectionId | null => {
    if (shown === null) return null;
    return shown.section ?? (phone.current ? null : defaultSection(shown.scope.kind));
  });
  const contentKey = $derived(
    shown === null || section === null ? null : `${shown.scope.id}/${section}`,
  );
  const current = $derived(
    openScope === null || listing ? null : (openSection ?? defaultSection(scopeKind(openScope))),
  );

  $effect(() =>
    router.guard.register((target) =>
      target === null && settingsModel.stagedScopes.length > 0 ? "Unsaved settings changes" : null,
    ),
  );

  $effect(() => {
    const open = scope;
    if (open === null) return;
    return untrack(() => actionRegistry.register("settings-modal", () => modalActions(open)));
  });

  let frame = $state<HTMLElement>();
  let body = $state<HTMLElement>();

  $effect(() => {
    if (contentKey !== null && body !== undefined) body.scrollTop = 0;
  });

  let wasListing: boolean | null = null;
  let leftSection: SectionId | null = null;
  $effect(() => {
    const now = listing;
    const root = frame;
    if (root === undefined) {
      wasListing = null;
      return;
    }
    if (wasListing !== null && wasListing !== now) {
      const target = now
        ? (root.querySelector(`[data-section="${leftSection ?? ""}"]`) ??
          root.querySelector("[data-scope-picker]"))
        : root.querySelector(".settings-back");
      void tick().then(() => {
        if (target instanceof HTMLElement) target.focus();
      });
    }
    wasListing = now;
    if (!now) leftSection = openSection;
  });

  function openSectionFromList(id: SectionId): void {
    if (phone.current) void router.openSettingsSection(id);
    else void router.switchSettingsSection(id);
  }

  function close(): void {
    void router.closeSettings();
  }
</script>

{#snippet sectionView(open: SettingsScope, id: SectionId)}
  {#if open.kind === "agent"}
    {@const View = AGENT_SECTION_VIEWS[id as AgentSectionId]}
    <View scope={open} section={id as AgentSectionId} />
  {:else}
    {@const View = ALL_SECTION_VIEWS[id as AllSectionId]}
    <View scope={open} section={id as AllSectionId} />
  {/if}
{/snippet}

<ModalLayer
  open={openScope !== null}
  historyEntry={false}
  fullscreenOnPhone
  keepBottomBar
  width="1040px"
  label="Settings"
  class="settings-modal"
  initialFocus={phone.current && openSection !== null ? ".settings-back" : "[data-scope-picker]"}
  onclose={close}
>
  <div class="settings-frame" class:listing bind:this={frame}>
    <SettingsNav
      scopeId={openScope ?? ""}
      {current}
      scope={shown?.scope ?? null}
      onsection={openSectionFromList}
      onscope={(id) => void router.switchSettingsScope(id)}
      onclose={close}
    />
    <div class="settings-body" bind:this={body}>
      <div class="settings-top">
        <IconButton
          class="settings-back"
          icon="chevron-left"
          label="Back to all settings"
          onclick={() => void router.closeSettingsSection()}
        />
        <span class="settings-top-gap"></span>
        {#if scope !== null}
          {@const reloading = scope}
          <IconButton
            icon="reload"
            label="Reload from disk"
            onclick={() => void reloadScope(reloading)}
          />
        {/if}
        <IconButton icon="close" label="Close settings" onclick={close} />
      </div>
      {#if shown !== null && section !== null}
        {@const open = shown.scope}
        <div class="settings-inner">
          {#key contentKey}
            <div class="settings-content">
              {#if open.loadError !== null}
                <div class="settings-load-error">
                  <Banner tone="error">
                    {open.loadError}
                    {#snippet actions()}
                      <Button size="sm" onclick={() => void open.load()}>Try again</Button>
                    {/snippet}
                  </Banner>
                </div>
              {/if}
              {#if open.loaded}
                {@render sectionView(open, section)}
              {:else}
                <Skeleton lines={6} label="Loading settings" />
              {/if}
            </div>
          {/key}
          <SaveBar scope={open} />
        </div>
      {/if}
    </div>
  </div>
</ModalLayer>
<ChangedOnDiskDialog />

<style>
  :global(dialog.ui-modal.settings-modal) {
    --ui-quiet-hover: var(--color-stone-3);

    height: min(760px, 100%);
    background: var(--color-stone-1);
  }

  .settings-frame {
    display: grid;
    flex: 1;
    grid-template-columns: 260px minmax(0, 1fr);
    min-height: 0;
  }

  .settings-body {
    position: relative;
    display: flex;
    flex-direction: column;
    min-width: 0;
    overflow-y: auto;
  }

  .settings-top {
    position: sticky;
    top: 0;
    z-index: var(--z-sticky);
    display: flex;
    align-items: center;
    gap: var(--space-4);
    padding: var(--space-12) var(--space-12) var(--space-4);
    background: var(--color-stone-1);
  }

  .settings-top-gap {
    flex: 1;
  }

  .settings-body :global(.settings-back) {
    display: none;
  }

  .settings-inner {
    flex: 1;
    width: 100%;
    max-width: 760px;
    padding: 0 var(--space-40) var(--space-40);
  }

  .settings-content {
    animation: settings-swap var(--duration-swap) var(--ease-out);
  }

  .settings-load-error {
    max-width: 640px;
    margin-bottom: var(--space-16);
  }

  @media (max-width: 760px) {
    .settings-frame {
      grid-template-columns: minmax(0, 1fr);
    }

    .settings-frame.listing > .settings-body,
    .settings-frame:not(.listing) > :global(.settings-nav) {
      display: none;
    }

    .settings-body :global(.settings-back) {
      display: inline-grid;
    }

    .settings-inner {
      padding: 0 var(--space-16) var(--space-32);
    }
  }

  @keyframes settings-swap {
    from {
      opacity: 0;
    }
  }
</style>
