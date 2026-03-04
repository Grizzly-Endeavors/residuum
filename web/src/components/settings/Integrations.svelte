<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable() }: { fields: ConfigFields } = $props();

  let newSkillDir = $state("");

  function addSkillDir() {
    const dir = newSkillDir.trim();
    if (dir && !fields.skills_dirs.includes(dir)) {
      fields.skills_dirs = [...fields.skills_dirs, dir];
      newSkillDir = "";
    }
  }

  function removeSkillDir(idx: number) {
    fields.skills_dirs = fields.skills_dirs.filter((_, i) => i !== idx);
  }
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Discord</div>
    <div class="integration-card">
      <div class="integration-desc">
        Connect a Discord bot so your agent can communicate via DMs.
        Create a bot at <a href="https://discord.com/developers/applications" target="_blank" rel="noopener">discord.com/developers</a>.
      </div>
      <div class="settings-field">
        <label>Bot Token</label>
        {#if fields.discord_token.startsWith("secret:")}
          <div class="secret-stored">
            <span class="secret-badge">Stored securely</span>
            <button class="btn btn-sm btn-secondary" onclick={() => { fields.discord_token = ""; }}>Change</button>
          </div>
        {:else}
          <input type="password" bind:value={fields.discord_token} placeholder="Discord bot token" />
        {/if}
      </div>
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Telegram</div>
    <div class="integration-card">
      <div class="integration-desc">
        Connect a Telegram bot for DM-based interaction.
        Create a bot via <a href="https://t.me/BotFather" target="_blank" rel="noopener">@BotFather</a>.
      </div>
      <div class="settings-field">
        <label>Bot Token</label>
        {#if fields.telegram_token.startsWith("secret:")}
          <div class="secret-stored">
            <span class="secret-badge">Stored securely</span>
            <button class="btn btn-sm btn-secondary" onclick={() => { fields.telegram_token = ""; }}>Change</button>
          </div>
        {:else}
          <input type="password" bind:value={fields.telegram_token} placeholder="Telegram bot token" />
        {/if}
      </div>
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Webhook</div>
    <div class="integration-card">
      <div class="integration-desc">
        Enable an HTTP webhook endpoint for external integrations.
      </div>
      <div class="settings-field">
        <label>
          <span class="toggle-switch">
            <input type="checkbox" bind:checked={fields.webhook_enabled} />
            <span class="toggle-slider"></span>
          </span>
          Webhook Enabled
        </label>
      </div>
      {#if fields.webhook_enabled}
        <div class="settings-field">
          <label>Secret</label>
          {#if fields.webhook_secret.startsWith("secret:")}
            <div class="secret-stored">
              <span class="secret-badge">Stored securely</span>
              <button class="btn btn-sm btn-secondary" onclick={() => { fields.webhook_secret = ""; }}>Change</button>
            </div>
          {:else}
            <input type="password" bind:value={fields.webhook_secret} placeholder="Bearer token for authentication (optional)" />
          {/if}
        </div>
      {/if}
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Skills</div>
    <div class="integration-card">
      <div class="integration-desc">
        Directories to scan for custom skills.
      </div>
      {#each fields.skills_dirs as dir, i}
        <div class="skill-dir-entry">
          <span class="skill-dir-path">{dir}</span>
          <button class="btn btn-sm btn-danger" onclick={() => removeSkillDir(i)}>Remove</button>
        </div>
      {/each}
      <div class="skill-dir-add">
        <input type="text" bind:value={newSkillDir} placeholder="Path to skills directory" onkeydown={(e) => { if (e.key === "Enter") addSkillDir(); }} />
        <button class="btn btn-sm btn-secondary" onclick={addSkillDir}>Add</button>
      </div>
    </div>
  </div>
</div>
