// ── The pairing page's state ─────────────────────────────────────────
//
// A browser reaches the pairing page without a credential. It leaves with
// one by a link made on the machine Residuum runs on (the token in the URL's
// fragment), by a code a paired device approves, or by a recovery code.

import { userErrorMessage } from "./errors";
import {
  createPairingRequest,
  fetchPairingState,
  pollPairingRequest,
  redeemPairingToken,
  redeemRecoveryCode,
} from "./pairing-api";
import type { PairingRequestStatus } from "./generated/PairingRequestStatus";
import { defaultDeviceName, tokenFromFragment } from "./pairing";

/** How often a waiting browser asks whether it was approved. */
export const POLL_INTERVAL_MS = 2000;

export type PairingPhase =
  /** Asking the gateway whether this browser needs to pair. */
  | "checking"
  /** Opened from a pairing link; waiting for the person to confirm. */
  | "link"
  /** Choosing how to pair: ask a paired device, or use a recovery code. */
  | "choose"
  /** Waiting for a paired device to approve the code on screen. */
  | "waiting"
  /** Paired; moving to the app. */
  | "done";

/** What the flow reads and does outside itself; tests replace it. */
export interface PairingEnvironment {
  /** The page's URL fragment, `#token=…` when opened from a link. */
  hash: () => string;
  /** Drop the fragment from the address bar, so a token isn't left in history. */
  clearHash: () => void;
  userAgent: () => string;
  /** Move the page to the app. */
  openApp: () => void;
}

export function browserEnvironment(): PairingEnvironment {
  return {
    hash: () => window.location.hash,
    clearHash: () => {
      window.history.replaceState(null, "", window.location.pathname + window.location.search);
    },
    userAgent: () => navigator.userAgent,
    openApp: () => {
      window.location.assign("/");
    },
  };
}

export class PairingFlow {
  phase = $state<PairingPhase>("checking");
  /** What this browser is called in the list of paired devices. */
  deviceName = $state("");
  /** The code the approving device shows next to this browser's request. */
  code = $state("");
  /** Why the last step failed, or how a request ended. */
  problem = $state("");
  busy = $state(false);

  private token: string | null = null;
  private requestId = "";
  private timer: ReturnType<typeof setInterval> | undefined;

  constructor(private readonly env: PairingEnvironment = browserEnvironment()) {
    this.deviceName = defaultDeviceName(env.userAgent());
  }

  /** Find out whether this browser needs to pair, and what it arrived with. */
  async start(): Promise<void> {
    this.token = tokenFromFragment(this.env.hash());
    if (this.token !== null) this.env.clearHash();
    try {
      const state = await fetchPairingState();
      if (!state.remote || state.paired) {
        this.finish();
        return;
      }
    } catch (err) {
      this.problem = userErrorMessage(err, { action: "Couldn't reach Residuum." });
    }
    this.phase = this.token === null ? "choose" : "link";
  }

  /** Stop waiting; the page is going away. */
  stop(): void {
    clearInterval(this.timer);
    this.timer = undefined;
  }

  /** Pair with the link this page was opened from. */
  async pairWithLink(): Promise<void> {
    const token = this.token;
    if (token === null) return;
    await this.attempt("Couldn't pair this browser.", async () => {
      await redeemPairingToken(token, this.deviceName);
      this.token = null;
    });
  }

  /** Ask for a code a paired device can approve, and wait for the answer. */
  async requestApproval(): Promise<void> {
    this.busy = true;
    this.problem = "";
    try {
      const created = await createPairingRequest(this.deviceName);
      this.requestId = created.request_id;
      this.code = created.code;
      this.phase = "waiting";
      this.timer = setInterval(() => void this.poll(), POLL_INTERVAL_MS);
    } catch (err) {
      this.problem = userErrorMessage(err, { action: "Couldn't ask for approval." });
    } finally {
      this.busy = false;
    }
  }

  /** Give up waiting and go back to choosing. */
  cancelRequest(): void {
    this.stop();
    this.requestId = "";
    this.phase = "choose";
  }

  /** Pair with a recovery code. */
  async pairWithRecoveryCode(code: string): Promise<void> {
    await this.attempt("Couldn't pair with that recovery code.", async () => {
      await redeemRecoveryCode(code, this.deviceName);
    });
  }

  private async poll(): Promise<void> {
    let status: PairingRequestStatus;
    try {
      status = (await pollPairingRequest(this.requestId)).status;
    } catch (err) {
      // A failed check isn't an answer; the next tick tries again.
      this.problem = userErrorMessage(err, { action: "Couldn't check for approval." });
      return;
    }
    this.problem = "";
    if (status === "approved") {
      this.stop();
      this.finish();
    } else if (status === "denied" || status === "expired") {
      this.stop();
      this.phase = "choose";
      this.problem =
        status === "denied"
          ? "That request was refused on the other device."
          : "That request expired before anyone approved it. Ask again.";
    }
  }

  private async attempt(failure: string, step: () => Promise<void>): Promise<void> {
    this.busy = true;
    this.problem = "";
    try {
      await step();
      this.finish();
    } catch (err) {
      this.problem = userErrorMessage(err, { action: failure });
    } finally {
      this.busy = false;
    }
  }

  private finish(): void {
    this.phase = "done";
    this.env.openApp();
  }
}
