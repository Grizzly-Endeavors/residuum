// A session the owner is starting from Activity: the task typed, the model
// size picked, and once the agent has taken it, the address its run
// announces itself at. The task sent stays in the box until the run starts,
// so a start that fails after the agent took it gives the task back. The
// panel host keeps it while the frame draws its content again at another
// width, so the task typed survives.

import { startSession, type SessionModelSize } from "../../lib/api";
import { userErrorMessage } from "../../lib/errors";
import type { SessionStartFailure } from "../../lib/sessions.svelte";
import type { SessionSummary } from "../../lib/types";
import { ws } from "../../lib/ws.svelte";

export class NewSession {
  readonly agent: string;
  prompt = $state("");
  model = $state<SessionModelSize>("medium");
  /** The start request is out. */
  sending = $state(false);
  /** Why the last start failed, in plain words. */
  error = $state<string | null>(null);
  /** The technical cause behind `error`, for a details toggle, when there is one. */
  errorDetails = $state<string | null>(null);
  /** The started session's address, once the agent has taken it. */
  address = $state<string | null>(null);
  /** The task the agent took, until its run starts or its start fails. */
  private sent: string | null = null;

  constructor(agent: string) {
    this.agent = agent;
  }

  /** The started session's run, once the agent's sessions list it. */
  get run(): SessionSummary | undefined {
    if (this.address === null || ws.agent !== this.agent) return undefined;
    return ws.sessions.findByAddress(this.address);
  }

  /** Why the started session couldn't start, once the agent says it failed to. */
  get failure(): SessionStartFailure | undefined {
    if (this.address === null || ws.agent !== this.agent) return undefined;
    return ws.sessions.startFailure(this.address);
  }

  /** Started, and neither its run nor its failure has shown up yet. */
  get waiting(): boolean {
    return this.address !== null && this.run === undefined && this.failure === undefined;
  }

  /** Start a clean session with the typed task. A failure keeps the task and says why. */
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

  /** The run started: the task is its now, so the box is clear for the next one. */
  handOver(): void {
    this.sent = null;
    this.prompt = "";
  }

  /**
   * The agent took the task but couldn't start the session: say why, put the
   * task back in the box if it was emptied while waiting, and let Start send
   * it again.
   */
  takeBack(failure: SessionStartFailure): void {
    this.error = failure.error;
    this.errorDetails = failure.details;
    if (this.prompt.trim() === "" && this.sent !== null) this.prompt = this.sent;
    this.sent = null;
    this.address = null;
  }
}
