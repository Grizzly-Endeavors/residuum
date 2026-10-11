import { describe, expect, it, vi } from "vitest";
import {
  PAIRING_PATH,
  defaultDeviceName,
  handoffUrl,
  isDeviceRequiredBody,
  redirectToPairing,
  tokenFromFragment,
} from "./pairing";

describe("isDeviceRequiredBody", () => {
  it("recognises the gateway's refusal of an unpaired browser", () => {
    expect(isDeviceRequiredBody('{"error":"x","code":"device_required"}')).toBe(true);
  });

  it("ignores every other body", () => {
    expect(isDeviceRequiredBody('{"error":"x"}')).toBe(false);
    expect(isDeviceRequiredBody("unauthorized")).toBe(false);
    expect(isDeviceRequiredBody("")).toBe(false);
  });
});

describe("redirectToPairing", () => {
  it("moves a page elsewhere in the app to the pairing page", () => {
    const assign = vi.fn();
    redirectToPairing({ pathname: "/agent/scout", assign });
    expect(assign).toHaveBeenCalledWith(PAIRING_PATH);
  });

  it("leaves the pairing page where it is, so a failing call can't loop", () => {
    const assign = vi.fn();
    redirectToPairing({ pathname: PAIRING_PATH, assign });
    expect(assign).not.toHaveBeenCalled();
  });
});

describe("tokenFromFragment", () => {
  it("reads the token from a pairing link's fragment", () => {
    expect(tokenFromFragment("#token=abc_DEF-123")).toBe("abc_DEF-123");
  });

  it("is null without one", () => {
    expect(tokenFromFragment("")).toBeNull();
    expect(tokenFromFragment("#other=1")).toBeNull();
    expect(tokenFromFragment("#token=")).toBeNull();
  });
});

describe("defaultDeviceName", () => {
  it("names the browser and the system", () => {
    expect(
      defaultDeviceName("Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0"),
    ).toBe("Firefox on Linux");
    expect(
      defaultDeviceName(
        "Mozilla/5.0 (Linux; Android 14) AppleWebKit/537.36 Chrome/126.0 Mobile Safari/537.36",
      ),
    ).toBe("Chrome on Android");
    expect(
      defaultDeviceName(
        "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5 like Mac OS X) AppleWebKit/605.1.15 Version/17.5 Mobile Safari/604.1",
      ),
    ).toBe("Safari on iPhone");
  });

  it("falls back to something a person can recognise", () => {
    expect(defaultDeviceName("")).toBe("This browser");
  });
});

describe("handoffUrl", () => {
  it("carries the token and the page to open in the fragment, which never reaches a server", () => {
    const url = handoffUrl("https://bear.workbench.agent-residuum.com", "notes", "tok-1");
    expect(url).toBe(
      "https://bear.workbench.agent-residuum.com/_handoff#token=tok-1&next=%2Fnotes%2F",
    );
    expect(new URL(url).search).toBe("");
  });
});
