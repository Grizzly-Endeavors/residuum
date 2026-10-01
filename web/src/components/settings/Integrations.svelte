<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";
  import { isSecretReference, isEnvReference, envReferenceName } from "../../lib/secrets";
  import { toast } from "../../lib/toast.svelte";
  import { notifyStagedRemoval } from "../../lib/form-undo";

  let {
    fields = $bindable(),
    part,
    agent,
  }: {
    fields: ConfigFields;
    /** Which of this file's groups to show. */
    part: "channels" | "webhooks" | "tools";
    /** The agent whose config holds the webhooks. */
    agent: string | null;
  } = $props();

  // ── Skills ─────────────────────────────────────────────────────────

  let newSkillDir = $state("");

  function addSkillDir() {
    const dir = newSkillDir.trim();
    if (dir && !fields.skills_dirs.includes(dir)) {
      fields.skills_dirs = [...fields.skills_dirs, dir];
      newSkillDir = "";
    }
  }

  function removeSkillDir(idx: number) {
    const removed = fields.skills_dirs[idx];
    if (removed === undefined) return;
    fields.skills_dirs = fields.skills_dirs.filter((_, i) => i !== idx);
    toast.success(`Removed ${removed}.`, {
      label: "Undo",
      onClick: () => {
        fields.skills_dirs = [
          ...fields.skills_dirs.slice(0, idx),
          removed,
          ...fields.skills_dirs.slice(idx),
        ];
      },
    });
  }

  // ── Tools ──────────────────────────────────────────────────────────

  let newToolDir = $state("");

  function addToolDir() {
    const dir = newToolDir.trim();
    if (dir && !fields.tools_path.includes(dir)) {
      fields.tools_path = [...fields.tools_path, dir];
      newToolDir = "";
    }
  }

  function removeToolDir(idx: number) {
    const removed = fields.tools_path[idx];
    if (removed === undefined) return;
    fields.tools_path = fields.tools_path.filter((_, i) => i !== idx);
    toast.success(`Removed ${removed}.`, {
      label: "Undo",
      onClick: () => {
        fields.tools_path = [
          ...fields.tools_path.slice(0, idx),
          removed,
          ...fields.tools_path.slice(idx),
        ];
      },
    });
  }

  // ── Webhooks ───────────────────────────────────────────────────────

  function addWebhook() {
    fields.webhooks = [
      ...fields.webhooks,
      { name: "", secret: "", routing: "inbox", format: "parsed", content_fields: "" },
    ];
  }

  function removeWebhook(idx: number) {
    const removed = fields.webhooks[idx];
    if (!removed) return;
    fields.webhooks = fields.webhooks.filter((_, i) => i !== idx);
    notifyStagedRemoval(`Removed ${removed.name || "webhook"}.`, () => {
      fields.webhooks = [...fields.webhooks.slice(0, idx), removed, ...fields.webhooks.slice(idx)];
    });
  }

  // ── Web Search ─────────────────────────────────────────────────────

  let nativeOverridesOpen = $state(false);

  // ── Teams ──────────────────────────────────────────────────────────

  let teamsPartiallyFilled = $derived.by(() => {
    const filled = [fields.teams_app_id, fields.teams_tenant_id, fields.teams_app_password].filter(
      Boolean,
    ).length;
    return filled > 0 && filled < 3;
  });
</script>

