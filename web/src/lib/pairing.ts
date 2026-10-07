// ── Device pairing: the browser's side ───────────────────────────────
//
// Through Residuum Cloud the gateway answers only browsers it has paired.
// An unpaired browser's navigations land on the pairing page, and its API
// calls get a 401 carrying `device_required`, which sends the page there too.

/** The pairing page's path on the UI host. */
export const PAIRING_PATH = "/pair";

/** The `code` of the 401 an unpaired browser's API call gets. */
export const DEVICE_REQUIRED = "device_required";

/** Whether an error body is the gateway saying this browser isn't paired. */
export function isDeviceRequiredBody(body: string): boolean {
  try {
    const parsed: unknown = JSON.parse(body);
    return (
      typeof parsed === "object" &&
      parsed !== null &&
      "code" in parsed &&
      parsed.code === DEVICE_REQUIRED
    );
  } catch {
    return false;
  }
}

/** Where the page is, so tests can stand in for the browser's location. */
export interface PageLocation {
  pathname: string;
  assign: (url: string) => void;
}

/** Whether the page is already the pairing page. */
export function onPairingPage(page: Pick<PageLocation, "pathname"> = location): boolean {
  return page.pathname === PAIRING_PATH;
}

/** Move the page to the pairing page, unless it is there. */
export function redirectToPairing(page: PageLocation = location): void {
  if (!onPairingPage(page)) page.assign(PAIRING_PATH);
}

/** The single-use token in a pairing link's fragment (`#token=…`), or null. */
export function tokenFromFragment(hash: string): string | null {
  const token = new URLSearchParams(hash.startsWith("#") ? hash.slice(1) : hash).get("token");
  return token === null || token === "" ? null : token;
}

/** A name to offer for this browser, from its user agent: "Firefox on Linux". */
export function defaultDeviceName(userAgent: string): string {
  const browser = [
    ["Edg/", "Edge"],
    ["OPR/", "Opera"],
    ["Firefox/", "Firefox"],
    ["Chrome/", "Chrome"],
    ["Safari/", "Safari"],
  ].find(([marker]) => marker !== undefined && userAgent.includes(marker))?.[1];
  const system = [
    ["iPhone", "iPhone"],
    ["iPad", "iPad"],
    ["Android", "Android"],
    ["Windows", "Windows"],
    ["Mac OS X", "macOS"],
    ["Linux", "Linux"],
  ].find(([marker]) => marker !== undefined && userAgent.includes(marker))?.[1];
  if (browser !== undefined && system !== undefined) return `${browser} on ${system}`;
  return browser ?? system ?? "This browser";
}

/**
 * Where a paired browser goes to get the workbench host's own credential: the
 * handoff page, carrying the single-use `token` and the artifact to open next
 * in the fragment, which is never sent to a server.
 */
export function handoffUrl(origin: string, artifact: string, token: string): string {
  const next = encodeURIComponent(`/${artifact}/`);
  return `${origin}/_handoff#token=${encodeURIComponent(token)}&next=${next}`;
}
