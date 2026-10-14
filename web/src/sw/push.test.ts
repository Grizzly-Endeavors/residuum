import { describe, expect, it } from "vitest";
import { appTarget, clickTarget, notificationFor, rotationRequest } from "./push";

describe("appTarget", () => {
  it("keeps a path in the app", () => {
    expect(appTarget("/inbox?item=atlas:note-1")).toBe("/inbox?item=atlas:note-1");
    expect(appTarget("/agent/atlas/activity")).toBe("/agent/atlas/activity");
  });

  it("sends anything that could leave the app to Home", () => {
    for (const value of [
      "//evil.example",
      "/\\evil.example",
      "https://evil.example",
      "",
      7,
      null,
    ]) {
      expect(appTarget(value)).toBe("/home");
    }
  });
});

describe("notificationFor", () => {
  it("shows the payload's title, body and tag, and alerts again when a tag repeats", () => {
    const shown = notificationFor({
      v: 1,
      event: "reply_while_away",
      agent: "atlas",
      title: "atlas replied",
      body: "Here's the summary you asked for.",
      target: "/agent/atlas",
      tag: "reply:atlas",
      badge: 4,
    });
    expect(shown.title).toBe("atlas replied");
    expect(shown.options).toMatchObject({
      body: "Here's the summary you asked for.",
      tag: "reply:atlas",
      renotify: true,
      data: { target: "/agent/atlas" },
    });
    expect(shown.badge).toBe(4);
  });

  it("shows something for a push with no title or body, and leaves the badge alone", () => {
    const shown = notificationFor({ badge: -1 });
    expect(shown.title).toBe("Residuum");
    expect(shown.options.body).toBe("Something needs your attention. Open Residuum.");
    expect(shown.options.tag).toBeUndefined();
    expect(shown.options.data.target).toBe("/home");
    expect(shown.badge).toBeNull();
  });

  it("keeps a body without a title under the app's name", () => {
    expect(notificationFor({ body: "From atlas." })).toMatchObject({
      title: "Residuum",
      options: { body: "From atlas." },
    });
  });
});

describe("rotationRequest", () => {
  const subscription = {
    endpoint: "https://push.example/new",
    keys: { p256dh: "p256dh-key", auth: "auth-key" },
  };

  it("carries the new subscription and the endpoint it replaces", () => {
    expect(rotationRequest(subscription, "https://push.example/old")).toEqual({
      subscription: { endpoint: subscription.endpoint, keys: subscription.keys },
      previous_endpoint: "https://push.example/old",
    });
  });

  it("carries no previous endpoint when there was none", () => {
    expect(rotationRequest(subscription, undefined)).toEqual({
      subscription: { endpoint: subscription.endpoint, keys: subscription.keys },
      previous_endpoint: undefined,
    });
  });

  it("is null when the new subscription has no keys", () => {
    expect(rotationRequest({ endpoint: subscription.endpoint }, "https://push.example/old")).toBe(
      null,
    );
  });
});

describe("clickTarget", () => {
  it("reads the target the worker kept on the notification", () => {
    expect(clickTarget({ target: "/agent/scout" })).toBe("/agent/scout");
    expect(clickTarget(null)).toBe("/home");
    expect(clickTarget({ target: "//elsewhere" })).toBe("/home");
  });
});