<div class="settings-section">
  {#if part === "channels"}
    <!-- Discord -->
    <div class="settings-group">
      <div class="settings-group-label">Discord</div>
      <div class="integration-card">
        <div class="integration-desc">
          Connect a Discord bot so your agent can chat in DMs and, when @mentioned, in server
          channels. Create a bot at <a
            href="https://discord.com/developers/applications"
            target="_blank"
            rel="noopener">discord.com/developers</a
          >.
        </div>
        <div class="settings-field">
          <label for="integ-discord-token">Bot Token</label>
          {#if isSecretReference(fields.discord_token)}
            <div class="secret-stored">
              <span class="secret-badge">Stored securely</span>
              <button
                class="btn btn-sm btn-secondary"
                onclick={() => {
                  fields.discord_token = "";
                }}>Change</button
              >
            </div>
          {:else if isEnvReference(fields.discord_token)}
            <div class="secret-stored">
              <span class="secret-badge"
                >From environment variable {envReferenceName(fields.discord_token)}</span
              >
              <button
                class="btn btn-sm btn-secondary"
                onclick={() => {
                  fields.discord_token = "";
                }}>Replace</button
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
        <div class="settings-field">
          <label>
            <span class="toggle-switch">
              <input type="checkbox" bind:checked={fields.discord_respond_to_others} />
              <span class="toggle-slider"></span>
            </span>
            Let others use the agent
          </label>
          <span class="field-hint"
            >Off: only you (the first person to DM the bot). On: anyone who can message the bot can
            use the agent.</span
          >
        </div>
        <div class="settings-field">
          <label for="integ-discord-context-messages">Context messages</label>
          <input
            id="integ-discord-context-messages"
            type="number"
            min="0"
            bind:value={fields.discord_context_messages}
            placeholder="Default: 20"
          />
          <span class="field-hint"
            >Earlier unmentioned server messages shared with the agent when it's @mentioned.</span
          >
        </div>
      </div>
    </div>

    <!-- Telegram -->
    <div class="settings-group">
      <div class="settings-group-label">Telegram</div>
      <div class="integration-card">
        <div class="integration-desc">
          Connect a Telegram bot to chat privately and, when mentioned, in groups. Create a bot via <a
            href="https://t.me/BotFather"
            target="_blank"
            rel="noopener">@BotFather</a
          >.
        </div>
        <div class="settings-field">
          <label for="integ-telegram-token">Bot Token</label>
          {#if isSecretReference(fields.telegram_token)}
            <div class="secret-stored">
              <span class="secret-badge">Stored securely</span>
              <button
                class="btn btn-sm btn-secondary"
                onclick={() => {
                  fields.telegram_token = "";
                }}>Change</button
              >
            </div>
          {:else if isEnvReference(fields.telegram_token)}
            <div class="secret-stored">
              <span class="secret-badge"
                >From environment variable {envReferenceName(fields.telegram_token)}</span
              >
              <button
                class="btn btn-sm btn-secondary"
                onclick={() => {
                  fields.telegram_token = "";
                }}>Replace</button
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
        <div class="settings-field">
          <label>
            <span class="toggle-switch">
              <input type="checkbox" bind:checked={fields.telegram_respond_to_others} />
              <span class="toggle-slider"></span>
            </span>
            Let others use the agent
          </label>
          <span class="field-hint"
            >Off: only you (the first person to DM the bot). On: anyone who can message the bot can
            use the agent.</span
          >
        </div>
        <div class="settings-field">
          <label for="integ-telegram-context-messages">Context messages</label>
          <input
            id="integ-telegram-context-messages"
            type="number"
            min="0"
            bind:value={fields.telegram_context_messages}
            placeholder="Default: 20"
          />
          <span class="field-hint"
            >Earlier unmentioned group messages shared with the agent when addressed. Requires
            privacy mode off in BotFather (or the bot made a group admin).</span
          >
        </div>
      </div>
    </div>

    <!-- Microsoft Teams -->
    <div class="settings-group">
      <div class="settings-group-label">Microsoft Teams</div>
      <div class="integration-card">
        <div class="integration-desc">
          Connect a Microsoft Teams bot so your agent can chat in DMs, group chats, and channels.
          Register a bot in the <a
            href="https://dev.teams.microsoft.com/bots"
            target="_blank"
            rel="noopener">Teams Developer Portal</a
          >, then point its messaging endpoint at a tunnel to this machine's Teams port. See the
          Teams setup guide in the docs.
        </div>
        <div class="settings-field">
          <label for="integ-teams-app-id">App ID</label>
          <input
            id="integ-teams-app-id"
            type="text"
            bind:value={fields.teams_app_id}
            placeholder="11111111-2222-3333-4444-555555555555"
          />
        </div>
        <div class="settings-field">
          <label for="integ-teams-tenant-id">Tenant ID</label>
          <input
            id="integ-teams-tenant-id"
            type="text"
            bind:value={fields.teams_tenant_id}
            placeholder="Directory (tenant) ID"
          />
        </div>
        <div class="settings-field">
          <label for="integ-teams-app-password">Client Secret</label>
          {#if isSecretReference(fields.teams_app_password)}
            <div class="secret-stored">
              <span class="secret-badge">Stored securely</span>
              <button
                class="btn btn-sm btn-secondary"
                onclick={() => {
                  fields.teams_app_password = "";
                }}>Change</button
              >
            </div>
          {:else if isEnvReference(fields.teams_app_password)}
            <div class="secret-stored">
              <span class="secret-badge"
                >From environment variable {envReferenceName(fields.teams_app_password)}</span
              >
              <button
                class="btn btn-sm btn-secondary"
                onclick={() => {
                  fields.teams_app_password = "";
                }}>Replace</button
              >
            </div>
          {:else}
            <input
              id="integ-teams-app-password"
              type="password"
              bind:value={fields.teams_app_password}
              placeholder="Client secret"
            />
          {/if}
        </div>
        {#if teamsPartiallyFilled}
          <div class="validation-msg error">
            App ID, Tenant ID, and Client Secret are all required to enable Teams. Fill in all
            three, or clear them to leave Teams unconfigured.
          </div>
        {/if}
        <div class="settings-field">
          <label>
            <span class="toggle-switch">
              <input type="checkbox" bind:checked={fields.teams_respond_to_others} />
              <span class="toggle-slider"></span>
            </span>
            Let others use the agent
          </label>
          <span class="field-hint"
            >Off: only you (the first person to DM the bot). On: coworkers can @mention or message
            it too.</span
          >
        </div>
        <div class="settings-field">
          <label for="integ-teams-context-messages">Context messages</label>
          <input
            id="integ-teams-context-messages"
            type="number"
            min="0"
            bind:value={fields.teams_context_messages}
            placeholder="Default: 20"
          />
          <span class="field-hint"
            >Earlier group chat messages shared with the agent when it's @mentioned.</span
          >
        </div>
        <div class="settings-field">
          <label for="integ-teams-port">Listener port</label>
          <input
            id="integ-teams-port"
            type="number"
            bind:value={fields.teams_port}
            placeholder="Default: 7701"
          />
          <span class="field-hint">Expose only this port through your tunnel.</span>
        </div>
      </div>
    </div>
  {/if}

  {#if part === "webhooks"}
    <!-- Webhooks -->
    <div class="settings-group">
      <div class="settings-group-label">Webhooks</div>
      <div class="integration-card">
        <div class="integration-desc">
          Named HTTP webhook endpoints for external integrations. Each webhook gets its own
          <code>/webhook/&lbrace;agent&rbrace;/&lbrace;name&rbrace;</code> route with independent auth
          and payload handling.
        </div>

        {#each fields.webhooks as wh, i (i)}
          <div class="webhook-entry">
            <div class="webhook-entry-header">
              <span class="webhook-entry-label">
                {wh.name ? `/webhook/${agent ?? "agent"}/${wh.name}` : "New webhook"}
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
                {#if isSecretReference(wh.secret)}
                  <div class="secret-stored">
                    <span class="secret-badge">Stored securely</span>
                    <button
                      class="btn btn-sm btn-secondary"
                      onclick={() => {
                        wh.secret = "";
                      }}>Change</button
                    >
                  </div>
                {:else if isEnvReference(wh.secret)}
                  <div class="secret-stored">
                    <span class="secret-badge"
                      >From environment variable {envReferenceName(wh.secret)}</span
                    >
                    <button
                      class="btn btn-sm btn-secondary"
                      onclick={() => {
                        wh.secret = "";
                      }}>Replace</button
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
  {/if}

  {#if part === "tools"}
    <!-- Skills -->
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

    <!-- Tools -->
    <div class="settings-group">
      <div class="settings-group-label">Tools</div>
      <div class="integration-card">
        <div class="integration-desc">
          Extra directories prepended to the PATH of spawned commands (the exec tool and MCP stdio
          servers). Drop static binaries in here to make them available without rebuilding.
          <code>~/.residuum/hub/bin</code> is always included.
        </div>
        {#each fields.tools_path as dir, i (dir)}
          <div class="skill-dir-entry">
            <span class="skill-dir-path">{dir}</span>
            <button class="btn btn-sm btn-danger" onclick={() => removeToolDir(i)}>Remove</button>
          </div>
        {/each}
        <div class="skill-dir-add">
          <input
            type="text"
            bind:value={newToolDir}
            placeholder="Path to tools directory"
            onkeydown={(e) => {
              if (e.key === "Enter") addToolDir();
            }}
          />
          <button class="btn btn-sm btn-secondary" onclick={addToolDir}>Add</button>
        </div>
      </div>
    </div>

    <!-- Web Search -->
    <div class="settings-group">
      <div class="settings-group-label">Web Search</div>
      <div class="integration-card">
        <div class="integration-desc">
          Choose a dedicated web search backend. The agent will use this service for all web search
          tool calls. Leave set to "None" to rely on provider-native search only.
        </div>
        <div class="settings-field">
          <label for="ws-backend">Backend</label>
          <select id="ws-backend" bind:value={fields.ws_backend}>
            <option value="">None</option>
            <option value="brave">Brave</option>
            <option value="tavily">Tavily</option>
            <option value="ollama">Ollama Cloud</option>
          </select>
        </div>

        {#if fields.ws_backend === "brave"}
          <div class="settings-field">
            <label for="ws-brave-key">Brave API Key</label>
            {#if isSecretReference(fields.ws_brave_api_key)}
              <div class="secret-stored">
                <span class="secret-badge">Stored securely</span>
                <button
                  class="btn btn-sm btn-secondary"
                  onclick={() => {
                    fields.ws_brave_api_key = "";
                  }}>Change</button
                >
              </div>
            {:else if isEnvReference(fields.ws_brave_api_key)}
              <div class="secret-stored">
                <span class="secret-badge"
                  >From environment variable {envReferenceName(fields.ws_brave_api_key)}</span
                >
                <button
                  class="btn btn-sm btn-secondary"
                  onclick={() => {
                    fields.ws_brave_api_key = "";
                  }}>Replace</button
                >
              </div>
            {:else}
              <input
                id="ws-brave-key"
                type="password"
                bind:value={fields.ws_brave_api_key}
                placeholder="Brave Search API key"
              />
            {/if}
            <span class="field-hint"
              >Get a key at <a href="https://brave.com/search/api/" target="_blank" rel="noopener"
                >brave.com/search/api</a
              ></span
            >
          </div>
        {/if}

        {#if fields.ws_backend === "tavily"}
          <div class="settings-field">
            <label for="ws-tavily-key">Tavily API Key</label>
            {#if isSecretReference(fields.ws_tavily_api_key)}
              <div class="secret-stored">
                <span class="secret-badge">Stored securely</span>
                <button
                  class="btn btn-sm btn-secondary"
                  onclick={() => {
                    fields.ws_tavily_api_key = "";
                  }}>Change</button
                >
              </div>
            {:else if isEnvReference(fields.ws_tavily_api_key)}
              <div class="secret-stored">
                <span class="secret-badge"
                  >From environment variable {envReferenceName(fields.ws_tavily_api_key)}</span
                >
                <button
                  class="btn btn-sm btn-secondary"
                  onclick={() => {
                    fields.ws_tavily_api_key = "";
                  }}>Replace</button
                >
              </div>
            {:else}
              <input
                id="ws-tavily-key"
                type="password"
                bind:value={fields.ws_tavily_api_key}
                placeholder="Tavily API key"
              />
            {/if}
            <span class="field-hint"
              >Get a key at <a href="https://tavily.com" target="_blank" rel="noopener"
                >tavily.com</a
              ></span
            >
          </div>
        {/if}

        {#if fields.ws_backend === "ollama"}
          <div class="settings-field">
            <label for="ws-ollama-key">Ollama API Key</label>
            {#if isSecretReference(fields.ws_ollama_api_key)}
              <div class="secret-stored">
                <span class="secret-badge">Stored securely</span>
                <button
                  class="btn btn-sm btn-secondary"
                  onclick={() => {
                    fields.ws_ollama_api_key = "";
                  }}>Change</button
                >
              </div>
            {:else if isEnvReference(fields.ws_ollama_api_key)}
              <div class="secret-stored">
                <span class="secret-badge"
                  >From environment variable {envReferenceName(fields.ws_ollama_api_key)}</span
                >
                <button
                  class="btn btn-sm btn-secondary"
                  onclick={() => {
                    fields.ws_ollama_api_key = "";
                  }}>Replace</button
                >
              </div>
            {:else}
              <input
                id="ws-ollama-key"
                type="password"
                bind:value={fields.ws_ollama_api_key}
                placeholder="Ollama Cloud API key (optional)"
              />
            {/if}
          </div>
          <div class="settings-field">
            <label for="ws-ollama-url">Base URL</label>
            <input
              id="ws-ollama-url"
              type="text"
              bind:value={fields.ws_ollama_base_url}
              placeholder="https://api.ollama.com"
            />
            <span class="field-hint">Override the default Ollama Cloud endpoint.</span>
          </div>
        {/if}
      </div>
    </div>

    <!-- Web Search: Provider-Native Overrides -->
    <div class="settings-group">
      <div class="settings-group-label">
        <button
          class="collapsible-header"
          onclick={() => {
            nativeOverridesOpen = !nativeOverridesOpen;
          }}
        >
          <span class="collapse-icon">{nativeOverridesOpen ? "\u25BC" : "\u25B6"}</span>
          Provider-Native Search Overrides
        </button>
      </div>

      {#if nativeOverridesOpen}
        <div class="integration-card">
          <div class="integration-desc">
            Fine-tune how each LLM provider handles web search when using its built-in search
            capability. These settings apply regardless of the standalone backend above.
          </div>

          <div class="settings-group-label native-sub">Anthropic</div>
          <div class="settings-field">
            <label for="ws-anthropic-max-uses">Max Uses Per Turn</label>
            <input
              id="ws-anthropic-max-uses"
              type="number"
              bind:value={fields.ws_anthropic_max_uses}
              placeholder="e.g. 5"
              min="1"
            />
            <span class="field-hint"
              >Maximum number of web searches Anthropic can perform per turn.</span
            >
          </div>
          <div class="settings-field">
            <label for="ws-anthropic-allowed">Allowed Domains</label>
            <input
              id="ws-anthropic-allowed"
              type="text"
              bind:value={fields.ws_anthropic_allowed_domains}
              placeholder="example.com, docs.rs"
            />
            <span class="field-hint">Comma-separated list of domains to restrict searches to.</span>
          </div>
          <div class="settings-field">
            <label for="ws-anthropic-blocked">Blocked Domains</label>
            <input
              id="ws-anthropic-blocked"
              type="text"
              bind:value={fields.ws_anthropic_blocked_domains}
              placeholder="reddit.com, pinterest.com"
            />
            <span class="field-hint"
              >Comma-separated list of domains to exclude from search results.</span
            >
          </div>

          <div class="settings-group-label native-sub">OpenAI</div>
          <div class="settings-field">
            <label for="ws-openai-ctx">Search Context Size</label>
            <select id="ws-openai-ctx" bind:value={fields.ws_openai_search_context_size}>
              <option value="">Default</option>
              <option value="low">Low</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
            </select>
            <span class="field-hint">Amount of search context included in OpenAI responses.</span>
          </div>

          <div class="settings-group-label native-sub">Gemini</div>
          <div class="settings-field">
            <label for="ws-gemini-exclude">Exclude Domains</label>
            <input
              id="ws-gemini-exclude"
              type="text"
              bind:value={fields.ws_gemini_exclude_domains}
              placeholder="example.com, spam-site.net"
            />
            <span class="field-hint"
              >Comma-separated list of domains Gemini should never search.</span
            >
          </div>
        </div>
      {/if}
    </div>
  {/if}
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

  .collapsible-header {
    background: none;
    border: none;
    color: inherit;
    font: inherit;
    cursor: pointer;
    padding: 0;
    display: flex;
    align-items: center;
    gap: 6px;
  }

  .collapsible-header:hover {
    color: var(--accent);
  }

  .collapse-icon {
    font-size: 0.75em;
    width: 1em;
    display: inline-block;
  }

  .native-sub {
    margin-top: 12px;
    font-size: 0.85em;
    color: var(--text-dim);
    border-bottom: 1px solid var(--border);
    padding-bottom: 4px;
  }
</style>
