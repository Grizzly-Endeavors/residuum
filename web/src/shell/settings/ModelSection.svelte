<script lang="ts">
  import { tick, untrack } from "svelte";
  import { hub } from "../../lib/hub.svelte";
  import { JOB_ROLES } from "../../lib/model-roles";
  import type { FieldRef } from "../../lib/settings-fields";
  import type { ModelRoleKey } from "../../lib/types";
  import { Banner, Button, Disclosure } from "../../lib/ui";
  import { agentActions } from "../../places/home/agent-actions.svelte";
  import { focusField } from "./field-focus";
  import ModelChoice from "./ModelChoice.svelte";
  import ModelTuning from "./ModelTuning.svelte";
  import ProvidersGroup from "./ProvidersGroup.svelte";
  import type { AgentSectionProps } from "./sections";
  import SettingsGroup from "./SettingsGroup.svelte";
  import SettingsSection from "./SettingsSection.svelte";

  // The Model section (design §8): the main model and how hard it thinks,
  // thinking and temperature for every model, a model for each job that
  // needs its own, and the providers they come from. Roles are saved to the
  // agent's `providers.toml`, the settings for every model to its
  // `config.toml`. It stays editable while the agent is stopped, and a field
  // asked for on arrival, such as the one Fix settings found, is focused once
  // the section shows.

  let { scope, section }: AgentSectionProps = $props();

  const agent = $derived(scope.agent);
  const models = $derived(scope.models);

  type Tuning = "thinking" | "temperature";

  /** A role's own thinking or temperature, which is saved with its model. */
  const tuningOf = (role: ModelRoleKey, key: Tuning): string => models.overrides[role]?.[key] ?? "";

  function setTuning(role: ModelRoleKey, key: Tuning, next: string): void {
    const own = (models.overrides[role] ??= { thinking: "", temperature: "" });
    own[key] = next;
  }

  const jobHasProblem = $derived(
    scope.diagnostics.some(
      (placed) => placed.field?.kind === "role" && placed.field.role !== "main",
    ),
  );
  // Open when a job has a model of its own, so what is set is never hidden.
  let jobsOpen = $state(untrack(() => JOB_ROLES.some((job) => scope.models[job.key] !== "")));
  $effect(() => {
    if (jobHasProblem) jobsOpen = true;
  });

  const defaultThinking = $derived(scope.config.thinking);
  const defaultTemperature = $derived(scope.config.temperature);
  const thinkingDefaultHint = $derived(
    defaultThinking === ""
      ? "Default leaves it to the model."
      : `Default is ${defaultThinking}, as set under Every model.`,
  );

  let content = $state<HTMLElement>();
  const heldHere = (ref: FieldRef): boolean =>
    ref.kind === "role" ||
    ref.kind === "provider" ||
    (ref.kind === "config" && (ref.field === "thinking" || ref.field === "temperature"));
  $effect(() => {
    const ref = scope.focusRequest;
    const root = content;
    if (ref === null || root === undefined || !heldHere(ref)) return;
    untrack(() => scope.takeFocus());
    if (ref.kind === "role" && ref.role !== "main") jobsOpen = true;
    void tick().then(() => focusField(root, ref));
  });

  const failed = $derived(hub.agent(agent)?.state === "failed");
  const savedWhileFailed = $derived(
    failed && !scope.dirty && scope.lastResult?.outcome === "saved",
  );
  const restarting = $derived(agentActions.pendingOf(agent) === "restart");
</script>

<SettingsSection
  {scope}
  {section}
  title="Model"
  lede="Which model {agent} thinks with, and how hard it thinks before replying."
