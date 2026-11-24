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

export const POLL_INTERVAL_MS = 2000;

export type PairingPhase = "checking" | "link" | "choose" | "waiting" | "done";

export interface PairingEnvironment {
  hash: () => string;
  clearHash: () => void;
  userAgent: () => string;
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
  deviceName = $state("");
  code = $state("");
  problem = $state("");
  busy = $state(false);

  private token: string | null = null;
  private requestId = "";
  private timer: ReturnType<typeof setInterval> | undefined;

  constructor(private readonly env: PairingEnvironment = browserEnvironment()) {
    this.deviceName = defaultDeviceName(env.userAgent());
  }

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

  stop(): void {
    clearInterval(this.timer);
    this.timer = undefined;
  }

  async pairWithLink(): Promise<void> {
    const token = this.token;
    if (token === null) return;
    await this.attempt("Couldn't pair this browser.", async () => {
      await redeemPairingToken(token, this.deviceName);
      this.token = null;
    });
  }

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

  cancelRequest(): void {
    this.stop();
    this.requestId = "";
    this.phase = "choose";
  }

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
