import { afterEach, describe, expect, it } from "vitest";
import type { TeamEvent, TeamEventKind } from "../../lib/hub-types";
import { overview } from "../../lib/overview.svelte";
import { render, screen } from "../../test/component";
import AcrossTheTeam from "./AcrossTheTeam.svelte";

const NOW = Date.parse("2026-03-14T12:00:00Z");

function event(id: number, kind: TeamEventKind, summary: string): TeamEvent {
  return {
    id,
    at: new Date(NOW - 60_000).toISOString(),
    agent: "atlas",
    kind,
    level: "info",
    summary,
    target: null,
  };
}

afterEach(() => {
  overview.events = [];
  overview.eventsLoaded = false;
});

describe("AcrossTheTeam", () => {
  it("leaves replies out and still fills the list with the newest other events", () => {
    const others = Array.from({ length: 10 }, (_, i) =>
      event(i + 1, "session_started", `atlas started session ${String(i + 1)}`),
    );
    const replies = Array.from({ length: 5 }, (_, i) =>
      event(i + 11, "agent_replied", "atlas replied in your conversation"),
    );
    overview.events = [...replies, ...others].sort((a, b) => b.id - a.id);
    overview.eventsLoaded = true;

    render(AcrossTheTeam, { now: NOW });

    expect(screen.queryByText(/replied in your conversation/)).toBeNull();
    expect(screen.getAllByRole("listitem")).toHaveLength(10);
  });

  it("says nothing has happened when replies are all there is", () => {
    overview.events = [event(1, "agent_replied", "atlas replied in your conversation")];
    overview.eventsLoaded = true;

    render(AcrossTheTeam, { now: NOW });

    expect(screen.getByText("Nothing has happened yet.")).toBeTruthy();
  });
});
