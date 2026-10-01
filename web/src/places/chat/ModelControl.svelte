<script lang="ts">
  import { untrack } from "svelte";
  import { MediaQuery } from "svelte/reactivity";
  import { Icon } from "../../lib/icons";
  import { router } from "../../lib/router.svelte";
  import { Button, Popover, Sheet, type FloatTriggerProps } from "../../lib/ui";
  import { ws } from "../../lib/ws.svelte";
  import { PHONE_QUERY } from "../../styles/breakpoints";
  import { COMPOSER_THINKING, MainModel, modelControlLabel } from "./main-model.svelte";

  // The composer's model and thinking control (design §4): the agent's main
  // model and how hard it thinks, opened beside the message box in a popover,
  // or in a sheet on phones. A change is written through the config write
  // coordinator, and the agent reloads to take it up from its next reply.

  let { agent }: { agent: string } = $props();

  const uid = $props.id();
  const phone = new MediaQuery(PHONE_QUERY);
  const main = untrack(() => new MainModel(agent, () => ws.send({ type: "reload" })));
  $effect(() => main.follow());

  let open = $state(false);

  const triggerText = $derived.by(() => {
    if (main.loadError !== null) return "Model";
    if (!main.loaded) return "Loading model…";
    if (main.value === "") return "Choose a model";
    return modelControlLabel(main.modelName, main.thinking);
  });

  /** The provider's models, with the one in use first when its list doesn't have it. */
  const shownModels = $derived(
    main.models.some((entry) => entry.id === main.model)
      ? main.models
      : [{ id: main.model, name: main.model }, ...main.models],
  );

  const sheetTrigger = $derived<FloatTriggerProps>({
    "aria-haspopup": "dialog",
    "aria-expanded": open,
    "aria-controls": undefined,
    onclick: () => {
      open = true;
    },
  });

  function moreSettings(): void {
    open = false;
    void router.openSettings({ scope: agent, section: "model" });
  }

  // Up and Down move along the models; choosing one is a press.
  function moveAlong(event: KeyboardEvent & { currentTarget: HTMLElement }): void {
    if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
    const options = [...event.currentTarget.querySelectorAll<HTMLElement>("button")];
    const at = options.indexOf(document.activeElement as HTMLElement);
    if (at < 0) return;
    event.preventDefault();
    const step = event.key === "ArrowDown" ? 1 : -1;
    options[(at + step + options.length) % options.length]?.focus();
  }
</script>

