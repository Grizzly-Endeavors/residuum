import { describe, expect, it, vi, type Mock } from "vitest";
import {
  installContext,
  watchInstallOffer,
  type InstallContext,
  type InstallNavigator,
  type InstallOffer,
  type InstallWindow,
} from "./install";
import { waitFor } from "../test/wait";

const DESKTOP: InstallNavigator = {
  userAgent: "Mozilla/5.0 (X11; Linux x86_64) Chrome/140.0",
  platform: "Linux x86_64",
  maxTouchPoints: 0,
};

function windowWith(
  navigator: Partial<InstallNavigator>,
  options: { secure?: boolean; inOwnWindow?: boolean } = {},
): InstallWindow {
  return {
    isSecureContext: options.secure ?? true,
    navigator: { ...DESKTOP, ...navigator },
    matchMedia: (query) => ({
      matches: query === "(display-mode: browser)" && options.inOwnWindow !== true,
    }),
  };
}

describe("installContext", () => {
  it("reads a secure desktop browser tab as installable from the browser", () => {
    expect(installContext(windowWith({}))).toEqual({ secure: true, installed: false, ios: false });
  });

  it("reports an insecure origin", () => {
    expect(installContext(windowWith({}, { secure: false })).secure).toBe(false);
  });

  it("recognizes an iPhone and an iPad that reports itself as a Mac", () => {
    const iphone = windowWith({
      userAgent: "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)",
    });
    const ipad = windowWith({
      userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
      platform: "MacIntel",
      maxTouchPoints: 5,
    });
    const mac = windowWith({
      userAgent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
      platform: "MacIntel",
    });
    expect(installContext(iphone).ios).toBe(true);
    expect(installContext(ipad).ios).toBe(true);
    expect(installContext(mac).ios).toBe(false);
  });

  it("knows an app launched from the Home Screen or in its own window", () => {
    expect(installContext(windowWith({ standalone: true })).installed).toBe(true);
    expect(installContext(windowWith({}, { inOwnWindow: true })).installed).toBe(true);
  });
});

/** A browser's install prompt event, which carries the method the page runs. */
function promptEvent(prompt: () => Promise<void>): Event {
  return Object.assign(new Event("beforeinstallprompt", { cancelable: true }), { prompt });
}

function watch(context: Partial<InstallContext> = {}): {
  offer: InstallOffer;
  target: EventTarget;
  showIosHelp: Mock;
  reportFailure: Mock;
  stop: () => void;
} {
  const offer: InstallOffer = { install: () => undefined };
  const target = new EventTarget();
  const showIosHelp = vi.fn();
  const reportFailure = vi.fn();
  const stop = watchInstallOffer(offer, {
    context: { secure: true, installed: false, ios: false, ...context },
    target,
    showIosHelp,
    reportFailure,
  });
  return { offer, target, showIosHelp, reportFailure, stop };
}

describe("watchInstallOffer", () => {
  it("offers nothing until the browser hands over its prompt", () => {
    expect(watch().offer.install).toBeNull();
  });

  it("offers to run the prompt once the browser fires it, and keeps the browser's own bar away", () => {
    const { offer, target } = watch();
    const prompt = vi.fn(() => Promise.resolve());
    const event = promptEvent(prompt);
    target.dispatchEvent(event);
    expect(event.defaultPrevented).toBe(true);
    expect(offer.install).not.toBeNull();
    expect(prompt).not.toHaveBeenCalled();

    offer.install?.();
    expect(prompt).toHaveBeenCalledOnce();
  });

  it("withdraws the offer once it is used, since a prompt shows only once", () => {
    const { offer, target } = watch();
    target.dispatchEvent(promptEvent(() => Promise.resolve()));
    offer.install?.();
    expect(offer.install).toBeNull();
  });

  it("offers the new prompt when the browser fires another after a dismissal", () => {
    const { offer, target } = watch();
    target.dispatchEvent(promptEvent(() => Promise.resolve()));
    offer.install?.();
    const second = vi.fn(() => Promise.resolve());
    target.dispatchEvent(promptEvent(second));
    offer.install?.();
    expect(second).toHaveBeenCalledOnce();
  });

  it("tells the person when the prompt would not open", async () => {
    const { offer, target, reportFailure } = watch();
    target.dispatchEvent(promptEvent(() => Promise.reject(new Error("already used"))));
    offer.install?.();
    await waitFor(() => {
      expect(reportFailure).toHaveBeenCalledWith(
        expect.stringContaining("Couldn't open the install prompt"),
      );
    });
  });

  it("withdraws the offer once the app is installed", () => {
    const { offer, target } = watch();
    target.dispatchEvent(promptEvent(() => Promise.resolve()));
    target.dispatchEvent(new Event("appinstalled"));
    expect(offer.install).toBeNull();
  });

  it("offers nothing without a secure context, even if the browser fires its prompt", () => {
    const { offer, target } = watch({ secure: false });
    target.dispatchEvent(promptEvent(() => Promise.resolve()));
    expect(offer.install).toBeNull();
  });

  it("offers nothing to an app that is already installed", () => {
    const { offer, target } = watch({ installed: true });
    target.dispatchEvent(promptEvent(() => Promise.resolve()));
    expect(offer.install).toBeNull();
  });

  it("opens the Add to Home Screen explanation on an iPhone or iPad", () => {
    const { offer, showIosHelp } = watch({ ios: true });
    offer.install?.();
    expect(showIosHelp).toHaveBeenCalledOnce();
  });

  it("offers nothing on an iPhone without a secure context", () => {
    expect(watch({ ios: true, secure: false }).offer.install).toBeNull();
  });

  it("stops listening when asked", () => {
    const { offer, target, stop } = watch();
    stop();
    target.dispatchEvent(promptEvent(() => Promise.resolve()));
    expect(offer.install).toBeNull();
  });
});
