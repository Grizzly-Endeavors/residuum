import { afterEach, describe, expect, it, vi } from "vitest";
import { fireEvent, jsonResponse, mockFetch, render, screen, settle } from "../test/component";
import { ws } from "../lib/ws.svelte";
import type { OutboundA2aTaskSummary } from "../lib/types";
import OutboundTaskRow from "./OutboundTaskRow.svelte";

const TASK: OutboundA2aTaskSummary = {
  task_id: "t1",
  agent: "laptop",
  sender_address: "main",
  state: "working",
  status_text: "Indexing the photo library",
  open: true,
  started_at: "2026-09-26T14:00:00.000Z",
  unreachable_since: null,
};

afterEach(() => {
  vi.unstubAllGlobals();
  ws.sessions.outbound = [];
  ws.sessions.outboundUnreachable.clear();
});

describe("OutboundTaskRow", () => {
  it("shows the agent, what it reported, and where it stands", () => {
    render(OutboundTaskRow, { task: TASK });
    expect(screen.getByText("a2a:laptop")).toBeTruthy();
    expect(screen.getByText("Indexing the photo library")).toBeTruthy();
    expect(screen.getByText("working")).toBeTruthy();
  });

  it("says when the agent can't be reached", () => {
    ws.sessions.now = Date.now();
    render(OutboundTaskRow, {
      task: { ...TASK, unreachable_since: new Date(Date.now() - 11 * 60_000).toISOString() },
    });
    expect(screen.getByText(/can't reach laptop for 11m, still retrying/)).toBeTruthy();
  });

  it("stops the task, and offers stop-watching when the agent is unreachable", async () => {
    const calls: string[] = [];
    mockFetch((url, init) => {
      calls.push(`${init?.method ?? "GET"} ${url}`);
      if (url.endsWith("/api/a2a/outbound/t1/stop")) {
        return jsonResponse(
          {
            error: "Couldn't reach laptop to cancel the task. You can stop watching it instead.",
            code: "unreachable",
          },
          502,
        );
      }
      if (url.endsWith("/api/a2a/outbound/t1/stop-watching")) {
        return jsonResponse({ ...TASK, state: "canceled", open: false });
      }
      throw new Error(`unexpected ${url}`);
    });
    render(OutboundTaskRow, { task: TASK });

    await fireEvent.click(screen.getByRole("button", { name: "Stop the task sent to a2a:laptop" }));
    await settle();
    expect(screen.getByText(/Couldn't reach laptop to cancel the task/)).toBeTruthy();

    await fireEvent.click(screen.getByRole("button", { name: "Stop watching" }));
    await settle();
    expect(calls).toEqual([
      "POST /api/a2a/outbound/t1/stop",
      "POST /api/a2a/outbound/t1/stop-watching",
    ]);
  });
});
