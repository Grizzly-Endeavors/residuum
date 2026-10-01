<script lang="ts">
  import { router } from "../lib/router.svelte";
  import { defaultSection, scopeKind } from "../lib/settings-sections";
  import { ModalLayer } from "../lib/ui";
  import Settings from "../Settings.svelte";

  // The Settings modal, open while the URL names a `settings` scope. The URL
  // parameter is its history entry, so the layer pushes none of its own, and
  // closing goes through the router. It hosts the current Settings page,
  // which lists the registry's sections.

  const target = $derived(router.settings);
</script>

{#if target !== null}
  <ModalLayer
    open
    historyEntry={false}
    fullscreenOnPhone
    width="1040px"
    label="Settings"
    class="shell-settings"
    onclose={() => void router.closeSettings()}
  >
    <div class="shell-settings-page" data-legacy-view>
      {#key target.scope}
        <Settings
          scope={target.scope}
          section={target.section ?? defaultSection(scopeKind(target.scope))}
          onSelectSection={(section) => void router.switchSettingsSection(section)}
          onClose={() => void router.closeSettings()}
        />
      {/key}
    </div>
  </ModalLayer>
{/if}

<style>
  :global(.shell-settings) {
    height: min(760px, 100%);
  }

  .shell-settings-page {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
  }
</style>
