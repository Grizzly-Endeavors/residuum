<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { Icon } from "../lib/icons";
  import { PairingFlow } from "../lib/pairing-flow.svelte";
  import { Banner, Button, Disclosure, Spinner, TextField, VisuallyHidden } from "../lib/ui";

  // The page an unpaired browser lands on when it reaches Residuum through
  // Residuum Cloud. It is a page of its own, not part of the app: nothing else
  // loads until the browser is paired. A pairing link settles it in one step.
  // Otherwise the person asks a paired device (or Settings on the machine
  // running Residuum) to approve the code shown here, or types a recovery code.

  interface Props {
    flow?: PairingFlow;
  }

  let { flow = new PairingFlow() }: Props = $props();

  let recoveryOpen = $state(false);
  let recoveryCode = $state("");

  onMount(() => {
    void flow.start();
  });
  onDestroy(() => flow.stop());
</script>

<main class="pair">
  <header class="pair-bar">
    <span class="pair-wordmark"><Icon name="mark" size={18} />Residuum</span>
  </header>

  <div class="pair-scroller">
    <div class="pair-column">
      {#if flow.phase === "checking" || flow.phase === "done"}
        <div class="pair-wait" role="status">
          <Spinner size={16} />
          <VisuallyHidden>
            {flow.phase === "done" ? "Opening Residuum" : "Checking this browser"}
          </VisuallyHidden>
        </div>
      {:else}
        <h1 class="pair-title">Pair this browser</h1>

        {#if flow.problem !== ""}
          <Banner tone="error">{flow.problem}</Banner>
        {/if}

        {#if flow.phase === "link"}
          <p class="pair-lede">
            You opened a pairing link from Residuum. Pairing lets this browser use your agents
            through Residuum Cloud.
          </p>
          <TextField label="Name for this browser" bind:value={flow.deviceName} maxlength={64} />
          <div class="pair-actions">
            <Button variant="primary" loading={flow.busy} onclick={() => void flow.pairWithLink()}>
              Pair this browser
            </Button>
          </div>
        {:else if flow.phase === "waiting"}
          <p class="pair-lede">
            Open Residuum on a paired device, or on the machine it runs on, and approve the request
            that shows this code (Settings, All agents, Residuum Cloud).
          </p>
          <p class="pair-code" aria-label="Pairing code {flow.code.split('').join(' ')}">
            {flow.code}
          </p>
          <p class="pair-waiting" role="status">
            <Spinner size={14} />Waiting for approval. Approve only if the codes match.
          </p>
          <div class="pair-actions">
            <Button onclick={() => flow.cancelRequest()}>Cancel</Button>
          </div>
        {:else}
          <p class="pair-lede">
            This Residuum only answers browsers it has paired with. Ask for approval from a device
            that is already paired.
          </p>
          <TextField label="Name for this browser" bind:value={flow.deviceName} maxlength={64} />
          <div class="pair-actions">
            <Button
              variant="primary"
              loading={flow.busy}
              onclick={() => void flow.requestApproval()}
            >
              Ask for approval
            </Button>
          </div>
          <Disclosure summary="Use a recovery code instead" bind:open={recoveryOpen}>
            <div class="pair-recovery">
              <TextField
                label="Recovery code"
                bind:value={recoveryCode}
                placeholder="ABCD-EFGH-IJKL-MNOP"
                autocomplete="off"
                spellcheck={false}
                code
                hint="Each code pairs one browser, once. You were shown them when remote access was set up."
              />
              <div class="pair-actions">
                <Button
                  loading={flow.busy}
                  disabled={recoveryCode.trim() === ""}
                  onclick={() => void flow.pairWithRecoveryCode(recoveryCode)}
                >
                  Pair with recovery code
                </Button>
              </div>
            </div>
          </Disclosure>
        {/if}
      {/if}
    </div>
  </div>
</main>

<style>
  .pair {
    display: flex;
    flex: 1;
    flex-direction: column;
    min-height: 0;
    height: 100%;
    padding: var(--safe-top) var(--safe-right) var(--safe-bottom) var(--safe-left);
  }

  .pair-bar {
    display: flex;
    flex: none;
    align-items: center;
    height: var(--layout-place-header-height);
    padding: 0 var(--space-20);
    border-bottom: 1px solid var(--color-line-soft);
  }

  .pair-wordmark {
    display: inline-flex;
    align-items: center;
    gap: var(--space-10);
    font-family: var(--font-mark);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    letter-spacing: 0.2em;
    text-transform: uppercase;

    & > :global(svg) {
      color: var(--color-vein);
    }
  }

  .pair-scroller {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
  }

  .pair-column {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
    max-width: calc(480px + 2 * var(--space-16));
    margin: 0 auto;
    padding: var(--space-32) var(--space-16) var(--space-48);
  }

  .pair-wait {
    display: flex;
    justify-content: center;
    padding: var(--space-48) 0;
    color: var(--color-text-3);
  }

  .pair-title {
    font-size: var(--font-size-title);
    font-weight: var(--font-weight-semibold);
    line-height: var(--line-height-tight);
  }

  .pair-lede {
    color: var(--color-text-2);
    line-height: var(--line-height-ui);
  }

  .pair-code {
    align-self: flex-start;
    padding: var(--space-12) var(--space-20);
    border: 1px solid var(--color-line);
    border-radius: var(--corner-md);
    background: var(--color-stone-2);
    font-family: var(--font-code);
    font-size: var(--font-size-title);
    font-weight: var(--font-weight-semibold);
    letter-spacing: 0.35em;
    user-select: all;
  }

  .pair-waiting {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    color: var(--color-text-2);
    font-size: var(--font-size-sm);
  }

  .pair-actions {
    display: flex;
    flex-wrap: wrap;
    gap: var(--space-8);
  }

  .pair-recovery {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
  }
</style>
