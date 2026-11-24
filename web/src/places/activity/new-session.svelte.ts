import { startSession, type SessionModelSize } from "../../lib/api";
import { userErrorMessage } from "../../lib/errors";
import type { SessionStartFailure } from "../../lib/sessions.svelte";
import type { SessionSummary } from "../../lib/types";
import { ws } from "../../lib/ws.svelte";

export class NewSession {
  readonly agent: string;
  prompt = $state("");
  model = $state<SessionModelSize>("medium");
  sending = $state(false);
  error = $state<string | null>(null);
  errorDetails = $state<string | null>(null);
  address = $state<string | null>(null);
  private sent: string | null = null;

  constructor(agent: string) {
    this.agent = agent;
  }

  get run(): SessionSummary | undefined {
    if (this.address === null || ws.agent !== this.agent) return undefined;
    return ws.sessions.findByAddress(this.address);
  }

  get failure(): SessionStartFailure | undefined {
    if (this.address === null || ws.agent !== this.agent) return undefined;
    return ws.sessions.startFailure(this.address);
  }

  get waiting(): boolean {
    return this.address !== null && this.run === undefined && this.failure === undefined;
  }

  async start(): Promise<void> {
    const prompt = this.prompt.trim();
    if (prompt === "" || this.sending || this.address !== null) return;
    this.sending = true;
    this.error = null;
    this.errorDetails = null;
    try {
      const address = await startSession(this.agent, { prompt, model: this.model });
      this.sent = prompt;
      this.address = address;
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't start the session." });
    } finally {
      this.sending = false;
    }
  }

  handOver(): void {
    this.sent = null;
    this.prompt = "";
  }

  takeBack(failure: SessionStartFailure): void {
    this.error = failure.error;
    this.errorDetails = failure.details;
    if (this.prompt.trim() === "" && this.sent !== null) this.prompt = this.sent;
    this.sent = null;
    this.address = null;
  }
}
