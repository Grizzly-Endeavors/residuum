<script lang="ts">
  import { tick } from "svelte";
  import { notifyStagedRemoval } from "../../lib/form-undo";
  import type { WebhookFormEntry } from "../../lib/settings-toml";
  import type { AgentScopeModel } from "../../lib/settings-model.svelte";
  import { isStoredReference } from "../../lib/secrets";
  import { Button, EmptyState, SelectField, TextField } from "../../lib/ui";
  import SecretConfigField from "./SecretConfigField.svelte";
  import { fieldError } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";

  // The agent's incoming webhooks: addresses other services post to. Each has
  // its own secret, destination and message format. Removing one is staged
  // like every other edit.

  let { scope }: { scope: AgentScopeModel } = $props();

  const uid = $props.id();
  let list = $state<HTMLElement>();

  const FORMATS = [
    { value: "parsed", label: "Parsed: pick fields out of the JSON" },
    { value: "raw", label: "Raw: pass the body through as it is" },
  ] as const;

  function route(hook: WebhookFormEntry): string {
    const name = hook.name.trim();
    return name === "" ? "New webhook" : `/webhook/${scope.agent}/${name}`;
  }

  /** The secret this webhook has on disk: kept under its name, or the reference it carries while it is renamed. */
  function savedSecret(hook: WebhookFormEntry): string {
    const name = hook.name.trim();
    const onDisk = scope.configFile.baseline.webhooks.find((entry) => entry.name.trim() === name);
    if (onDisk !== undefined) return onDisk.secret;
    return isStoredReference(hook.secret) ? hook.secret : "";
  }

  /** The save's problems with a webhook, or with one of its fields. */
  function problem(
    hook: WebhookFormEntry,
    field?: Exclude<keyof WebhookFormEntry, "name">,
  ): string | undefined {
    const name = hook.name.trim();
    return fieldError(
      scope,
      field === undefined ? { kind: "webhook", name } : { kind: "webhook", name, field },
    );
  }

  async function add(): Promise<void> {
    scope.config.webhooks = [
      ...scope.config.webhooks,
      { name: "", secret: "", routing: "inbox", format: "parsed", content_fields: "" },
    ];
    await tick();
    const names = list?.querySelectorAll<HTMLInputElement>("input[data-hook-name]");
    names?.item(names.length - 1).focus();
  }

  function remove(index: number): void {
    const removed = scope.config.webhooks[index];
    if (removed === undefined) return;
    scope.config.webhooks = scope.config.webhooks.filter((_, at) => at !== index);
    notifyStagedRemoval(`Removed ${removed.name.trim() || "the new webhook"}.`, () => {
      const hooks = scope.config.webhooks;
      scope.config.webhooks = [...hooks.slice(0, index), removed, ...hooks.slice(index)];
    });
  }
</script>

<SettingsGroup
  title="Incoming webhooks"
  lede="Addresses other services can post to. Each message lands in {scope.agent}'s inbox or starts a background session."
>
  {#if scope.config.webhooks.length === 0}
    <EmptyState>No webhooks yet.</EmptyState>
  {:else}
    <ul class="hook-list" bind:this={list}>
      {#each scope.config.webhooks as hook, index (index)}
        <li class="hook">
          <div class="hook-body" role="group" aria-labelledby="{uid}-{index}">
            <div class="hook-head">
              <code id="{uid}-{index}" class="hook-route">{route(hook)}</code>
              <Button
                variant="danger"
                size="sm"
                aria-label="Remove {hook.name.trim() || 'new webhook'}"
                onclick={() => {
                  remove(index);
                }}
              >
                Remove
              </Button>
            </div>
            <TextField
              label="Name"
              bind:value={hook.name}
              placeholder="github-issues"
              autocomplete="off"
              spellcheck={false}
              data-hook-name=""
              error={problem(hook)}
            />
            <SecretConfigField
              label="Secret"
              bind:value={hook.secret}
              saved={savedSecret(hook)}
              placeholder="Optional bearer token"
              hint="Callers send it as a bearer token. Without one, anyone who knows the address can post."
              error={problem(hook, "secret")}
            />
            <TextField
              label="Where it goes"
              bind:value={hook.routing}
              placeholder="inbox"
              autocomplete="off"
              spellcheck={false}
              hint="Type inbox to file each message in the inbox, or agent: and a skill name (agent:code-review) to run that skill in a background session."
              error={problem(hook, "routing")}
            />
            <SelectField
              label="Message format"
              bind:value={
                () => hook.format || "parsed",
                (format) => {
                  hook.format = format;
                }
              }
              options={FORMATS}
              error={problem(hook, "format")}
            />
            {#if hook.format !== "raw"}
              <TextField
                label="Fields to read"
                bind:value={hook.content_fields}
                placeholder="issue.title, issue.body"
                autocomplete="off"
                spellcheck={false}
                hint="Paths into the JSON body, separated by commas."
                error={problem(hook, "content_fields")}
              />
            {/if}
          </div>
        </li>
      {/each}
    </ul>
  {/if}
  <div>
    <Button
      icon="plus"
      onclick={() => {
        void add();
      }}
    >
      Add webhook
    </Button>
  </div>
</SettingsGroup>

<style>
  .hook-list {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .hook {
    padding: var(--space-16) 0;
    border-top: 1px solid var(--color-line-soft);
  }

  .hook-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
  }

  .hook:first-child {
    padding-top: 0;
    border-top: 0;
  }

  .hook-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
  }

  .hook-route {
    min-width: 0;
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
    overflow-wrap: anywhere;
  }
</style>
