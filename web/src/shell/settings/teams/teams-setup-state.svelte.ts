import { SvelteSet } from "svelte/reactivity";
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
import { userErrorMessage } from "../../../lib/errors";
import type {
  LogLine,
  TeamsSetupForm,
  TeamsSetupJob,
  TeamsSetupPhase,
  TeamsSetupPrereqs,
  TeamsSetupStart,
} from "../../../lib/types";
import { validateIcon } from "./icon-utils";

export type WizardStep = "prereqs" | "form" | "progress" | "failed" | "done";

export const TEAMS_SETUP_PHASES: readonly { phase: TeamsSetupPhase; label: string }[] = [
  { phase: "check_prereqs", label: "Check prerequisites" },
  { phase: "install_cli", label: "Install Agents Toolkit CLI" },
  { phase: "sign_in", label: "Sign in to Microsoft 365" },
  { phase: "scaffold", label: "Scaffold Teams app" },
  { phase: "provision", label: "Provision cloud resources" },
  { phase: "import", label: "Import bot registration" },
  { phase: "install_app", label: "Install in Teams" },
];

export const MAX_CLIENT_LOG_LINES = 1000;

export class TeamsSetupState {
  private readonly getAgent: () => string;

  get agent(): string {
    return this.getAgent();
  }

  currentStep = $state<WizardStep>("prereqs");
  loadingPrereqs = $state(true);
  prereqsError = $state<string | null>(null);
  prereqs = $state<TeamsSetupPrereqs | null>(null);

  job = $state<TeamsSetupJob | null>(null);
  logs = $state<LogLine[]>([]);
  lastSeq = $state(0);
  trimmedLogsCount = $state(0);

  lostContact = $state(false);
  lastSuccessfulUpdate = $state<number | null>(null);
  now = $state(Date.now());

  // Step 1: Pre-requisites & Consent
  replaceExisting = $state(false);
  consentInstallCli = $state(false);

  // Step 2: Form fields
  botName = $state("");
  shortDescription = $state("");
  longDescription = $state("");
  developerName = $state("");
  developerUrl = $state("");
  privacyUrl = $state("");
  termsUrl = $state("");
  messagingEndpoint = $state("");
  colorIconBase64 = $state<string | null>(null);
  outlineIconBase64 = $state<string | null>(null);
  colorIconError = $state<string | null>(null);
  outlineIconError = $state<string | null>(null);
  colorIconPreview = $state<string | null>(null);
  outlineIconPreview = $state<string | null>(null);
  formSubmitting = $state(false);
  formError = $state<string | null>(null);

  // Step 3: Progress & Redirect
  redirectUrl = $state("");
  redirectSubmitting = $state(false);
  redirectError = $state<string | null>(null);
  logsOpen = $state(false);
  cancelling = $state(false);

  // Step 4: Failed & Retry
  retrying = $state(false);
  errorDetailsOpen = $state(false);

  // Step 5: Done & Post-setup
  installingApp = $state(false);
  installError = $state<string | null>(null);
  cleanupLoading = $state(false);
  cleanupSuccess = $state(false);

  private pollingActive = false;
  private pollTimer: ReturnType<typeof setTimeout> | undefined;
  private tickerTimer: ReturnType<typeof setInterval> | undefined;

  constructor(getAgent: () => string) {
    this.getAgent = getAgent;
  }

  // Derived validations for Step 1
  get nodeMissing(): boolean {
    return (
      this.prereqs !== null && (!this.prereqs.node.found || this.prereqs.node.version === null)
    );
  }

  get replaceRequired(): boolean {
    return this.prereqs?.teams_already_configured ?? false;
  }

  get consentRequired(): boolean {
    return this.prereqs !== null && !this.prereqs.atk.installed;
  }

  get canContinueFromPrereqs(): boolean {
    if (this.loadingPrereqs || this.prereqs === null) return false;
    if (this.nodeMissing) return false;
    if (this.replaceRequired && !this.replaceExisting) return false;
    if (this.consentRequired && !this.consentInstallCli) return false;
    return true;
  }

  // Derived validations for Step 2
  get formValid(): boolean {
    if (this.botName.trim() === "") return false;
    if (this.shortDescription.trim() === "" || this.shortDescription.length > 80) return false;
    if (this.longDescription.trim() === "" || this.longDescription.length > 4000) return false;
    if (this.developerName.trim() === "") return false;
    if (this.developerUrl.trim() === "") return false;
    if (this.messagingEndpoint.trim() === "") return false;
    if (this.colorIconError !== null || this.outlineIconError !== null) return false;
    return true;
  }

