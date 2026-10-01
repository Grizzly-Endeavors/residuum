<script lang="ts">
  import { onMount, tick } from "svelte";
  import { fetchMcpCatalog } from "../../lib/api";
  import { userErrorMessage } from "../../lib/errors";
  import { serverFromCatalog } from "../../lib/mcp-form";
  import type { McpCatalogEntry, McpServerEntry } from "../../lib/types";
  import { Badge, Banner, Button, EmptyState, Skeleton, TextField } from "../../lib/ui";

  // Common tool servers, added with a press. One that needs a key asks for
  // it first. A catalog that can't be read says why, with Try again.

  interface Props {
    /** Whether the form already has a server of this name. */
    isAdded: (name: string) => boolean;
    onadd: (server: McpServerEntry) => void;
  }

  let { isAdded, onadd }: Props = $props();

  let catalog = $state.raw<McpCatalogEntry[] | null>(null);
  let loadError = $state<string | null>(null);
  let loading = $state(false);
  /** The entry asking for its inputs. */
  let asking = $state<string | null>(null);
  let inputs = $state<Record<string, string>>({});
  let tried = $state(false);
  let list = $state<HTMLElement>();

  /** Read the catalog. A failed read stays on screen while Try again reads it once more. */
  async function load(): Promise<void> {
    loading = true;
    try {
      catalog = await fetchMcpCatalog();
      loadError = null;
    } catch (err) {
      loadError = userErrorMessage(err, { action: "Couldn't read the tool server catalog." });
    } finally {
      loading = false;
    }
  }

  onMount(() => {
    void load();
  });

  const blank = (field: string): boolean => (inputs[field] ?? "").trim() === "";

  async function focusEntry(name: string, selector: string): Promise<void> {
    await tick();
    const row = list?.querySelector<HTMLElement>(`[data-entry="${CSS.escape(name)}"]`);
    (row?.querySelector<HTMLElement>(selector) ?? row)?.focus();
  }

  function begin(entry: McpCatalogEntry): void {
    if (entry.requires_input.length === 0) {
      add(entry);
      return;
    }
    asking = entry.name;
    inputs = Object.fromEntries(entry.requires_input.map((input) => [input.field, ""]));
    tried = false;
    void focusEntry(entry.name, "input");
  }

  function add(entry: McpCatalogEntry): void {
    onadd(serverFromCatalog(entry, inputs));
    asking = null;
    void focusEntry(entry.name, ".entry-name");
  }

  function confirm(entry: McpCatalogEntry): void {
    tried = true;
    if (entry.requires_input.some((input) => blank(input.field))) return;
    add(entry);
  }
</script>

{#if loadError !== null}
  <Banner tone="error">
    {loadError}
    {#snippet actions()}
      <Button size="sm" {loading} onclick={() => void load()}>Try again</Button>
    {/snippet}
  </Banner>
{:else if catalog === null}
  <Skeleton lines={3} label="Loading the catalog" />
{:else if catalog.length === 0}
  <EmptyState>The catalog has no servers in it.</EmptyState>
{:else}
  <ul class="entries" aria-label="Catalog" bind:this={list}>
    {#each catalog as entry (entry.name)}
      <li class="entry" data-entry={entry.name}>
        <div class="entry-head">
          <div class="entry-text">
            <span class="entry-name" tabindex="-1">{entry.name}</span>
            <span class="entry-desc">{entry.description}</span>
            {#if entry.install_hint}<span class="entry-hint">{entry.install_hint}</span>{/if}
          </div>
          {#if isAdded(entry.name)}
            <Badge tone="positive" dot>Added</Badge>
          {:else if asking !== entry.name}
            <Button
              size="sm"
              icon="plus"
              aria-label="Add {entry.name}"
              onclick={() => begin(entry)}
            >
              Add
            </Button>
          {/if}
        </div>
        {#if asking === entry.name && !isAdded(entry.name)}
          <form
            class="entry-inputs"
            aria-label="Add {entry.name}"
            onsubmit={(event) => {
              event.preventDefault();
              confirm(entry);
            }}
          >
            {#each entry.requires_input as input (input.field)}
              <TextField
                label={input.label}
                code
                autocomplete="off"
                spellcheck="false"
                bind:value={inputs[input.field]}
                error={tried && blank(input.field) ? `Enter the ${input.label}.` : undefined}
              />
            {/each}
            <p class="entry-note">
              Saved in mcp.json as you type it. To keep a key out of the file, save it under Saved
              keys for all agents and enter <code>{"${agent-key:its_name}"}</code> here instead.
            </p>
            <div class="entry-actions">
              <Button variant="quiet" onclick={() => (asking = null)}>Cancel</Button>
              <Button type="submit" variant="primary">Add {entry.name}</Button>
            </div>
          </form>
        {/if}
      </li>
    {/each}
  </ul>
{/if}

<style>
  .entries {
    display: flex;
    flex-direction: column;
    list-style: none;
  }

  .entry {
    padding: var(--space-12) 0;

    & + & {
      border-top: 1px solid var(--color-line-soft);
    }
  }

  .entry-head {
    display: flex;
    align-items: center;
    gap: var(--space-12);
  }

  .entry-text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .entry-name {
    font-weight: var(--font-weight-medium);
  }

  .entry-desc {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .entry-hint,
  .entry-note {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
  }

  .entry-inputs {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    margin-top: var(--space-12);
  }

  .entry-actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-8);
  }
</style>
