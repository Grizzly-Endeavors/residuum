<script lang="ts">
  import {
    ApiError,
    cancelTeamsSetupJob,
    cleanupTeamsSetup,
    deleteTeamsSetupJob,
    fetchTeamsAppPackage,
    fetchTeamsSetupJob,
    fetchTeamsSetupPrereqs,
    installTeamsApp,
    retryTeamsSetupJob,
    startTeamsSetupJob,
    submitTeamsSetupRedirect,
    teamsAppPackageUrl,
  } from "../../../lib/api";
  import { Icon } from "../../../lib/icons";
  import type {
    TeamsSetupForm,
    TeamsSetupJob,
    TeamsSetupPhase,
    TeamsSetupPrereqs,
    TeamsSetupStart,
  } from "../../../lib/types";
  import { Badge, Banner, Button, Dialog, Disclosure, Spinner, TextField } from "../../../lib/ui";
  import { validateIcon } from "./icon-utils";

  interface Props {
    agent: string;
    open: boolean;
    onclose?: () => void;
    onsuccess?: () => void;
  }

  let { agent, open = $bindable(false), onclose, onsuccess }: Props = $props();

  type WizardStep = "prereqs" | "form" | "progress" | "failed" | "done";

  let currentStep = $state<WizardStep>("prereqs");
  let loadingPrereqs = $state(true);
  let prereqsError = $state<string | null>(null);
  let prereqs = $state<TeamsSetupPrereqs | null>(null);

  let job = $state<TeamsSetupJob | null>(null);

  // Step 1: Pre-requisites & Consent
  let replaceExisting = $state(false);
  let consentInstallCli = $state(false);

  // Step 2: Form fields
  let botName = $state("");
  let shortDescription = $state("");
  let longDescription = $state("");
  let developerName = $state("");
  let developerUrl = $state("");
  let privacyUrl = $state("");
  let termsUrl = $state("");
  let messagingEndpoint = $state("");
  let colorIconBase64 = $state<string | null>(null);
  let outlineIconBase64 = $state<string | null>(null);
  let colorIconError = $state<string | null>(null);
  let outlineIconError = $state<string | null>(null);
  let colorIconPreview = $state<string | null>(null);
  let outlineIconPreview = $state<string | null>(null);
  let formSubmitting = $state(false);
  let formError = $state<string | null>(null);

  // Step 3: Progress & Redirect
  let redirectUrl = $state("");
  let redirectSubmitting = $state(false);
  let redirectError = $state<string | null>(null);
  let logsOpen = $state(false);
  let cancelling = $state(false);
  let now = $state(Date.now());

  // Step 4: Failed & Retry
  let retrying = $state(false);
  let errorDetailsOpen = $state(false);

  // Step 5: Done & Post-setup
  let installingApp = $state(false);
  let installError = $state<string | null>(null);
  let cleanupLoading = $state(false);
  let cleanupSuccess = $state(false);

  const PHASES: readonly { phase: TeamsSetupPhase; label: string }[] = [
    { phase: "check_prereqs", label: "Check prerequisites" },
    { phase: "install_cli", label: "Install Agents Toolkit CLI" },
    { phase: "sign_in", label: "Sign in to Microsoft 365" },
    { phase: "scaffold", label: "Scaffold Teams app" },
    { phase: "provision", label: "Provision cloud resources" },
    { phase: "import", label: "Import bot registration" },
  ];

  // Derived validations for Step 1
  const nodeMissing = $derived(
    prereqs !== null && (!prereqs.node.found || prereqs.node.version === null),
  );
  const replaceRequired = $derived(prereqs?.teams_already_configured ?? false);
  const consentRequired = $derived(prereqs !== null && !prereqs.atk.installed);
  const canContinueFromPrereqs = $derived.by(() => {
    if (loadingPrereqs || prereqs === null) return false;
    if (nodeMissing) return false;
    if (replaceRequired && !replaceExisting) return false;
    if (consentRequired && !consentInstallCli) return false;
    return true;
  });

  // Derived validations for Step 2
  const formValid = $derived.by(() => {
    if (botName.trim() === "") return false;
    if (shortDescription.trim() === "" || shortDescription.length > 80) return false;
    if (longDescription.trim() === "" || longDescription.length > 4000) return false;
    if (developerName.trim() === "") return false;
    if (developerUrl.trim() === "") return false;
    if (messagingEndpoint.trim() === "") return false;
    if (colorIconError !== null || outlineIconError !== null) return false;
    return true;
  });

  const stepTitle = $derived.by(() => {
    switch (currentStep) {
      case "prereqs":
      case "form":
        return "Set up Microsoft Teams";
      case "progress":
        return "Setting up Microsoft Teams";
      case "failed":
        return job?.state === "cancelled" ? "Teams setup cancelled" : "Teams setup failed";
      case "done":
        return "Teams setup complete";
    }
  });

  const stepDescription = $derived.by(() => {
    switch (currentStep) {
      case "prereqs":
        return "Step 1 of 5: Prerequisites and environment check";
      case "form":
        return "Step 2 of 5: Bot manifest and details";
      case "progress":
        return "Step 3 of 5: Provisioning and configuration in progress";
      case "failed":
        return "Step 4 of 5: Review errors and cloud resources";
      case "done":
        return "Step 5 of 5: Install app and finish configuration";
    }
  });

  async function loadInitial(): Promise<void> {
    loadingPrereqs = true;
    prereqsError = null;
    try {
      const [existingJob, p] = await Promise.all([
        fetchTeamsSetupJob(agent),
        fetchTeamsSetupPrereqs(agent),
      ]);
      job = existingJob;
      if (existingJob !== null) {
        if (existingJob.state === "running" || existingJob.state === "waiting_for_user") {
          currentStep = "progress";
        } else if (existingJob.state === "failed" || existingJob.state === "cancelled") {
          currentStep = "failed";
        } else if (existingJob.state === "succeeded") {
          currentStep = "done";
        } else if (currentStep !== "form") {
          currentStep = "prereqs";
        }
      } else if (currentStep !== "form") {
        currentStep = "prereqs";
      }

      prereqs = p;
      if (botName === "") {
        botName = agent;
      }
      if (messagingEndpoint === "" && p.suggested_endpoint !== null) {
        messagingEndpoint = p.suggested_endpoint;
      }
    } catch (err: unknown) {
      if (err instanceof Error) {
        prereqsError = err.message;
      } else {
        prereqsError = "Failed to load prerequisites";
      }
    } finally {
      loadingPrereqs = false;
    }
  }

  $effect(() => {
    if (open) {
      void loadInitial();
    }
  });

  // Elapsed time ticker
  $effect(() => {
    if (!open || currentStep !== "progress") return;
    const ticker = setInterval(() => {
      now = Date.now();
    }, 1000);
    return () => clearInterval(ticker);
  });

  // Polling loop for active progress
  $effect(() => {
    if (!open || currentStep !== "progress") return;
    let active = true;
    let pollTimer: ReturnType<typeof setTimeout> | undefined;

    async function poll(): Promise<void> {
      if (!active) return;
      try {
        const updated = await fetchTeamsSetupJob(agent);
        if (!active) return;
        if (updated !== null) {
          job = updated;
          if (updated.state === "succeeded") {
            currentStep = "done";
            return;
          }
          if (updated.state === "failed" || updated.state === "cancelled") {
            currentStep = "failed";
            return;
          }
        }
      } catch {
        // Polling retry on next tick
      }
      if (!active) return;
      const interval = job?.state === "waiting_for_user" ? 3000 : 1000;
      pollTimer = setTimeout(() => {
        void poll();
      }, interval);
    }

    const interval = job?.state === "waiting_for_user" ? 3000 : 1000;
    pollTimer = setTimeout(() => {
      void poll();
    }, interval);

    return () => {
      active = false;
      if (pollTimer !== undefined) clearTimeout(pollTimer);
    };
  });

  function formatElapsed(startedAt: string | null | undefined): string {
    if (startedAt === null || startedAt === undefined || startedAt === "") return "0s";
    const start = Date.parse(startedAt);
    if (Number.isNaN(start)) return "0s";
    const sec = Math.max(0, Math.floor((now - start) / 1000));
    const mins = Math.floor(sec / 60);
    const remSec = sec % 60;
    return mins > 0 ? `${mins}m ${remSec}s` : `${sec}s`;
  }

  async function handleColorIconChange(
    e: Event & { currentTarget: HTMLInputElement },
  ): Promise<void> {
    const file = e.currentTarget.files?.[0];
    if (file === undefined) return;
    const res = await validateIcon(file, 192, 192);
    if (res.valid && res.base64 !== null) {
      colorIconBase64 = res.base64;
      colorIconPreview = `data:image/png;base64,${res.base64}`;
      colorIconError = null;
    } else {
      colorIconBase64 = null;
      colorIconPreview = null;
      colorIconError = res.error ?? "Invalid color icon";
    }
  }

  async function handleOutlineIconChange(
    e: Event & { currentTarget: HTMLInputElement },
  ): Promise<void> {
    const file = e.currentTarget.files?.[0];
    if (file === undefined) return;
    const res = await validateIcon(file, 32, 32);
    if (res.valid && res.base64 !== null) {
      outlineIconBase64 = res.base64;
      outlineIconPreview = `data:image/png;base64,${res.base64}`;
      outlineIconError = null;
    } else {
      outlineIconBase64 = null;
      outlineIconPreview = null;
      outlineIconError = res.error ?? "Invalid outline icon";
    }
  }

  function extractErrorMessage(err: unknown): string {
    if (err instanceof ApiError) {
      try {
        const parsed: unknown = JSON.parse(err.body);
        if (typeof parsed === "object" && parsed !== null) {
          if ("message" in parsed && typeof (parsed as { message: unknown }).message === "string") {
            return (parsed as { message: string }).message;
          }
          if ("error" in parsed && typeof (parsed as { error: unknown }).error === "string") {
            return (parsed as { error: string }).error;
          }
        }
      } catch {
        if (err.body.trim() !== "") return err.body.trim();
      }
    }
    if (err instanceof Error) {
      return err.message;
    }
    return "An unexpected error occurred.";
  }

  async function handleStartSetup(): Promise<void> {
    formSubmitting = true;
    formError = null;
    try {
      const form: TeamsSetupForm = {
        bot_name: botName.trim(),
        short_description: shortDescription.trim(),
        long_description: longDescription.trim(),
        developer_name: developerName.trim(),
        developer_url: developerUrl.trim(),
        privacy_url: privacyUrl.trim() !== "" ? privacyUrl.trim() : null,
        terms_url: termsUrl.trim() !== "" ? termsUrl.trim() : null,
        messaging_endpoint: messagingEndpoint.trim(),
        color_icon_png_base64: colorIconBase64,
        outline_icon_png_base64: outlineIconBase64,
      };
      const startReq: TeamsSetupStart = {
        form,
        consent_install_cli: consentInstallCli,
        replace_existing: replaceExisting,
      };
      const started = await startTeamsSetupJob(agent, startReq);
      job = started;
      currentStep = "progress";
    } catch (err: unknown) {
      formError = extractErrorMessage(err);
    } finally {
      formSubmitting = false;
    }
  }

  async function handleRedirectSubmit(): Promise<void> {
    redirectSubmitting = true;
    redirectError = null;
    try {
      const updated = await submitTeamsSetupRedirect(agent, redirectUrl.trim());
      job = updated;
      redirectUrl = "";
      if (updated.state === "succeeded") {
        currentStep = "done";
      } else if (updated.state === "failed") {
        currentStep = "failed";
      }
    } catch (err: unknown) {
      redirectError = extractErrorMessage(err);
    } finally {
      redirectSubmitting = false;
    }
  }

  async function handleCancel(): Promise<void> {
    cancelling = true;
    try {
      const updated = await cancelTeamsSetupJob(agent);
      job = updated;
      currentStep = "failed";
    } catch {
      // Continue to check state
    } finally {
      cancelling = false;
    }
  }

  async function handleRetry(): Promise<void> {
    retrying = true;
    try {
      const updated = await retryTeamsSetupJob(agent);
      job = updated;
      currentStep = "progress";
    } catch (err: unknown) {
      if (err instanceof Error) {
        prereqsError = err.message;
      }
    } finally {
      retrying = false;
    }
  }

  async function handleInstallApp(): Promise<void> {
    installingApp = true;
    installError = null;
    try {
      const updated = await installTeamsApp(agent);
      job = updated;
    } catch (err: unknown) {
      installError =
        extractErrorMessage(err) ||
        "Could not install app. Custom app uploading may be disabled by your tenant admin or requires IT approval.";
    } finally {
      installingApp = false;
    }
  }

  async function handleDownloadPackage(): Promise<void> {
    try {
      const blob = await fetchTeamsAppPackage(agent);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${agent}-teams-package.zip`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
    } catch {
      window.open(teamsAppPackageUrl(agent), "_blank");
    }
  }

  async function handleCleanup(): Promise<void> {
    cleanupLoading = true;
    try {
      await cleanupTeamsSetup(agent, {
        project_files: true,
        cli: false,
        sign_out: false,
      });
      cleanupSuccess = true;
    } catch {
      // Ignore
    } finally {
      cleanupLoading = false;
    }
  }

  async function handleStartOver(): Promise<void> {
    try {
      await deleteTeamsSetupJob(agent);
    } catch {
      // Ignore
    }
    job = null;
    currentStep = "prereqs";
    void loadInitial();
  }

  function handleClose(): void {
    open = false;
    onclose?.();
  }

  function handleDoneFinish(): void {
    open = false;
    onsuccess?.();
    onclose?.();
  }
</script>

<Dialog bind:open size="lg" title={stepTitle} description={stepDescription} onclose={handleClose}>
  <div class="wizard-container">
    {#if currentStep === "prereqs"}
      {#if loadingPrereqs}
        <div class="loading-state">
          <Spinner size={24} />
          <p>Checking prerequisites...</p>
        </div>
      {:else if prereqsError !== null}
        <Banner tone="error" title="Prerequisites check failed">
          {prereqsError}
        </Banner>
        <div class="action-buttons-single">
          <Button variant="secondary" onclick={() => void loadInitial()}>Retry</Button>
        </div>
      {:else if prereqs !== null}
        <div class="step-content">
          <p class="step-lede">
            Residuum uses Microsoft 365 Agents Toolkit to configure, provision, and package your
            Teams bot.
          </p>

          <div class="card">
            <h3 class="card-title">Environment status</h3>

            <div class="status-grid">
              <div class="status-item">
                <span class="status-label">Node.js:</span>
                <span class="status-value">
                  {#if prereqs.node.found && prereqs.node.version !== null}
                    <Badge tone="positive" dot>{prereqs.node.version}</Badge>
                  {:else}
                    <Badge tone="danger" dot>Missing</Badge>
                  {/if}
                </span>
              </div>

              <div class="status-item">
                <span class="status-label">npm:</span>
                <span class="status-value">
                  {#if prereqs.npm.found && prereqs.npm.version !== null}
                    <Badge tone="positive" dot>{prereqs.npm.version}</Badge>
                  {:else}
                    <Badge tone="neutral" dot>Missing</Badge>
                  {/if}
                </span>
              </div>

              <div class="status-item">
                <span class="status-label">Agents Toolkit CLI:</span>
                <span class="status-value">
                  {#if prereqs.atk.installed}
                    <Badge tone="positive" dot
                      >Installed ({prereqs.atk.version ?? prereqs.atk.pinned_version})</Badge
                    >
                  {:else}
                    <Badge tone="neutral" dot>Not installed</Badge>
                  {/if}
                </span>
              </div>
            </div>

            {#if !prereqs.atk.installed}
              <p class="install-dir-note">
                CLI pinned version <code>{prereqs.atk.pinned_version}</code> will be installed to:
                <code>{prereqs.atk.install_dir}</code>
              </p>
            {/if}
          </div>

          {#if nodeMissing}
            <Banner tone="error" title="Node.js 18+ required">
              Node.js was not found on your system. Microsoft 365 Agents Toolkit requires Node.js {prereqs.min_node_version}
              or newer. Please install Node.js and restart Residuum.
            </Banner>
          {/if}

          {#if prereqs.teams_already_configured}
            <div class="checkbox-container">
              <Banner tone="warn">
                Teams is already configured for this agent. Setting it up again will overwrite your
                current credentials and manifest.
              </Banner>
              <label class="checkbox-row">
                <input type="checkbox" class="checkbox-input" bind:checked={replaceExisting} />
                <span>Replace existing Teams configuration for {agent}</span>
              </label>
            </div>
          {/if}

          {#if !prereqs.atk.installed}
            <div class="checkbox-container">
              <label class="checkbox-row">
                <input type="checkbox" class="checkbox-input" bind:checked={consentInstallCli} />
                <span>
                  I agree to install @microsoft/teams-app-cli ({prereqs.atk.pinned_version}) in
                  Residuum's tools directory
                </span>
              </label>
            </div>
          {/if}

          <div class="manual-guide-link-row">
            <a
              href={prereqs.manual_guide_url}
              target="_blank"
              rel="noopener noreferrer"
              class="guide-link"
            >
              Manual setup guide
              <Icon name="external-link" size={13} />
            </a>
          </div>
        </div>
      {/if}
    {:else if currentStep === "form"}
      <div class="step-content">
        {#if formError !== null}
          <Banner tone="error" title="Setup error">{formError}</Banner>
        {/if}

        <TextField label="Bot name" bind:value={botName} placeholder={agent} required />

        <TextField
          label="Short description"
          bind:value={shortDescription}
          hint={`${shortDescription.length}/80`}
          maxlength={80}
          placeholder="Brief summary of the bot for the Teams app catalog"
          required
        />

        <TextField
          label="Long description"
          bind:value={longDescription}
          hint={`${longDescription.length}/4000`}
          maxlength={4000}
          multiline
          rows={3}
          placeholder="Detailed description of what the bot does and how to interact with it"
          required
        />

        <div class="form-row-2">
          <TextField
            label="Developer name"
            bind:value={developerName}
            placeholder="Your name or organization"
            required
          />
          <TextField
            label="Developer website"
            type="url"
            bind:value={developerUrl}
            placeholder="https://example.com"
            required
          />
        </div>

        <div class="form-row-2">
          <TextField
            label="Privacy policy URL (optional)"
            type="url"
            bind:value={privacyUrl}
            placeholder="https://example.com/privacy"
          />
          <TextField
            label="Terms of use URL (optional)"
            type="url"
            bind:value={termsUrl}
            placeholder="https://example.com/terms"
          />
        </div>

        <TextField
          label="Messaging endpoint"
          type="url"
          bind:value={messagingEndpoint}
          placeholder="https://your-domain.com/api/messages"
          required
        />

        {#if messagingEndpoint.trim() !== "" && !messagingEndpoint.startsWith("https://")}
          <Banner tone="warn">
            Microsoft Teams requires an HTTPS messaging endpoint. Residuum Cloud or an HTTPS tunnel
            is needed.
          </Banner>
        {/if}

        <div class="icons-section">
          <h4 class="icons-title">App Icons</h4>

          <div class="icons-grid">
            <div class="icon-upload-box">
              <span class="icon-upload-label">Color Icon (192×192 PNG)</span>
              {#if colorIconPreview !== null}
                <div class="icon-preview-wrapper">
                  <img src={colorIconPreview} alt="Color icon preview" class="icon-preview-color" />
                </div>
              {/if}
              <input
                type="file"
                accept="image/png"
                class="file-input"
                aria-label="Color Icon (192x192 PNG)"
                onchange={handleColorIconChange}
              />
              {#if colorIconError !== null}
                <p class="field-error">{colorIconError}</p>
              {/if}
            </div>

            <div class="icon-upload-box">
              <span class="icon-upload-label">Outline Icon (32×32 PNG)</span>
              {#if outlineIconPreview !== null}
                <div class="icon-preview-wrapper">
                  <img
                    src={outlineIconPreview}
                    alt="Outline icon preview"
                    class="icon-preview-outline"
                  />
                </div>
              {/if}
              <input
                type="file"
                accept="image/png"
                class="file-input"
                aria-label="Outline Icon (32x32 PNG)"
                onchange={handleOutlineIconChange}
              />
              {#if outlineIconError !== null}
                <p class="field-error">{outlineIconError}</p>
              {/if}
            </div>
          </div>
        </div>
      </div>
    {:else if currentStep === "progress"}
      <div class="step-content">
        <div class="progress-header">
          <span class="progress-title">Provisioning Teams integration</span>
          <span class="progress-elapsed">Elapsed: {formatElapsed(job?.started_at)}</span>
        </div>

        <div class="phase-list">
          {#each PHASES as item (item.phase)}
            {@const isCompleted = job?.completed_phases.includes(item.phase) ?? false}
            {@const isCurrent = job?.phase === item.phase}
            <div class="phase-row" data-current={isCurrent || undefined}>
              <span class="phase-icon">
                {#if isCompleted}
                  <Icon name="check" size={16} />
                {:else if isCurrent && job?.state === "running"}
                  <Spinner size={14} />
                {:else if isCurrent && job?.state === "waiting_for_user"}
                  <Icon name="info" size={16} />
                {:else if isCurrent && job?.state === "failed"}
                  <Icon name="warning" size={16} />
                {:else}
                  <span class="phase-bullet"></span>
                {/if}
              </span>
              <span class="phase-label">{item.label}</span>
              <span class="phase-status">
                {#if isCompleted}
                  Completed
                {:else if isCurrent && job?.state === "running"}
                  In progress...
                {:else if isCurrent && job?.state === "waiting_for_user"}
                  Waiting for sign-in
                {:else if isCurrent && job?.state === "failed"}
                  Failed
                {:else}
                  Pending
                {/if}
              </span>
            </div>
          {/each}
        </div>

        {#if job?.state === "waiting_for_user" && job.sign_in !== null}
          <div class="sign-in-box">
            <Banner tone="info" title="Authentication required">
              Sign in to your Microsoft 365 tenant to authorize the Agents Toolkit.
            </Banner>

            <div class="sign-in-body">
              <a
                href={job.sign_in.login_url}
                target="_blank"
                rel="noopener noreferrer"
                class="sign-in-link-btn"
              >
                Sign in with Microsoft 365
                <Icon name="external-link" size={14} />
              </a>

              <p class="sign-in-note">
                Once signed in in your browser, copy the full URL from the browser's address bar and
                paste it below:
              </p>

              <div class="redirect-submit-row">
                <div class="redirect-field">
                  <TextField
                    label="Redirect URL"
                    bind:value={redirectUrl}
                    placeholder="http://localhost:... or paste redirect URL"
                    error={redirectError ?? undefined}
                  />
                </div>
                <Button
                  variant="primary"
                  loading={redirectSubmitting}
                  disabled={redirectUrl.trim() === ""}
                  onclick={handleRedirectSubmit}
                >
                  Submit
                </Button>
              </div>
            </div>
          </div>
        {/if}

        {#if job !== null && job.log.length > 0}
          <div class="logs-wrapper">
            <Disclosure summary="Show logs ({job.log.length} lines)" bind:open={logsOpen}>
              <div class="log-console">
                {#each job.log as line (line.seq)}
                  <div class="log-row" data-stream={line.stream}>
                    <span class="log-time">{line.at}</span>
                    <span class="log-text">{line.text}</span>
                  </div>
                {/each}
              </div>
            </Disclosure>
          </div>
        {/if}
      </div>
    {:else if currentStep === "failed"}
      <div class="step-content">
        <Banner
          tone="error"
          title={job?.state === "cancelled" ? "Setup cancelled" : "Setup failed"}
        >
          {#if job?.state === "cancelled"}
            The Teams setup operation was cancelled.
          {:else}
            {job?.error?.message ?? "An unexpected error occurred during Teams setup."}
          {/if}
        </Banner>

        {#if job?.error?.phase !== undefined}
          <p class="failed-phase-note">
            Failed during phase: <strong>{job.error.phase}</strong>
          </p>
        {/if}

        {#if job?.error?.detail !== null && job?.error?.detail !== undefined}
          <Disclosure summary="Technical error details" bind:open={errorDetailsOpen}>
            <pre class="error-detail-content">{job.error.detail}</pre>
          </Disclosure>
        {/if}

        {#if job?.created !== undefined && (job.created.bot_id !== null || job.created.teams_app_id !== null || job.created.tenant_id !== null)}
          <div class="card">
            <h3 class="card-title">Created tenant resources</h3>

            {#if job.created.bot_id !== null}
              <div class="resource-item">
                <span class="resource-label">Bot ID:</span>
                <code>{job.created.bot_id}</code>
                <a
                  href={job.created.entra_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  class="resource-link"
                >
                  Entra Admin Center
                  <Icon name="external-link" size={13} />
                </a>
              </div>
            {/if}

            {#if job.created.teams_app_id !== null}
              <div class="resource-item">
                <span class="resource-label">Teams App ID:</span>
                <code>{job.created.teams_app_id}</code>
                <a
                  href={job.created.dev_portal_url}
                  target="_blank"
                  rel="noopener noreferrer"
                  class="resource-link"
                >
                  Developer Portal
                  <Icon name="external-link" size={13} />
                </a>
              </div>
            {/if}

            {#if job.created.tenant_id !== null}
              <div class="resource-item">
                <span class="resource-label">Tenant ID:</span>
                <code>{job.created.tenant_id}</code>
              </div>
            {/if}
          </div>
        {/if}

        <div class="manual-guide-link-row">
          <a
            href={prereqs?.manual_guide_url ?? "https://dev.teams.microsoft.com/bots"}
            target="_blank"
            rel="noopener noreferrer"
            class="guide-link"
          >
            Manual setup guide
            <Icon name="external-link" size={13} />
          </a>
        </div>
      </div>
    {:else if currentStep === "done"}
      <div class="step-content">
        <Banner tone="info" title="Teams setup complete">
          Microsoft Teams integration has been provisioned and configured for {agent}.
        </Banner>

        <div class="card">
          <h3 class="card-title">Configured Bot Resources</h3>
          <div class="resource-item">
            <span class="resource-label">Bot ID:</span>
            <code>{job?.result?.bot_id ?? job?.created.bot_id}</code>
          </div>
          <div class="resource-item">
            <span class="resource-label">Tenant ID:</span>
            <code>{job?.result?.tenant_id ?? job?.created.tenant_id}</code>
          </div>
        </div>

        <div class="done-actions-row">
          {#if job?.app_installed}
            <Badge tone="positive" dot>App installed in Teams</Badge>
          {:else}
            <Button variant="primary" loading={installingApp} onclick={handleInstallApp}>
              Install in Teams
            </Button>
          {/if}

          <Button variant="secondary" onclick={handleDownloadPackage}>Download app package</Button>
        </div>

        {#if installError !== null}
          <Banner tone="error">{installError}</Banner>
        {/if}

        <div class="owner-reminder-box">
          <p class="reminder-text">
            <strong>Direct message reminder:</strong> Before coworkers can message the bot, you (the owner)
            must initiate the first direct message with the bot in Microsoft Teams, unless "Let others
            talk to this agent" is enabled.
          </p>
        </div>

        <div class="cleanup-box">
          <Button
            variant="quiet"
            size="sm"
            loading={cleanupLoading}
            disabled={cleanupSuccess}
            onclick={handleCleanup}
          >
            {cleanupSuccess ? "Temporary files cleaned up" : "Clean up temporary files"}
          </Button>
        </div>
      </div>
    {/if}
  </div>

  {#snippet actions()}
    {#if currentStep === "prereqs"}
      <Button variant="quiet" onclick={handleClose}>Cancel</Button>
      <Button
        variant="primary"
        disabled={!canContinueFromPrereqs}
        onclick={() => (currentStep = "form")}
      >
        Next
      </Button>
    {:else if currentStep === "form"}
      <Button variant="quiet" onclick={() => (currentStep = "prereqs")}>Back</Button>
      <Button
        variant="primary"
        loading={formSubmitting}
        disabled={!formValid}
        onclick={handleStartSetup}
      >
        Start setup
      </Button>
    {:else if currentStep === "progress"}
      <Button variant="danger" loading={cancelling} onclick={handleCancel}>Cancel setup</Button>
    {:else if currentStep === "failed"}
      <Button variant="quiet" onclick={handleStartOver}>Start over</Button>
      <Button variant="secondary" loading={cleanupLoading} onclick={handleCleanup}>Clean up</Button>
      <Button variant="primary" loading={retrying} onclick={handleRetry}>Retry</Button>
    {:else if currentStep === "done"}
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

  .step-content {
    display: flex;
    flex-direction: column;
    gap: var(--space-16);
  }

  .step-lede {
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
    line-height: var(--line-height-ui);
  }

  .card {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-16);
    background: var(--color-stone-2);
    border: 1px solid var(--color-line);
    border-radius: var(--corner-md);
  }

  .card-title {
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-semibold);
    color: var(--color-text);
  }

  .status-grid {
    display: grid;
    grid-template-columns: repeat(auto-fit, minmax(160px, 1fr));
    gap: var(--space-12);
  }

  .status-item {
    display: flex;
    flex-direction: column;
    gap: var(--space-4);
  }

  .status-label {
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
  }

  .install-dir-note {
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
    word-break: break-all;
  }

  .checkbox-container {
    display: flex;
    flex-direction: column;
    gap: var(--space-8);
  }

  .checkbox-row {
    display: flex;
    align-items: flex-start;
    gap: var(--space-8);
    font-size: var(--font-size-sm);
    color: var(--color-text);
    cursor: pointer;
    user-select: none;
  }

  .checkbox-input {
    margin-top: var(--space-2);
    cursor: pointer;
    accent-color: var(--color-vein);
  }

  .manual-guide-link-row {
    display: flex;
    align-items: center;
  }

  .guide-link {
    display: inline-flex;
    align-items: center;
    gap: var(--space-6);
    font-size: var(--font-size-sm);
    color: var(--color-vein-bright);
    text-decoration: none;

    &:hover {
      text-decoration: underline;
    }
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

  .progress-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
  }

  .progress-title {
    color: var(--color-text);
  }

  .progress-elapsed {
    color: var(--color-text-3);
    font-variant-numeric: tabular-nums;
  }

  .phase-list {
    display: flex;
    flex-direction: column;
    gap: var(--space-6);
    padding: var(--space-12);
    background: var(--color-stone-2);
    border-radius: var(--corner-md);
  }

  .phase-row {
    display: flex;
    align-items: center;
    gap: var(--space-10);
    padding: var(--space-6) var(--space-8);
    border-radius: var(--corner-sm);
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .phase-row[data-current] {
    color: var(--color-text);
    background: var(--color-stone-3);
  }

  .phase-icon {
    display: grid;
    place-items: center;
    width: 18px;
    height: 18px;
    color: var(--color-vein-bright);
  }

  .phase-bullet {
    width: 6px;
    height: 6px;
    border-radius: 50%;
    background: var(--color-text-3);
  }

  .phase-label {
    flex: 1;
    min-width: 0;
  }

  .phase-status {
    font-size: var(--font-size-xs);
    color: var(--color-text-3);
  }

  .sign-in-box {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
    padding: var(--space-16);
    background: var(--color-stone-2);
    border: 1px solid var(--color-vein-line);
    border-radius: var(--corner-md);
  }

  .sign-in-body {
    display: flex;
    flex-direction: column;
    gap: var(--space-12);
  }

  .sign-in-link-btn {
    display: inline-flex;
    align-items: center;
    align-self: flex-start;
    gap: var(--space-8);
    height: 32px;
    padding: 0 var(--space-12);
    background: var(--color-vein-dim);
    color: var(--color-on-accent);
    border-radius: var(--corner-sm);
    font-size: var(--font-size-sm);
    font-weight: var(--font-weight-medium);
    text-decoration: none;

    &:hover {
      background: var(--color-vein-hover);
    }
  }

  .sign-in-note {
    font-size: var(--font-size-xs);
    color: var(--color-text-2);
  }

  .redirect-submit-row {
    display: flex;
    align-items: flex-end;
    gap: var(--space-8);
  }

  .redirect-field {
    flex: 1;
    min-width: 0;
  }

  .logs-wrapper {
    margin-top: var(--space-4);
  }

  .log-console {
    max-height: 200px;
    overflow-y: auto;
    padding: var(--space-8);
    background: var(--color-stone-1);
    border-radius: var(--corner-sm);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    line-height: var(--line-height-tight);
  }

  .log-row {
    display: flex;
    gap: var(--space-8);
    color: var(--color-text-2);
    white-space: pre-wrap;
    word-break: break-all;
  }

  .log-row[data-stream="stderr"] {
    color: var(--color-err-text);
  }

  .log-time {
    color: var(--color-text-3);
    flex-shrink: 0;
  }

  .log-text {
    flex: 1;
  }

  .failed-phase-note {
    font-size: var(--font-size-sm);
    color: var(--color-text-2);
  }

  .error-detail-content {
    padding: var(--space-8);
    background: var(--color-stone-1);
    border-radius: var(--corner-sm);
    font-family: var(--font-code);
    font-size: var(--font-size-xs);
    color: var(--color-err-text);
    white-space: pre-wrap;
    word-break: break-all;
    max-height: 200px;
    overflow-y: auto;
  }

  .resource-item {
    display: flex;
    align-items: center;
    gap: var(--space-8);
    font-size: var(--font-size-sm);
  }

  .resource-label {
    color: var(--color-text-3);
  }

  .resource-link {
    display: inline-flex;
    align-items: center;
    gap: var(--space-4);
    font-size: var(--font-size-xs);
    color: var(--color-vein-bright);
    text-decoration: none;

    &:hover {
      text-decoration: underline;
    }
  }

  .done-actions-row {
    display: flex;
    align-items: center;
    gap: var(--space-12);
  }

  .owner-reminder-box {
    padding: var(--space-12);
    background: var(--color-stone-2);
    border-left: 3px solid var(--color-vein-bright);
    border-radius: var(--corner-sm);
  }

  .reminder-text {
    font-size: var(--font-size-xs);
    color: var(--color-text-2);
    line-height: var(--line-height-ui);
  }

  .cleanup-box {
    margin-top: var(--space-4);
  }

  .loading-state {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: var(--space-12);
    padding: var(--space-32) 0;
    color: var(--color-text-3);
    font-size: var(--font-size-sm);
  }

  .action-buttons-single {
    display: flex;
    justify-content: flex-end;
  }

  @container (max-width: 500px) {
    .form-row-2,
    .icons-grid {
      grid-template-columns: 1fr;
    }
  }
</style>
