<script lang="ts">
  import { ApiError, submitBugReport, submitFeedback } from "../lib/api";
  import type { BugSeverity, FeedbackReceipt } from "../lib/api";
  import { Icon } from "../lib/icons";
  import Modal from "./Modal.svelte";

  type Tab = "bug" | "feedback";

  interface Props {
    open: boolean;
    initialTab?: Tab;
    onClose: () => void;
  }

  let { open, initialTab = "bug", onClose }: Props = $props();

  // ── Form state (kept across tab switches so users don't lose drafts)
  // Initialized to a literal — synced from the `initialTab` prop in the
  // open-effect below so re-opening with a different tab works.
  let tab = $state<Tab>("bug");
  let happened = $state("");
  let expected = $state("");
  let doing = $state("");
  let severity = $state<BugSeverity | null>(null);
  let message = $state("");
  let category = $state("");

  // ── Submission state
  let submitting = $state(false);
  let receipt = $state<FeedbackReceipt | null>(null);
  let errorMsg = $state("");
  let copied = $state(false);
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  // Reset transient state whenever the modal is reopened or the tab changes
  // (drafts persist; only success / error feedback resets).
  $effect(() => {
    if (!open) return;
    tab = initialTab;
    receipt = null;
    errorMsg = "";
    copied = false;
  });

  $effect(() => {
    // Tab changed → clear the prior submission summary so the form is
    // unambiguous about which surface a fresh submit will hit.
    void tab;
    receipt = null;
    errorMsg = "";
  });

  const bugReady = $derived(
    happened.trim().length > 0 &&
      expected.trim().length > 0 &&
      doing.trim().length > 0 &&
      severity !== null,
  );
  const feedbackReady = $derived(message.trim().length > 0);

  async function send() {
    if (submitting) return;
    submitting = true;
    errorMsg = "";
    receipt = null;
    try {
      if (tab === "bug") {
        if (!bugReady || !severity) return;
        receipt = await submitBugReport({
          what_happened: happened.trim(),
          what_expected: expected.trim(),
          what_doing: doing.trim(),
          severity,
        });
      } else {
        if (!feedbackReady) return;
        receipt = await submitFeedback({
          message: message.trim(),
          category: category.trim() || undefined,
        });
      }
    } catch (err) {
      errorMsg = formatError(err);
    } finally {
      submitting = false;
    }
  }

  function formatError(err: unknown): string {
    if (err instanceof ApiError) {
      // Try to surface the upstream { "error": "..." } structure when present.
      try {
        const parsed = JSON.parse(err.body) as { error?: string };
        if (parsed.error) return parsed.error;
      } catch {
        // fall through
      }
      return err.body || `${err.status} ${err.statusText}`;
    }
    if (err instanceof Error) return err.message;
    return String(err);
  }

  async function copyId() {
    if (!receipt) return;
    try {
      await navigator.clipboard.writeText(receipt.public_id);
      copied = true;
      clearTimeout(copyTimer);
      copyTimer = setTimeout(() => {
        copied = false;
      }, 1800);
    } catch {
      // Clipboard write can fail in insecure contexts; the ID is already
      // visible on screen so failure is non-blocking.
    }
  }

  function close() {
    onClose();
  }
</script>

