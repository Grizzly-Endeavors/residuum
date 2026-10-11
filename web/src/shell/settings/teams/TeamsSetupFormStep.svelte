<script lang="ts">
  import { Banner, TextField } from "../../../lib/ui";
  import type { TeamsSetupState } from "./teams-setup-state.svelte";

  interface Props {
    state: TeamsSetupState;
  }

  let { state }: Props = $props();

  function onColorChange(e: Event & { currentTarget: HTMLInputElement }): void {
    const file = e.currentTarget.files?.[0];
    void state.handleColorIconChange(file);
  }

  function onOutlineChange(e: Event & { currentTarget: HTMLInputElement }): void {
    const file = e.currentTarget.files?.[0];
    void state.handleOutlineIconChange(file);
  }
</script>

<div class="step-content">
  {#if state.formError !== null}
    <Banner tone="error" title="Setup error">{state.formError}</Banner>
  {/if}

  <TextField label="Bot name" bind:value={state.botName} placeholder={state.agent} required />

  <TextField
    label="Short description"
    bind:value={state.shortDescription}
    hint={`${state.shortDescription.length}/80`}
    maxlength={80}
    placeholder="Brief summary of the bot for the Teams app catalog"
    required
  />

  <TextField
    label="Long description"
    bind:value={state.longDescription}
    hint={`${state.longDescription.length}/4000`}
    maxlength={4000}
    multiline
    rows={3}
    placeholder="Detailed description of what the bot does and how to interact with it"
    required
  />

  <div class="form-row-2">
    <TextField
      label="Developer name"
      bind:value={state.developerName}
      placeholder="Your name or organization"
      required
    />
    <TextField
      label="Developer website"
      type="url"
      bind:value={state.developerUrl}
      placeholder="https://example.com"
      required
    />
  </div>

  <div class="form-row-2">
    <TextField
      label="Privacy policy URL (optional)"
      type="url"
      bind:value={state.privacyUrl}
      placeholder="https://example.com/privacy"
    />
    <TextField
      label="Terms of use URL (optional)"
      type="url"
      bind:value={state.termsUrl}
      placeholder="https://example.com/terms"
    />
  </div>

  <TextField
    label="Messaging endpoint"
    type="url"
    bind:value={state.messagingEndpoint}
    placeholder="https://your-domain.com/api/messages"
    required
  />

  {#if state.messagingEndpoint.trim() !== "" && !state.messagingEndpoint.startsWith("https://")}
    <Banner tone="warn">
      Microsoft Teams requires an HTTPS messaging endpoint. Residuum Cloud or an HTTPS tunnel is
      needed.
    </Banner>
  {/if}

  <div class="icons-section">
    <h4 class="icons-title">App Icons</h4>

    <div class="icons-grid">
      <div class="icon-upload-box">
        <label for="color-icon-input" class="icon-upload-label">Color Icon (192×192 PNG)</label>
        {#if state.colorIconPreview !== null}
          <div class="icon-preview-wrapper">
            <img src={state.colorIconPreview} alt="Color icon preview" class="icon-preview-color" />
          </div>
        {/if}
        <input
          id="color-icon-input"
          type="file"
          accept="image/png"
          class="file-input"
          aria-label="Color Icon (192x192 PNG)"
          onchange={onColorChange}
        />
        {#if state.colorIconError !== null}
          <p class="field-error">{state.colorIconError}</p>
        {/if}
      </div>

      <div class="icon-upload-box">
        <label for="outline-icon-input" class="icon-upload-label">Outline Icon (32×32 PNG)</label>
        {#if state.outlineIconPreview !== null}
          <div class="icon-preview-wrapper">
            <img
              src={state.outlineIconPreview}
              alt="Outline icon preview"
              class="icon-preview-outline"
            />
          </div>
        {/if}
        <input
          id="outline-icon-input"
          type="file"
          accept="image/png"
          class="file-input"
          aria-label="Outline Icon (32x32 PNG)"
          onchange={onOutlineChange}
        />
        {#if state.outlineIconError !== null}
          <p class="field-error">{state.outlineIconError}</p>
        {/if}
      </div>
    </div>
  </div>
</div>

<style>
  .step-content {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }

  .form-row-2 {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-12);
  }

  .icons-section {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    margin-top: var(--space-4);
  }

  .icons-title {
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    color: var(--color-text);
  }

  .icons-grid {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: var(--space-16);
  }

  .icon-upload-box {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
    padding: var(--space-12);
    background: var(--color-stone-2);
    border: 1px dashed var(--color-control-border);
    border-radius: var(--corner-sm);
  }

  .icon-upload-label {
    font-size: var(--font-size-xs);
    font-weight: var(--font-weight-medium);
    color: var(--color-text-2);
  }

  .file-input {
    font-size: var(--font-size-xs);
    color: var(--color-text-2);
  }

  .icon-preview-wrapper {
    display: flex;
    align-items: center;
    justify-content: center;
    padding: var(--space-8);
    background: var(--color-stone-1);
    border-radius: var(--corner-sm);
  }

  .icon-preview-color {
    width: 64px;
    height: 64px;
    object-fit: contain;
  }

  .icon-preview-outline {
    width: 32px;
    height: 32px;
    object-fit: contain;
  }

  .field-error {
    font-size: var(--font-size-xs);
    color: var(--color-err-text);
  }

  @container (max-width: 500px) {
    .form-row-2,
    .icons-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
