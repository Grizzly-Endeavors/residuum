<script lang="ts">
  import { tick } from "svelte";
  import { notifyStagedRemoval } from "../../lib/form-undo";
  import { numberOfText, textOfNumber } from "../../lib/settings-bind";
  import type { PriceFormEntry, PriceRate } from "../../lib/settings-toml";
  import { Button, EmptyState, NumberField, TextField } from "../../lib/ui";
  import { fieldError, type AllSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // Estimated costs use these prices in place of the built-in ones, for the
  // model each row names. The hub writes every rate of a changed row at once,
  // and the hub's validation refuses a row missing one.

  let { scope, section }: AllSectionProps = $props();

  const uid = $props.id();
  let list = $state<HTMLElement>();

  const RATES: readonly { rate: PriceRate; label: string }[] = [
    { rate: "input", label: "Input" },
    { rate: "output", label: "Output" },
    { rate: "cache_read", label: "Cache read" },
    { rate: "cache_write", label: "Cache write, 5 min" },
    { rate: "cache_write_long", label: "Cache write, 1 hour" },
  ];

  function model(price: PriceFormEntry): string {
    return price.name.trim() || "New price";
  }

  /** What the row's name needs: a name, unique among the rows, and no problem the last save found. */
  function nameProblem(price: PriceFormEntry): string | undefined {
    const name = price.name.trim();
    if (name === "") return "Enter a provider and model, such as anthropic/claude-sonnet-5-5.";
    if (scope.config.pricing.filter((other) => other.name.trim() === name).length > 1) {
      return "Another row already prices this model. Keep one.";
    }
    return fieldError(scope, { kind: "price", name });
  }

  function rateProblem(price: PriceFormEntry, rate: PriceRate): string | undefined {
    const value = numberOfText(price[rate]);
    if (value !== null && value < 0) return "Use a price of 0 or more.";
    return fieldError(scope, { kind: "price", name: price.name.trim(), field: rate });
  }

  async function add(): Promise<void> {
    scope.config.pricing = [
      ...scope.config.pricing,
      { name: "", input: "", output: "", cache_read: "", cache_write: "", cache_write_long: "" },
    ];
    await tick();
    const names = list?.querySelectorAll<HTMLInputElement>("input[data-price-name]");
    names?.item(names.length - 1).focus();
  }

  function remove(index: number): void {
    const removed = scope.config.pricing[index];
    if (removed === undefined) return;
    scope.config.pricing = scope.config.pricing.filter((_, at) => at !== index);
    notifyStagedRemoval(`Removed ${model(removed)}.`, () => {
      const prices = scope.config.pricing;
      scope.config.pricing = [...prices.slice(0, index), removed, ...prices.slice(index)];
    });
  }
</script>

<SettingsSection
  {scope}
  {section}
  title="Model prices"
  lede="Estimated costs are worked out from token counts and these prices, so they are estimates, not a provider's bill. Leave a model out to use its built-in price."
>
  <SettingsGroup
    title="Price overrides"
    lede="Dollars per million tokens. Name each row provider/model as usage records name it."
  >
    {#if scope.config.pricing.length === 0}
      <EmptyState>
        No overrides. Models use their built-in prices, and a model without one shows no estimate.
      </EmptyState>
    {:else}
      <ul class="price-list" bind:this={list}>
        {#each scope.config.pricing as price, index (index)}
          <li class="price">
            <div class="price-body" role="group" aria-labelledby="{uid}-{index}">
              <div class="price-head">
                <code id="{uid}-{index}" class="price-model">{model(price)}</code>
                <Button
                  variant="danger"
                  size="sm"
                  aria-label="Remove {model(price)}"
                  onclick={() => {
                    remove(index);
                  }}
                >
                  Remove
                </Button>
              </div>
              <TextField
                label="Provider and model"
                bind:value={price.name}
                placeholder="anthropic/claude-sonnet-5-5"
                autocomplete="off"
                spellcheck={false}
                code
                data-price-name=""
                error={nameProblem(price)}
              />
              <div class="price-rates">
                {#each RATES as { rate, label } (rate)}
                  <NumberField
                    {label}
                    min={0}
                    step="any"
                    placeholder="0"
                    error={rateProblem(price, rate)}
                    bind:value={
                      () => numberOfText(price[rate]),
                      (next) => {
                        price[rate] = textOfNumber(next);
                      }
                    }
                  />
                {/each}
              </div>
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
        Add price
      </Button>
    </div>
  </SettingsGroup>
</SettingsSection>

<style>
  .price-list {
    display: flex;
    flex-direction: column;
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .price {
    padding: var(--space-16) 0;
    border-top: 1px solid var(--color-line-soft);
  }

  .price:first-child {
    padding-top: 0;
    border-top: 0;
  }

  .price-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
  }

  .price-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: var(--space-12);
  }

  .price-model {
    min-width: 0;
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
    overflow-wrap: anywhere;
  }

  .price-rates {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(8rem, 1fr));
    gap: var(--space-14) var(--space-12);
  }
</style>