<Modal title="Send to the maintainer" {open} onClose={close}>
  <div class="strata-tabs" role="tablist">
    <button
      class="strata-tab"
      class:active={tab === "bug"}
      role="tab"
      aria-selected={tab === "bug"}
      onclick={() => {
        tab = "bug";
      }}
    >
      <Icon name="bug" size={14} />
      <span>Report a bug</span>
    </button>
    <button
      class="strata-tab"
      class:active={tab === "feedback"}
      role="tab"
      aria-selected={tab === "feedback"}
      onclick={() => {
        tab = "feedback";
      }}
    >
      <Icon name="spark" size={14} />
      <span>Send feedback</span>
    </button>
  </div>

  {#if receipt}
    <div class="receipt emerges" role="status" aria-live="polite">
      <div class="receipt-eyebrow">
        {tab === "bug" ? "Bug report submitted" : "Feedback submitted"}
      </div>
      <div class="receipt-id-row">
        <code class="receipt-id">{receipt.public_id}</code>
        <button
          class="copy-btn"
          onclick={copyId}
          title="Copy reference ID"
          aria-label="Copy reference ID"
        >
          {#if copied}
            <Icon name="check" size={14} />
            <span>Copied</span>
          {:else}
            <Icon name="copy" size={14} />
            <span>Copy</span>
          {/if}
        </button>
      </div>
      <p class="receipt-hint">Reference this ID in a GitHub issue if you have more to add.</p>
    </div>
  {:else if tab === "bug"}
    <div class="form">
      <div class="settings-field">
        <label for="happened">What happened?</label>
        <textarea
          id="happened"
          rows="3"
          bind:value={happened}
          placeholder="The actual behavior you observed."
          disabled={submitting}
        ></textarea>
      </div>
      <div class="settings-field">
        <label for="expected">What did you expect?</label>
        <textarea
          id="expected"
          rows="2"
          bind:value={expected}
          placeholder="What should have happened instead."
          disabled={submitting}
        ></textarea>
      </div>
      <div class="settings-field">
        <label for="doing">What were you doing?</label>
        <textarea
          id="doing"
          rows="2"
          bind:value={doing}
          placeholder="Steps, context, or what you'd just done."
          disabled={submitting}
        ></textarea>
      </div>
      <div class="settings-field">
        <div class="severity-label">Severity</div>
        <div class="severity-row" role="radiogroup" aria-label="Severity">
          {#each ["broken", "wrong", "annoying"] as const as opt (opt)}
            <button
              type="button"
              class="severity-pill"
              class:active={severity === opt}
              role="radio"
              aria-checked={severity === opt}
              onclick={() => {
                severity = opt;
              }}
              disabled={submitting}
            >
              {opt}
            </button>
          {/each}
        </div>
      </div>
    </div>
  {:else}
    <div class="form">
      <div class="settings-field">
        <label for="message">Your feedback</label>
        <textarea
          id="message"
          rows="5"
          bind:value={message}
          placeholder="Anything on your mind — friction, confusion, or a half-formed idea."
          disabled={submitting}
        ></textarea>
      </div>
      <div class="settings-field">
        <label for="category">Category <span class="optional">(optional)</span></label>
        <input
          id="category"
          type="text"
          bind:value={category}
          placeholder="ui, docs, models…"
          disabled={submitting}
        />
      </div>
    </div>
  {/if}

  {#if errorMsg && !receipt}
    <p class="validation-msg error">{errorMsg}</p>
  {/if}

  {#snippet actions()}
    {#if receipt}
      <button class="btn btn-primary" onclick={close}>Done</button>
    {:else}
      <button class="btn btn-secondary" onclick={close} disabled={submitting}>Cancel</button>
      <button
        class="btn btn-primary"
        onclick={send}
        disabled={submitting || (tab === "bug" ? !bugReady : !feedbackReady)}
      >
        {#if submitting}
          Sending…
        {:else if tab === "bug"}
          Send report
        {:else}
          Send feedback
        {/if}
      </button>
    {/if}
  {/snippet}
</Modal>

<style>
  /* ── Tab strata: thin horizontal seams that echo the geological theme.
     Inactive tabs sit "below the surface"; the active one rises with a
     vein-bright underline that takes a slow breath to land. */
  .strata-tabs {
    display: flex;
    gap: 0;
    margin: 0 calc(-1 * var(--s-3)) var(--s-4);
    padding: 0 var(--s-3);
    border-bottom: 1px solid var(--border-subtle);
  }

  .strata-tab {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    padding: var(--s-2) var(--s-4);
    background: transparent;
    border: none;
    border-bottom: 1px solid transparent;
    color: var(--text-dim);
    font-family: var(--font-display);
    font-size: var(--fs-sm);
    letter-spacing: 0.12em;
    text-transform: uppercase;
    cursor: pointer;
    margin-bottom: -1px;
    transition:
      color var(--dur-default) var(--ease-out-stone),
      border-color var(--dur-default) var(--ease-out-stone);
  }

  .strata-tab:hover {
    color: var(--text-muted);
  }

  .strata-tab.active {
    color: var(--text);
    border-bottom-color: var(--vein);
    box-shadow: 0 6px 14px -10px rgba(59, 139, 219, 0.55);
  }

  /* ── Forms ──────────────────────────────────────────────────────── */

  .form {
    display: flex;
    flex-direction: column;
    gap: var(--s-3);
  }

  .form :global(.settings-field) {
    margin-bottom: 0;
  }

  .form textarea {
    width: 100%;
    padding: var(--s-2) var(--s-3);
    font-family: var(--font-body);
    font-size: var(--fs-md);
    line-height: 1.55;
    color: var(--text);
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    box-shadow: var(--input-inset);
    resize: vertical;
    outline: none;
    transition:
      border-color var(--dur-default) var(--ease-out-stone),
      box-shadow var(--dur-default) var(--ease-out-stone);
  }

  .form textarea:focus,
  .form input:focus {
    border-color: var(--vein-dim);
    box-shadow:
      var(--input-inset),
      0 0 0 1px rgba(59, 139, 219, 0.15),
      0 0 8px rgba(59, 139, 219, 0.06);
  }

  .form textarea::placeholder,
  .form input::placeholder {
    color: var(--text-dim);
    font-style: italic;
  }

  .optional {
    color: var(--text-dim);
    font-style: italic;
    font-weight: 400;
  }

  /* ── Severity pills: discrete, tactile, named after themselves ─── */

  .severity-label {
    display: block;
    font-size: var(--fs-sm);
    color: var(--text-muted);
    margin-bottom: 4px;
    font-weight: 500;
  }

  .severity-row {
    display: flex;
    gap: var(--s-2);
  }

  .severity-pill {
    flex: 1;
    padding: var(--s-2) var(--s-3);
    background: var(--bg-input);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
    letter-spacing: 0.05em;
    text-transform: lowercase;
    cursor: pointer;
    transition:
      background-color var(--dur-quick),
      color var(--dur-quick),
      border-color var(--dur-quick),
      box-shadow var(--dur-default) var(--ease-out-stone);
  }

  .severity-pill:hover:not(:disabled) {
    color: var(--text);
    border-color: var(--border);
    background: var(--bg-raised);
  }

  .severity-pill.active {
    color: var(--text);
    border-color: var(--vein-dim);
    background: var(--vein-glow);
    box-shadow: var(--vein-ring);
  }

  .severity-pill:disabled {
    cursor: not-allowed;
    opacity: 0.55;
  }

  /* ── Receipt panel: monospace ID with a soft vein halo, slow reveal ─ */

  .receipt {
    text-align: center;
    padding: var(--s-3) 0 var(--s-2);
  }

  .receipt-eyebrow {
    font-family: var(--font-display);
    font-size: var(--fs-sm);
    letter-spacing: 0.18em;
    text-transform: uppercase;
    color: var(--text-muted);
    margin-bottom: var(--s-3);
  }

  .receipt-id-row {
    display: inline-flex;
    align-items: center;
    gap: var(--s-2);
    padding: var(--s-2) var(--s-3);
    background: var(--bg-input);
    border: 1px solid var(--vein-dim);
    border-radius: var(--radius);
    box-shadow:
      0 0 24px rgba(59, 139, 219, 0.2),
      inset 0 0 0 1px var(--vein-faint);
  }

  .receipt-id {
    font-family: var(--font-mono);
    font-size: var(--fs-body);
    letter-spacing: 0.06em;
    color: var(--vein-bright);
    user-select: all;
  }

  .copy-btn {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px var(--s-2);
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    color: var(--text-muted);
    font-family: var(--font-body);
    font-size: var(--fs-sm);
    cursor: pointer;
    transition:
      color var(--dur-quick),
      background-color var(--dur-quick),
      border-color var(--dur-quick);
  }

  .copy-btn:hover {
    color: var(--text);
    background: var(--vein-faint);
    border-color: var(--vein-dim);
  }

  .receipt-hint {
    margin-top: var(--s-3);
    font-family: var(--font-body);
    font-size: var(--fs-md);
    color: var(--text-muted);
    font-style: italic;
  }

  /* ── Inline error (mirrors .validation-msg pattern from forms.css) ─ */

  .validation-msg.error {
    margin-top: var(--s-3);
    padding: var(--s-2) var(--s-3);
    background: var(--error-bg);
    border-left: 2px solid var(--error);
    border-radius: var(--radius-sm);
    color: var(--error);
    font-family: var(--font-body);
    font-size: var(--fs-md);
  }

  /* Slow stone-reveal for the success state. */
  .emerges {
    animation: emerge var(--dur-slow) var(--ease-out-stone) both;
  }

  @keyframes emerge {
    from {
      opacity: 0;
      transform: translateY(4px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .emerges {
      animation: none;
    }
    .strata-tab,
    .severity-pill {
      transition: none;
    }
  }
</style>
