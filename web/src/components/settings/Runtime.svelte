<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable(), simple = false }: { fields: ConfigFields; simple?: boolean } =
    $props();
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">General</div>
    <div class="settings-field">
      <label for="rt-timeout">Timeout (seconds)</label>
      <input
        id="rt-timeout"
        type="number"
        bind:value={fields.timeout_secs}
        placeholder="Default: 120"
      />
    </div>
    <div class="settings-field">
      <label for="rt-max-tokens">Max Tokens</label>
      <input
        id="rt-max-tokens"
        type="number"
        bind:value={fields.max_tokens}
        placeholder="Default: 8192"
      />
    </div>
    <div class="settings-field">
      <label for="rt-temperature">Default Temperature</label>
      <input
        id="rt-temperature"
        type="number"
        step="0.1"
        min="0"
        max="2"
        bind:value={fields.temperature}
        placeholder="Provider default"
      />
      <div class="field-hint">Per-role overrides can be set in the Providers panel.</div>
    </div>
    <div class="settings-field">
      <label for="rt-thinking">Default Thinking</label>
      <select id="rt-thinking" bind:value={fields.thinking}>
        <option value="">Default (off)</option>
        <option value="low">Low</option>
        <option value="medium">Medium</option>
        <option value="high">High</option>
      </select>
      <div class="field-hint">Per-role overrides can be set in the Providers panel.</div>
    </div>
  </div>

  {#if !simple}
    <div class="settings-group">
      <div class="settings-group-label">Subconscious</div>
      <div class="field-hint">
        A small model that watches conversations and steers the agent when it drifts from its
        instructions. Off by default — it adds a classifier call per evaluated turn. Assign a cheap
        model to the <code>subconscious</code> role in the Providers panel.
      </div>
      <div class="settings-field">
        <label>
          <span class="toggle-switch">
            <input type="checkbox" bind:checked={fields.subconscious_enabled} />
            <span class="toggle-slider"></span>
          </span>
          Subconscious Enabled
        </label>
      </div>
      {#if fields.subconscious_enabled}
        <div class="settings-field">
          <label>
            <span class="toggle-switch">
              <input type="checkbox" bind:checked={fields.subconscious_mid_turn} />
              <span class="toggle-slider"></span>
            </span>
            Watch Mid-Turn
          </label>
          <div class="field-hint">
            Also evaluate during the agent's tool loop, not just after the turn ends.
          </div>
        </div>
        <div class="settings-field">
          <label for="rt-sub-every-n">Mid-Turn Cadence (iterations)</label>
          <input
            id="rt-sub-every-n"
            type="number"
            bind:value={fields.subconscious_every_n_iterations}
            placeholder="Default: 3"
          />
        </div>
        <div class="settings-field">
          <label for="rt-sub-max-transcript">Max Transcript Tokens</label>
          <input
            id="rt-sub-max-transcript"
            type="number"
            bind:value={fields.subconscious_max_transcript_tokens}
            placeholder="Default: 12000"
          />
        </div>
        <div class="settings-field">
          <label>
            <span class="toggle-switch">
              <input type="checkbox" bind:checked={fields.subconscious_learning} />
              <span class="toggle-slider"></span>
            </span>
            Learn From Conversations
          </label>
          <div class="field-hint">
            When a turn reveals something durable — a correction, a preference, a hard-won fix —
            spawn a background learner to verify it against memory and keep it. Adds occasional
            sub-agent runs.
          </div>
        </div>
        {#if fields.subconscious_learning}
          <div class="settings-field">
            <label for="rt-sub-learning-cooldown">Learning Cooldown (minutes)</label>
            <input
              id="rt-sub-learning-cooldown"
              type="number"
              bind:value={fields.subconscious_learning_cooldown_minutes}
              placeholder="Default: 240"
            />
          </div>
        {/if}
      {/if}
    </div>

    <div class="settings-group">
      <div class="settings-group-label">Learning Fallback</div>
      <div class="field-hint">
        Without the subconscious, the agent can still review conversations for things worth keeping:
        every N turns, a background learner checks recent history for preferences and fixes to
        persist. Leave at 0 to disable.
      </div>
      <div class="settings-field">
        <label for="rt-learning-nudge">Review Every N Turns</label>
        <input
          id="rt-learning-nudge"
          type="number"
          bind:value={fields.learning_nudge_after_turns}
          placeholder="Default: 0 (off)"
        />
      </div>
    </div>

    <div class="settings-group">
      <div class="settings-group-label">Retry</div>
      <div class="settings-field">
        <label for="rt-retry-max-retries">Max Retries</label>
        <input
          id="rt-retry-max-retries"
          type="number"
          bind:value={fields.retry_max_retries}
          placeholder="Default: 3"
        />
      </div>
      <div class="settings-field">
        <label for="rt-retry-initial-delay">Initial Delay (ms)</label>
        <input
          id="rt-retry-initial-delay"
          type="number"
          bind:value={fields.retry_initial_delay_ms}
          placeholder="Default: 1000"
        />
      </div>
      <div class="settings-field">
        <label for="rt-retry-max-delay">Max Delay (ms)</label>
        <input
          id="rt-retry-max-delay"
          type="number"
          bind:value={fields.retry_max_delay_ms}
          placeholder="Default: 30000"
        />
      </div>
      <div class="settings-field">
        <label for="rt-retry-backoff">Backoff Multiplier</label>
        <input
          id="rt-retry-backoff"
          type="number"
          step="0.1"
          bind:value={fields.retry_backoff_multiplier}
          placeholder="Default: 2.0"
        />
      </div>
    </div>

    <div class="settings-group">
      <div class="settings-group-label">Agent Abilities</div>
      <div class="settings-field">
        <label>
          <span class="toggle-switch">
            <input type="checkbox" bind:checked={fields.agent_modify_mcp} />
            <span class="toggle-slider"></span>
          </span>
          Allow MCP Modifications
        </label>
      </div>
      <div class="settings-field">
        <label>
          <span class="toggle-switch">
            <input type="checkbox" bind:checked={fields.agent_modify_channels} />
            <span class="toggle-slider"></span>
          </span>
          Allow Channel Modifications
        </label>
      </div>
      <div class="settings-field">
        <label for="rt-agent-max-tool-iterations">Max Tool Calls Per Turn</label>
        <input
          id="rt-agent-max-tool-iterations"
          type="number"
          min="1"
          bind:value={fields.agent_max_tool_iterations}
          placeholder="Unlimited"
        />
        <div class="field-hint">
          Stop a turn gracefully after this many tool calls. Leave blank for no limit — a runaway
          turn can still be stopped at any time (Cancel in the web UI, /stop, or stop_agent for
          background sessions).
        </div>
      </div>
      <div class="settings-field">
        <label>
          <span class="toggle-switch">
            <input type="checkbox" bind:checked={fields.agent_repeat_call_guard_enabled} />
            <span class="toggle-slider"></span>
          </span>
          Repeat Call Guard
        </label>
        <div class="field-hint">
          Watches for a model repeating the exact same tool call (same tool, byte-identical
          arguments) over and over.
        </div>
      </div>
      <div class="settings-field">
        <label for="rt-agent-repeat-steer">Steer After (consecutive identical calls)</label>
        <input
          id="rt-agent-repeat-steer"
          type="number"
          min="1"
          bind:value={fields.agent_repeat_call_steer_after}
          placeholder="Default: 3"
        />
        <div class="field-hint">
          The call still runs, but its result carries a note nudging the model to try something
          else.
        </div>
      </div>
      <div class="settings-field">
        <label for="rt-agent-repeat-stop">Stop After (consecutive identical calls)</label>
        <input
          id="rt-agent-repeat-stop"
          type="number"
          min="1"
          bind:value={fields.agent_repeat_call_stop_after}
          placeholder="Default: 6"
        />
        <div class="field-hint">The turn ends instead of running the call again.</div>
      </div>
    </div>

    <div class="settings-group">
      <div class="settings-group-label">Idle</div>
      <div class="settings-field">
        <label for="rt-idle-timeout">Timeout (minutes)</label>
        <input
          id="rt-idle-timeout"
          type="number"
          bind:value={fields.idle_timeout_minutes}
          placeholder="Default: 30 (0 = disabled)"
        />
      </div>
      <div class="settings-field">
        <label for="rt-idle-channel">Idle Channel</label>
        <select id="rt-idle-channel" bind:value={fields.idle_channel}>
          <option value="">Keep current</option>
          <option value="websocket">WebSocket</option>
          <option value="telegram">Telegram</option>
          <option value="discord">Discord</option>
          <option value="teams">Microsoft Teams</option>
        </select>
      </div>
    </div>
  {/if}
</div>