  get stepTitle(): string {
    switch (this.currentStep) {
      case "prereqs":
      case "form":
        return "Set up Microsoft Teams";
      case "progress":
        return "Setting up Microsoft Teams";
      case "failed":
        return this.job?.state === "cancelled" ? "Teams setup cancelled" : "Teams setup failed";
      case "done":
        return "Teams setup complete";
    }
  }

  get stepDescription(): string {
    switch (this.currentStep) {
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
  }

  get timeSinceLastUpdate(): string {
    if (this.lastSuccessfulUpdate === null) return "never";
    const sec = Math.max(0, Math.floor((this.now - this.lastSuccessfulUpdate) / 1000));
    if (sec < 60) return `${sec}s ago`;
    const min = Math.floor(sec / 60);
    return `${min}m ${sec % 60}s ago`;
  }

  get liveAnnouncement(): string {
    if (this.lostContact) {
      return `Lost contact with Residuum, retrying. Last update was ${this.timeSinceLastUpdate}.`;
    }
    if (this.job?.state === "waiting_for_user" && this.job.phase === "sign_in") {
      return "Action required: Microsoft 365 sign-in required.";
    }
    if (this.currentStep === "progress" && this.job?.phase) {
      const match = TEAMS_SETUP_PHASES.find((p) => p.phase === this.job?.phase);
      return `Teams setup in progress: ${match?.label ?? this.job.phase}.`;
    }
    return "";
  }

  formatElapsed(startedAt: string | null | undefined): string {
    if (!startedAt) return "0s";
    const start = Date.parse(startedAt);
    if (Number.isNaN(start)) return "0s";
    const sec = Math.max(0, Math.floor((this.now - start) / 1000));
    const mins = Math.floor(sec / 60);
    const remSec = sec % 60;
    return mins > 0 ? `${mins}m ${remSec}s` : `${sec}s`;
  }

  private extractError(err: unknown, defaultMessage: string): string {
    if (err instanceof ApiError) {
      try {
        const parsed = JSON.parse(err.body) as unknown;
        if (typeof parsed === "object" && parsed !== null) {
          if ("message" in parsed && typeof (parsed as { message: unknown }).message === "string") {
            const msg = (parsed as { message: string }).message.trim();
            if (msg.length > 0) return msg;
          }
          if ("error" in parsed && typeof (parsed as { error: unknown }).error === "string") {
            const msg = (parsed as { error: string }).error.trim();
            if (msg.length > 0) return msg;
          }
        }
      } catch {
        if (err.body.trim() !== "") return err.body.trim();
      }
      return userErrorMessage(err, { action: defaultMessage });
    }
    if (err instanceof TypeError) {
      return userErrorMessage(err, { action: defaultMessage });
    }
    if (err instanceof Error) {
      return err.message;
    }
    return defaultMessage;
  }

  private appendLogs(newLogs: LogLine[]): void {
    if (newLogs.length === 0) return;
    const existingSeqs = new SvelteSet(this.logs.map((l) => l.seq));
    const added = newLogs.filter((l) => !existingSeqs.has(l.seq));
    if (added.length === 0) return;

    const merged = [...this.logs, ...added];
    for (const l of added) {
      if (l.seq > this.lastSeq) {
        this.lastSeq = l.seq;
      }
    }

    if (merged.length > MAX_CLIENT_LOG_LINES) {
      const excess = merged.length - MAX_CLIENT_LOG_LINES;
      this.logs = merged.slice(excess);
      this.trimmedLogsCount += excess;
    } else {
      this.logs = merged;
    }
  }

  async loadInitial(): Promise<void> {
    this.loadingPrereqs = true;
    this.prereqsError = null;
    this.logs = [];
    this.lastSeq = 0;
    this.trimmedLogsCount = 0;
    this.lostContact = false;

    try {
      // Reopen fetches from 0 per contract
      const [existingJob, p] = await Promise.all([
        fetchTeamsSetupJob(this.agent, 0),
        fetchTeamsSetupPrereqs(this.agent),
      ]);

      this.job = existingJob;
      this.lastSuccessfulUpdate = Date.now();
      if (existingJob !== null) {
        this.appendLogs(existingJob.log);
        if (existingJob.last_seq > this.lastSeq) {
          this.lastSeq = existingJob.last_seq;
        }

        if (existingJob.state === "running" || existingJob.state === "waiting_for_user") {
          this.currentStep = "progress";
        } else if (existingJob.state === "failed" || existingJob.state === "cancelled") {
          this.currentStep = "failed";
        } else {
          this.currentStep = "done";
        }
      } else if (this.currentStep !== "form") {
        this.currentStep = "prereqs";
      }

      this.prereqs = p;
      if (this.botName === "") {
        this.botName = this.agent;
      }
      if (this.messagingEndpoint === "" && p.suggested_endpoint !== null) {
        this.messagingEndpoint = p.suggested_endpoint;
      }
    } catch (err: unknown) {
      this.prereqsError = this.extractError(err, "Couldn't load prerequisites.");
    } finally {
      this.loadingPrereqs = false;
    }
  }

  startPolling(): void {
    if (this.pollingActive) return;
    this.pollingActive = true;
    this.startTicker();
    const interval = this.job?.state === "waiting_for_user" ? 3000 : 1000;
    this.pollTimer = setTimeout(() => {
      void this.pollTick();
    }, interval);
  }

  stopPolling(): void {
    this.pollingActive = false;
    if (this.pollTimer !== undefined) {
      clearTimeout(this.pollTimer);
      this.pollTimer = undefined;
    }
    this.stopTicker();
  }

  private startTicker(): void {
    if (this.tickerTimer !== undefined) return;
    this.tickerTimer = setInterval(() => {
      this.now = Date.now();
    }, 1000);
  }

  private stopTicker(): void {
    if (this.tickerTimer !== undefined) {
      clearInterval(this.tickerTimer);
      this.tickerTimer = undefined;
    }
  }

  private isPollingActive(): boolean {
    return this.pollingActive;
  }

  private async pollTick(): Promise<void> {
    if (!this.isPollingActive()) return;

    try {
      const updated = await fetchTeamsSetupJob(this.agent, this.lastSeq);
      if (!this.isPollingActive()) return;

      if (updated !== null) {
        this.job = updated;
        this.appendLogs(updated.log);
        if (updated.last_seq > this.lastSeq) {
          this.lastSeq = updated.last_seq;
        }
        this.lostContact = false;
        this.lastSuccessfulUpdate = Date.now();

        if (updated.state === "succeeded") {
          this.currentStep = "done";
          this.stopPolling();
          return;
        }
        if (updated.state === "failed" || updated.state === "cancelled") {
          this.currentStep = "failed";
          this.stopPolling();
          return;
        }
      }
    } catch {
      if (!this.isPollingActive()) return;
      this.lostContact = true;
    }

    if (!this.isPollingActive()) return;

    const interval = this.job?.state === "waiting_for_user" ? 3000 : 1000;

    this.pollTimer = setTimeout(() => {
      void this.pollTick();
    }, interval);
  }

  async handleColorIconChange(file: File | undefined): Promise<void> {
    if (file === undefined) return;
    const res = await validateIcon(file, 192, 192);
    if (res.valid && res.base64 !== null) {
      this.colorIconBase64 = res.base64;
      this.colorIconPreview = `data:image/png;base64,${res.base64}`;
      this.colorIconError = null;
    } else {
      this.colorIconBase64 = null;
      this.colorIconPreview = null;
      this.colorIconError = res.error ?? "Invalid color icon";
    }
  }

  async handleOutlineIconChange(file: File | undefined): Promise<void> {
    if (file === undefined) return;
    const res = await validateIcon(file, 32, 32);
    if (res.valid && res.base64 !== null) {
      this.outlineIconBase64 = res.base64;
      this.outlineIconPreview = `data:image/png;base64,${res.base64}`;
      this.outlineIconError = null;
    } else {
      this.outlineIconBase64 = null;
      this.outlineIconPreview = null;
      this.outlineIconError = res.error ?? "Invalid outline icon";
    }
  }

  async handleStartSetup(): Promise<void> {
    this.formSubmitting = true;
    this.formError = null;
    try {
      const form: TeamsSetupForm = {
        bot_name: this.botName.trim(),
        short_description: this.shortDescription.trim(),
        long_description: this.longDescription.trim(),
        developer_name: this.developerName.trim(),
        developer_url: this.developerUrl.trim(),
        privacy_url: this.privacyUrl.trim() !== "" ? this.privacyUrl.trim() : null,
        terms_url: this.termsUrl.trim() !== "" ? this.termsUrl.trim() : null,
        messaging_endpoint: this.messagingEndpoint.trim(),
        color_icon_png_base64: this.colorIconBase64,
        outline_icon_png_base64: this.outlineIconBase64,
      };
      const startReq: TeamsSetupStart = {
        form,
        consent_install_cli: this.consentInstallCli,
        replace_existing: this.replaceExisting,
      };
      const started = await startTeamsSetupJob(this.agent, startReq);
      this.job = started;
      this.appendLogs(started.log);
      if (started.last_seq > this.lastSeq) {
        this.lastSeq = started.last_seq;
      }
      this.lastSuccessfulUpdate = Date.now();
      this.lostContact = false;
      this.currentStep = "progress";
    } catch (err: unknown) {
      this.formError = this.extractError(err, "Couldn't start Teams setup.");
    } finally {
      this.formSubmitting = false;
    }
  }

  async handleRedirectSubmit(): Promise<void> {
    this.redirectSubmitting = true;
    this.redirectError = null;
    try {
      const updated = await submitTeamsSetupRedirect(this.agent, this.redirectUrl.trim());
      this.job = updated;
      this.appendLogs(updated.log);
      if (updated.last_seq > this.lastSeq) {
        this.lastSeq = updated.last_seq;
      }
      this.lastSuccessfulUpdate = Date.now();
      this.lostContact = false;
      this.redirectUrl = "";
      if (updated.state === "succeeded") {
        this.currentStep = "done";
      } else if (updated.state === "failed") {
        this.currentStep = "failed";
      }
    } catch (err: unknown) {
      this.redirectError = this.extractError(err, "Redirect verification failed.");
    } finally {
      this.redirectSubmitting = false;
    }
  }

  async handleCancel(): Promise<void> {
    this.cancelling = true;
    try {
      const updated = await cancelTeamsSetupJob(this.agent);
      this.job = updated;
      this.appendLogs(updated.log);
      if (updated.last_seq > this.lastSeq) {
        this.lastSeq = updated.last_seq;
      }
      this.lastSuccessfulUpdate = Date.now();
      this.lostContact = false;
      this.currentStep = "failed";
    } catch {
      // Polling or retry handles state
    } finally {
      this.cancelling = false;
    }
  }

  async handleRetry(): Promise<void> {
    this.retrying = true;
    try {
      const updated = await retryTeamsSetupJob(this.agent);
      this.job = updated;
      this.appendLogs(updated.log);
      if (updated.last_seq > this.lastSeq) {
        this.lastSeq = updated.last_seq;
      }
      this.lastSuccessfulUpdate = Date.now();
      this.lostContact = false;
      this.currentStep = "progress";
    } catch (err: unknown) {
      this.prereqsError = this.extractError(err, "Couldn't retry setup.");
    } finally {
      this.retrying = false;
    }
  }

  async handleInstallApp(): Promise<void> {
    this.installingApp = true;
    this.installError = null;
    try {
      const updated = await installTeamsApp(this.agent);
      this.job = updated;
      this.appendLogs(updated.log);
      if (updated.last_seq > this.lastSeq) {
        this.lastSeq = updated.last_seq;
      }
      this.lastSuccessfulUpdate = Date.now();
      this.lostContact = false;
    } catch (err: unknown) {
      this.installError = this.extractError(
        err,
        "Custom app uploading may be disabled by your tenant admin or requires IT approval.",
      );
    } finally {
      this.installingApp = false;
    }
  }

  async handleDownloadPackage(): Promise<void> {
    try {
      const blob = await fetchTeamsAppPackage(this.agent);
      const url = URL.createObjectURL(blob);
      const a = document.createElement("a");
      a.href = url;
      a.download = `${this.agent}-teams-package.zip`;
      document.body.appendChild(a);
      a.click();
      document.body.removeChild(a);
      URL.revokeObjectURL(url);
    } catch {
      window.open(teamsAppPackageUrl(this.agent), "_blank", "noopener,noreferrer");
    }
  }

  async handleCleanup(): Promise<void> {
    this.cleanupLoading = true;
    try {
      await cleanupTeamsSetup(this.agent, {
        project_files: true,
        cli: false,
        sign_out: false,
      });
      this.cleanupSuccess = true;
    } catch {
      // Ignored
    } finally {
      this.cleanupLoading = false;
    }
  }

  async handleStartOver(): Promise<void> {
    try {
      await deleteTeamsSetupJob(this.agent);
    } catch {
      // Ignored
    }
    this.job = null;
    this.logs = [];
    this.lastSeq = 0;
    this.trimmedLogsCount = 0;
    this.lostContact = false;
    this.currentStep = "prereqs";
    void this.loadInitial();
  }
}
