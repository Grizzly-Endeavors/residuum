<script lang="ts">
  import { tick } from "svelte";
  import { SvelteSet } from "svelte/reactivity";
  import { notifyStagedRemoval } from "../../lib/form-undo";
  import { serverNameProblem } from "../../lib/mcp-form";
  import type { McpServerEntry } from "../../lib/types";
  import {
    Badge,
    Button,
    EmptyState,
    IconButton,
    SegmentedControl,
    TextField,
    type Choice,
  } from "../../lib/ui";
  import McpCatalog from "./McpCatalog.svelte";
  import McpServerFields from "./McpServerFields.svelte";
  import { fieldError, type AgentSectionProps } from "./sections";
  import SettingsSection from "./SettingsSection.svelte";

  // Advanced → Tool servers: the agent's MCP servers, from its `mcp.json`.
  // Adding, editing and removing a server are all staged in the scope's form
  // and written by Save changes; Discard brings the list back as it was.

  let { scope, section }: AgentSectionProps = $props();

  type Transport = "stdio" | "http";
  const TRANSPORTS: readonly Choice<Transport>[] = [
    { value: "stdio", label: "Command" },
    { value: "http", label: "Web address" },
  ];
  const TRANSPORT_HINTS: Readonly<Record<Transport, string>> = {
    stdio: "Residuum starts it on this computer.",
    http: "It runs somewhere else, and Residuum connects to its address.",
  };

  /** Open servers, by name, so they stay open when a save gives the form new objects. */
  const open = new SvelteSet<string>();

  const blankServer = (): McpServerEntry => ({
    name: "",
    transport: "stdio",
    command: "",
    args: [],
    env: {},
    url: "",
    headers: {},
  });

  let adding = $state(false);
  let draft = $state<McpServerEntry>(blankServer());
  let tried = $state(false);
  let nameField = $state<HTMLInputElement | HTMLTextAreaElement>();
  let addButton = $state<HTMLButtonElement>();

  const draftHttp = $derived(draft.transport === "http");
  const nameProblem = $derived(tried ? serverNameProblem(draft.name, scope.mcpServers) : null);
  const missing = $derived.by(() => {
    if (!tried) return {};
    if (draftHttp) return (draft.url ?? "").trim() === "" ? { url: "Enter its address." } : {};
    return draft.command.trim() === "" ? { command: "Enter the command that starts it." } : {};
  });

  const hasProblems = (name: string): boolean =>
    scope.diagnostics.some((placed) => placed.field?.kind === "mcp" && placed.field.name === name);

  function summary(server: McpServerEntry): string {
    return server.transport === "http"
      ? (server.url ?? "")
      : [server.command, ...server.args].join(" ");
  }

  function remove(server: McpServerEntry): void {
    const servers = scope.mcpServers;
    const at = servers.indexOf(server);
    if (at < 0) return;
    const removed = $state.snapshot(server);
    servers.splice(at, 1);
    notifyStagedRemoval(`Removed ${removed.name}. Save changes to keep it removed.`, () => {
      const now = scope.mcpServers;
      if (!now.some((entry) => entry.name === removed.name)) {
        now.splice(Math.min(at, now.length), 0, removed);
      }
    });
  }

  async function startAdding(): Promise<void> {
    adding = true;
    await tick();
    nameField?.focus();
  }

  async function stopAdding(): Promise<void> {
    adding = false;
    draft = blankServer();
    tried = false;
    await tick();
    addButton?.focus();
  }

  function addDraft(): void {
    tried = true;
    if (nameProblem !== null || Object.keys(missing).length > 0) return;
    const name = draft.name.trim();
    scope.mcpServers.push(
      draftHttp
        ? {
            ...blankServer(),
            name,
            transport: "http",
            url: (draft.url ?? "").trim(),
            headers: draft.headers,
          }
        : {
            ...blankServer(),
            name,
            command: draft.command.trim(),
            args: draft.args,
            env: draft.env,
          },
    );
    void stopAdding();
  }
</script>

