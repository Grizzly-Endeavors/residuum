<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable(), simple = false }: { fields: ConfigFields; simple?: boolean } =
    $props();
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Observation Thresholds</div>
    <div class="settings-field">
      <label for="mem-observer-threshold">Observer Threshold (tokens)</label>
      <input
        id="mem-observer-threshold"
        type="number"
        bind:value={fields.observer_threshold_tokens}
        placeholder="Default: 2000"
      />
      <span class="field-hint">Token count before the observer fires.</span>
    </div>
    <div class="settings-field">
      <label for="mem-reflector-threshold">Reflector Threshold (tokens)</label>
      <input
        id="mem-reflector-threshold"
        type="number"
        bind:value={fields.reflector_threshold_tokens}
        placeholder="Default: 8000"
      />
      <span class="field-hint">Token count before the reflector compresses memories.</span>
    </div>
    <div class="settings-field">
      <label for="mem-observer-cooldown">Observer Cooldown (seconds)</label>
      <input
        id="mem-observer-cooldown"
        type="number"
        bind:value={fields.observer_cooldown_secs}
        placeholder="Default: 120"
      />
      <span class="field-hint">Cooldown after soft threshold is crossed.</span>
    </div>
    <div class="settings-field">
      <label for="mem-observer-force">Observer Force Threshold (tokens)</label>
      <input
        id="mem-observer-force"
        type="number"
        bind:value={fields.observer_force_threshold_tokens}
        placeholder="Default: 6000"
      />
      <span class="field-hint">Forces immediate observation, bypassing cooldown.</span>
    </div>
  </div>

  {#if !simple}
    <div class="settings-group">
      <div class="settings-group-label">Search Tuning</div>
      <div class="settings-field">
        <label for="mem-vector-weight">Vector Weight</label>
        <input
          id="mem-vector-weight"
          type="number"
          step="0.05"
          bind:value={fields.search_vector_weight}
          placeholder="Default: 0.6"
        />
        <span class="field-hint">Weight for vector similarity in hybrid search (0.0-1.0).</span>
      </div>
      <div class="settings-field">
        <label for="mem-text-weight">Text Weight</label>
        <input
          id="mem-text-weight"
          type="number"
          step="0.05"
          bind:value={fields.search_text_weight}
          placeholder="Default: 0.4"
        />
        <span class="field-hint">Weight for BM25 text scores in hybrid search (0.0-1.0).</span>
      </div>
      <div class="settings-field">
        <label for="mem-min-score">Min Score</label>
        <input
          id="mem-min-score"
          type="number"
          step="0.01"
          bind:value={fields.search_min_score}
          placeholder="Default: 0.3"
        />
        <span class="field-hint">Minimum hybrid score threshold for results.</span>
      </div>
      <div class="settings-field">
        <label for="mem-candidate-multiplier">Candidate Multiplier</label>
        <input
          id="mem-candidate-multiplier"
          type="number"
          bind:value={fields.search_candidate_multiplier}
          placeholder="Default: 3"
        />
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
          <label for="mem-decay-half-life">Decay Half-Life (days)</label>
          <input
            id="mem-decay-half-life"
            type="number"
            step="1"
            bind:value={fields.search_temporal_decay_half_life_days}
            placeholder="Default: 30"
          />
          <span class="field-hint">Number of days for memory relevance to halve.</span>
        </div>
      {/if}
    </div>
  {/if}
</div>
