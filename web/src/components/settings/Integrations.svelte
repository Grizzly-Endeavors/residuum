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

  function addWebhook() {
    fields.webhooks = [
      ...fields.webhooks,
      { name: "", secret: "", routing: "inbox", format: "parsed", content_fields: "" },
    ];
  }

  function removeWebhook(idx: number) {
    fields.webhooks = fields.webhooks.filter((_, i) => i !== idx);
  }
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Discord</div>
    <div class="integration-card">
      <div class="integration-desc">
        Connect a Discord bot so your agent can communicate via DMs. Create a bot at <a
          href="https://discord.com/developers/applications"
          target="_blank"
          rel="noopener">discord.com/developers</a
        >.
      </div>
      <div class="settings-field">
        <label for="integ-discord-token">Bot Token</label>
        {#if fields.discord_token.startsWith("secret:")}
          <div class="secret-stored">
            <span class="secret-badge">Stored securely</span>
            <button
              class="btn btn-sm btn-secondary"
              onclick={() => {
                fields.discord_token = "";
              }}>Change</button
            >
          </div>
        {:else}
          <input
            id="integ-discord-token"
            type="password"
            bind:value={fields.discord_token}
            placeholder="Discord bot token"
          />
        {/if}
      </div>
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Telegram</div>
    <div class="integration-card">
      <div class="integration-desc">
        Connect a Telegram bot for DM-based interaction. Create a bot via <a
          href="https://t.me/BotFather"
          target="_blank"
          rel="noopener">@BotFather</a
        >.
      </div>
      <div class="settings-field">
        <label for="integ-telegram-token">Bot Token</label>
        {#if fields.telegram_token.startsWith("secret:")}
          <div class="secret-stored">
            <span class="secret-badge">Stored securely</span>
            <button
              class="btn btn-sm btn-secondary"
              onclick={() => {
                fields.telegram_token = "";
              }}>Change</button
            >
          </div>
        {:else}
          <input
            id="integ-telegram-token"
            type="password"
            bind:value={fields.telegram_token}
            placeholder="Telegram bot token"
          />
        {/if}
      </div>
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Webhooks</div>
    <div class="integration-card">
      <div class="integration-desc">
        Named HTTP webhook endpoints for external integrations. Each webhook gets its own
        <code>/webhook/&lbrace;name&rbrace;</code> route with independent auth and payload handling.
      </div>

      {#each fields.webhooks as wh, i (i)}
        <div class="webhook-entry">
          <div class="webhook-entry-header">
            <span class="webhook-entry-label">
              {wh.name ? `/webhook/${wh.name}` : "New webhook"}
            </span>
            <button class="btn btn-sm btn-danger" onclick={() => removeWebhook(i)}>Remove</button>
          </div>

          <div class="webhook-entry-fields">
            <div class="settings-field">
              <label for="wh-name-{i}">Name</label>
              <input
                id="wh-name-{i}"
                type="text"
                bind:value={wh.name}
                placeholder="e.g. github-issues"
              />
            </div>

            <div class="settings-field">
              <label for="wh-secret-{i}">Secret</label>
              {#if wh.secret.startsWith("secret:")}
                <div class="secret-stored">
                  <span class="secret-badge">Stored securely</span>
                  <button
                    class="btn btn-sm btn-secondary"
                    onclick={() => {
                      wh.secret = "";
                    }}>Change</button
                  >
                </div>
              {:else}
                <input
                  id="wh-secret-{i}"
                  type="password"
                  bind:value={wh.secret}
                  placeholder="Bearer token (optional)"
                />
              {/if}
            </div>

            <div class="settings-field">
              <label for="wh-routing-{i}">Routing</label>
              <input
                id="wh-routing-{i}"
                type="text"
                bind:value={wh.routing}
                placeholder="inbox or agent:preset_name"
              />
            </div>

            <div class="settings-field">
              <label for="wh-format-{i}">Format</label>
              <select id="wh-format-{i}" bind:value={wh.format}>
                <option value="parsed">Parsed (extract JSON fields)</option>
                <option value="raw">Raw (pass body as-is)</option>
              </select>
            </div>

            {#if wh.format !== "raw"}
              <div class="settings-field">
                <label for="wh-fields-{i}">Content Fields</label>
                <input
                  id="wh-fields-{i}"
                  type="text"
                  bind:value={wh.content_fields}
                  placeholder="e.g. issue.title, issue.body (comma-separated, dot-notation)"
                />
              </div>
            {/if}
          </div>
        </div>
      {/each}

      <button class="btn btn-sm btn-secondary webhook-add-btn" onclick={addWebhook}
        >Add Webhook</button
      >
    </div>
  </div>

  <div class="settings-group">
    <div class="settings-group-label">Skills</div>
    <div class="integration-card">
      <div class="integration-desc">Directories to scan for custom skills.</div>
      {#each fields.skills_dirs as dir, i (dir)}
        <div class="skill-dir-entry">
          <span class="skill-dir-path">{dir}</span>
          <button class="btn btn-sm btn-danger" onclick={() => removeSkillDir(i)}>Remove</button>
        </div>
      {/each}
      <div class="skill-dir-add">
        <input
          type="text"
          bind:value={newSkillDir}
          placeholder="Path to skills directory"
          onkeydown={(e) => {
            if (e.key === "Enter") addSkillDir();
          }}
        />
        <button class="btn btn-sm btn-secondary" onclick={addSkillDir}>Add</button>
      </div>
    </div>
  </div>
</div>

<style>
  .webhook-entry {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 12px;
    margin-bottom: 10px;
    background: var(--bg);
  }

  .webhook-entry-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
  }

  .webhook-entry-label {
    font-family: var(--font-mono);
    font-size: 12px;
    color: var(--accent);
  }

  .webhook-entry-fields {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .webhook-add-btn {
    margin-top: 8px;
  }
</style>
