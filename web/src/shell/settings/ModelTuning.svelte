<script lang="ts">
  import { thinkingChoices } from "../../lib/model-roles";
  import { numberOfText, textOfNumber } from "../../lib/settings-bind";
  import type { FieldRef } from "../../lib/settings-fields";
  import { NumberField, SegmentedControl } from "../../lib/ui";
  import { fieldMark } from "./field-focus";
  import { fieldError, type SettingsScope } from "./sections";

  interface Props {
    scope: SettingsScope;
    thinking: string;
    temperature: string;
    thinkingRef: FieldRef;
    temperatureRef: FieldRef;
    thinkingHint: string;
    temperatureHint: string;
    temperatureFallback?: string;
  }

  let {
    scope,
    thinking = $bindable(),
    temperature = $bindable(),
    thinkingRef,
    temperatureRef,
    thinkingHint,
    temperatureHint,
    temperatureFallback,
  }: Props = $props();
</script>

<div class="model-tuning">
  <div class="model-tuning-thinking" data-field={fieldMark(thinkingRef)}>
    <SegmentedControl
      label="Thinking"
      options={thinkingChoices(thinking)}
      hint={thinkingHint}
      error={fieldError(scope, thinkingRef)}
      bind:value={thinking}
    />
  </div>
  <div class="model-tuning-temperature" data-field={fieldMark(temperatureRef)}>
    <NumberField
      label="Temperature"
      min={0}
      max={2}
      step={0.1}
      placeholder={temperatureFallback}
      hint={temperatureHint}
      error={fieldError(scope, temperatureRef)}
      bind:value={
        () => numberOfText(temperature),
        (next) => {
          temperature = textOfNumber(next);
        }
      }
    />
  </div>
</div>

<style>
  .model-tuning {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-16) var(--space-24);
    container-type: inline-size;
  }

  @container (max-width: 400px) {
    .model-tuning-thinking :global(.ui-segment) {
      padding: 0 var(--space-10);
    }
  }

  .model-tuning-thinking {
    flex: 0 1 auto;
    max-width: 100%;
  }

  .model-tuning-temperature {
    flex: 1 1 200px;
  }
</style>