>
  <div class="model-section" bind:this={content}>
    {#if savedWhileFailed}
      <div class="model-notice">
        <Banner icon="check">
          Saved. {agent} is still stopped. Restart it to use these settings.
          {#snippet actions()}
            <Button
              size="sm"
              icon="reload"
              loading={restarting}
              onclick={() => void agentActions.restart(agent)}
            >
              Restart {agent}
            </Button>
          {/snippet}
        </Banner>
      </div>
    {/if}

    <SettingsGroup title="Main model" lede="What {agent} uses for your conversations with it.">
      <ModelChoice {scope} role="main" />
      {#if models.main !== ""}
        <ModelTuning
          {scope}
          bind:thinking={
            () => tuningOf("main", "thinking"), (next) => setTuning("main", "thinking", next)
          }
          bind:temperature={
            () => tuningOf("main", "temperature"), (next) => setTuning("main", "temperature", next)
          }
          thinkingRef={{ kind: "role", role: "main", field: "thinking" }}
          temperatureRef={{ kind: "role", role: "main", field: "temperature" }}
          thinkingHint="More thinking helps with work in several steps. Replies take longer and cost more. {thinkingDefaultHint}"
          temperatureHint="How much replies vary, from 0 (the most predictable) to 2. Leave it blank for the setting under Every model."
          temperatureFallback={defaultTemperature || undefined}
        />
      {/if}
    </SettingsGroup>

    <SettingsGroup
      title="Every model"
      lede="Thinking and temperature for each model {agent} uses, unless that model sets its own here."
    >
      <ModelTuning
        {scope}
        bind:thinking={scope.config.thinking}
        bind:temperature={scope.config.temperature}
        thinkingRef={{ kind: "config", field: "thinking" }}
        temperatureRef={{ kind: "config", field: "temperature" }}
        thinkingHint="Default leaves it to each model."
        temperatureHint="How much replies vary, from 0 to 2. Leave it blank for each model's own."
      />
    </SettingsGroup>

    <div class="model-jobs">
      <Disclosure summary="Use different models for specific jobs" bind:open={jobsOpen}>
        <SettingsGroup lede="Every job uses the main model unless it's given its own here.">
          <div class="jobs">
            {#each JOB_ROLES as job (job.key)}
              {@const titleId = `model-job-${job.key}`}
              <div class="job" role="group" aria-labelledby={titleId}>
                <div class="job-text">
                  <h3 id={titleId} class="job-title">{job.label}</h3>
                  <p class="job-lede">{job.description.replaceAll("{agent}", agent)}</p>
                </div>
                <ModelChoice {scope} role={job.key} />
                {#if job.key !== "embedding" && models[job.key] !== ""}
                  <ModelTuning
                    {scope}
                    bind:thinking={
                      () => tuningOf(job.key, "thinking"),
                      (next) => setTuning(job.key, "thinking", next)
                    }
                    bind:temperature={
                      () => tuningOf(job.key, "temperature"),
                      (next) => setTuning(job.key, "temperature", next)
                    }
                    thinkingRef={{ kind: "role", role: job.key, field: "thinking" }}
                    temperatureRef={{ kind: "role", role: job.key, field: "temperature" }}
                    thinkingHint={thinkingDefaultHint}
                    temperatureHint="Leave it blank for the setting under Every model."
                    temperatureFallback={defaultTemperature || undefined}
                  />
                {/if}
              </div>
            {/each}
          </div>
        </SettingsGroup>
      </Disclosure>
    </div>

    <ProvidersGroup {scope} />
  </div>
</SettingsSection>

<style>
  /* The field asked for on arrival, until focus leaves it. */
  .model-section :global([data-arrived]) {
    border-radius: var(--corner-sm);
    outline: 1px solid var(--color-vein);
    outline-offset: var(--space-6);
  }

  .model-notice {
    max-width: 640px;
    margin-bottom: var(--space-16);
  }

  .model-jobs {
    max-width: 640px;
    margin-bottom: var(--space-16);

    & :global(.ui-disclosure),
    & :global(.ui-disclosure-panel) {
      align-self: stretch;
      width: 100%;
    }

    & :global(.ui-disclosure-panel) {
      margin-top: var(--space-12);
    }
  }

  .jobs {
    display: flex;
    flex-direction: column;
  }

  .job {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-16) 0;

    &:first-child {
      padding-top: 0;
    }

    &:last-child {
      padding-bottom: 0;
    }

    & + .job {
      border-top: 1px solid var(--color-line-soft);
    }
  }

  .job-text {
    display: flex;
    flex-direction: column;
    gap: var(--space-2);
  }

  .job-title {
    font-size: var(--font-size-ui);
    font-weight: var(--font-weight-medium);
  }

  .job-lede {
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
    line-height: var(--line-height-ui);
  }
</style>
