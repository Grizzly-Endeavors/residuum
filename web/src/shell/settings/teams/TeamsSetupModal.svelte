<script lang="ts">
  import { untrack } from "svelte";
  import { Button, Dialog } from "../../../lib/ui";
  import TeamsSetupDoneStep from "./TeamsSetupDoneStep.svelte";
  import TeamsSetupFailedStep from "./TeamsSetupFailedStep.svelte";
  import TeamsSetupFormStep from "./TeamsSetupFormStep.svelte";
  import TeamsSetupPrereqsStep from "./TeamsSetupPrereqsStep.svelte";
  import TeamsSetupProgressStep from "./TeamsSetupProgressStep.svelte";
  import { TeamsSetupState } from "./teams-setup-state.svelte";

  interface Props {
    agent: string;
    open: boolean;
    onclose?: () => void;
    onsuccess?: () => void;
  }

  let { agent, open = $bindable(false), onclose, onsuccess }: Props = $props();

  const state = new TeamsSetupState(() => agent);

  function handleClose(): void {
    open = false;
    state.stopPolling();
    onclose?.();
  }

  function handleDoneFinish(): void {
    open = false;
    state.stopPolling();
    onsuccess?.();
    onclose?.();
  }

  $effect(() => {
    if (open) {
      untrack(() => {
        void state.loadInitial();
      });
    }
  });

  $effect(() => {
    if (open && state.currentStep === "progress") {
      state.startPolling();
      return () => {
        state.stopPolling();
      };
    } else {
      state.stopPolling();
    }
  });
</script>

<Dialog
  bind:open
  size="lg"
  title={state.stepTitle}
  description={state.stepDescription}
  onclose={handleClose}
>
  <div class="wizard-container" data-step={state.currentStep}>
    {#if state.currentStep === "prereqs"}
      <TeamsSetupPrereqsStep {state} />
    {:else if state.currentStep === "form"}
      <TeamsSetupFormStep {state} />
    {:else if state.currentStep === "progress"}
      <TeamsSetupProgressStep {state} />
    {:else if state.currentStep === "failed"}
      <TeamsSetupFailedStep {state} />
    {:else if state.currentStep === "done"}
      <TeamsSetupDoneStep {state} />
    {/if}
  </div>

  {#snippet actions()}
    {#if state.currentStep === "prereqs"}
      <Button variant="quiet" onclick={handleClose}>Cancel</Button>
      <Button
        variant="primary"
        disabled={!state.canContinueFromPrereqs}
        onclick={() => (state.currentStep = "form")}
      >
        Next
      </Button>
    {:else if state.currentStep === "form"}
      <Button variant="quiet" onclick={() => (state.currentStep = "prereqs")}>Back</Button>
      <Button
        variant="primary"
        loading={state.formSubmitting}
        disabled={!state.formValid}
        onclick={() => void state.handleStartSetup()}
      >
        Start setup
      </Button>
    {:else if state.currentStep === "progress"}
      <Button variant="danger" loading={state.cancelling} onclick={() => void state.handleCancel()}>
        Cancel setup
      </Button>
    {:else if state.currentStep === "failed"}
      <Button variant="quiet" onclick={() => void state.handleStartOver()}>Start over</Button>
      <Button
        variant="secondary"
        loading={state.cleanupLoading}
        onclick={() => void state.handleCleanup()}
      >
        Clean up
      </Button>
      <Button variant="primary" loading={state.retrying} onclick={() => void state.handleRetry()}>
        Retry
      </Button>
    {:else if state.currentStep === "done"}
      <Button variant="primary" onclick={handleDoneFinish}>Done</Button>
    {/if}
  {/snippet}
</Dialog>

<style>
  .wizard-container {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }
</style>
