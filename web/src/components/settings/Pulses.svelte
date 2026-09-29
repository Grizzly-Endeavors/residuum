<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable() }: { fields: ConfigFields } = $props();
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Pulse & Background</div>
    <div class="settings-field">
      <label>
        <span class="toggle-switch">
          <input type="checkbox" bind:checked={fields.pulse_enabled} />
          <span class="toggle-slider"></span>
        </span>
        Pulse Enabled
      </label>
    </div>
    <div class="settings-field">
      <label for="rt-bg-idle-scheduled">Scheduled Session Idle Timeout (minutes)</label>
      <input
        id="rt-bg-idle-scheduled"
        type="number"
        bind:value={fields.bg_idle_timeout_scheduled_minutes}
        placeholder="Default: 2"
      />
      <div class="field-hint">
        How long a pulse or scheduled-action session lingers idle before completing. Webhook
        sessions use this timeout too.
      </div>
    </div>
    <div class="settings-field">
      <label for="rt-bg-idle-spawned">Spawned Session Idle Timeout (minutes)</label>
      <input
        id="rt-bg-idle-spawned"
        type="number"
        bind:value={fields.bg_idle_timeout_spawned_minutes}
        placeholder="Default: 10"
      />
      <div class="field-hint">
        How long a session started by subagent_spawn or the learner lingers idle before completing.
      </div>
    </div>
    <div class="settings-field">
      <label for="rt-bg-idle-external">External Session Idle Timeout (minutes)</label>
      <input
        id="rt-bg-idle-external"
        type="number"
        bind:value={fields.bg_idle_timeout_external_minutes}
        placeholder="Default: 30"
      />
      <div class="field-hint">Idle timeout for non-webhook external sessions.</div>
    </div>
    <div class="settings-field">
      <label for="rt-bg-idle-artifact">Artifact Session Idle Timeout (minutes)</label>
      <input
        id="rt-bg-idle-artifact"
        type="number"
        bind:value={fields.bg_idle_timeout_artifact_minutes}
        placeholder="Default: 10"
      />
      <div class="field-hint">
        How long a session started by a workbench artifact lingers idle before completing.
      </div>
    </div>
    <div class="settings-field">
      <label for="rt-bg-episode-skip-floor">Episode Skip Token Floor</label>
      <input
        id="rt-bg-episode-skip-floor"
        type="number"
        bind:value={fields.bg_episode_skip_token_floor}
        placeholder="Default: 2000"
      />
      <div class="field-hint">
        A completed session run below this many transcript tokens, with nothing staged, produces no
        episode. Its transcript is still kept.
      </div>
    </div>
    <div class="settings-field">
      <label for="rt-bg-subagent-depth-cap">Subagent Nesting Depth Cap</label>
      <input
        id="rt-bg-subagent-depth-cap"
        type="number"
        bind:value={fields.bg_subagent_depth_cap}
        placeholder="Default: 3"
      />
      <div class="field-hint">
        Maximum nesting depth for subagent_spawn (main is depth 0). A session at the cap is refused
        when it tries to spawn further.
      </div>
    </div>
  </div>
</div>
