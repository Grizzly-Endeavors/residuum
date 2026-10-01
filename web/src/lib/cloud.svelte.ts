// ── Residuum Cloud connection ────────────────────────────────────────
//
// What Settings → Residuum Cloud shows and does. The tunnel's state comes
// from the hub (`GET /api/hub/cloud/status`) and has no change feed, so this
// reads it when the section opens, when the window gets focus back from the
// relay's sign-in page, when the hub's config reloads, and every few seconds
// while the tunnel is connecting or the user has started something that will
// change it. The actions act at once: they don't go through the staged form.

import { ApiError, disconnectCloud, fetchCloudStatus, storeSecret } from "./api";
import { configCoordinator, HUB_CONFIG_FILE } from "./config-coordinator";
import { userErrorMessage } from "./errors";
import type { CloudStatusResponse, ValidateResponse } from "./types";

/** The relay Residuum Cloud's tunnel dials when `[cloud] relay_url` is empty. */
export const DEFAULT_RELAY_URL = "wss://agent-residuum.com/tunnel/register";

/** The port the gateway listens on when `[gateway] port` is empty. */
const DEFAULT_GATEWAY_PORT = "7700";

const POLL_INTERVAL_MS = 3000;
/** How long after the user starts signing in on the relay's page the section keeps looking for the result. */
const EXPECT_WINDOW_MS = 120_000;

/** The relay's sign-in page for a gateway, and the host it is on. */
export interface ConnectTarget {
  url: string;
  host: string;
}

const RELAY_ADDRESS = /^(wss?):\/\/([^/?#\s]+)/i;

/** Where the relay's sign-in page for a gateway is, from the relay URL the tunnel would dial. Null when that URL isn't a `ws://` or `wss://` address. */
export function connectTarget(relayUrl: string, gatewayPort: string): ConnectTarget | null {
  const found = RELAY_ADDRESS.exec(relayUrl.trim() === "" ? DEFAULT_RELAY_URL : relayUrl.trim());
  if (found === null) return null;
  const [, scheme = "", host = ""] = found;
  const port = gatewayPort.trim() === "" ? DEFAULT_GATEWAY_PORT : gatewayPort.trim();
  const origin = `${scheme.toLowerCase() === "wss" ? "https" : "http"}://${host}`;
  return { url: `${origin}/connect?port=${encodeURIComponent(port)}`, host };
}

/** What the section shows for a tunnel status. A saved token that is switched on counts as connecting: the tunnel is being started, or retrying. */
export type CloudPhase = "connected" | "connecting" | "disconnected" | "none";

export function phaseOf(status: CloudStatusResponse): CloudPhase {
  if (status.status === "connected") return "connected";
  if (status.status === "connecting" || (status.has_token && status.enabled)) return "connecting";
  return status.has_token ? "disconnected" : "none";
}

/** A config change the server refused, with its reason. */
class Refused extends Error {
  constructor(result: ValidateResponse) {
    super(result.error ?? result.diagnostics?.[0]?.message ?? "Residuum refused the change.");
  }
}

type CloudAction = "disconnect" | "reconnect" | "token";

const ACTION_FAILED: Readonly<Record<CloudAction, string>> = {
  disconnect: "Couldn't disconnect from Residuum Cloud.",
  reconnect: "Couldn't reconnect to Residuum Cloud.",
  token: "Couldn't connect with that token.",
};

/** The gateway refuses to disconnect for someone viewing it through the tunnel (`remote_control_guard`); its 403 says so, where `userErrorMessage` would say to reload. */
function problemOf(kind: CloudAction, err: unknown): string {
  if (err instanceof Refused) return `${ACTION_FAILED[kind]} ${err.message}`;
  if (kind === "disconnect" && err instanceof ApiError && err.status === 403) {
    return `${ACTION_FAILED[kind]} It can't be done remotely, because nothing could bring Residuum back. Do it on the machine running Residuum.`;
  }
  return userErrorMessage(err, { action: ACTION_FAILED[kind] });
}

export class CloudConnection {
  status = $state.raw<CloudStatusResponse | null>(null);
  /** Why the last read of the status failed. Any status shown with it is the last one that was read. */
  loadError = $state("");
  /** The action under way, for its button. */
  busy = $state<CloudAction | null>(null);
  /** Why the last action failed. */
  problem = $state("");

  private awaiting: { phase: CloudPhase | null; until: number } | null = null;

  /** Read the status now. */
  async refresh(): Promise<void> {
    try {
      const next = await fetchCloudStatus();
      if (this.awaiting !== null && phaseOf(next) !== this.awaiting.phase) this.awaiting = null;
      this.status = next;
      this.loadError = "";
    } catch (err) {
      this.loadError = userErrorMessage(err, { action: "Couldn't read the Cloud status." });
    }
  }

  /** Follow the tunnel until the section closes. Returns what stops that. */
  follow(): () => void {
    void this.refresh();
    const timer = setInterval(() => {
      if (this.polling()) void this.refresh();
    }, POLL_INTERVAL_MS);
    const onFocus = (): void => void this.refresh();
    window.addEventListener("focus", onFocus);
    const unsubscribe = configCoordinator.subscribe(HUB_CONFIG_FILE, () => void this.refresh());
    return () => {
      clearInterval(timer);
      window.removeEventListener("focus", onFocus);
      unsubscribe();
    };
  }

  private polling(): boolean {
    if (this.status === null) return false;
    if (phaseOf(this.status) === "connecting") return true;
    return this.awaiting !== null && Date.now() < this.awaiting.until;
  }

  /** Something that changes the status behind this page's back has started, such as signing in on the relay: look for it for a while. */
  expectChange(): void {
    this.awaiting = {
      phase: this.status === null ? null : phaseOf(this.status),
      until: Date.now() + EXPECT_WINDOW_MS,
    };
  }

  /** Switch the tunnel off, keeping the account. Also cancels a connection that is still starting. */
  async disconnect(): Promise<void> {
    await this.act("disconnect", async () => {
      await disconnectCloud();
    });
  }

  /** Switch the tunnel back on for the account already saved. */
  async reconnect(): Promise<void> {
    await this.act("reconnect", () => this.writeCloud({ enabled: true }));
  }

  /** Keep a token the user pasted as a secret and switch the tunnel on with it. */
  async connectWithToken(token: string): Promise<void> {
    await this.act("token", async () => {
      const { reference } = await storeSecret("cloud_token", token.trim());
      await this.writeCloud({ enabled: true, token: reference });
    });
  }

  private async writeCloud(cloud: Record<string, unknown>): Promise<void> {
    const saved = await configCoordinator.edit(HUB_CONFIG_FILE, () => ({ cloud }));
    if (!saved.result.valid) throw new Refused(saved.result);
  }

  private async act(kind: CloudAction, run: () => Promise<void>): Promise<void> {
    this.busy = kind;
    this.problem = "";
    try {
      await run();
      this.expectChange();
    } catch (err) {
      this.problem = problemOf(kind, err);
    } finally {
      this.busy = null;
    }
    await this.refresh();
  }
}
