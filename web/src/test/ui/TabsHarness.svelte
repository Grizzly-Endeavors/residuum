<script lang="ts">
  import Tabs from "../../lib/ui/Tabs.svelte";
  import type { TabItem } from "../../lib/ui/types";

  // Tabs whose panel names the selected tab, so a test can see it follow.
  let {
    tabs,
    selected,
    onchange,
  }: { tabs: readonly TabItem[]; selected: string; onchange: (selected: string) => void } =
    $props();
</script>

<Tabs label="Inbox views" {tabs} {selected} {onchange}>
  {#snippet children(tab)}
    <p>Showing {tab}</p>
  {/snippet}
</Tabs>
