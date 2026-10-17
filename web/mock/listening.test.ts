import { describe, expect, it } from "vitest";
import { mockListeningLine, mockListeningPattern } from "./listening";

describe("the mock's listening line", () => {
  const pattern = mockListeningPattern("server_port", "artifacts_port");

  it("gives both ports to the pattern that reads it", () => {
    const match = pattern.exec(`noise before\n${mockListeningLine(10_003, 43_367)}\nnoise after`);
    expect(match?.groups).toEqual({ server_port: "10003", artifacts_port: "43367" });
  });

  it("doesn't match a server whose artifacts listener couldn't listen", () => {
    expect(mockListeningLine(10_003, null)).toContain("artifacts unavailable");
    expect(pattern.exec(mockListeningLine(10_003, null))).toBeNull();
  });
});
