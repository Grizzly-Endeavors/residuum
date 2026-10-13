import { describe, expect, it } from "vitest";
import { landingNotices } from "./landing-notices";
import { locationAt, type Place } from "./routes";

const chat: Place = { kind: "chat", agent: "scout" };
const workbench: Place = { kind: "workbench", artifact: null };
const inbox: Place = { kind: "inbox", agent: null, tab: "active", item: null };

describe("landing notices", () => {
  it("names the agent whose file link expired", () => {
    expect(landingNotices("?notice=file-link-expired", locationAt(chat))).toEqual([
      "That file link has expired. Ask scout to send the file again.",
    ]);
  });

  it("names a missing artifact only when the name could be one", () => {
    expect(
      landingNotices("?notice=artifact-missing&artifact=tip-splitter", locationAt(workbench)),
    ).toEqual([
      `There's no workbench artifact named "tip-splitter". It may have been renamed or deleted.`,
    ]);
    expect(
      landingNotices("?notice=artifact-missing&artifact=%3Cscript%3E", locationAt(workbench)),
    ).toEqual([
      "There's no workbench artifact at that address. It may have been renamed or deleted.",
    ]);
  });

  it("says what a webhook address is", () => {
    const [notice] = landingNotices("?notice=webhook-address", locationAt(chat));
    expect(notice).toContain("webhook");
    expect(notice).toContain("POST");
  });

  it("explains a missing inbox attachment", () => {
    expect(landingNotices("?notice=inbox-attachment-missing", locationAt(inbox))).toEqual([
      "That attachment is no longer available. It may have been removed from its inbox item.",
    ]);
  });

  it("ignores codes it doesn't know, and an address with none", () => {
    expect(landingNotices("?notice=Your%20account%20is%20locked", locationAt(chat))).toEqual([]);
    expect(landingNotices("?notice=toString", locationAt(chat))).toEqual([]);
    expect(landingNotices("", locationAt(chat))).toEqual([]);
  });

  it("says each notice once", () => {
    expect(
      landingNotices("?notice=agent-stopped&notice=agent-stopped", locationAt(chat)),
    ).toHaveLength(1);
  });
});
