// Installing the app: what this browser can do, and the one "Install app"
// offer the palette and the help menu list.
//
// Chromium browsers hand the page an install prompt in a `beforeinstallprompt`
// event, and the page runs it from a click. iPhones and iPads have no such
// prompt: the person adds the app from the share sheet, so the offer opens an
// explanation instead. A page that isn't a secure context (plain HTTP on a
// LAN) can't run a service worker or be installed, and gets no offer.

export interface InstallContext {
  secure: boolean;
  installed: boolean;
  ios: boolean;
}

/** The part of `navigator` that tells an iPhone or iPad apart and says whether it launched from the Home Screen. */
export interface InstallNavigator {
  userAgent: string;
  platform: string;
  maxTouchPoints: number;
  /** Set only by Safari on iOS: true when launched from the Home Screen. */
  standalone?: boolean;
}

export interface InstallWindow {
  isSecureContext: boolean;
  navigator: InstallNavigator;
  matchMedia: (query: string) => { matches: boolean };
}

interface InstallPromptEvent extends Event {
  prompt: () => Promise<void>;
}

function isInstallPrompt(event: Event): event is InstallPromptEvent {
  return "prompt" in event && typeof event.prompt === "function";
}

export interface InstallOffer {
  install: (() => void) | null;
}

export function installContext(win: InstallWindow = window): InstallContext {
  const nav = win.navigator;
  // iPadOS reports itself as a Mac, with a touch screen.
  const ios =
    /iPhone|iPad|iPod/.test(nav.userAgent) ||
    (nav.platform === "MacIntel" && nav.maxTouchPoints > 1);
  return {
    secure: win.isSecureContext,
    installed: nav.standalone === true || !win.matchMedia("(display-mode: browser)").matches,
    ios,
  };
}

export interface WatchInstallOptions {
  context: InstallContext;
  target: EventTarget;
  showIosHelp: () => void;
  reportFailure: (message: string) => void;
}

export function watchInstallOffer(offer: InstallOffer, options: WatchInstallOptions): () => void {
  const { context, target, showIosHelp, reportFailure } = options;
  offer.install = null;
  if (!context.secure || context.installed) return () => undefined;
  if (context.ios) {
    offer.install = showIosHelp;
    return () => undefined;
  }

  const onPrompt = (event: Event): void => {
    if (!isInstallPrompt(event)) return;
    // Without this the browser shows its own mini bar, and the prompt is spent.
    event.preventDefault();
    offer.install = () => {
      offer.install = null;
      event.prompt().catch(() => {
        reportFailure(
          "Couldn't open the install prompt. Try Install or Add to Home Screen in your browser's menu.",
        );
      });
    };
  };
  const onInstalled = (): void => {
    offer.install = null;
  };
  target.addEventListener("beforeinstallprompt", onPrompt);
  target.addEventListener("appinstalled", onInstalled);
  return () => {
    target.removeEventListener("beforeinstallprompt", onPrompt);
    target.removeEventListener("appinstalled", onInstalled);
  };
}
