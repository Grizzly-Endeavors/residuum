<script lang="ts">
  import { Icon } from "../../lib/icons";
  import { numberOfText, textOfNumber } from "../../lib/settings-bind";
  import type { ConfigFields } from "../../lib/settings-toml";
  import { Disclosure, NumberField, SelectField, TextField, VisuallyHidden } from "../../lib/ui";
  import PathList from "./PathList.svelte";
  import SecretConfigField from "./SecretConfigField.svelte";
  import { fieldError, type AgentSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // The agent's Tools & skills section: the folders it finds skills and
  // programs in, and the service behind its web searches.

  let { scope, section }: AgentSectionProps = $props();

  const agent = $derived(scope.agent);
  const saved = $derived(scope.configFile.baseline);
  const backend = $derived(scope.config.ws_backend);

  const BACKENDS = [
    { value: "", label: "None, use each model provider's own search" },
    { value: "brave", label: "Brave" },
    { value: "tavily", label: "Tavily" },
    { value: "ollama", label: "Ollama Cloud" },
  ] as const;

  const CONTEXT_SIZES = [
    { value: "", label: "Default" },
    { value: "low", label: "Low" },
    { value: "medium", label: "Medium" },
    { value: "high", label: "High" },
  ] as const;

  const problem = (field: keyof ConfigFields): string | undefined =>
    fieldError(scope, { kind: "config", field });
</script>

{#snippet keyLink(href: string, label: string)}
  <a class="key-link" {href} target="_blank" rel="noopener noreferrer">
    Get a key at {label}
    <Icon name="external-link" size={13} />
    <VisuallyHidden>(opens in a new tab)</VisuallyHidden>
  </a>
{/snippet}

<SettingsSection
  {scope}
  {section}
  title="Tools & skills"
  lede="Where {agent} finds skills and programs, and how it searches the web."
>
  <SettingsGroup
    title="Skill folders"
    lede="Extra folders {agent} looks in for skills. Its own skills folder and the team's are always included."
  >
    <PathList
      bind:paths={scope.config.skills_dirs}
      addLabel="Skill folder to add"
      placeholder="/path/to/skills"
      empty="No extra skill folders."
      error={problem("skills_dirs")}
    />
  </SettingsGroup>

  <SettingsGroup
    title="Tool folders"
    lede="Folders added to the PATH of the commands {agent} runs, including its tool servers. Put a program in one and the agent can use it without a rebuild. ~/.residuum/hub/bin is always included."
  >
    <PathList
      bind:paths={scope.config.tools_path}
      addLabel="Tool folder to add"
      placeholder="/path/to/tools"
      empty="No extra tool folders."
      error={problem("tools_path")}
    />
  </SettingsGroup>

  <SettingsGroup
    title="Web search"
    lede="The service {agent} uses for every web search. With none chosen, it relies on each model provider's built-in search."
  >
    <SelectField
      label="Search service"
      bind:value={scope.config.ws_backend}
      options={BACKENDS}
      error={problem("ws_backend")}
    />
    {#if backend === "brave"}
      <SecretConfigField
        label="Brave API key"
        bind:value={scope.config.ws_brave_api_key}
        saved={saved.ws_brave_api_key}
        placeholder="Paste your Brave Search API key"
        error={problem("ws_brave_api_key")}
      />
      {@render keyLink("https://brave.com/search/api/", "brave.com/search/api")}
    {:else if backend === "tavily"}
      <SecretConfigField
        label="Tavily API key"
        bind:value={scope.config.ws_tavily_api_key}
        saved={saved.ws_tavily_api_key}
        placeholder="Paste your Tavily API key"
        error={problem("ws_tavily_api_key")}
      />
      {@render keyLink("https://tavily.com", "tavily.com")}
    {:else if backend === "ollama"}
      <SecretConfigField
        label="Ollama Cloud API key"
        bind:value={scope.config.ws_ollama_api_key}
        saved={saved.ws_ollama_api_key}
        placeholder="Optional"
        error={problem("ws_ollama_api_key")}
      />
      <TextField
        label="Ollama Cloud address"
        bind:value={scope.config.ws_ollama_base_url}
        placeholder="https://api.ollama.com"
        autocomplete="off"
        spellcheck={false}
        code
        hint="Only needed to use a different endpoint than Ollama Cloud's own."
        error={problem("ws_ollama_base_url")}
      />
    {/if}
  </SettingsGroup>

  <Disclosure summary="Search options for each model provider">
    <p class="native-lede">
      Fine-tune how a model provider searches when it uses its own built-in search. These apply
      whichever search service is chosen above.
    </p>
    <SettingsGroup title="Anthropic">
      <NumberField
        label="Searches per reply"
        bind:value={
          () => numberOfText(scope.config.ws_anthropic_max_uses),
          (value) => (scope.config.ws_anthropic_max_uses = textOfNumber(value))
        }
        placeholder="5"
        min={1}
        hint="The most web searches Anthropic can make while writing one reply."
        error={problem("ws_anthropic_max_uses")}
      />
      <TextField
        label="Only search these domains"
        bind:value={scope.config.ws_anthropic_allowed_domains}
        placeholder="example.com, docs.rs"
        autocomplete="off"
        spellcheck={false}
        hint="Separate domains with commas."
        error={problem("ws_anthropic_allowed_domains")}
      />
      <TextField
        label="Never search these domains"
        bind:value={scope.config.ws_anthropic_blocked_domains}
        placeholder="reddit.com, pinterest.com"
        autocomplete="off"
        spellcheck={false}
        hint="Separate domains with commas."
        error={problem("ws_anthropic_blocked_domains")}
      />
    </SettingsGroup>
    <SettingsGroup title="OpenAI">
      <SelectField
        label="Search context size"
        bind:value={scope.config.ws_openai_search_context_size}
        options={CONTEXT_SIZES}
        hint="How much of what it finds OpenAI includes with its reply."
        error={problem("ws_openai_search_context_size")}
      />
    </SettingsGroup>
    <SettingsGroup title="Gemini">
      <TextField
        label="Never search these domains"
        bind:value={scope.config.ws_gemini_exclude_domains}
        placeholder="example.com, spam-site.net"
        autocomplete="off"
        spellcheck={false}
        hint="Separate domains with commas."
        error={problem("ws_gemini_exclude_domains")}
      />
    </SettingsGroup>
  </Disclosure>
</SettingsSection>

<style>
  .key-link {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    align-self: flex-start;
    font-size: var(--font-size-sm);
  }

  .native-lede {
    max-width: 640px;
    margin-bottom: var(--space-16);
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }
</style>
