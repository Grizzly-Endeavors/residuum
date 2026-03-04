<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable() }: { fields: ConfigFields } = $props();
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Observation Thresholds</div>
    <div class="settings-field">
      <label>Observer Threshold (tokens)</label>
      <input type="number" bind:value={fields.observer_threshold_tokens} placeholder="Default: 2000" />
      <span class="field-hint">Token count before the observer fires.</span>
    </div>
    <div class="settings-field">
      <label>Reflector Threshold (tokens)</label>
      <input type="number" bind:value={fields.reflector_threshold_tokens} placeholder="Default: 8000" />
      <span class="field-hint">Token count before the reflector compresses memories.</span>
    </div>
    <div class="settings-field">
      <label>Observer Cooldown (seconds)</label>
      <input type="number" bind:value={fields.observer_cooldown_secs} placeholder="Default: 120" />
      <span class="field-hint">Cooldown after soft threshold is crossed.</span>
    </div>
    <div class="settings-field">
      <label>Observer Force Threshold (tokens)</label>
      <input type="number" bind:value={fields.observer_force_threshold_tokens} placeholder="Default: 6000" />
      <span class="field-hint">Forces immediate observation, bypassing cooldown.</span>
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Search Tuning</div>
    <div class="settings-field">
      <label>Vector Weight</label>
      <input type="number" step="0.05" bind:value={fields.search_vector_weight} placeholder="Default: 0.6" />
      <span class="field-hint">Weight for vector similarity in hybrid search (0.0-1.0).</span>
    </div>
    <div class="settings-field">
      <label>Text Weight</label>
      <input type="number" step="0.05" bind:value={fields.search_text_weight} placeholder="Default: 0.4" />
      <span class="field-hint">Weight for BM25 text scores in hybrid search (0.0-1.0).</span>
    </div>
    <div class="settings-field">
      <label>Min Score</label>
      <input type="number" step="0.01" bind:value={fields.search_min_score} placeholder="Default: 0.3" />
      <span class="field-hint">Minimum hybrid score threshold for results.</span>
    </div>
    <div class="settings-field">
      <label>Candidate Multiplier</label>
      <input type="number" bind:value={fields.search_candidate_multiplier} placeholder="Default: 3" />
      <span class="field-hint">Multiplier on limit for candidate retrieval before merge.</span>
    </div>
    <div class="settings-field">
      <label>
        <span class="toggle-switch">
          <input type="checkbox" bind:checked={fields.search_temporal_decay} />
          <span class="toggle-slider"></span>
        </span>
        Temporal Decay
      </label>
      <span class="field-hint">Reduce relevance of older memories over time.</span>
    </div>
    {#if fields.search_temporal_decay}
      <div class="settings-field">
        <label>Decay Half-Life (days)</label>
        <input type="number" step="1" bind:value={fields.search_temporal_decay_half_life_days} placeholder="Default: 30" />
        <span class="field-hint">Number of days for memory relevance to halve.</span>
      </div>
    {/if}
  </div>
</div>
