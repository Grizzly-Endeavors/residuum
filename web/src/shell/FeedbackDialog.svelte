<script lang="ts">
  import { submitBugReport, submitFeedback } from "../lib/api";
  import type { BugSeverity, FeedbackReceipt } from "../lib/api";
  import { userErrorMessage } from "../lib/errors";
  import { Banner, Button, Dialog, SegmentedControl, TextField } from "../lib/ui";
  import type { Choice } from "../lib/ui";
  import type { FeedbackTab } from "./shell-actions";

  // Feedback and bug reports for the maintainer. Each kind keeps its draft
  // while the dialog is closed or the other kind is chosen, until it is sent.

  interface Props {
    open: boolean;
    tab: FeedbackTab;
  }

  let { open = $bindable(), tab = $bindable() }: Props = $props();

  const KINDS: readonly Choice<FeedbackTab>[] = [
    { value: "bug", label: "Report a bug" },
    { value: "feedback", label: "Send feedback" },
  ];
  const SEVERITIES: readonly Choice<BugSeverity>[] = [
    { value: "broken", label: "Broken" },
    { value: "wrong", label: "Wrong" },
    { value: "annoying", label: "Annoying" },
  ];

  let happened = $state("");
  let expected = $state("");
  let doing = $state("");
  let severity = $state<BugSeverity | "">("");
  let message = $state("");
  let category = $state("");

  let submitting = $state(false);
  let receipt = $state<FeedbackReceipt | null>(null);
  let errorMsg = $state("");
  let copy = $state<"idle" | "copied" | "failed">("idle");
  let copyTimer: ReturnType<typeof setTimeout> | undefined;

  // A new opening, or the other kind, starts without the last result.
  $effect(() => {
    void tab;
    if (!open) return;
    receipt = null;
    errorMsg = "";
    copy = "idle";
  });

  const ready = $derived(
    tab === "bug"
      ? happened.trim() !== "" && expected.trim() !== "" && doing.trim() !== "" && severity !== ""
      : message.trim() !== "",
  );

  async function send(): Promise<void> {
    if (submitting || !ready) return;
    submitting = true;
    errorMsg = "";
    try {
      if (tab === "bug" && severity !== "") {
        receipt = await submitBugReport({
          what_happened: happened.trim(),
          what_expected: expected.trim(),
          what_doing: doing.trim(),
          severity,
        });
        happened = "";
        expected = "";
        doing = "";
        severity = "";
      } else {
        const topic = category.trim();
        receipt = await submitFeedback({
          message: message.trim(),
          category: topic === "" ? undefined : topic,
        });
        message = "";
        category = "";
      }
    } catch (err) {
      errorMsg = userErrorMessage(err, {
        action: "Couldn't send it.",
        serverFault:
          "The feedback service didn't accept it. Nothing was sent; try again in a little while.",
      });
    } finally {
      submitting = false;
    }
  }

  async function copyId(id: string): Promise<void> {
    clearTimeout(copyTimer);
    try {
      await navigator.clipboard.writeText(id);
      copy = "copied";
      copyTimer = setTimeout(() => (copy = "idle"), 1800);
    } catch {
      copy = "failed";
    }
  }
</script>

<Dialog
  bind:open
  title={tab === "bug" ? "Report a bug" : "Send feedback"}
  description="It goes to Residuum's maintainer."
  size="md"
  fullscreenOnPhone
  {actions}
>
  {#if receipt !== null}
    <div class="feedback-receipt" role="status">
      <p>{tab === "bug" ? "Bug report sent." : "Feedback sent."} Its reference:</p>
      <div class="feedback-reference">
        <code>{receipt.public_id}</code>
        <Button
          variant="quiet"
          size="sm"
          icon={copy === "copied" ? "check" : "copy"}
          onclick={() => copyId(receipt?.public_id ?? "")}
        >
          {copy === "copied" ? "Copied" : "Copy"}
        </Button>
      </div>
      <p class="feedback-note">
        {copy === "failed"
          ? "Couldn't copy it here. Select the reference to copy it yourself."
          : "Mention it in a GitHub issue if you have more to add."}
      </p>
    </div>
  {:else}
    <div class="feedback-form">
      <SegmentedControl label="What to send" labelHidden bind:value={tab} options={KINDS} />
      {#if tab === "bug"}
        <TextField
          label="What happened?"
          multiline
          rows={3}
          bind:value={happened}
          placeholder="What you saw."
          disabled={submitting}
          data-autofocus
        />
        <TextField
          label="What did you expect?"
          multiline
          rows={2}
          bind:value={expected}
          placeholder="What should have happened instead."
          disabled={submitting}
        />
        <TextField
          label="What were you doing?"
          multiline
          rows={2}
          bind:value={doing}
          placeholder="Steps, or what you'd just done."
          disabled={submitting}
        />
        <SegmentedControl
          label="How bad is it?"
          bind:value={severity}
          options={SEVERITIES}
          disabled={submitting}
        />
      {:else}
        <TextField
          label="Your feedback"
          multiline
          rows={5}
          bind:value={message}
          placeholder="Friction, confusion, or a half-formed idea."
          disabled={submitting}
          data-autofocus
        />
        <TextField
          label="Topic"
          hint="Optional, such as UI, docs or models."
          bind:value={category}
          disabled={submitting}
        />
      {/if}
      {#if errorMsg !== ""}
        <Banner tone="error">{errorMsg}</Banner>
      {/if}
    </div>
  {/if}
</Dialog>

{#snippet actions()}
  {#if receipt !== null}
    <Button variant="primary" onclick={() => (open = false)}>Done</Button>
  {:else}
    <Button variant="quiet" disabled={submitting} onclick={() => (open = false)}>Cancel</Button>
    <Button variant="primary" loading={submitting} disabled={!ready} onclick={send}>
      {tab === "bug" ? "Send report" : "Send feedback"}
    </Button>
  {/if}
{/snippet}

<style>
  .feedback-form {
    display: flex;
    flex-direction: column;
    gap: var(--space-14);
  }

  .feedback-receipt {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: var(--space-10);
    font-size: var(--font-size-sm);
  }

  .feedback-reference {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    padding: var(--space-4) var(--space-4) var(--space-4) var(--space-12);
    border-radius: var(--corner-sm);
    background: var(--color-stone-2);

    & code {
      color: var(--color-vein-bright);
      font-size: var(--font-size-ui);
      user-select: all;
    }
  }

  .feedback-note {
    color: var(--color-text-2);
  }
</style>
