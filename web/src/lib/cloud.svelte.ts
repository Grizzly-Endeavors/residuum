import { ApiError, disconnectCloud, fetchCloudStatus, startCloudSignIn, storeSecret } from "./api";
import { configCoordinator, HUB_CONFIG_FILE } from "./config-coordinator";
import { userErrorMessage } from "./errors";
import type { CloudStatusResponse, ValidateResponse } from "./types";

export const DEFAULT_RELAY_URL = "wss://agent-residuum.com/tunnel/v2/register";

const DEFAULT_GATEWAY_PORT = "7700";

const POLL_INTERVAL_MS = 3000;
const EXPECT_WINDOW_MS = 120_000;

export type EndpointSource = Pick<CloudStatusResponse, "instance_origin">;

function withoutTrailingSlash(origin: string): string {
  return origin.endsWith("/") ? origin.slice(0, -1) : origin;
}

export function teamsMessagingEndpoint(source: EndpointSource, agent: string): string | null {
  const instanceOrigin = source.instance_origin;
  if (instanceOrigin == null || instanceOrigin === "") return null;
  return `${withoutTrailingSlash(instanceOrigin)}/teams/${agent}`;
}

export interface RelayPage {
  url: string;
  host: string;
}

export interface SignInTarget {
  relayUrl: string;
  port: string;
  host: string;
}

const RELAY_ADDRESS = /^(wss?):\/\/([^/?#\s]+)/i;

function relayWeb(relayUrl: string): { origin: string; host: string } | null {
  const found = RELAY_ADDRESS.exec(relayUrl.trim() === "" ? DEFAULT_RELAY_URL : relayUrl.trim());
  if (found === null) return null;
  const [, scheme = "", host = ""] = found;
  return { origin: `${scheme.toLowerCase() === "wss" ? "https" : "http"}://${host}`, host };
}

export function signInTarget(relayUrl: string, gatewayPort: string): SignInTarget | null {
  const relay = relayWeb(relayUrl);
  if (relay === null) return null;
  const port = gatewayPort.trim() === "" ? DEFAULT_GATEWAY_PORT : gatewayPort.trim();
  return { relayUrl: relayUrl.trim(), port, host: relay.host };
}

export function accountTarget(relayUrl: string): RelayPage | null {
  const relay = relayWeb(relayUrl);
  return relay === null ? null : { url: `${relay.origin}/dashboard`, host: relay.host };
}

const WEB_ORIGIN = /^https?:\/\/[^/?#\s]+$/i;

export function accountDashboardUrl(relayOrigin: string | null | undefined): string | null {
  if (relayOrigin === null || relayOrigin === undefined || !WEB_ORIGIN.test(relayOrigin)) {
    return null;
  }
  return `${relayOrigin}/dashboard`;
}

export type CloudPhase = "connected" | "connecting" | "refused" | "disconnected" | "none";

export function phaseOf(status: CloudStatusResponse): CloudPhase {
  if (status.status === "connected") return "connected";
  if (status.status === "refused" && status.has_token && status.enabled) return "refused";
  if (status.status === "connecting" || (status.has_token && status.enabled)) return "connecting";
  return status.has_token ? "disconnected" : "none";
}

class Refused extends Error {
  constructor(result: ValidateResponse) {
    super(result.error ?? result.diagnostics?.[0]?.message ?? "Residuum refused the change.");
  }
}

type CloudAction = "sign-in" | "disconnect" | "reconnect" | "token";

const ACTION_FAILED: Readonly<Record<CloudAction, string>> = {
  "sign-in": "Couldn't start signing in to Residuum Cloud.",
  disconnect: "Couldn't disconnect from Residuum Cloud.",
  reconnect: "Couldn't reconnect to Residuum Cloud.",
  token: "Couldn't connect with that token.",
};

export const SIGN_IN_TAB_BLOCKED =
  "Your browser blocked the Residuum Cloud sign-in tab. Allow pop-ups for this page (your browser offers it in or beside the address bar), then try again.";

function problemOf(kind: CloudAction, err: unknown): string {
  if (err instanceof Refused) return `${ACTION_FAILED[kind]} ${err.message}`;
  if (kind === "disconnect" && err instanceof ApiError && err.status === 403) {
    return `${ACTION_FAILED[kind]} It can't be done remotely, because nothing could bring Residuum back. Do it on the machine running Residuum.`;
  }
  return userErrorMessage(err, { action: ACTION_FAILED[kind] });
}

export class CloudConnection {
  status = $state.raw<CloudStatusResponse | null>(null);
  loadError = $state("");
  busy = $state<CloudAction | null>(null);
  problem = $state("");

  private awaiting: { phase: CloudPhase | null; until: number } | null = null;

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

  expectChange(): void {
    this.awaiting = {
      phase: this.status === null ? null : phaseOf(this.status),
      until: Date.now() + EXPECT_WINDOW_MS,
    };
  }

  async signIn(target: SignInTarget): Promise<void> {
    const tab = window.open("about:blank", "_blank");
    if (tab === null) {
      this.problem = SIGN_IN_TAB_BLOCKED;
      return;
    }
    await this.act("sign-in", async () => {
      try {
        const url = await startCloudSignIn(target.relayUrl, target.port);
        tab.opener = null;
        tab.location.replace(url);
      } catch (err) {
        tab.close();
        throw err;
      }
    });
  }

  async disconnect(): Promise<void> {
    await this.act("disconnect", async () => {
      await disconnectCloud();
    });
  }

  async reconnect(): Promise<void> {
    await this.act("reconnect", () => this.writeCloud({ enabled: true }));
  }

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
