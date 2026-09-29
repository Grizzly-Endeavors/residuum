<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable() }: { fields: ConfigFields } = $props();
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Session budget</div>
    <div class="field-hint">
      Limits that apply to every agent on this install. A message hop is one agent handing a message
      to another.
    </div>
    <div class="settings-field">
      <label for="rt-bg-max-concurrent">Max Concurrent Session Turns</label>
      <input
        id="rt-bg-max-concurrent"
        type="number"
        bind:value={fields.bg_max_concurrent}
        placeholder="Default: 3"
      />
    </div>
    <div class="settings-field">
      <label for="rt-bg-hop-soft-limit">Hop Soft Limit</label>
      <input
        id="rt-bg-hop-soft-limit"
        type="number"
        bind:value={fields.bg_hop_soft_limit}
        placeholder="Default: 8"
      />
      <div class="field-hint">
        At or above this many hops, a delivered agent message carries a note asking the receiver to
        reply only if a reply is actually needed.
      </div>
    </div>
    <div class="settings-field">
      <label for="rt-bg-hop-hard-limit">Hop Hard Limit</label>
      <input
        id="rt-bg-hop-hard-limit"
        type="number"
        bind:value={fields.bg_hop_hard_limit}
        placeholder="Default: 32"
      />
      <div class="field-hint">
        At or above this many hops, agent message delivery is refused outright, to bound message
        loops.
      </div>
    </div>
  </div>
</div>
