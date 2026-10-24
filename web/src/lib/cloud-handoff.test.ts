import { describe, expect, it } from "vitest";
import { captureCloudHandoff, handoffOf, handoffProblem, takeCloudHandoff } from "./cloud-handoff";

describe("handoffOf", () => {
  it("reads each outcome the gateway's callback names", () => {
    expect(handoffOf("?settings=_all/cloud&cloud=connecting")).toBe("connecting");
    expect(handoffOf("?settings=_all/cloud&cloud=missing-token")).toBe("missing-token");
    expect(handoffOf("?cloud=save-failed&settings=_all/cloud")).toBe("save-failed");
    expect(handoffOf("?settings=_all/cloud&cloud=not-started-here")).toBe("not-started-here");
  });

  it("reads nothing from an address without one, or with one it doesn't know", () => {
    expect(handoffOf("")).toBeNull();
    expect(handoffOf("?settings=_all/cloud")).toBeNull();
    expect(handoffOf("?cloud=sideways")).toBeNull();
    expect(handoffOf("?cloud=")).toBeNull();
  });
});

describe("takeCloudHandoff", () => {
  it("hands the outcome the page opened with to the first taker only", () => {
    captureCloudHandoff("?settings=_all/cloud&cloud=missing-token");
    expect(takeCloudHandoff()).toBe("missing-token");
    expect(takeCloudHandoff()).toBeNull();
  });

  it("has nothing to hand over when the page opened without an outcome", () => {
    captureCloudHandoff("?settings=_all/cloud");
    expect(takeCloudHandoff()).toBeNull();
  });
});

describe("handoffProblem", () => {
  it("says what to do next for each failure, and nothing for a sign-in that is connecting", () => {
    expect(handoffProblem("missing-token")).toMatch(/Try connecting again\.$/);
    expect(handoffProblem("save-failed")).toMatch(/^You signed in, but .*Try connecting again/);
    expect(handoffProblem("not-started-here")).toMatch(
      /^This sign-in link didn't come from this app.*Try connecting again\.$/,
    );
    expect(handoffProblem("connecting")).toBeNull();
    expect(handoffProblem(null)).toBeNull();
  });
});