<SettingsSection
  {scope}
  {section}
  title="Tool servers"
  lede={`Programs that give ${scope.agent} more tools to use. Changes apply when you save them.`}
>
  <div class="groups">
    <section class="group" aria-labelledby="servers-added">
      <h3 class="group-title" id="servers-added">Servers</h3>
      {#if scope.mcpServers.length === 0}
        <EmptyState>No tool servers yet. Add one, or pick one from the catalog.</EmptyState>
      {:else}
        <ul class="servers" aria-labelledby="servers-added">
          {#each scope.mcpServers as server (server)}
            {@const expanded = open.has(server.name) || hasProblems(server.name)}
            {@const problem = fieldError(scope, { kind: "mcp", name: server.name })}
            <li class="server">
              <div class="server-head">
                <div class="server-text">
                  <span class="server-name">
                    {server.name}
                    {#if server.transport === "http"}<Badge>HTTP</Badge>{/if}
                  </span>
                  <code class="server-line">{summary(server)}</code>
                </div>
                <IconButton
                  icon="edit"
                  label="Edit {server.name}"
                  aria-expanded={expanded}
                  onclick={() => {
                    if (expanded) open.delete(server.name);
                    else open.add(server.name);
                  }}
                />
                <IconButton
                  icon="trash"
                  label="Remove {server.name}"
                  onclick={() => remove(server)}
                />
              </div>
              {#if problem}<p class="server-problem">{problem}</p>{/if}
              {#if expanded}
                <div class="fields">
                  <McpServerFields
                    {server}
                    errorOf={(field) =>
                      fieldError(scope, { kind: "mcp", name: server.name, field })}
                  />
                </div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}

      {#if adding}
        <form
          class="fields add"
          aria-label="Add a tool server"
          onsubmit={(event) => {
            event.preventDefault();
            addDraft();
          }}
        >
          <TextField
            label="Name"
            code
            autocomplete="off"
            spellcheck="false"
            bind:value={draft.name}
            bind:element={nameField}
            error={nameProblem ?? undefined}
          />
          <SegmentedControl
            label="How Residuum reaches it"
            value={draftHttp ? "http" : "stdio"}
            options={TRANSPORTS}
            hint={TRANSPORT_HINTS[draftHttp ? "http" : "stdio"]}
            onchange={(transport) => (draft.transport = transport)}
          />
          <McpServerFields server={draft} {missing} />
          <div class="actions">
            <Button variant="quiet" onclick={() => void stopAdding()}>Cancel</Button>
            <Button type="submit" variant="primary">Add server</Button>
          </div>
        </form>
      {:else}
        <div>
          <Button icon="plus" bind:element={addButton} onclick={() => void startAdding()}>
            Add a server
          </Button>
        </div>
      {/if}
    </section>

    <section class="group" aria-labelledby="servers-catalog">
      <h3 class="group-title" id="servers-catalog">Add from the catalog</h3>
      <McpCatalog
        isAdded={(name) => scope.mcpServers.some((server) => server.name === name)}
        onadd={(server) => scope.mcpServers.push(server)}
      />
    </section>
  </div>
</SettingsSection>

<style>
  .groups {
    display: flex;
    flex-direction: column;
    gap: var(--space-32);
    max-width: 640px;
  }

  .group {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  .group-title {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
  }

  .servers {
    display: flex;
    flex-direction: column;
    list-style: none;
  }

  .server {
    padding: var(--space-8) 0;

    & + & {
      border-top: 1px solid var(--color-line-soft);
    }
  }

  .server-head {
    display: flex;
    align-items: center;
    gap: var(--space-4);
  }

  .server-text {
    display: flex;
    flex: 1;
    flex-direction: column;
    gap: var(--space-2);
    min-width: 0;
  }

  .server-name {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    font-weight: var(--font-weight-medium);
  }

  .server-line {
    overflow: hidden;
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .server-problem {
    margin-top: var(--space-4);
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .fields {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-12) 0 var(--space-8);
  }

  .add {
    padding: var(--space-16);
    border-radius: var(--corner-lg);
    background: var(--color-stone-2);
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    gap: var(--space-8);
  }
</style>
