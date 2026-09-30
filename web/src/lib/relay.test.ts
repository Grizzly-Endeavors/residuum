import { describe, expect, it } from "vitest";
import { parseAgentMessage } from "./relay";

describe("parseAgentMessage", () => {
  it("recognizes a teammate's main and strips its header", () => {
    const content =
      '[Message from teammate agent:writer, not the user. Your response in this turn is not shown to them; to reply, call message_agent with to="agent:writer".]\nchapter two is ready';
    expect(parseAgentMessage(content)).toEqual({
      from: "agent:writer",
      category: "teammate",
      body: "chapter two is ready",
    });
  });

  it("recognizes a teammate's session", () => {
    const content =
      '[Message from teammate agent:writer/spawned-draft-3f9a, not the user. Your response in this turn is not shown to them; to reply, call message_agent with to="agent:writer/spawned-draft-3f9a".]\nnotes attached';
    expect(parseAgentMessage(content)).toEqual({
      from: "agent:writer/spawned-draft-3f9a",
      category: "teammate",
      body: "notes attached",
    });
  });

  it("still recognizes a local agent message", () => {
    expect(parseAgentMessage("[Agent Message from spawned-a-1 (spawned)]\ndone")).toEqual({
      from: "spawned-a-1",
      category: "spawned",
      body: "done",
    });
  });

  it("does not treat a look-alike header as a teammate message", () => {
    expect(parseAgentMessage("[Message from teammate someone, not the user.]\nhi")).toBeNull();
  });
});
