// A session the owner is starting from Activity: the task typed, the model
// size picked, and once the agent has taken it, the address its run
// announces itself at. The panel host keeps it while the frame draws its
// content again at another width, so the task typed survives.

import { startSession, type SessionModelSize } from "../../lib/api";
import { userErrorMessage } from "../../lib/errors";
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
  /** The started session's address, once the agent has taken it. */
  address = $state<string | null>(null);

  constructor(agent: string) {
    this.agent = agent;
  }

  /** The started session's run, once the agent's sessions list it. */
  get run(): SessionSummary | undefined {
    if (this.address === null || ws.agent !== this.agent) return undefined;
    return ws.sessions.findByAddress(this.address);
  }

  /** Started, and its run hasn't shown up yet. */
  get waiting(): boolean {
    return this.address !== null && this.run === undefined;
  }

  /** Start a clean session with the typed task. A failure keeps the task and says why. */
  async start(): Promise<void> {
    const prompt = this.prompt.trim();
    if (prompt === "" || this.sending || this.address !== null) return;
    this.sending = true;
    this.error = null;
    try {
      this.address = await startSession(this.agent, { prompt, model: this.model });
      this.prompt = "";
    } catch (err) {
      this.error = userErrorMessage(err, { action: "Couldn't start the session." });
    } finally {
      this.sending = false;
    }
  }
}
