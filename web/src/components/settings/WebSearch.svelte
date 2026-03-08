<script lang="ts">
  import type { ConfigFields } from "../../lib/settings-toml";

  let { fields = $bindable() }: { fields: ConfigFields } = $props();

  let nativeOverridesOpen = $state(false);
</script>

<div class="settings-section">
  <div class="settings-group">
    <div class="settings-group-label">Standalone Backend</div>
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
          {#if fields.ws_brave_api_key.startsWith("secret:")}
            <div class="secret-stored">
              <span class="secret-badge">Stored securely</span>
              <button
                class="btn btn-sm btn-secondary"
                onclick={() => {
                  fields.ws_brave_api_key = "";
                }}>Change</button
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
          {#if fields.ws_tavily_api_key.startsWith("secret:")}
            <div class="secret-stored">
              <span class="secret-badge">Stored securely</span>
              <button
                class="btn btn-sm btn-secondary"
                onclick={() => {
                  fields.ws_tavily_api_key = "";
                }}>Change</button
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
            >Get a key at <a href="https://tavily.com" target="_blank" rel="noopener">tavily.com</a
            ></span
          >
        </div>
      {/if}

      {#if fields.ws_backend === "ollama"}
        <div class="settings-field">
          <label for="ws-ollama-key">Ollama API Key</label>
          {#if fields.ws_ollama_api_key.startsWith("secret:")}
            <div class="secret-stored">
              <span class="secret-badge">Stored securely</span>
              <button
                class="btn btn-sm btn-secondary"
                onclick={() => {
                  fields.ws_ollama_api_key = "";
                }}>Change</button
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

  <div class="settings-group">
    <div class="settings-group-label">
      <button
        class="collapsible-header"
        onclick={() => {
          nativeOverridesOpen = !nativeOverridesOpen;
        }}
      >
        <span class="collapse-icon">{nativeOverridesOpen ? "\u25BC" : "\u25B6"}</span>
        Provider-Native Overrides
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
          <span class="field-hint">Comma-separated list of domains Gemini should never search.</span
          >
        </div>
      </div>
    {/if}
  </div>
</div>

<style>
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