{#snippet trigger(props: FloatTriggerProps)}
  <button
    type="button"
    class="model-trigger"
    aria-label="Model: {triggerText}"
    aria-busy={main.saving || undefined}
    {...props}
  >
    <span class="model-trigger-text">{triggerText}</span>
    <Icon name="chevron-down" size={14} />
  </button>
{/snippet}

{#snippet choices()}
  <div class="model-choices" aria-busy={main.saving || undefined}>
    {#if main.loadError !== null}
      <p class="model-problem" role="alert">{main.loadError}</p>
      <Button size="sm" icon="reload" onclick={() => void main.load()}>Try again</Button>
    {:else if main.loaded && main.value === ""}
      <p class="model-note">{agent} has no main model yet. Choose one in its Model settings.</p>
    {:else if main.loaded}
      <div class="model-group" role="group" aria-labelledby="{uid}-models">
        <p class="model-heading" id="{uid}-models">Model, from {main.providerLabel}</p>
        <!-- svelte-ignore a11y_no_static_element_interactions -->
        <div class="model-list" onkeydown={moveAlong}>
          {#each shownModels as entry (entry.id)}
            {@const chosen = entry.id === main.model}
            <button
              type="button"
              class="model-option"
              aria-pressed={chosen}
              data-autofocus={chosen ? "" : undefined}
              onclick={() => void main.choose(entry.id)}
            >
              <span class="model-option-name">{entry.name || entry.id}</span>
              {#if chosen}<Icon name="check" size={15} />{/if}
            </button>
          {/each}
        </div>
        {#if main.listError !== null}
          <p class="model-note">
            Couldn't load {main.providerLabel}'s models, so these are common ones.
          </p>
        {/if}
      </div>
      <div class="model-group" role="group" aria-labelledby="{uid}-thinking">
        <p class="model-heading" id="{uid}-thinking">Thinking</p>
        <div class="thinking-levels">
          {#each COMPOSER_THINKING as level (level.value)}
            <button
              type="button"
              class="thinking-level"
              aria-pressed={main.thinking === level.value}
              onclick={() => void main.toggleThinking(level.value)}>{level.label}</button
            >
          {/each}
        </div>
        <p class="model-note">
          {main.thinking === ""
            ? `No level chosen, so ${agent} uses its default.`
            : "Press the chosen level again to use the default."}
        </p>
      </div>
    {/if}
    <p class="model-note">
      Applies to {agent} from its next reply.
      <button type="button" class="model-more" onclick={moreSettings}>More model settings</button>
    </p>
  </div>
{/snippet}

{#if phone.current}
  {@render trigger(sheetTrigger)}
  <Sheet bind:open title="Model for {agent}">
    {@render choices()}
  </Sheet>
{:else}
  <Popover bind:open label="Model for {agent}" side="top" {trigger}>
    {@render choices()}
  </Popover>
{/if}

<style>
  .model-trigger {
    display: flex;
    flex: 0 1 auto;
    align-items: center;
    gap: var(--space-6);
    min-width: 0;
    height: 32px;
    padding: 0 var(--space-6) 0 var(--space-10);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover,
    &[aria-expanded="true"] {
      background: var(--color-stone-3);
      color: var(--color-text);
    }

    & :global(svg) {
      flex: none;
    }
  }

  .model-trigger-text {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .model-choices {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
  }

  .model-group {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
  }

  .model-heading,
  .model-note {
    color: var(--color-text-3);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-ui);
  }

  .model-heading {
    padding: 0 var(--space-6);
  }

  .model-problem {
    color: var(--color-err-text);
    font-size: var(--font-size-sm);
  }

  .model-list {
    display: flex;
    flex-direction: column;
    max-height: 240px;
    overflow-y: auto;
  }

  .model-option {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    min-height: 34px;
    padding: 0 var(--space-8);
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    text-align: start;

    &:hover {
      background: var(--color-stone-4);
      color: var(--color-text);
    }

    &[aria-pressed="true"] {
      color: var(--color-text);
    }

    & :global(svg) {
      flex: none;
      margin-left: auto;
      color: var(--color-vein-bright);
    }
  }

  .model-option-name {
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .thinking-levels {
    display: grid;
    grid-template-columns: repeat(4, 1fr);
    gap: var(--space-2);
    padding: var(--space-2);
    border-radius: var(--corner-sm);
    background: var(--color-stone-4);
  }

  .thinking-level {
    height: 28px;
    border-radius: var(--corner-sm);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    transition:
      background-color var(--duration-fast) var(--ease-out),
      color var(--duration-fast) var(--ease-out);

    &:hover {
      color: var(--color-text);
    }

    &[aria-pressed="true"] {
      background: var(--color-stone-2);
      color: var(--color-text);
      font-weight: var(--font-weight-medium);
    }
  }

  .model-more {
    color: var(--color-vein-bright);
    text-decoration: underline;
    text-decoration-color: var(--color-vein-line);
    text-underline-offset: 2px;

    &:hover {
      text-decoration-color: currentcolor;
    }
  }

  @media (max-width: 760px) {
    .model-trigger {
      height: var(--layout-touch-target);
    }

    .model-option,
    .thinking-level {
      min-height: var(--layout-touch-target);
    }

    .model-list {
      max-height: none;
    }
  }
</style>
