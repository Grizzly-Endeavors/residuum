<script lang="ts">
  import { thinkingChoices } from "../../lib/model-roles";
  import { numberOfText, textOfNumber } from "../../lib/settings-bind";
  import type { FieldRef } from "../../lib/settings-fields";
  import { NumberField, SegmentedControl } from "../../lib/ui";
  import { fieldMark } from "./field-focus";
  import { fieldError, type SettingsScope } from "./sections";

  // How hard a model thinks and how varied its replies are, for one role or
  // for every model. The form keeps both as text: empty thinking is Default,
  // and an empty temperature leaves it to whatever comes next.

  interface Props {
    scope: SettingsScope;
    thinking: string;
    temperature: string;
    /** The fields these save to, for their problems and focus. */
    thinkingRef: FieldRef;
    temperatureRef: FieldRef;
    /** What Default means here. */
    thinkingHint: string;
    /** What an empty temperature means here. */
    temperatureHint: string;
    /** The temperature used while the box is empty, when one is known. */
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
  /* Side by side when both fit, the temperature under the thinking when they don't. */
  .model-tuning {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-16) var(--space-24);
    container-type: inline-size;
  }

  /* On a phone the five levels keep their touch height and fit one line. */
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
